//! The fixed-host Arbeitnow transport in-process, over loopback TLS with the test seams.

mod fetch_support;

use std::net::IpAddr;

use fetch_support::{Reply, declared_response, env_for_name, pki, serve};
use jobniac_infra::workers::fetch::board::{
    self, BoardCategory, BoardFailure, BoardResponse, HOST,
};

fn public() -> IpAddr {
    "8.8.8.8".parse().expect("public address")
}

async fn run(
    reply: Reply,
    page: i64,
    visa: Option<bool>,
) -> (
    Result<BoardResponse, BoardFailure>,
    std::sync::Arc<jobniac_infra::workers::fetch::testing::MappingDialer>,
) {
    let pki = pki(&[HOST], false);
    let server = serve(&pki, reply).await;
    let (env, _, dialer) = env_for_name(&pki, HOST, public(), server);
    (board::fetch(&env, page, visa).await, dialer)
}

#[tokio::test]
async fn request_is_the_fixed_host_path_and_headers_with_the_page_and_visa_filter() {
    let (outcome, dialer) = run(Reply::EchoHead, 1, Some(false)).await;
    let response = outcome.expect("echo succeeds");
    assert_eq!(response.status, 200);
    // Header names are case-insensitive; the parser writes them lowercase.
    let head = String::from_utf8(response.body)
        .expect("utf-8 head")
        .to_ascii_lowercase();
    assert!(
        head.starts_with("get /api/job-board-api?page=1&visa_sponsorship=false http/1.1"),
        "{head}"
    );
    assert!(head.contains("host: www.arbeitnow.com"), "{head}");
    assert!(head.contains("accept: application/json"), "{head}");
    assert!(head.contains("accept-encoding: identity"), "{head}");
    assert!(!head.contains("cookie"));
    assert!(!head.contains("authorization"));
    assert_eq!(dialer.attempts.lock().expect("attempts").len(), 1);
}

#[tokio::test]
async fn non_200_status_is_returned_with_an_empty_unread_body() {
    let response = b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/PRIVATE\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsecret".to_vec();
    let (outcome, _) = run(Reply::Bytes(response), 2, None).await;
    let response = outcome.expect("status is returned");
    assert_eq!(response.status, 302);
    assert!(response.body.is_empty());
}

#[tokio::test]
async fn chunked_body_of_three_mebibytes_is_accepted_and_returned_whole() {
    // A chunked body below the 4 MiB bound.
    let payload = vec![b'j'; 3 * 1024 * 1024];
    let mut response = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(format!("{:x}\r\n", payload.len()).as_bytes());
    response.extend_from_slice(&payload);
    response.extend_from_slice(b"\r\n0\r\n\r\n");
    let (outcome, _) = run(Reply::Bytes(response), 3, None).await;
    assert_eq!(outcome.expect("body accepted").body.len(), payload.len());
}

#[tokio::test]
async fn declared_content_length_of_three_mebibytes_is_admitted_and_read_whole() {
    // Board bound is four mebibytes: a 2-4 MiB declared length must not hit the job page limit.
    let payload = vec![b'j'; 3 * 1024 * 1024];
    let (outcome, _) = run(
        Reply::Bytes(declared_response(payload.len(), &payload)),
        6,
        None,
    )
    .await;
    assert_eq!(outcome.expect("declared 3 MiB admitted").body, payload);
}

#[tokio::test]
async fn declared_content_length_over_four_mebibytes_is_refused_as_transport() {
    let declared = 4 * 1024 * 1024 + 1;
    let (outcome, _) = run(Reply::Bytes(declared_response(declared, b"x")), 7, None).await;
    assert_eq!(
        outcome.err(),
        Some(BoardFailure {
            category: BoardCategory::Transport,
            retryable: true
        })
    );
}

#[tokio::test]
async fn chunked_body_over_four_mebibytes_is_a_body_size_failure() {
    // One byte over the 4 MiB bound.
    let payload = vec![b'j'; 4 * 1024 * 1024 + 1];
    let mut response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(format!("{:x}\r\n", payload.len()).as_bytes());
    response.extend_from_slice(&payload);
    response.extend_from_slice(b"\r\n0\r\n\r\n");
    let (outcome, _) = run(Reply::Bytes(response), 4, None).await;
    assert_eq!(
        outcome.err(),
        Some(BoardFailure {
            category: BoardCategory::BodySize,
            retryable: false
        })
    );
}

#[tokio::test]
async fn duplicate_framing_on_a_200_is_a_retryable_transport_failure() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx".to_vec();
    let (outcome, _) = run(Reply::Bytes(response), 5, None).await;
    assert_eq!(
        outcome.err(),
        Some(BoardFailure {
            category: BoardCategory::Transport,
            retryable: true
        })
    );
}

#[tokio::test]
async fn invalid_page_is_refused_before_any_connection() {
    // Page 0 and out-of-range pages.
    for page in [0_i64, -1, 2_147_483_648] {
        let (outcome, dialer) = run(Reply::Stall, page, None).await;
        assert_eq!(
            outcome.err(),
            Some(BoardFailure {
                category: BoardCategory::HttpStatus,
                retryable: false
            })
        );
        assert!(dialer.attempts.lock().expect("attempts").is_empty());
    }
}

