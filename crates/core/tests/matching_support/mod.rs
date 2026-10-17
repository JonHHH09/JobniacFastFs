//! Shared helpers of the matching tests: graph parsing from the golden vectors and an in-memory
//! repository that fakes the matching port.

#![allow(dead_code)]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use jobniac_core::clock::parse_instant;
use jobniac_core::errors::AppError;
use jobniac_core::matching::{
    JobGraph, MatchReport, MatchView, MatchingRepository, PairScorer, ProfileGraph, uuid_from_text,
};
use jobniac_core::pagination::{Page, PageRequest};
use serde_json::{Map, Value};
use time::OffsetDateTime;
use uuid::Uuid;

/// The cursor key of the golden vectors: 32 ASCII characters.
pub const CURSOR_SECRET: &str = "0123456789abcdef0123456789abcdef";

/// The fixed instant of the vector fixture (the fake signs its cursors with it too).
pub const FIXTURE_NOW: &str = "2026-10-02T12:00:00.123456Z";

/// The text of a required string field of a vector object.
pub fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    match value[key].as_str() {
        Some(text) => text,
        None => panic!("missing string {key} in {value}"),
    }
}

/// The array of a required field of a vector object.
pub fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    match value[key].as_array() {
        Some(items) => items,
        None => panic!("missing array {key} in {value}"),
    }
}

/// Skill names of a vector list. Entries are strings (the fixture form of the golden file) or the
/// `{"normalizedSkill": ...}` records of the upstream golden file.
pub fn skill_names(value: &Value) -> Vec<String> {
    let Some(items) = value.as_array() else {
        panic!("skills must be an array: {value}");
    };
    items
        .iter()
        .map(|item| match item.as_str() {
            Some(name) => name.to_owned(),
            None => text(item, "normalizedSkill").to_owned(),
        })
        .collect()
}

/// The profile graph of a vector input (`id`, `updatedAt`, `skills` and `hasExperience` or
/// `experiences`).
pub fn profile_graph(input: &Value) -> ProfileGraph {
    let has_experience = match input.get("hasExperience").and_then(Value::as_bool) {
        Some(flag) => flag,
        None => input["experiences"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
    };
    ProfileGraph {
        id: text(input, "id").to_owned(),
        updated_at: text(input, "updatedAt").to_owned(),
        skills: skill_names(&input["skills"]),
        has_experience,
    }
}

/// The job graph of a vector input.
pub fn job_graph(input: &Value) -> JobGraph {
    JobGraph {
        id: text(input, "id").to_owned(),
        updated_at: text(input, "updatedAt").to_owned(),
        experience_requirement: input["experienceRequirement"].as_str().map(str::to_owned),
        seniority: input["seniority"].as_str().map(str::to_owned),
        skills: skill_names(&input["skills"]),
    }
}

/// The refusal of a vector as the golden vectors record it: `DomainValue` with its message,
/// `Application` with code, message and details, or `Internal` for any other failure.
pub fn error_value(error: &AppError) -> Value {
    match error {
        AppError::DomainValue(message) => {
            serde_json::json!({"error": {"kind": "DomainValue", "message": message}})
        }
        AppError::Application(app) => serde_json::json!({
            "error": {
                "kind": "Application",
                "code": app.code().code(),
                "message": app.message(),
                "details": app.details(),
            }
        }),
        _ => serde_json::json!({"error": {"kind": "Internal"}}),
    }
}

/// A result as the vectors record it: `{"ok": value}` or the refusal of [`error_value`].
pub fn outcome_value<T: serde::Serialize>(result: Result<T, AppError>) -> Value {
    match result {
        Ok(value) => match serde_json::to_value(value) {
            Ok(json) => serde_json::json!({ "ok": json }),
            Err(error) => panic!("value does not serialize: {error}"),
        },
        Err(error) => error_value(&error),
    }
}

/// Parses a vector file that must be valid JSON.
pub fn parse(text: &str) -> Value {
    match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => panic!("vector does not parse: {error}"),
    }
}

/// The object of a vector field.
pub fn object<'a>(value: &'a Value, key: &str) -> &'a Map<String, Value> {
    match value[key].as_object() {
        Some(object) => object,
        None => panic!("missing object {key} in {value}"),
    }
}

/// Identifier of a canonical or alternatively spelled identifier text (test helper for vectors).
pub fn uuid_of(text: &str) -> Uuid {
    match uuid_from_text(text) {
        Some(value) => value,
        None => panic!("vector identifier is not a UUID: {text}"),
    }
}

/// The fixed instant of the fixture, as a timestamp.
pub fn fixture_instant() -> OffsetDateTime {
    match parse_instant(FIXTURE_NOW) {
        Ok(instant) => instant,
        Err(error) => panic!("fixture instant is refused: {error}"),
    }
}

/// What the fake recorded for the last page request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSnapshot {
    /// The scope the request was validated for.
    pub scope: String,
    /// The resolved page size.
    pub limit: usize,
    /// The filter fingerprint the request was validated for.
    pub fingerprint: String,
    /// Whether the request carried a decoded cursor (page two and later).
    pub has_cursor: bool,
}

/// The shared record of one fake repository.
#[derive(Debug, Default)]
pub struct FakeState {
    /// Repository calls in order, by port method name.
    pub calls: Vec<&'static str>,
    /// The page request of the last `load_batch` or `list_reports`.
    pub page: Option<PageSnapshot>,
    /// Stored reports, in insertion order (a report with an existing identity is not replaced).
    pub saved: Vec<MatchReport>,
    /// The (profile, job) identifiers of each `analyze_pair` call, in order.
    pub single_calls: Vec<(Uuid, Uuid)>,
    /// The profile identifier of each `load_batch` call, in order.
    pub batch_profiles: Vec<Uuid>,
    /// The (profile, job) filters of each `list_reports` call, in order.
    pub list_filters: Vec<(Option<Uuid>, Option<Uuid>)>,
}

