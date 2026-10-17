//! The job-page fetch in-process, over loopback TLS with the test seams.

mod fetch_support;

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use fetch_support::{Pki, Reply, declared_response, env_for_name, html_response, pki, serve};
use jobniac_core::job_links::JobLinkFetchFailure;
use jobniac_infra::workers::fetch::FetchEnvironment;
use jobniac_infra::workers::fetch::link::{fetch, fetch_within};
use jobniac_infra::workers::fetch::testing::{
    FixedLookup, MappingDialer, environment, roots_config,
};

const PUBLIC: &str = "8.8.8.8";
const HOST: &str = "jobs.example.com";

fn public() -> IpAddr {
    PUBLIC.parse().expect("public address")
}

/// An environment whose resolver answers `answers` for `host` and whose dialer maps each
/// `(validated, server)` pair; every other address is refused by the dialer.
fn custom_env(
    pki: &Pki,
    host: &str,
    answers: &[&str],
    map: &[(SocketAddr, SocketAddr)],
) -> (FetchEnvironment, Arc<FixedLookup>, Arc<MappingDialer>) {
    let mut table = BTreeMap::new();
    table.insert(
        host.to_owned(),
        answers
            .iter()
            .map(|text| text.parse().expect("address"))
            .collect::<Vec<IpAddr>>(),
    );
    let lookup = Arc::new(FixedLookup {
        answers: table,
        calls: Default::default(),
    });
    let dialer = Arc::new(MappingDialer {
        map: map.iter().copied().collect::<HashMap<_, _>>(),
        attempts: Default::default(),
    });
    let tls = roots_config(&pki.authority).expect("roots");
    let env = environment(lookup.clone(), dialer.clone(), tls);
    (env, lookup, dialer)
}

/// A 200 response with a chunked body of one data chunk and the given raw trailer section.
fn chunked_with_trailer(data: &[u8], trailer: &[u8]) -> Vec<u8> {
    let mut response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(format!("{:x}\r\n", data.len()).as_bytes());
    response.extend_from_slice(data);
    response.extend_from_slice(b"\r\n0\r\n");
    response.extend_from_slice(trailer);
    response.extend_from_slice(b"\r\n");
    response
}

/// `https://jobs.example.com/a/../b?x=1`, served from the loopback server.
async fn run(
    reply: Reply,
    names: &[&str],
    expired: bool,
) -> (
    Result<jobniac_core::job_links::JobLinkFetchResult, JobLinkFetchFailure>,
    std::sync::Arc<jobniac_infra::workers::fetch::testing::FixedLookup>,
    std::sync::Arc<jobniac_infra::workers::fetch::testing::MappingDialer>,
) {
    let pki = pki(names, expired);
    let server = serve(&pki, reply).await;
    let (env, lookup, dialer) = env_for_name(&pki, HOST, public(), server);
    let outcome = fetch(&env, "https://jobs.example.com/a/../b?x=1").await;
    (outcome, lookup, dialer)
}

#[tokio::test]
async fn successful_fetch_connects_to_the_validated_address_once() {
    let body = "<title>Checked</title><p>body</p>";
    let (outcome, lookup, dialer) = run(Reply::Bytes(html_response(body)), &[HOST], false).await;
    let result = outcome.expect("fetch succeeds");
    assert_eq!(result.title.as_deref(), Some("Checked"));
    assert_eq!(result.http_status, 200);
    assert_eq!(
        lookup.calls.lock().expect("calls").len(),
        1,
        "one resolution per hop"
    );
    let attempts = dialer.attempts.lock().expect("attempts").clone();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].ip(), public());
}

#[tokio::test]
async fn request_head_keeps_the_raw_target_and_the_original_host() {
    // The raw target and Host of the pinned numeric-TLS transport.
    let (outcome, _, _) = run(Reply::EchoHead, &[HOST], false).await;
    let result = outcome.expect("echo succeeds");
    let description = result
        .description
        .expect("echoed head")
        .to_ascii_lowercase();
    // Header names are case-insensitive; the parser writes them lowercase.
    assert!(
        description.contains("get /a/../b?x=1 http/1.1"),
        "{description}"
    );
    assert!(
        description.contains("host: jobs.example.com"),
        "{description}"
    );
    assert!(
        description.contains("accept-encoding: identity"),
        "{description}"
    );
    assert!(!description.contains("authorization"));
}

