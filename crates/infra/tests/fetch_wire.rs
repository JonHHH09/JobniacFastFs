//! Framed binary results of the fetch workers, the request-target split and the strict decoders
//! Pure functions: no socket and no child.

use jobniac_core::job_links::{JobLinkFetchFailure, JobLinkFetchResult};
use jobniac_infra::workers::fetch::board::{BoardCategory, BoardFailure, BoardResponse};
use jobniac_infra::workers::fetch::target;
use jobniac_infra::workers::fetch::wire;
use std::net::IpAddr;

fn result(title: Option<&str>, description: Option<&str>, status: i64) -> JobLinkFetchResult {
    JobLinkFetchResult {
        title: title.map(str::to_owned),
        description: description.map(str::to_owned),
        http_status: status,
    }
}

#[test]
fn link_success_round_trips() {
    let outcome = Ok(result(Some("A < B"), None, 200));
    let encoded = wire::encode_link(&outcome, 1 << 20);
    assert_eq!(wire::decode_link(&encoded), Ok(outcome));
}

#[test]
fn link_failure_codes_round_trip_and_unknown_codes_are_generic() {
    for failure in [
        JobLinkFetchFailure::Unavailable,
        JobLinkFetchFailure::Interrupted,
        JobLinkFetchFailure::InvalidUrl,
        JobLinkFetchFailure::RedirectNotFollowed,
        JobLinkFetchFailure::UnsafeAddress,
        JobLinkFetchFailure::IpLiteralRequired,
    ] {
        let encoded = wire::encode_link(&Err(failure), 1 << 20);
        assert_eq!(wire::decode_link(&encoded), Ok(Err(failure)));
    }
    assert_eq!(
        wire::decode_link(&[0, 200]),
        Ok(Err(JobLinkFetchFailure::Unavailable))
    );
}

#[test]
fn oversized_link_success_becomes_the_generic_failure() {
    let big = "x".repeat(2000);
    let outcome = Ok(result(Some(&big), None, 200));
    let encoded = wire::encode_link(&outcome, 1000);
    assert_eq!(
        wire::decode_link(&encoded),
        Ok(Err(JobLinkFetchFailure::Unavailable))
    );
}

#[test]
fn malformed_link_frames_are_rejected() {
    assert!(wire::decode_link(&[]).is_err());
    assert!(wire::decode_link(&[9]).is_err());
    assert!(wire::decode_link(&[1, 0, 200, 0, 0, 0xFF]).is_err());
    let mut trailing = wire::encode_link(&Err(JobLinkFetchFailure::Unavailable), 100);
    trailing.push(0);
    assert!(wire::decode_link(&trailing).is_err());
    // A status outside 100..=599 is refused as a malformed success.
    assert!(wire::decode_link(&[1, 0, 99, 0, 0]).is_err());
}

#[test]
fn board_request_round_trips_and_rejects_bad_visa_bytes() {
    for visa in [None, Some(false), Some(true)] {
        let encoded = wire::encode_board_request(7, visa);
        assert_eq!(wire::decode_board_request(&encoded), Some((7, visa)));
    }
    assert_eq!(wire::decode_board_request(&[0, 0, 0, 1, 9]), None);
    assert_eq!(wire::decode_board_request(&[0, 0, 0]), None);
}

#[test]
fn board_success_and_failure_round_trip() {
    let response = BoardResponse {
        status: 200,
        headers: vec![(b"content-type".to_vec(), b"application/json".to_vec())],
        body: b"{}".to_vec(),
    };
    let encoded = wire::encode_board(&Ok(response.clone()), 1 << 20);
    assert_eq!(wire::decode_board(&encoded), Ok(Ok(response)));
    let failure = BoardFailure {
        category: BoardCategory::BodySize,
        retryable: false,
    };
    let encoded = wire::encode_board(&Err(failure), 1 << 20);
    assert_eq!(wire::decode_board(&encoded), Ok(Err(failure)));
}

#[test]
fn board_header_block_over_the_bound_is_malformed() {
    let response = BoardResponse {
        status: 200,
        headers: vec![(b"x".to_vec(), vec![b'a'; 17_000])],
        body: Vec::new(),
    };
    let encoded = wire::encode_board(&Ok(response), 1 << 20);
    assert!(wire::decode_board(&encoded).is_err());
}

#[test]
fn request_target_keeps_dot_segments_and_quotes_verbatim() {
    let split = target::split("https://jobs.example.com:444/a/../b?z=%2f&b=2 x").expect("wire url");
    assert_eq!(split.port, 444);
    assert_eq!(split.host_header, "jobs.example.com:444");
    assert_eq!(split.request_target, "/a/../b?z=%2f&b=2%20x");
    let root = target::split("https://jobs.example.com?x=1").expect("wire url");
    assert_eq!(root.request_target, "/?x=1");
    assert_eq!(root.host_header, "jobs.example.com");
    assert_eq!(root.port, 443);
}

#[test]
fn literal_authorities_are_split_and_bracketed_in_the_host_header() {
    let v6 = target::split("https://[2606:4700:4700::1111]/").expect("wire url");
    assert_eq!(
        v6.literal,
        Some("2606:4700:4700::1111".parse::<IpAddr>().expect("address"))
    );
    assert_eq!(v6.host_header, "[2606:4700:4700::1111]");
    let plain = target::split("http://8.8.8.8:80/x").expect("wire url");
    assert!(!plain.https);
    assert_eq!(plain.host_header, "8.8.8.8");
    assert_eq!(plain.request_target, "/x");
}

#[test]
fn target_debug_never_prints_the_host_path_or_query() {
    // The request target may carry credentials in its query; Debug must not print it.
    let split = target::split("https://jobs.example.com/a?token=SECRET-QUERY").expect("wire url");
    let text = format!("{split:?}");
    assert!(!text.contains("jobs.example.com"), "{text}");
    assert!(!text.contains("SECRET-QUERY"), "{text}");
    assert!(!text.contains("/a"), "{text}");
}

#[test]
fn board_response_debug_never_prints_the_body_or_headers() {
    let response = BoardResponse {
        status: 200,
        headers: vec![(b"x-token".to_vec(), b"SECRET-HEADER".to_vec())],
        body: b"SECRET-BODY".to_vec(),
    };
    let text = format!("{response:?}");
    assert!(text.contains("200"), "{text}");
    assert!(!text.contains("SECRET"), "{text}");
}
