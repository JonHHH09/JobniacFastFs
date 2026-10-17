//! Keyset listing and reading of jobs: the list envelope, the signed cursor scope, the bounds of the
//! limit, and the identifier and not-found envelopes.

mod job_support;

use std::sync::Arc;

use jobniac_core::errors::error_envelope;
use jobniac_core::jobs::JobService;
use jobniac_core::pagination::{CursorCodec, CursorKey};
use serde_json::{Value, json};

use job_support::{CounterIds, MemoryJobs, MemoryUow, fixed_clock};

const SECRET: &str = "jobs-list-test-secret-0123456789abcdef";
const MISSING: &str = "99999999-9999-9999-9999-999999999999";

fn codec() -> CursorCodec {
    match CursorKey::from_secret(SECRET) {
        Ok(key) => CursorCodec::new(key),
        Err(error) => panic!("test secret is refused: {error}"),
    }
}

fn service(
    repo: &MemoryJobs,
    clock: &str,
    ids: &Arc<CounterIds>,
) -> JobService<MemoryUow, MemoryJobs> {
    JobService::with_ids(
        MemoryUow,
        repo.clone(),
        fixed_clock(clock),
        codec(),
        ids.clone(),
    )
}

/// Captures three jobs at three distinct instants and returns their identifiers in creation order.
async fn seed_three(repo: &MemoryJobs, ids: &Arc<CounterIds>) -> Vec<String> {
    let mut created = Vec::new();
    for (clock, text) in [
        ("2026-09-28T12:00:00Z", "First role"),
        ("2026-09-28T12:00:01Z", "Second role"),
        ("2026-09-28T12:00:02Z", "Third role"),
    ] {
        let result = service(repo, clock, ids)
            .add_job_from_text(&json!({ "text": text }))
            .await;
        match result {
            Ok(captured) => created.push(captured.job.job.id.to_string()),
            Err(error) => panic!("seed capture refused: {error}"),
        }
    }
    created
}

#[tokio::test]
async fn list_returns_the_envelope_with_the_jobs_scope() {
    let repo = MemoryJobs::new();
    let ids = Arc::new(CounterIds::default());
    seed_three(&repo, &ids).await;
    let page = match service(&repo, "2026-09-29T00:00:00Z", &ids)
        .list_jobs(None, None)
        .await
    {
        Ok(page) => page,
        Err(error) => panic!("list refused: {error}"),
    };
    assert_eq!(page.jobs.len(), 3);
    assert!(page.next_cursor.is_none());
    let lists = job_support::locked(&repo.store.lists).clone();
    let expected_fingerprint = codec()
        .page_request(None, None, "jobs", "all")
        .map(|request| request.fingerprint().to_owned());
    assert_eq!(
        lists,
        vec![(
            "jobs".to_owned(),
            expected_fingerprint.unwrap_or_default(),
            20,
            false
        )]
    );
}

#[tokio::test]
async fn a_follow_up_page_uses_the_signed_cursor_and_a_new_limit() {
    let repo = MemoryJobs::new();
    let ids = Arc::new(CounterIds::default());
    let created = seed_three(&repo, &ids).await;
    let first = match service(&repo, "2026-09-29T00:00:00Z", &ids)
        .list_jobs(Some(2), None)
        .await
    {
        Ok(page) => page,
        Err(error) => panic!("first page refused: {error}"),
    };
    assert_eq!(first.jobs.len(), 2);
    let Some(cursor) = first.next_cursor.clone() else {
        panic!("a full first page must have a next cursor");
    };
    let second = match service(&repo, "2026-09-29T00:00:00Z", &ids)
        .list_jobs(Some(7), Some(&cursor))
        .await
    {
        Ok(page) => page,
        Err(error) => panic!("second page refused: {error}"),
    };
    assert_eq!(second.jobs.len(), 1);
    assert_eq!(second.jobs[0].id.to_string(), created[2]);
    assert!(second.next_cursor.is_none());
    let lists = job_support::locked(&repo.store.lists).clone();
    assert_eq!(lists.last().map(|call| (call.2, call.3)), Some((7, true)));
}