#[tokio::test]
async fn any_non_public_answer_refuses_the_whole_answer_without_connecting() {
    for bad in [
        "10.0.0.5",
        "127.0.0.1",
        "169.254.169.254",
        "100.64.0.1",
        "224.0.0.1",
        "192.0.2.1",
        "fc00::1",
        "fe80::1",
        "::ffff:127.0.0.1",
        "::1",
    ] {
        let pki = pki(&[HOST], false);
        let server = serve(&pki, Reply::Stall).await;
        let mut answers = std::collections::BTreeMap::new();
        answers.insert(
            HOST.to_owned(),
            vec![public(), bad.parse().expect("address")],
        );
        let lookup = std::sync::Arc::new(jobniac_infra::workers::fetch::testing::FixedLookup {
            answers,
            calls: Default::default(),
        });
        let dialer = std::sync::Arc::new(jobniac_infra::workers::fetch::testing::MappingDialer {
            map: [(std::net::SocketAddr::new(public(), 443), server)]
                .into_iter()
                .collect(),
            attempts: Default::default(),
        });
        let tls =
            jobniac_infra::workers::fetch::testing::roots_config(&pki.authority).expect("roots");
        let env = jobniac_infra::workers::fetch::testing::environment(lookup, dialer.clone(), tls);
        let outcome = fetch(&env, "https://jobs.example.com/").await;
        assert_eq!(
            outcome.err(),
            Some(JobLinkFetchFailure::UnsafeAddress),
            "{bad}"
        );
        assert!(
            dialer.attempts.lock().expect("attempts").is_empty(),
            "no connection for {bad}"
        );
    }
}

#[tokio::test]
async fn literal_non_public_address_is_refused_before_any_connection() {
    // The IP literal path.
    let pki = pki(&[HOST], false);
    let (env, _, dialer) = env_for_name(
        &pki,
        HOST,
        public(),
        "127.0.0.1:9".parse().expect("address"),
    );
    assert_eq!(
        fetch(&env, "https://127.0.0.1/").await.err(),
        Some(JobLinkFetchFailure::UnsafeAddress)
    );
    assert_eq!(
        fetch(&env, "https://[::ffff:10.0.0.1]/").await.err(),
        Some(JobLinkFetchFailure::UnsafeAddress)
    );
    assert!(dialer.attempts.lock().expect("attempts").is_empty());
}

#[tokio::test]
async fn connect_attempts_are_capped_at_four_validated_addresses() {
    // MAX_CONNECT_ATTEMPTS bounds the attempts.
    let pki = pki(&[HOST], false);
    let server = serve(&pki, Reply::Stall).await;
    let addresses: Vec<IpAddr> = [
        "8.8.8.8",
        "8.8.4.4",
        "1.1.1.1",
        "9.9.9.9",
        "208.67.222.222",
        "4.4.4.4",
    ]
    .iter()
    .map(|text| text.parse().expect("address"))
    .collect();
    let mut answers = std::collections::BTreeMap::new();
    answers.insert(HOST.to_owned(), addresses);
    let lookup = std::sync::Arc::new(jobniac_infra::workers::fetch::testing::FixedLookup {
        answers,
        calls: Default::default(),
    });
    // No mapping: every dial is refused, so the attempts are exactly the validated addresses.
    let dialer =
        std::sync::Arc::new(jobniac_infra::workers::fetch::testing::MappingDialer::default());
    let tls = jobniac_infra::workers::fetch::testing::roots_config(&pki.authority).expect("roots");
    let env: FetchEnvironment =
        jobniac_infra::workers::fetch::testing::environment(lookup, dialer.clone(), tls);
    let _ = server;
    assert_eq!(
        fetch(&env, "https://jobs.example.com/").await.err(),
        Some(JobLinkFetchFailure::Unavailable)
    );
    let attempts = dialer.attempts.lock().expect("attempts").clone();
    assert_eq!(attempts.len(), 4);
    assert!(attempts.iter().all(|address| address.port() == 443));
}

#[tokio::test]
async fn redirects_are_never_followed() {
    let redirect = |location: &str| {
        format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .into_bytes()
    };
    let (outcome, _, _) = run(
        Reply::Bytes(redirect("https://elsewhere.example.org/x")),
        &[HOST],
        false,
    )
    .await;
    assert_eq!(
        outcome.err(),
        Some(JobLinkFetchFailure::RedirectNotFollowed)
    );
    let (outcome, _, _) = run(Reply::Bytes(redirect("http://8.8.4.4/new")), &[HOST], false).await;
    assert_eq!(
        outcome.err(),
        Some(JobLinkFetchFailure::RedirectNotFollowed)
    );
    let (outcome, _, _) = run(
        Reply::Bytes(redirect("http://127.0.0.1/private")),
        &[HOST],
        false,
    )
    .await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::UnsafeAddress));
    let no_location =
        b"HTTP/1.1 302 Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
    let (outcome, _, _) = run(Reply::Bytes(no_location), &[HOST], false).await;
    assert_eq!(
        outcome.err(),
        Some(JobLinkFetchFailure::RedirectNotFollowed)
    );
}

