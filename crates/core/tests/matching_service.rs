//! The matching use cases against their golden vectors.
//!
//! The vectors in `golden/matching-requests.json` come from one in-memory fixture. Each step records
//! the outcome (the result or the refusal), the repository calls in order and the page request. The
//! explicit tests below check the obligations that the vectors do not express directly.

mod matching_support;

use std::sync::{Arc, PoisonError};

use jobniac_core::clock::{FixedClock, parse_instant};
use jobniac_core::errors::AppError;
use jobniac_core::matching::{MatchAnalysisService, MatchingRepository};
use jobniac_core::pagination::CursorCodec;
use jobniac_core::pagination::CursorKey;
use serde_json::{Value, json};

use matching_support::{
    CURSOR_SECRET, FakeRepository, PRIVATE_SENTINEL, array, outcome_value, parse, text, uuid_of,
};

const REQUESTS: &str = include_str!("golden/matching-requests.json");
const NOW: &str = "2026-10-02T12:00:00.123456Z";
const PROFILE_ID: &str = "11111111-1111-4111-8111-111111111111";
const JOB_ID: &str = "22222222-2222-4222-8222-222222222222";
const OTHER_PROFILE: &str = "33333333-3333-4333-8333-333333333333";

fn codec() -> CursorCodec {
    match CursorKey::from_secret(CURSOR_SECRET) {
        Ok(key) => CursorCodec::new(key),
        Err(error) => panic!("test cursor key is refused: {error}"),
    }
}

fn service(repository: FakeRepository) -> MatchAnalysisService<FakeRepository> {
    let now = match parse_instant(NOW) {
        Ok(instant) => instant,
        Err(error) => panic!("test instant is refused: {error}"),
    };
    MatchAnalysisService::new(repository, codec(), Arc::new(FixedClock::new(now)))
}

/// Runs one step of a vector and returns its outcome, the repository calls and the page request.
async fn run_step<R: MatchingRepository>(
    service: &MatchAnalysisService<R>,
    method: &str,
    request: &Value,
) -> Value {
    match method {
        "analyze_job_match" => outcome_value(service.analyze_job_match(request).await),
        "analyze_all_job_matches" => outcome_value(service.analyze_all_job_matches(request).await),
        "get_match_report" => outcome_value(service.get_match_report(request).await),
        "list_match_reports" => outcome_value(service.list_match_reports(request).await),
        other => panic!("unknown service method in vector: {other}"),
    }
}

#[tokio::test]
async fn service_steps_match_the_golden_vectors() {
    let document = parse(REQUESTS);
    let fixture = &document["fixture"];
    let cases = array(&document, "cases");
    assert_eq!(cases.len(), 34, "the vector count drifted");
    for case in cases {
        let name = text(case, "name");
        let repository = FakeRepository::new(fixture);
        let state = repository.state();
        let service = service(repository);
        for step in array(case, "steps") {
            let method = text(step, "method");
            {
                let mut record = state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                record.calls.clear();
                record.page = None;
            }
            let outcome = run_step(&service, method, &step["request"]).await;
            assert_eq!(outcome, step["outcome"], "{name}: {method} outcome");
            let record = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let calls: Vec<Value> = record.calls.iter().map(|call| json!(call)).collect();
            assert_eq!(
                Value::Array(calls),
                step["repositoryCalls"],
                "{name}: {method} repository calls"
            );
            let page = record.page.as_ref().map(|page| {
                json!({
                    "scope": page.scope,
                    "limit": page.limit,
                    "fingerprint": page.fingerprint,
                })
            });
            assert_eq!(
                page.unwrap_or(Value::Null),
                step["page"],
                "{name}: {method} page request"
            );
        }
    }
}

#[tokio::test]
async fn a_single_analysis_reuses_the_immutable_report() {
    let repository = FakeRepository::new(&parse(REQUESTS)["fixture"]);
    let state = repository.state();
    let service = service(repository);
    let request = json!({"profileId": PROFILE_ID, "jobId": JOB_ID});
    let first = service.analyze_job_match(&request).await;
    let again = service.analyze_job_match(&request).await;
    assert_eq!(first, again);
    let Ok(view) = first else {
        panic!("single analysis refused");
    };
    assert!(!view.stale);
    {
        let record = state.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(
            record.saved.len(),
            1,
            "one report is stored for one identity"
        );
        let pair = (uuid_of(PROFILE_ID), uuid_of(JOB_ID));
        assert_eq!(
            record.single_calls,
            vec![pair, pair],
            "both calls name the profile and the job"
        );
    }
    let fetched = service
        .get_match_report(&json!({"id": view.report.id.to_string()}))
        .await;
    assert_eq!(fetched, Ok(view));
}

