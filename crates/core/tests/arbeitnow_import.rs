//! Trusted Arbeitnow import: verified token, no refetch, reuse and provenance.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jobniac_core::arbeitnow::candidates::{ArbeitnowCandidate, CandidateCodec};
use jobniac_core::arbeitnow::import::{
    ArbeitnowJobImportService, ImportRequest, ImportStatus, TrustedImportRepository,
};
use jobniac_core::arbeitnow::instant::ArbeitnowInstant;
use jobniac_core::clock::FixedClock;
use jobniac_core::errors::{ApplicationError, ErrorCode};
use jobniac_core::jobs::model::{IdSource, JobAggregate};
use time::OffsetDateTime;
use uuid::Uuid;

const URL: &str = "https://arbeitnow.com/view/test-job";
const INVALID: &str = "Invalid Arbeitnow candidate token";
/// 2026-10-09T12:00:00Z.
const NOW_SECOND: i64 = 1_791_547_200;
/// Largest supported datetime second (9999-12-31T23:59:59Z) and the smallest (0001-01-01T00:00:00Z).
const MAX_POSTED: i64 = 253_402_300_799;
const MIN_POSTED: i64 = -62_135_596_800;

fn key() -> Vec<u8> {
    vec![0; 32]
}

fn instant(second: i64, nano: u32) -> ArbeitnowInstant {
    ArbeitnowInstant::new(second, nano).expect("in range")
}

fn codec_at(second: i64, key: &[u8]) -> CandidateCodec {
    let now = instant(second, 0);
    CandidateCodec::with_instant_source(key, move || Ok(now)).expect("codec")
}

fn candidate() -> ArbeitnowCandidate {
    ArbeitnowCandidate {
        issued_at: instant(100, 0),
        expires_at: instant(1000, 0),
        slug: "test-job".to_owned(),
        canonical_url: URL.to_owned(),
        company: " Company ".to_owned(),
        title: " Platform Engineer ".to_owned(),
        location: "Berlin".to_owned(),
        remote: true,
        posted_at: Some(instant(1_760_000_000, 0)),
        tags: ["Python", "sql, Docker", "python"]
            .map(str::to_owned)
            .to_vec(),
        job_types: ["full_time", "contract"].map(str::to_owned).to_vec(),
        description_excerpt: " Build things ".to_owned(),
    }
}

#[derive(Debug, Default)]
struct Counter(AtomicU64);

impl IdSource for Counter {
    fn next_id(&self) -> Uuid {
        Uuid::from_u128(u128::from(self.0.fetch_add(1, Ordering::SeqCst) + 1))
    }
}

#[derive(Default)]
struct State {
    by_url: Vec<(String, JobAggregate)>,
    by_fingerprint: Vec<(String, JobAggregate)>,
    calls: Vec<String>,
    fail_find: bool,
    fail_create: bool,
}

#[derive(Clone, Default)]
struct FakeRepository(Arc<Mutex<State>>);

impl FakeRepository {
    fn calls(&self) -> Vec<String> {
        self.0.lock().expect("lock").calls.clone()
    }
}

#[async_trait]
impl TrustedImportRepository for FakeRepository {
    async fn find_by_normalized_url(
        &self,
        url: &str,
    ) -> Result<Option<JobAggregate>, ApplicationError> {
        let mut state = self.0.lock().expect("lock");
        state.calls.push(format!("find {url}"));
        if state.fail_find {
            return Err(ApplicationError::with_message(
                ErrorCode::InternalError,
                "find failed",
            ));
        }
        Ok(state
            .by_url
            .iter()
            .find(|(stored, _)| stored == url)
            .map(|(_, graph)| graph.clone()))
    }