#[tokio::test]
async fn oversize_header_block_is_refused() {
    let filler = "x".repeat(17_000);
    let response = format!("HTTP/1.1 200 OK\r\nX-Filler: {filler}\r\nContent-Length: 0\r\n\r\n");
    let (outcome, _, _) = run(Reply::Bytes(response.into_bytes()), &[HOST], false).await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::Unavailable));
}

#[tokio::test]
async fn framing_abuse_is_refused_before_the_body() {
    let cases = [
        "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx",
        "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nx\r\n0\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 1\r\n\r\nx",
        "HTTP/1.1 200 OK\r\nContent-Length: +1\r\n\r\nx",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 2097153\r\n\r\n",
    ];
    for response in cases {
        let (outcome, _, _) = run(Reply::Bytes(response.as_bytes().to_vec()), &[HOST], false).await;
        assert_eq!(
            outcome.err(),
            Some(JobLinkFetchFailure::Unavailable),
            "{response:?}"
        );
    }
}

#[tokio::test]
async fn chunked_body_over_the_two_mebibyte_bound_is_refused() {
    let chunk = vec![b'a'; 2 * 1024 * 1024 + 1];
    let mut response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
    response.extend_from_slice(&chunk);
    response.extend_from_slice(b"\r\n0\r\n\r\n");
    let (outcome, _, _) = run(Reply::Bytes(response), &[HOST], false).await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::Unavailable));
}

#[tokio::test]
async fn declared_content_length_of_three_mebibytes_is_refused_on_the_link_path() {
    // The job page bound is two mebibytes; the board's four mebibytes do not apply here.
    let (outcome, _, _) = run(
        Reply::Bytes(declared_response(3 * 1024 * 1024, b"x")),
        &[HOST],
        false,
    )
    .await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::Unavailable));
}

#[tokio::test]
async fn declared_length_mismatch_is_refused() {
    let response =
        b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nshort".to_vec();
    let (outcome, _, _) = run(Reply::Bytes(response), &[HOST], false).await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::Unavailable));
}

#[tokio::test]
async fn content_encoding_other_than_identity_is_refused() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabcd".to_vec();
    let (outcome, _, _) = run(Reply::Bytes(response), &[HOST], false).await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::Unavailable));
}

#[tokio::test]
async fn tls_name_mismatch_is_refused() {
    // TLS name mismatch of the pinned transport (SNI keeps the original name).
    let (outcome, _, _) = run(
        Reply::Bytes(html_response("<p>x</p>")),
        &["other.example.net"],
        false,
    )
    .await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::Unavailable));
}

#[tokio::test]
async fn expired_certificate_is_refused() {
    let (outcome, _, _) = run(Reply::Bytes(html_response("<p>x</p>")), &[HOST], true).await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::Unavailable));
}

#[tokio::test]
async fn slow_peer_is_cut_by_the_total_deadline() {
    let pki = pki(&[HOST], false);
    let server = serve(&pki, Reply::Stall).await;
    let (env, _, _) = env_for_name(&pki, HOST, public(), server);
    let started = std::time::Instant::now();
    let outcome = fetch_within(
        &env,
        "https://jobs.example.com/",
        Duration::from_millis(400),
    )
    .await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::Unavailable));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn production_connector_refuses_loopback_names_and_addresses() {
    // The production environment must never reach loopback: a literal is refused by the URL check,
    // and a name that resolves to loopback is refused by the admission of the answer.
    let env = FetchEnvironment::production().expect("system configuration");
    assert_eq!(
        fetch(&env, "https://127.0.0.1:9/").await.err(),
        Some(JobLinkFetchFailure::UnsafeAddress)
    );
    // A host name that is not a public-looking DNS name is refused by the URL check
    // (IP_LITERAL_REQUIRED for https://localhost), so no resolver answer is even requested.
    assert_eq!(
        fetch(&env, "https://localhost:9/").await.err(),
        Some(JobLinkFetchFailure::IpLiteralRequired)
    );
}