#[tokio::test]
async fn a_batch_loads_once_and_isolates_a_failed_pair() {
    let fixture = parse(REQUESTS)["fixture"].clone();
    let repository = FakeRepository::new(&fixture);
    let state = repository.state();
    let service = service(repository);
    let result = service
        .analyze_all_job_matches(&json!({"profileId": PROFILE_ID, "limit": 3}))
        .await;
    let Ok(batch) = result else {
        panic!("batch refused");
    };
    let loads = state
        .lock()
        .map(|record| {
            record
                .calls
                .iter()
                .filter(|call| **call == "load_batch")
                .count()
        })
        .unwrap_or(0);
    assert_eq!(loads, 1);
    let profiles = state
        .lock()
        .map(|record| record.batch_profiles.clone())
        .unwrap_or_default();
    assert_eq!(profiles, vec![uuid_of(PROFILE_ID)]);
    assert_eq!(batch.succeeded.len(), 2);
    assert_eq!(batch.failed.len(), 1);
    assert_eq!(
        batch.failed[0].job_id,
        "00000000-0000-4000-8000-000000000002"
    );
    assert_eq!(batch.failed[0].error, "analysis failed");
    assert!(batch.next_cursor.is_some(), "a full page carries a cursor");
}

#[tokio::test]
async fn a_persistence_failure_is_sanitized_and_later_pairs_continue() {
    let fixture = parse(REQUESTS)["fixture"].clone();
    let failing = uuid_of(JOB_ID);
    let repository = FakeRepository::new(&fixture).failing_save_for(failing);
    let service = service(repository);
    let result = service
        .analyze_all_job_matches(&json!({"profileId": PROFILE_ID}))
        .await;
    let Ok(batch) = result else {
        panic!("batch refused");
    };
    // Job one fails at save, job two is refused by the scorer (unsafe skill), job three succeeds.
    assert_eq!(batch.succeeded.len(), 1);
    assert_eq!(batch.failed.len(), 2);
    assert_eq!(batch.failed[0].job_id, JOB_ID);
    assert_eq!(
        batch.failed[1].job_id,
        "00000000-0000-4000-8000-000000000002"
    );
    // The failing save carries the sentinel; the batch must report only the fixed message.
    let rendered = format!("{batch:?}");
    assert!(
        !rendered.contains(PRIVATE_SENTINEL),
        "the save failure text reached the batch"
    );
}