/// In-memory stand-in for the matching port (a fake).
///
/// `analyze_pair` scores the job whose identifier it is given, as the golden fixture does. Pages end
/// with a signed cursor built by the request, as the golden fixture does. `fail_save_for` makes saving
/// the report of that job fail with an error that carries a private sentinel, to exercise the
/// per-pair isolation of a batch and the sanitisation of its failure.
#[derive(Debug)]
pub struct FakeRepository {
    profile: ProfileGraph,
    jobs: Vec<JobGraph>,
    state: Arc<Mutex<FakeState>>,
    fail_save_for: Option<Uuid>,
}

/// The private text of a failing save. A batch must never surface it.
pub const PRIVATE_SENTINEL: &str = "PRIVATE_DATABASE_CONTENT";

impl FakeRepository {
    /// A repository over the `profile` and `jobs` of a vector fixture.
    pub fn new(fixture: &Value) -> Self {
        Self {
            profile: profile_graph(&fixture["profile"]),
            jobs: array(fixture, "jobs").iter().map(job_graph).collect(),
            state: Arc::new(Mutex::new(FakeState::default())),
            fail_save_for: None,
        }
    }

    /// Makes saving the report of `job_id` fail.
    pub fn failing_save_for(mut self, job_id: Uuid) -> Self {
        self.fail_save_for = Some(job_id);
        self
    }

    /// The shared record, readable after the repository moves into a service.
    pub fn state(&self) -> Arc<Mutex<FakeState>> {
        Arc::clone(&self.state)
    }

    fn lock(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn store(&self, report: MatchReport) -> Result<MatchReport, AppError> {
        let mut state = self.lock();
        state.calls.push("save_report");
        if self.fail_save_for == Some(report.job_id) {
            return Err(AppError::DomainValue(PRIVATE_SENTINEL.to_owned()));
        }
        if let Some(existing) = state.saved.iter().find(|stored| stored.id == report.id) {
            return Ok(existing.clone());
        }
        state.saved.push(report.clone());
        Ok(report)
    }
}

/// The page snapshot of a validated request.
fn snapshot_of(request: &PageRequest<'_>) -> PageSnapshot {
    PageSnapshot {
        scope: request.scope().to_owned(),
        limit: request.limit(),
        fingerprint: request.fingerprint().to_owned(),
        has_cursor: request.cursor().is_some(),
    }
}

/// The signed cursor of a page that ends at `last`, or `None` for an empty page. The cursor is
/// built by the request, so it carries the request's scope and fingerprint.
fn cursor_after(request: &PageRequest<'_>, last: Option<Uuid>) -> Option<String> {
    let last = last?;
    let instant = fixture_instant();
    match request.next_cursor(instant, instant, last) {
        Ok(cursor) => Some(cursor),
        Err(error) => panic!("test cursor is refused: {error}"),
    }
}

#[async_trait]
impl MatchingRepository for FakeRepository {
    async fn analyze_pair(
        &self,
        profile_id: Uuid,
        job_id: Uuid,
        scorer: &PairScorer<'_>,
    ) -> Result<MatchReport, AppError> {
        {
            let mut state = self.lock();
            state.calls.push("analyze_pair");
            state.single_calls.push((profile_id, job_id));
        }
        let Some(job) = self.jobs.iter().find(|job| job.id == job_id.to_string()) else {
            return Err(AppError::DomainValue(format!(
                "job not in fixture: {job_id}"
            )));
        };
        let report = scorer(&self.profile, job)?;
        self.store(report)
    }

    async fn load_batch(
        &self,
        profile_id: Uuid,
        request: &PageRequest<'_>,
    ) -> Result<(ProfileGraph, Page<JobGraph>), AppError> {
        {
            let mut state = self.lock();
            state.calls.push("load_batch");
            state.batch_profiles.push(profile_id);
            state.page = Some(snapshot_of(request));
        }
        let last = self.jobs.last().map(|job| uuid_of(&job.id));
        Ok((
            self.profile.clone(),
            Page {
                items: self.jobs.clone(),
                next_cursor: cursor_after(request, last),
            },
        ))
    }

    async fn save_report(&self, report: MatchReport) -> Result<MatchReport, AppError> {
        self.store(report)
    }

    async fn get_report(&self, report_id: Uuid) -> Result<Option<MatchView>, AppError> {
        let mut state = self.lock();
        state.calls.push("get_report");
        Ok(state
            .saved
            .iter()
            .find(|stored| stored.id == report_id)
            .map(|stored| MatchView {
                report: stored.clone(),
                stale: false,
            }))
    }

    async fn list_reports(
        &self,
        profile_id: Option<Uuid>,
        job_id: Option<Uuid>,
        request: &PageRequest<'_>,
    ) -> Result<Page<MatchView>, AppError> {
        let mut state = self.lock();
        state.calls.push("list_reports");
        state.page = Some(snapshot_of(request));
        state.list_filters.push((profile_id, job_id));
        let items: Vec<MatchView> = state
            .saved
            .iter()
            .map(|stored| MatchView {
                report: stored.clone(),
                stale: false,
            })
            .collect();
        let next_cursor = cursor_after(request, state.saved.last().map(|stored| stored.id));
        Ok(Page { items, next_cursor })
    }
}