#[tokio::test]
async fn literal_forms_of_loopback_never_reach_dns_or_connect() {
    // Unsafe names and IPv6 literals. Each form is refused by the URL check, so neither the
    // resolver nor the dialer sees it.
    let pki = pki(&[HOST], false);
    let server = serve(&pki, Reply::Stall).await;
    let (env, lookup, dialer) = custom_env(
        &pki,
        HOST,
        &[PUBLIC],
        &[(SocketAddr::new(public(), 443), server)],
    );
    let names = [
        "https://127.1/",
        "https://0x7f.1/",
        "https://2130706433/",
        "https://017700000001/",
        "https://0x7f000001/",
        "https://0x7f.0.0.1/",
        "http://127.1/",
        "http://2130706433/",
    ];
    for url in names {
        assert_eq!(
            fetch(&env, url).await.err(),
            Some(JobLinkFetchFailure::IpLiteralRequired),
            "{url}"
        );
    }
    let literals = [
        "https://[::1]/",
        "https://[::ffff:127.0.0.1]/",
        "https://[::ffff:7f00:1]/",
    ];
    for url in literals {
        assert_eq!(
            fetch(&env, url).await.err(),
            Some(JobLinkFetchFailure::UnsafeAddress),
            "{url}"
        );
    }
    assert!(lookup.calls.lock().expect("calls").is_empty());
    assert!(dialer.attempts.lock().expect("attempts").is_empty());
}

#[test]
fn production_environment_is_built_outside_a_runtime_and_refuses_loopback() {
    // The child builds the production environment on a plain thread, then starts its own runtime.
    let env = FetchEnvironment::production().expect("system configuration");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    assert_eq!(
        runtime.block_on(fetch(&env, "https://127.0.0.1:9/")).err(),
        Some(JobLinkFetchFailure::UnsafeAddress)
    );
}

#[tokio::test]
async fn declared_body_of_exactly_two_mebibytes_is_admitted() {
    let body = vec![b'a'; 2 * 1024 * 1024];
    let (outcome, _, _) = run(
        Reply::Bytes(declared_response(body.len(), &body)),
        &[HOST],
        false,
    )
    .await;
    assert!(outcome.is_ok(), "{:?}", outcome.err());
}

#[tokio::test]
async fn declared_body_one_over_two_mebibytes_is_refused() {
    let (outcome, _, _) = run(
        Reply::Bytes(declared_response(2 * 1024 * 1024 + 1, b"x")),
        &[HOST],
        false,
    )
    .await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::Unavailable));
}

