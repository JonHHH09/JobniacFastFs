//! Parent-side admission of the worker's raw Arbeitnow answer: status and header classification,
//! the body bound, the transport failure mapping and the body parse. Bodies are synthetic.

use jobniac_core::arbeitnow::board::MAX_BODY_BYTES;
use jobniac_core::arbeitnow::failure::{BoardCategory, BoardFailure};
use jobniac_core::arbeitnow::instant::ArbeitnowInstant;
use jobniac_infra::arbeitnow::board::{
    admit_board_response, admit_board_result, map_transport_failure,
};
use jobniac_infra::workers::fetch::board::{
    BoardCategory as TransportCategory, BoardFailure as TransportFailure, BoardResponse,
};
use serde_json::json;

fn now() -> ArbeitnowInstant {
    ArbeitnowInstant::new(1_791_115_200, 0).expect("in range")
}

fn pair(name: &str, value: &str) -> (Vec<u8>, Vec<u8>) {
    (name.as_bytes().to_vec(), value.as_bytes().to_vec())
}

fn page_body() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "data": [{
            "slug": "platform-engineer-example-berlin",
            "company_name": "Example Works GmbH",
            "title": "Platform Engineer",
            "description": "<p>Build things</p>",
            "url": "https://example.com/jobs/1",
            "remote": true,
            "tags": ["Rust"],
            "job_types": ["Full Time"],
            "location": "Berlin",
            "created_at": 1_760_000_000,
        }],
        "links": {"next": "https://www.arbeitnow.com/api/job-board-api?page=2"},
        "meta": {"current_page": 1, "last_page": 2},
    }))
    .expect("serialize")
}

fn answer(status: u16, headers: Vec<(Vec<u8>, Vec<u8>)>, body: Vec<u8>) -> BoardResponse {
    BoardResponse {
        status,
        headers,
        body,
    }
}

fn ok(body: Vec<u8>) -> BoardResponse {
    answer(200, vec![pair("content-type", "application/json")], body)
}

fn failure(response: BoardResponse) -> BoardFailure {
    admit_board_response(response, 1, None, now()).expect_err("must be refused")
}

#[test]
fn status_200_with_a_valid_body_is_a_page() {
    let page = admit_board_response(ok(page_body()), 1, None, now()).expect("page");
    assert_eq!(page.jobs.len(), 1);
    assert_eq!(page.jobs[0].company, "Example Works GmbH");
    assert!(page.another_page_may_remain);
}

#[test]
fn status_429_is_rate_limited_with_the_bounded_retry_after() {
    let failure = failure(answer(429, vec![pair("retry-after", "30")], Vec::new()));
    assert_eq!(failure.category(), BoardCategory::RateLimited);
    assert!(failure.retryable());
    assert_eq!(failure.retry_after_seconds(), Some(30));
}

#[test]
fn status_429_with_an_oversized_retry_after_carries_none() {
    let failure = failure(answer(429, vec![pair("retry-after", "86401")], Vec::new()));
    assert_eq!(failure.category(), BoardCategory::RateLimited);
    assert_eq!(failure.retry_after_seconds(), None);
}

#[test]
fn status_404_is_a_non_retryable_http_status() {
    assert_eq!(
        failure(answer(404, Vec::new(), Vec::new())),
        BoardFailure::HttpStatus { retryable: false }
    );
}

#[test]
fn status_503_is_a_retryable_http_status() {
    assert_eq!(
        failure(answer(503, Vec::new(), Vec::new())),
        BoardFailure::HttpStatus { retryable: true }
    );
}

#[test]
fn a_status_outside_the_http_range_is_a_transport_failure() {
    for status in [0, 99, 600, 1000] {
        assert_eq!(
            failure(answer(status, Vec::new(), Vec::new())),
            BoardFailure::Transport
        );
    }
}

#[test]
fn a_non_200_answer_with_a_body_is_a_transport_failure() {
    assert_eq!(
        failure(answer(404, Vec::new(), b"x".to_vec())),
        BoardFailure::Transport
    );
}

#[test]
fn a_body_over_four_mebibytes_is_a_body_size_failure() {
    assert_eq!(
        failure(ok(vec![b' '; MAX_BODY_BYTES + 1])),
        BoardFailure::BodySize
    );
}

#[test]
fn a_declared_length_over_four_mebibytes_is_a_body_size_failure() {
    let declared = (MAX_BODY_BYTES + 1).to_string();
    let response = answer(
        200,
        vec![
            pair("content-type", "application/json"),
            pair("content-length", &declared),
        ],
        Vec::new(),
    );
    assert_eq!(failure(response), BoardFailure::BodySize);
}

#[test]
fn a_declared_length_that_differs_from_the_body_is_a_transport_failure() {
    let response = answer(
        200,
        vec![
            pair("content-type", "application/json"),
            pair("content-length", "5"),
        ],
        page_body(),
    );
    assert_eq!(failure(response), BoardFailure::Transport);
}

#[test]
fn a_non_json_media_type_is_a_content_type_failure() {
    let response = answer(200, vec![pair("content-type", "text/html")], page_body());
    assert_eq!(failure(response), BoardFailure::ContentType);
}

#[test]
fn an_invalid_body_is_a_json_or_schema_failure() {
    assert_eq!(failure(ok(b"{not json".to_vec())), BoardFailure::Json);
    assert_eq!(failure(ok(b"[]".to_vec())), BoardFailure::Schema);
}

#[test]
fn a_page_outside_the_valid_range_is_refused_before_anything_else() {
    let refused = admit_board_response(ok(page_body()), 0, None, now()).expect_err("page 0");
    assert_eq!(refused, BoardFailure::HttpStatus { retryable: false });
}

#[test]
fn transport_failures_map_to_the_core_categories() {
    let transport = TransportFailure {
        category: TransportCategory::Transport,
        retryable: true,
    };
    let body_size = TransportFailure {
        category: TransportCategory::BodySize,
        retryable: false,
    };
    let http_status = TransportFailure {
        category: TransportCategory::HttpStatus,
        retryable: false,
    };
    assert_eq!(map_transport_failure(transport), BoardFailure::Transport);
    assert_eq!(map_transport_failure(body_size), BoardFailure::BodySize);
    assert_eq!(
        map_transport_failure(http_status),
        BoardFailure::HttpStatus { retryable: false }
    );
    assert_eq!(
        admit_board_result(Err(transport), 1, None, now()).expect_err("transport"),
        BoardFailure::Transport
    );
    assert!(admit_board_result(Ok(ok(page_body())), 1, None, now()).is_ok());
}
