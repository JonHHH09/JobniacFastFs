//! Arbeitnow board failure type: the failure categories and their names.

use jobniac_core::arbeitnow::failure::{
    BoardCategory, BoardFailure, RetryAfterSeconds, SAFE_MESSAGE,
};
use jobniac_core::errors::ErrorCode;

fn rate_limited(seconds: Option<u32>) -> BoardFailure {
    BoardFailure::RateLimited {
        retry_after: seconds.map(|s| RetryAfterSeconds::new(s).expect("in range")),
    }
}

#[test]
fn category_names_match_the_golden_vectors() {
    let names: Vec<_> = [
        BoardCategory::Transport,
        BoardCategory::HttpStatus,
        BoardCategory::RateLimited,
        BoardCategory::ContentType,
        BoardCategory::BodySize,
        BoardCategory::Json,
        BoardCategory::Schema,
        BoardCategory::Pagination,
    ]
    .iter()
    .map(|c| c.name())
    .collect();
    assert_eq!(
        names,
        [
            "transport",
            "http_status",
            "rate_limited",
            "content_type",
            "body_size",
            "json",
            "schema",
            "pagination"
        ]
    );
}

#[test]
fn retryable_matrix_and_code_mapping() {
    let cases = [
        (
            BoardFailure::Transport,
            true,
            ErrorCode::UpstreamUnavailable,
        ),
        (
            BoardFailure::HttpStatus { retryable: true },
            true,
            ErrorCode::UpstreamUnavailable,
        ),
        (
            BoardFailure::HttpStatus { retryable: false },
            false,
            ErrorCode::UpstreamUnavailable,
        ),
        (rate_limited(None), true, ErrorCode::UpstreamRateLimited),
        (
            BoardFailure::ContentType,
            false,
            ErrorCode::UpstreamInvalidResponse,
        ),
        (
            BoardFailure::BodySize,
            false,
            ErrorCode::UpstreamInvalidResponse,
        ),
        (
            BoardFailure::Json,
            false,
            ErrorCode::UpstreamInvalidResponse,
        ),
        (
            BoardFailure::Schema,
            false,
            ErrorCode::UpstreamInvalidResponse,
        ),
        (
            BoardFailure::Pagination,
            false,
            ErrorCode::UpstreamInvalidResponse,
        ),
    ];
    for (failure, retryable, code) in cases {
        assert_eq!(failure.retryable(), retryable, "{failure:?}");
        assert_eq!(failure.code(), code, "{failure:?}");
        assert_eq!(failure.safe_message(), SAFE_MESSAGE);
        assert_eq!(failure.to_string(), SAFE_MESSAGE);
    }
}

#[test]
fn details_are_exact_and_ordered() {
    let strings = |f: BoardFailure| -> Vec<(String, String)> { f.details() };
    let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
    assert_eq!(
        strings(BoardFailure::Transport),
        vec![
            pair("provider", "arbeitnow"),
            pair("failureCategory", "transport"),
            pair("retryable", "true")
        ]
    );
    assert_eq!(
        strings(BoardFailure::HttpStatus { retryable: false }),
        vec![
            pair("provider", "arbeitnow"),
            pair("failureCategory", "http_status"),
            pair("retryable", "false")
        ]
    );
    assert_eq!(
        strings(rate_limited(Some(86_400))),
        vec![
            pair("provider", "arbeitnow"),
            pair("failureCategory", "rate_limited"),
            pair("retryable", "true"),
            pair("retryAfterSeconds", "86400")
        ]
    );
    assert_eq!(rate_limited(Some(0)).retry_after_seconds(), Some(0));
    assert_eq!(rate_limited(None).retry_after_seconds(), None);
    assert_eq!(BoardFailure::Json.retry_after_seconds(), None);
}

#[test]
fn retry_after_range_is_enforced() {
    assert!(RetryAfterSeconds::new(0).is_some());
    assert!(RetryAfterSeconds::new(86_400).is_some());
    assert!(RetryAfterSeconds::new(86_401).is_none());
    assert!(RetryAfterSeconds::new(u32::MAX).is_none());
}

#[test]
fn application_error_carries_only_safe_text() {
    let error = rate_limited(Some(30)).to_application_error();
    assert_eq!(error.code(), ErrorCode::UpstreamRateLimited);
    assert_eq!(error.message(), SAFE_MESSAGE);
    let details: Vec<_> = error
        .details()
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        details,
        [
            ("provider", "arbeitnow"),
            ("failureCategory", "rate_limited"),
            ("retryable", "true"),
            ("retryAfterSeconds", "30")
        ]
    );
    let error = BoardFailure::Schema.to_application_error();
    assert_eq!(error.code(), ErrorCode::UpstreamInvalidResponse);
    assert_eq!(
        error.details().get("retryable").map(String::as_str),
        Some("false")
    );
}