#[tokio::test]
async fn interim_header_block_over_the_bound_is_refused() {
    let filler = "x".repeat(17_000);
    let response = format!(
        "HTTP/1.1 103 Early Hints\r\nX-Filler: {filler}\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let (outcome, _, _) = run(Reply::Bytes(response.into_bytes()), &[HOST], false).await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::Unavailable));
}

#[tokio::test]
async fn connect_falls_back_to_the_next_validated_address_only() {
    let pki = pki(&[HOST], false);
    let server = serve(&pki, Reply::Bytes(html_response("<title>Second</title>"))).await;
    let second: IpAddr = "8.8.4.4".parse().expect("address");
    let (env, lookup, dialer) = custom_env(
        &pki,
        HOST,
        &[PUBLIC, "8.8.4.4"],
        &[(SocketAddr::new(second, 443), server)],
    );
    let result = fetch(&env, "https://jobs.example.com/")
        .await
        .expect("the second address serves");
    assert_eq!(result.title.as_deref(), Some("Second"));
    assert_eq!(
        dialer.attempts.lock().expect("attempts").clone(),
        vec![SocketAddr::new(public(), 443), SocketAddr::new(second, 443)]
    );
    assert_eq!(lookup.calls.lock().expect("calls").len(), 1);
}

#[tokio::test]
async fn all_connects_failing_is_unavailable_without_leaking_the_cause() {
    // One resolution, no second lookup after the refused connects.
    let pki = pki(&[HOST], false);
    let (env, lookup, dialer) = custom_env(&pki, HOST, &[PUBLIC, "8.8.4.4"], &[]);
    assert_eq!(
        fetch(&env, "https://jobs.example.com/").await.err(),
        Some(JobLinkFetchFailure::Unavailable)
    );
    assert_eq!(dialer.attempts.lock().expect("attempts").len(), 2);
    assert_eq!(lookup.calls.lock().expect("calls").len(), 1);
}

#[tokio::test]
async fn one_resolution_per_hop_even_after_a_redirect() {
    // Rebinding. The second hop is never taken, so its resolution is never consulted.
    let pki = pki(&[HOST], false);
    let server = serve(
        &pki,
        Reply::Bytes(
            b"HTTP/1.1 302 Found\r\nLocation: https://jobs.example.com/next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_vec(),
        ),
    )
    .await;
    let (env, lookup, dialer) = custom_env(
        &pki,
        HOST,
        &[PUBLIC],
        &[(SocketAddr::new(public(), 443), server)],
    );
    assert_eq!(
        fetch(&env, "https://jobs.example.com/").await.err(),
        Some(JobLinkFetchFailure::RedirectNotFollowed)
    );
    assert_eq!(lookup.calls.lock().expect("calls").len(), 1);
    assert_eq!(dialer.attempts.lock().expect("attempts").len(), 1);
}

#[tokio::test]
async fn idna_hostname_uses_punycode_for_resolution_connect_sni_and_host() {
    let ascii = "xn--mnchen-3ya.de";
    let pki = pki(&[ascii], false);
    let server = serve(&pki, Reply::EchoHead).await;
    let (env, lookup, dialer) = custom_env(
        &pki,
        ascii,
        &[PUBLIC],
        &[(SocketAddr::new(public(), 443), server)],
    );
    let result = fetch(&env, "https://münchen.de/x")
        .await
        .expect("echo succeeds");
    let head = result
        .description
        .expect("echoed head")
        .to_ascii_lowercase();
    assert!(head.contains("host: xn--mnchen-3ya.de"), "{head}");
    assert_eq!(lookup.calls.lock().expect("calls")[0].0, ascii);
    assert_eq!(dialer.attempts.lock().expect("attempts").len(), 1);
}

#[tokio::test]
async fn explicit_port_is_kept_for_resolution_connect_and_host_header() {
    let pki = pki(&[HOST], false);
    let server = serve(&pki, Reply::EchoHead).await;
    let (env, lookup, dialer) = custom_env(
        &pki,
        HOST,
        &[PUBLIC],
        &[(SocketAddr::new(public(), 8443), server)],
    );
    let result = fetch(&env, "https://jobs.example.com:8443/a")
        .await
        .expect("echo succeeds");
    let head = result
        .description
        .expect("echoed head")
        .to_ascii_lowercase();
    assert!(head.contains("host: jobs.example.com:8443"), "{head}");
    assert_eq!(lookup.calls.lock().expect("calls")[0].1, 8443);
    assert_eq!(dialer.attempts.lock().expect("attempts")[0].port(), 8443);
}

#[tokio::test]
async fn bounded_complete_trailer_is_discarded_and_never_exposed() {
    // 8 KiB is under the 16 KiB trailer limit of hyper (see the deviation note on the next test).
    let trailer = format!("X-Private-Trailer: {}\r\n", "T".repeat(8 * 1024));
    let (outcome, _, _) = run(
        Reply::Bytes(chunked_with_trailer(b"x", trailer.as_bytes())),
        &[HOST],
        false,
    )
    .await;
    let result = outcome.expect("trailer is discarded");
    assert_eq!(result.description.as_deref(), Some("x"));
    assert!(!format!("{result:?}").contains("X-Private-Trailer"));
}

#[tokio::test]
async fn complete_trailer_over_hyper_16_kib_limit_is_refused() {
    // hyper refuses a trailer section over its own TRAILER_LIMIT (16 KiB), so a complete 20 KiB
    // trailer is refused as a framing failure. The refusal never exposes trailer bytes.
    let trailer = format!("X-Private-Trailer: {}\r\n", "T".repeat(20 * 1024));
    let (outcome, _, _) = run(
        Reply::Bytes(chunked_with_trailer(b"x", trailer.as_bytes())),
        &[HOST],
        false,
    )
    .await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::Unavailable));
}

#[tokio::test]
async fn trailer_bytes_count_against_the_two_mebibyte_budget() {
    // The data fills the budget exactly, so any trailer byte is over it.
    let data = vec![b'a'; 2 * 1024 * 1024];
    let (outcome, _, _) = run(
        Reply::Bytes(chunked_with_trailer(&data, b"X-Late: y\r\n")),
        &[HOST],
        false,
    )
    .await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::Unavailable));
}

#[tokio::test]
async fn oversize_incomplete_trailer_is_refused() {
    // The peer closes inside the trailer section, so the framing fails before the deadline.
    let trailer = format!("X-Trailer: {}", "T".repeat(160 * 1024));
    let mut response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n1\r\nx\r\n0\r\n"
            .to_vec();
    response.extend_from_slice(trailer.as_bytes());
    let (outcome, _, _) = run(Reply::Bytes(response), &[HOST], false).await;
    assert_eq!(outcome.err(), Some(JobLinkFetchFailure::Unavailable));
}