    async fn create_link_job(
        &self,
        graph: JobAggregate,
    ) -> Result<(JobAggregate, bool), ApplicationError> {
        let mut state = self.0.lock().expect("lock");
        state.calls.push("create".to_owned());
        if state.fail_create {
            return Err(ApplicationError::with_message(
                ErrorCode::Conflict,
                "create failed",
            ));
        }
        let fingerprint = graph.job.canonical_fingerprint.clone();
        if let Some((_, existing)) = state
            .by_fingerprint
            .iter()
            .find(|(stored, _)| *stored == fingerprint)
        {
            return Ok((existing.clone(), false));
        }
        let url = graph
            .link_ingestion
            .as_ref()
            .expect("link")
            .normalized_url
            .clone();
        state.by_url.push((url, graph.clone()));
        state.by_fingerprint.push((fingerprint, graph.clone()));
        Ok((graph, true))
    }
}

/// An issuing codec at second 500 and a service whose verifier clock is at `verify_second`.
fn make(
    verify_second: i64,
) -> (
    CandidateCodec,
    FakeRepository,
    ArbeitnowJobImportService<FakeRepository>,
) {
    let repository = FakeRepository::default();
    let clock = Arc::new(FixedClock::new(
        OffsetDateTime::from_unix_timestamp(NOW_SECOND).expect("now"),
    ));
    let service = ArbeitnowJobImportService::with_ids(
        codec_at(verify_second, &key()),
        repository.clone(),
        clock,
        Arc::new(Counter::default()),
    );
    (codec_at(500, &key()), repository, service)
}

fn request(token: &str) -> ImportRequest {
    ImportRequest {
        candidate_token: Some(token.to_owned()),
    }
}

fn assert_invalid(error: &ApplicationError) {
    assert_eq!(error.code(), ErrorCode::ValidationError);
    assert_eq!(error.message(), INVALID);
    assert!(error.details().is_empty());
}

#[tokio::test]
async fn valid_import_maps_candidate_to_link_job_with_provenance() {
    let (codec, repository, service) = make(500);
    let token = codec.issue(&candidate()).expect("issue");
    let result = service.import(request(&token)).await.expect("import");
    assert_eq!(result.status, ImportStatus::CreatedJob);
    assert_eq!(result.status.as_str(), "created_job");
    let graph = result.job;
    let job = &graph.job;
    assert_eq!(job.source_method, "link");
    assert_eq!(job.source_label.as_deref(), Some("arbeitnow"));
    assert_eq!(job.title, "Platform Engineer");
    assert_eq!(job.company.as_deref(), Some("Company"));
    assert_eq!(job.location.as_deref(), Some("Berlin"));
    assert_eq!(job.description, "Build things");
    assert_eq!(job.employment_type.as_deref(), Some("full_time"));
    assert_eq!(job.experience_requirement, None);
    assert_eq!(job.seniority, None);
    assert_eq!(
        job.posted_at,
        Some(OffsetDateTime::from_unix_timestamp(1_760_000_000).expect("posted"))
    );
    let now = OffsetDateTime::from_unix_timestamp(NOW_SECOND).expect("now");
    assert_eq!(job.created_at, now);
    assert_eq!(job.updated_at, now);
    assert_eq!(job.revision, 0);
    assert_eq!(job.id, Uuid::from_u128(1));
    let skills: Vec<&str> = graph.skills.iter().map(|s| s.skill.as_str()).collect();
    assert_eq!(skills, ["Python", "sql", "Docker"]);
    let normalized: Vec<&str> = graph
        .skills
        .iter()
        .map(|s| s.normalized_skill.as_str())
        .collect();
    assert_eq!(normalized, ["python", "sql", "docker"]);
    let orders: Vec<i64> = graph.skills.iter().map(|s| s.display_order).collect();
    assert_eq!(orders, [0, 1, 2]);
    assert!(
        graph
            .skills
            .iter()
            .all(|s| s.required && s.job_id == job.id)
    );
    assert!(graph.skills.iter().all(|s| s.created_at == now));
    let ids: Vec<Uuid> = graph.skills.iter().map(|s| s.id).collect();
    assert_eq!(ids, [2, 3, 4].map(Uuid::from_u128));
    let link = graph.link_ingestion.expect("link");
    assert_eq!(link.id, Uuid::from_u128(5));
    assert_eq!(link.job_id, job.id);
    assert_eq!(link.normalized_url, URL);
    assert_eq!(link.fields["url"], URL);
    assert_eq!(link.fields["httpStatus"], serde_json::Value::Null);
    assert_eq!(link.fields["fetchedAt"], "2026-10-09T12:00:00Z");
    assert_eq!(link.fields["createdAt"], "2026-10-09T12:00:00Z");
    assert_eq!(
        link.fields["sourceTitle"],
        "Arbeitnow scan candidate (remote=true)"
    );
    assert!(graph.text_ingestion.is_none());
    assert_eq!(repository.calls(), [format!("find {URL}"), "create".into()]);
}

