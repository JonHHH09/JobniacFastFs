//! Revision-checked update of jobs for the cases the golden scenarios do not reach: the revision at
//! the maximum, the lost compare-and-set race, the unchanged own fingerprint after a clearing edit,
//! and the request validation that never reaches the repository.

mod job_support;

use std::sync::Arc;

use jobniac_core::clock::parse_instant;
use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::jobs::{JobAggregate, JobService, LinkIngestionRecord, fingerprint};
use jobniac_core::pagination::{CursorCodec, CursorKey};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use job_support::{CounterIds, MemoryJobs, MemoryUow, fixed_clock};

const SECRET: &str = "jobs-update-test-secret-0123456789abcdef";

fn service(
    repo: &MemoryJobs,
    clock: &str,
    ids: &Arc<CounterIds>,
) -> JobService<MemoryUow, MemoryJobs> {
    let key = match CursorKey::from_secret(SECRET) {
        Ok(key) => key,
        Err(error) => panic!("test secret is refused: {error}"),
    };
    JobService::with_ids(
        MemoryUow,
        repo.clone(),
        fixed_clock(clock),
        CursorCodec::new(key),
        ids.clone(),
    )
}

async fn capture(repo: &MemoryJobs, ids: &Arc<CounterIds>, text: &str) -> JobAggregate {
    match service(repo, "2026-09-28T12:30:15.123Z", ids)
        .add_job_from_text(&json!({ "text": text }))
        .await
    {
        Ok(result) => result.job,
        Err(error) => panic!("seed capture refused: {error}"),
    }
}

fn envelope(error: &AppError) -> Value {
    match serde_json::to_value(error_envelope(error)) {
        Ok(value) => value,
        Err(failure) => panic!("envelope does not serialize: {failure}"),
    }
}

#[tokio::test]
async fn a_revision_at_the_maximum_is_a_conflict_that_writes_nothing() {
    let repo = MemoryJobs::new();
    let ids = Arc::new(CounterIds::default());
    let mut job = capture(&repo, &ids, "Maximal role\nBody").await;
    job.job.revision = i64::MAX;
    let job_id = job.job.id;
    repo.seed(job.clone());
    repo.take_calls();

    let error = match service(&repo, "2026-09-29T10:00:00Z", &ids)
        .update_job(&json!({
            "jobId": job_id.to_string(),
            "expectedRevision": i64::MAX,
            "title": "Overflow"
        }))
        .await
    {
        Ok(_) => panic!("the maximal revision must not be advanced"),
        Err(error) => error,
    };
    assert_eq!(
        envelope(&error),
        json!({
            "code": "conflict",
            "message": "Job revision conflict",
            "details": {
                "resource": "job",
                "jobId": job_id.to_string(),
                "expectedRevision": i64::MAX.to_string()
            }
        })
    );
    assert_eq!(
        repo.take_calls(),
        vec!["get_job", "find_by_canonical_fingerprint"]
    );
    assert_eq!(repo.stored(job_id), Some(job));
}

#[tokio::test]
async fn a_lost_compare_and_set_race_is_a_conflict_and_the_winner_is_kept() {
    let repo = MemoryJobs::new();
    let ids = Arc::new(CounterIds::default());
    let job = capture(&repo, &ids, "Raced role\nBody").await;
    let job_id = job.job.id;
    repo.lose_races(1);
    repo.take_calls();

    let error = match service(&repo, "2026-09-29T10:00:00Z", &ids)
        .update_job(&json!({ "jobId": job_id.to_string(), "expectedRevision": 0, "title": "Mine" }))
        .await
    {
        Ok(_) => panic!("a lost race must be a conflict"),
        Err(error) => error,
    };
    assert_eq!(envelope(&error)["code"], "conflict");
    assert_eq!(
        repo.take_calls(),
        vec![
            "get_job",
            "find_by_canonical_fingerprint",
            "update_job_aggregate"
        ]
    );
    let stored = match repo.stored(job_id) {
        Some(stored) => stored,
        None => panic!("the job must still exist"),
    };
    assert_eq!(stored.job.revision, 1);
    assert_eq!(stored.job.title, "Raced role");
}

#[tokio::test]
async fn an_unchanged_own_fingerprint_is_not_a_duplicate_after_a_clearing_edit() {
    let repo = MemoryJobs::new();
    let ids = Arc::new(CounterIds::default());
    let job = capture(&repo, &ids, "Clearable role\nSome body").await;
    let job_id = job.job.id;
    let updated = match service(&repo, "2026-09-29T10:00:00Z", &ids)
        .update_job(&json!({
            "jobId": job_id.to_string(),
            "expectedRevision": 0,
            "company": "   ",
            "seniority": ""
        }))
        .await
    {
        Ok(updated) => updated,
        Err(error) => panic!("update refused: {error}"),
    };
    assert_eq!(updated.job.revision, 1);
    assert_eq!(updated.job.company, None);
    assert_eq!(updated.job.seniority, None);
    assert_eq!(updated.job.created_at, job.job.created_at);
    let clock = match parse_instant("2026-09-29T10:00:00Z") {
        Ok(value) => value,
        Err(error) => panic!("instant refused: {error}"),
    };
    assert_eq!(updated.job.updated_at, clock);
    assert_eq!(updated.text_ingestion, job.text_ingestion);
}