#[tokio::test]
async fn non_public_answer_is_a_transport_failure_without_connection() {
    let pki = pki(&[HOST], false);
    let server = serve(&pki, Reply::Stall).await;
    let (_, _, dialer) = env_for_name(&pki, HOST, public(), server);
    // Replace the answer with a mixed one: the public record and a loopback record.
    let mut answers = std::collections::BTreeMap::new();
    answers.insert(
        HOST.to_owned(),
        vec![public(), "127.0.0.1".parse().expect("address")],
    );
    let lookup = std::sync::Arc::new(jobniac_infra::workers::fetch::testing::FixedLookup {
        answers,
        calls: Default::default(),
    });
    let tls = jobniac_infra::workers::fetch::testing::roots_config(&pki.authority).expect("roots");
    let env = jobniac_infra::workers::fetch::testing::environment(lookup, dialer.clone(), tls);
    assert_eq!(
        board::fetch(&env, 1, None).await.err(),
        Some(BoardFailure {
            category: BoardCategory::Transport,
            retryable: true
        })
    );
    assert!(dialer.attempts.lock().expect("attempts").is_empty());
}

fn transport_failure() -> BoardFailure {
    BoardFailure {
        category: BoardCategory::Transport,
        retryable: true,
    }
}

#[tokio::test]
async fn declared_body_of_exactly_four_mebibytes_is_admitted() {
    let body = vec![b'a'; 4 * 1024 * 1024];
    let (outcome, _) = run(Reply::Bytes(declared_response(body.len(), &body)), 3, None).await;
    assert_eq!(outcome.expect("admitted").body.len(), 4 * 1024 * 1024);
}

#[tokio::test]
async fn declared_body_one_over_four_mebibytes_is_refused() {
    let (outcome, _) = run(
        Reply::Bytes(declared_response(4 * 1024 * 1024 + 1, b"x")),
        3,
        None,
    )
    .await;
    assert_eq!(outcome.err(), Some(transport_failure()));
}

#[tokio::test]
async fn fixed_host_is_resolved_once_per_request() {
    let pki = pki(&[HOST], false);
    let server = serve(&pki, Reply::EchoHead).await;
    let (env, lookup, dialer) = env_for_name(&pki, HOST, public(), server);
    board::fetch(&env, 1, None).await.expect("echo succeeds");
    assert_eq!(lookup.calls.lock().expect("calls").len(), 1);
    assert_eq!(dialer.attempts.lock().expect("attempts").len(), 1);
}

#[tokio::test]
async fn tls_identity_must_match_the_fixed_host() {
    // The TLS identity refusal of the Arbeitnow transport (certificate for another name).
    let pki = pki(&["other.example.net"], false);
    let server = serve(&pki, Reply::Bytes(declared_response(1, b"x"))).await;
    let (env, _, _) = env_for_name(&pki, HOST, public(), server);
    assert_eq!(
        board::fetch(&env, 1, None).await.err(),
        Some(transport_failure())
    );
}

#[tokio::test]
async fn dns_failure_is_a_retryable_transport_failure_without_connection() {
    // DNS failure is sanitized. The fixed host has no answer, so the lookup fails.
    let pki = pki(&[HOST], false);
    let server = serve(&pki, Reply::Stall).await;
    let (env, _, dialer) = env_for_name(&pki, "other.example.net", public(), server);
    assert_eq!(
        board::fetch(&env, 1, None).await.err(),
        Some(transport_failure())
    );
    assert!(dialer.attempts.lock().expect("attempts").is_empty());
}

#[tokio::test]
async fn bounded_complete_trailer_is_discarded_and_never_exposed() {
    let mut response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n1\r\nx\r\n0\r\nX-Private-Trailer: "
            .to_vec();
    // 8 KiB is under the 16 KiB trailer limit of hyper (see the deviation note on the link test).
    response.extend(std::iter::repeat_n(b'T', 8 * 1024));
    response.extend_from_slice(b"\r\n\r\n");
    let (outcome, _) = run(Reply::Bytes(response), 4, None).await;
    let response = outcome.expect("trailer is discarded");
    assert_eq!(response.body, b"x");
    assert!(
        !response
            .headers
            .iter()
            .any(|(name, _)| name.as_slice() == &b"x-private-trailer"[..])
    );
    assert!(!format!("{response:?}").contains("TTTT"));
}

#[tokio::test]
async fn trailer_bytes_count_against_the_four_mebibyte_budget() {
    // The data fills the budget exactly, so any trailer byte is over it: a framing failure.
    let data = vec![b'a'; 4 * 1024 * 1024];
    let mut response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(format!("{:x}\r\n", data.len()).as_bytes());
    response.extend_from_slice(&data);
    response.extend_from_slice(b"\r\n0\r\nX-Late: y\r\n\r\n");
    let (outcome, _) = run(Reply::Bytes(response), 4, None).await;
    assert_eq!(outcome.err(), Some(transport_failure()));
}

#[tokio::test]
async fn redirect_is_not_followed_on_the_board_either() {
    // The board transport behaves like the link transport.
    let response = b"HTTP/1.1 301 Moved\r\nLocation: https://elsewhere.example.org/api\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
    let (outcome, dialer) = run(Reply::Bytes(response), 6, None).await;
    assert_eq!(outcome.expect("status is returned").status, 301);
    assert_eq!(dialer.attempts.lock().expect("attempts").len(), 1);
}