#[tokio::test]
async fn a_malformed_cursor_is_refused_before_the_repository() {
    let repo = MemoryJobs::new();
    let ids = Arc::new(CounterIds::default());
    let error = match service(&repo, "2026-09-29T00:00:00Z", &ids)
        .list_jobs(None, Some("not-a-cursor"))
        .await
    {
        Ok(_) => panic!("a malformed cursor must be refused"),
        Err(error) => error,
    };
    assert_eq!(
        to_value(&error),
        json!({
            "code": "validation_error",
            "message": "Invalid pagination cursor",
            "details": { "field": "cursor", "reason": "malformed or does not match this request" }
        })
    );
    assert!(repo.take_calls().is_empty());
}

#[tokio::test]
async fn a_validly_signed_cursor_for_another_scope_is_refused() {
    let repo = MemoryJobs::new();
    let ids = Arc::new(CounterIds::default());
    let profile_codec = codec();
    let profile_request = match profile_codec.page_request(None, None, "profiles", "all") {
        Ok(request) => request,
        Err(error) => panic!("profile request refused: {error}"),
    };
    let stamp = match jobniac_core::clock::parse_instant("2026-09-28T12:00:00Z") {
        Ok(value) => value,
        Err(error) => panic!("instant refused: {error}"),
    };
    let foreign = match profile_request.next_cursor(stamp, stamp, uuid::Uuid::from_u128(1)) {
        Ok(cursor) => cursor,
        Err(error) => panic!("cursor refused: {error}"),
    };
    let error = match service(&repo, "2026-09-29T00:00:00Z", &ids)
        .list_jobs(None, Some(&foreign))
        .await
    {
        Ok(_) => panic!("a foreign cursor must be refused"),
        Err(error) => error,
    };
    assert_eq!(
        to_value(&error),
        json!({
            "code": "validation_error",
            "message": "Invalid pagination cursor",
            "details": { "field": "cursor", "reason": "malformed or does not match this request" }
        })
    );
    assert!(repo.take_calls().is_empty());
}

#[tokio::test]
async fn an_out_of_bounds_limit_is_refused_before_the_repository() {
    let repo = MemoryJobs::new();
    let ids = Arc::new(CounterIds::default());
    for limit in [0_i64, 101] {
        let error = match service(&repo, "2026-09-29T00:00:00Z", &ids)
            .list_jobs(Some(limit), None)
            .await
        {
            Ok(_) => panic!("limit {limit} must be refused"),
            Err(error) => error,
        };
        assert_eq!(
            to_value(&error),
            json!({
                "code": "validation_error",
                "message": "limit must be between 1 and 100",
                "details": {}
            })
        );
    }
    assert!(repo.take_calls().is_empty());
}

#[tokio::test]
async fn get_returns_the_aggregate_and_sanitizes_its_failures() {
    let repo = MemoryJobs::new();
    let ids = Arc::new(CounterIds::default());
    let created = seed_three(&repo, &ids).await;
    repo.take_calls();
    let job = match service(&repo, "2026-09-29T00:00:00Z", &ids)
        .get_job(&created[1].to_uppercase())
        .await
    {
        Ok(job) => job,
        Err(error) => panic!("get refused: {error}"),
    };
    assert_eq!(job.job.id.to_string(), created[1]);
    assert_eq!(repo.take_calls(), vec!["get_job"]);

    let malformed = match service(&repo, "2026-09-29T00:00:00Z", &ids)
        .get_job("25")
        .await
    {
        Ok(_) => panic!("a malformed identifier must be refused"),
        Err(error) => error,
    };
    assert_eq!(
        to_value(&malformed),
        json!({
            "code": "validation_error",
            "message": "Invalid job request",
            "details": { "field": "jobId", "reason": "must be a valid UUID" }
        })
    );
    assert!(repo.take_calls().is_empty());

    let missing = match service(&repo, "2026-09-29T00:00:00Z", &ids)
        .get_job(MISSING)
        .await
    {
        Ok(_) => panic!("a missing job must be reported"),
        Err(error) => error,
    };
    assert_eq!(
        to_value(&missing),
        json!({
            "code": "not_found",
            "message": format!("Job not found: {MISSING}"),
            "details": { "resource": "job", "jobId": MISSING }
        })
    );
    assert_eq!(repo.take_calls(), vec!["get_job"]);
}

fn to_value(error: &jobniac_core::errors::AppError) -> Value {
    match serde_json::to_value(error_envelope(error)) {
        Ok(value) => value,
        Err(failure) => panic!("envelope does not serialize: {failure}"),
    }
}