#[tokio::test]
async fn an_invalid_request_is_refused_before_any_repository_call() {
    let repo = MemoryJobs::new();
    let ids = Arc::new(CounterIds::default());
    let cases = [
        (json!(null), "request", "must not be null"),
        (
            json!({ "expectedRevision": 0 }),
            "jobId",
            "must not be null",
        ),
        (
            json!({ "jobId": Uuid::nil().to_string(), "expectedRevision": 0, "title": " " }),
            "title",
            "must not be blank",
        ),
        (
            json!({ "jobId": Uuid::nil().to_string(), "expectedRevision": -1 }),
            "expectedRevision",
            "must not be negative",
        ),
        (
            json!({ "jobId": Uuid::nil().to_string(), "expectedRevision": 1.5 }),
            "expectedRevision",
            "must be an integer",
        ),
    ];
    for (request, field, reason) in cases {
        let error = match service(&repo, "2026-09-29T10:00:00Z", &ids)
            .update_job(&request)
            .await
        {
            Ok(_) => panic!("{request} must be refused"),
            Err(error) => error,
        };
        assert_eq!(
            envelope(&error),
            json!({
                "code": "validation_error",
                "message": "Invalid job request",
                "details": { "field": field, "reason": reason }
            })
        );
    }
    assert!(repo.take_calls().is_empty());
}

#[tokio::test]
async fn an_integer_beyond_the_64_bit_range_is_refused_with_the_golden_range_envelope_at_the_core()
{
    // Without serde_json's arbitrary_precision feature, out-of-range
    // integer literals are stored as floats. They are classified by sign and magnitude to give
    // these envelopes: "exceeds the supported revision range" above the 64-bit maximum and
    // "must not be negative" below the minimum. A fractional float is "must be an integer".
    let repo = MemoryJobs::new();
    let ids = Arc::new(CounterIds::default());
    let job_id = Uuid::nil().to_string();
    let cases = [
        (
            "9223372036854775808",
            "exceeds the supported revision range",
        ),
        (
            "18446744073709551616",
            "exceeds the supported revision range",
        ),
        (
            "1000000000000000000000000000000",
            "exceeds the supported revision range",
        ),
        ("-9223372036854775809", "must not be negative"),
        ("1.5", "must be an integer"),
    ];
    for (literal, reason) in cases {
        let revision: Value = match serde_json::from_str(literal) {
            Ok(value) => value,
            Err(error) => panic!("literal does not parse: {error}"),
        };
        let error = match service(&repo, "2026-09-29T10:00:00Z", &ids)
            .update_job(&json!({ "jobId": job_id, "expectedRevision": revision, "title": "Big" }))
            .await
        {
            Ok(_) => panic!("{literal} must be refused"),
            Err(error) => error,
        };
        assert_eq!(
            envelope(&error),
            json!({
                "code": "validation_error",
                "message": "Invalid job request",
                "details": { "field": "expectedRevision", "reason": reason }
            })
        );
    }
    assert!(repo.take_calls().is_empty());
}

#[tokio::test]
async fn a_link_sourced_job_keeps_its_provenance_and_fingerprints_with_the_stored_url() {
    let repo = MemoryJobs::new();
    let ids = Arc::new(CounterIds::default());
    let mut seeded = capture(&repo, &ids, "Linked role\nBody of the posting").await;
    let normalized_url = "https://example.com/jobs/linked-1";
    let link = LinkIngestionRecord {
        id: Uuid::from_u128(77),
        job_id: seeded.job.id,
        normalized_url: normalized_url.to_owned(),
        fields: Map::new(),
    };
    seeded.job.source_method = "link".to_owned();
    seeded.text_ingestion = None;
    seeded.link_ingestion = Some(link.clone());
    seeded.job.canonical_fingerprint = fingerprint(
        &seeded.job.title,
        None,
        None,
        &seeded.job.description,
        Some(normalized_url),
    );
    let job_id = seeded.job.id;
    repo.seed(seeded.clone());
    repo.take_calls();

    let updated = match service(&repo, "2026-09-29T10:00:00Z", &ids)
        .update_job(&json!({ "jobId": job_id.to_string(), "expectedRevision": 0, "title": "Renamed link role" }))
        .await
    {
        Ok(updated) => updated,
        Err(error) => panic!("link-sourced update refused: {error}"),
    };
    let expected_fingerprint = fingerprint(
        "Renamed link role",
        None,
        None,
        &seeded.job.description,
        Some(normalized_url),
    );
    assert_eq!(updated.job.canonical_fingerprint, expected_fingerprint);
    assert_eq!(updated.job.source_method, "link");
    assert_eq!(updated.link_ingestion, Some(link));
    assert_eq!(updated.text_ingestion, None);
    let stored = match repo.stored(job_id) {
        Some(stored) => stored,
        None => panic!("the job must still exist"),
    };
    assert_eq!(stored.job.canonical_fingerprint, expected_fingerprint);
    assert_eq!(stored.text_ingestion, None);
}