#[tokio::test]
async fn remote_false_and_missing_optionals() {
    let (codec, _repository, service) = make(500);
    let mut value = candidate();
    value.remote = false;
    value.posted_at = None;
    value.tags.clear();
    value.job_types.clear();
    value.company.clear();
    let token = codec.issue(&value).expect("issue");
    let graph = service.import(request(&token)).await.expect("import").job;
    assert_eq!(
        graph.link_ingestion.expect("link").fields["sourceTitle"],
        "Arbeitnow scan candidate (remote=false)"
    );
    assert_eq!(graph.job.posted_at, None);
    assert_eq!(graph.job.employment_type, None);
    assert_eq!(graph.job.company, None);
    assert!(graph.skills.is_empty());
}

#[tokio::test]
async fn blank_employment_type_and_location_become_none() {
    let (codec, _repository, service) = make(500);
    let mut value = candidate();
    value.job_types = vec!["  ".to_owned(), "contract".to_owned()];
    value.location = "   ".to_owned();
    let token = codec.issue(&value).expect("issue");
    let graph = service.import(request(&token)).await.expect("import").job;
    assert_eq!(graph.job.employment_type, None);
    assert_eq!(graph.job.location, None);
}

#[tokio::test]
async fn reuse_by_url_does_not_create() {
    let (codec, repository, service) = make(500);
    let token = codec.issue(&candidate()).expect("issue");
    let first = service.import(request(&token)).await.expect("first");
    let second = service.import(request(&token)).await.expect("second");
    assert_eq!(second.status, ImportStatus::ReusedExistingJob);
    assert_eq!(second.status.as_str(), "reused_existing_job");
    assert_eq!(second.job, first.job);
    let find = format!("find {URL}");
    assert_eq!(repository.calls(), [find.clone(), "create".into(), find]);
}