#[tokio::test]
async fn an_unknown_report_is_a_domain_error_with_the_documented_prefix() {
    let repository = FakeRepository::new(&parse(REQUESTS)["fixture"]);
    let service = service(repository);
    let result = service
        .get_match_report(&json!({"id": "00000000-0000-0000-0000-000000000099"}))
        .await;
    match result {
        Err(jobniac_core::errors::AppError::DomainValue(message)) => {
            assert!(message.starts_with("match report not found:"), "{message}");
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[tokio::test]
async fn invalid_requests_load_and_write_nothing() {
    let invalid = [
        (
            "analyze_job_match",
            json!({"profileId": null, "jobId": JOB_ID}),
        ),
        (
            "analyze_job_match",
            json!({"profileId": PROFILE_ID, "jobId": "bad"}),
        ),
        (
            "analyze_all_job_matches",
            json!({"profileId": PROFILE_ID, "limit": true}),
        ),
        (
            "analyze_all_job_matches",
            json!({"profileId": PROFILE_ID, "limit": 101}),
        ),
        (
            "analyze_all_job_matches",
            json!({"profileId": PROFILE_ID, "limit": 0}),
        ),
        (
            "analyze_all_job_matches",
            json!({"profileId": PROFILE_ID, "cursor": "bad"}),
        ),
        ("get_match_report", json!({"id": "bad"})),
        ("list_match_reports", json!({"profileId": "bad"})),
        ("list_match_reports", json!({"jobId": JOB_ID, "limit": 0})),
    ];
    for (method, request) in invalid {
        let repository = FakeRepository::new(&parse(REQUESTS)["fixture"]);
        let state = repository.state();
        let service = service(repository);
        let outcome = run_step(&service, method, &request).await;
        assert!(
            outcome.get("error").is_some(),
            "{method} {request} accepted"
        );
        let calls = state
            .lock()
            .map(|record| record.calls.len())
            .unwrap_or(usize::MAX);
        assert_eq!(calls, 0, "{method} {request} reached the repository");
    }
}

#[tokio::test]
async fn a_bad_cursor_wins_over_a_bad_limit() {
    let repository = FakeRepository::new(&parse(REQUESTS)["fixture"]);
    let service = service(repository);
    let outcome = service
        .analyze_all_job_matches(&json!({"profileId": PROFILE_ID, "cursor": "bad", "limit": 0}))
        .await;
    match outcome {
        Err(jobniac_core::errors::AppError::Application(app)) => {
            assert_eq!(
                app.details().get("field").map(String::as_str),
                Some("cursor")
            );
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

/// Asserts that a use case refused a cursor on its `cursor` field (`invalid_cursor`).
fn assert_cursor_refused<T: std::fmt::Debug>(outcome: Result<T, AppError>) {
    match outcome {
        Err(AppError::Application(app)) => assert_eq!(
            app.details().get("field").map(String::as_str),
            Some("cursor"),
            "the refusal names the cursor field"
        ),
        other => panic!("cursor was not refused: {other:?}"),
    }
}

#[tokio::test]
async fn page_two_keeps_the_scope_and_fingerprint_of_page_one() {
    let repository = FakeRepository::new(&parse(REQUESTS)["fixture"]);
    let state = repository.state();
    let service = service(repository);
    let first = service
        .analyze_all_job_matches(&json!({"profileId": PROFILE_ID, "limit": 1}))
        .await;
    let Ok(first) = first else {
        panic!("first page refused");
    };
    let Some(cursor) = first.next_cursor.clone() else {
        panic!("first page of three jobs has no cursor");
    };
    let first_page = state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .page
        .clone();
    let second = service
        .analyze_all_job_matches(&json!({"profileId": PROFILE_ID, "limit": 1, "cursor": cursor}))
        .await;
    assert!(second.is_ok(), "second page refused: {second:?}");
    let second_page = state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .page
        .clone();
    let (Some(first_page), Some(second_page)) = (first_page, second_page) else {
        panic!("a page request was not recorded");
    };
    assert_eq!(first_page.scope, "analyze-all-job-matches");
    assert!(!first_page.has_cursor, "page one has no cursor");
    assert!(second_page.has_cursor, "page two decodes its cursor");
    assert_eq!(second_page.scope, first_page.scope);
    assert_eq!(second_page.fingerprint, first_page.fingerprint);
}

#[tokio::test]
async fn cursors_are_bound_to_their_scope_and_filter() {
    let repository = FakeRepository::new(&parse(REQUESTS)["fixture"]);
    let service = service(repository);
    let stored = service
        .analyze_job_match(&json!({"profileId": PROFILE_ID, "jobId": JOB_ID}))
        .await;
    assert!(stored.is_ok(), "the report is stored: {stored:?}");
    let history = service
        .list_match_reports(&json!({"profileId": PROFILE_ID, "limit": 1}))
        .await;
    let Ok(history) = history else {
        panic!("history refused");
    };
    let Some(history_cursor) = history.next_cursor else {
        panic!("history has no cursor");
    };
    let batch = service
        .analyze_all_job_matches(&json!({"profileId": PROFILE_ID, "limit": 1}))
        .await;
    let Ok(batch) = batch else {
        panic!("batch refused");
    };
    let Some(batch_cursor) = batch.next_cursor else {
        panic!("batch has no cursor");
    };

    // A cursor of one scope is refused by the other scope.
    assert_cursor_refused(
        service
            .analyze_all_job_matches(&json!({"profileId": PROFILE_ID, "cursor": history_cursor}))
            .await,
    );
    assert_cursor_refused(
        service
            .list_match_reports(&json!({"profileId": PROFILE_ID, "cursor": batch_cursor}))
            .await,
    );
    // A cursor is bound to the profile it was issued for, and a history cursor to its job filter.
    assert_cursor_refused(
        service
            .analyze_all_job_matches(&json!({"profileId": OTHER_PROFILE, "cursor": batch_cursor}))
            .await,
    );
    assert_cursor_refused(
        service
            .list_match_reports(&json!({"jobId": JOB_ID, "cursor": history_cursor}))
            .await,
    );
    assert_cursor_refused(
        service
            .list_match_reports(&json!({"profileId": OTHER_PROFILE, "cursor": history_cursor}))
            .await,
    );
    // The same filter accepts its own cursor.
    let own = service
        .list_match_reports(&json!({"profileId": PROFILE_ID, "cursor": history_cursor, "limit": 1}))
        .await;
    assert!(own.is_ok(), "the same filter accepts its cursor: {own:?}");
}

#[tokio::test]
async fn history_filters_reach_the_repository_as_given() {
    let repository = FakeRepository::new(&parse(REQUESTS)["fixture"]);
    let state = repository.state();
    let service = service(repository);
    for request in [
        json!({}),
        json!({"profileId": PROFILE_ID}),
        json!({"jobId": JOB_ID}),
        json!({"profileId": PROFILE_ID, "jobId": JOB_ID}),
    ] {
        let outcome = service.list_match_reports(&request).await;
        assert!(outcome.is_ok(), "{request} refused: {outcome:?}");
    }
    let (filters, page) = {
        let record = state.lock().unwrap_or_else(PoisonError::into_inner);
        (record.list_filters.clone(), record.page.clone())
    };
    let profile = uuid_of(PROFILE_ID);
    let job = uuid_of(JOB_ID);
    assert_eq!(
        filters,
        vec![
            (None, None),
            (Some(profile), None),
            (None, Some(job)),
            (Some(profile), Some(job)),
        ]
    );
    assert_eq!(
        page.map(|page| page.scope).as_deref(),
        Some("match-reports")
    );
}

#[test]
fn a_lone_surrogate_in_a_request_never_reaches_the_use_cases() {
    // A Rust string cannot hold a lone surrogate, so the request decoder refuses the
    // escape before any matching use case sees the request.
    assert!(serde_json::from_str::<Value>(r#"{"profileId": "\ud800"}"#).is_err());
    assert!(serde_json::from_str::<Value>(r#"{"profileId": "\udc00"}"#).is_err());
}