#[tokio::test]
async fn reuse_by_fingerprint_when_repository_reports_existing() {
    let (codec, repository, service) = make(500);
    let token = codec.issue(&candidate()).expect("issue");
    let first = service.import(request(&token)).await.expect("first").job;
    repository.0.lock().expect("lock").by_url.clear();
    let again = codec.issue(&candidate()).expect("issue");
    let result = service.import(request(&again)).await.expect("second");
    assert_eq!(result.status, ImportStatus::ReusedExistingJob);
    assert_eq!(result.job, first);
    let kinds: Vec<String> = repository
        .calls()
        .iter()
        .map(|call| call.split(' ').next().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(kinds, ["find", "create", "find", "create"]);
}

#[tokio::test]
async fn normalized_url_is_looked_up_before_creating() {
    let (codec, repository, service) = make(500);
    let token = codec.issue(&candidate()).expect("issue");
    service.import(request(&token)).await.expect("import");
    assert_eq!(repository.calls()[0], format!("find {URL}"));
}

#[tokio::test]
async fn blank_or_missing_token_is_rejected_before_any_repository_access() {
    let (_codec, repository, service) = make(500);
    let requests = [
        ImportRequest::default(),
        request(""),
        request("   "),
        request(" \u{3000}\t\n"),
    ];
    for value in requests {
        let error = service.import(value).await.expect_err("refused");
        assert_eq!(error.code(), ErrorCode::ValidationError);
        assert_eq!(error.message(), "candidateToken is required");
        assert_eq!(error.details()["field"], "candidateToken");
        assert_eq!(error.details()["reason"], "must not be blank");
    }
    assert!(repository.calls().is_empty());
}

#[tokio::test]
async fn tampered_token_is_rejected_without_repository_access() {
    let (codec, repository, service) = make(500);
    let token = codec.issue(&candidate()).expect("issue");
    let (payload, signature) = token.split_once('.').expect("separator");
    let flipped = if payload.as_bytes()[5] == b'A' {
        'B'
    } else {
        'A'
    };
    let tampered = format!("{}{flipped}{}.{signature}", &payload[..5], &payload[6..]);
    for value in [tampered, format!("{token}x"), token.replace('.', "")] {
        let error = service.import(request(&value)).await.expect_err("refused");
        assert_invalid(&error);
    }
    assert!(repository.calls().is_empty());
}

#[tokio::test]
async fn expired_token_is_rejected() {
    let (codec, repository, service) = make(1500);
    let token = codec.issue(&candidate()).expect("issue");
    let error = service.import(request(&token)).await.expect_err("refused");
    assert_invalid(&error);
    assert!(repository.calls().is_empty());
}

#[tokio::test]
async fn wrong_key_is_sanitized() {
    let other = codec_at(500, &[1; 32]);
    let token = other.issue(&candidate()).expect("issue");
    let (_codec, repository, service) = make(500);
    let error = service.import(request(&token)).await.expect_err("refused");
    assert_invalid(&error);
    assert!(!format!("{error:?}{}", error.message()).contains(&token));
    assert!(repository.calls().is_empty());
}

#[tokio::test]
async fn posted_at_outside_golden_datetime_years_is_refused() {
    for second in [MAX_POSTED + 1, MIN_POSTED - 1] {
        let (codec, repository, service) = make(500);
        let mut value = candidate();
        value.posted_at = Some(instant(second, 0));
        let token = codec.issue(&value).expect("issue");
        let error = service.import(request(&token)).await.expect_err("refused");
        assert_invalid(&error);
        // The lookup by URL runs first; nothing is written.
        assert_eq!(repository.calls(), [format!("find {URL}")]);
    }
}

#[tokio::test]
async fn posted_at_at_the_golden_limits_is_kept() {
    let (codec, _repository, service) = make(500);
    let mut value = candidate();
    value.posted_at = Some(instant(MAX_POSTED, 0));
    let token = codec.issue(&value).expect("issue");
    let graph = service.import(request(&token)).await.expect("import").job;
    let posted = graph.job.posted_at.expect("posted");
    assert_eq!(posted.unix_timestamp(), MAX_POSTED);
    assert_eq!(posted.nanosecond(), 0);

    let (codec, _repository, service) = make(500);
    let mut value = candidate();
    value.posted_at = Some(instant(MIN_POSTED, 0));
    let token = codec.issue(&value).expect("issue");
    let graph = service.import(request(&token)).await.expect("import").job;
    assert_eq!(
        graph.job.posted_at.expect("posted").unix_timestamp(),
        MIN_POSTED
    );
}

#[tokio::test]
async fn repository_errors_propagate_unchanged() {
    let (codec, repository, service) = make(500);
    let token = codec.issue(&candidate()).expect("issue");
    repository.0.lock().expect("lock").fail_find = true;
    let error = service.import(request(&token)).await.expect_err("find");
    assert_eq!(error.code(), ErrorCode::InternalError);
    assert_eq!(error.message(), "find failed");

    {
        let mut state = repository.0.lock().expect("lock");
        state.fail_find = false;
        state.fail_create = true;
    }
    let error = service.import(request(&token)).await.expect_err("create");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.message(), "create failed");
}

#[tokio::test]
async fn provenance_url_equals_the_canonical_url() {
    let (codec, _repository, service) = make(500);
    let token = codec.issue(&candidate()).expect("issue");
    let graph = service.import(request(&token)).await.expect("import").job;
    let link = graph.link_ingestion.expect("link");
    assert_eq!(link.normalized_url, URL);
    assert_eq!(link.fields["url"], URL);
}
