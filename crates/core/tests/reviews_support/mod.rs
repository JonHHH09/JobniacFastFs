//! Shared helpers of the review tests: vector parsing, reports rebuilt from the golden records, a
//! stepped clock and an in-memory repository that fakes the review port.
//!
//! The fake returns the stored row on a resubmission (the original review with its creation time, the
//! stored disagreement with its status and linked issue), as the SQLite adapter does. Every
//! repository call is logged in the shape the golden vectors record, so the service steps can
//! assert which calls a request reached.

#![allow(dead_code)]

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::clock::{Clock, FixedClock, parse_instant, truncate_to_micros};
use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::matching::{
    MatchReport, Outcome, validate_match_component, validate_match_evidence,
};
use jobniac_core::pagination::{CursorCodec, CursorKey, Page, PageRequest};
use jobniac_core::reviews::{
    DisagreementFactory, DisagreementStatus, MatchDisagreement, MatchReview, MatchReviewRepository,
    MatchReviewService, ReviewFactory, ReviewSubmission,
};
use serde::Serialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

/// The cursor key of the golden vectors: 32 ASCII characters.
pub const CURSOR_SECRET: &str = "0123456789abcdef0123456789abcdef";

/// The synthetic report identifiers of the golden vectors, by letter.
pub const REPORT_LETTERS: [(&str, &str); 6] = [
    ("A", "11111111-1111-4111-8111-000000000001"),
    ("B", "11111111-1111-4111-8111-000000000002"),
    ("C", "11111111-1111-4111-8111-000000000003"),
    ("D", "11111111-1111-4111-8111-000000000004"),
    ("E", "11111111-1111-4111-8111-000000000005"),
    ("F", "11111111-1111-4111-8111-000000000006"),
];

/// Parses a golden vector document.
pub fn parse(text: &str) -> Value {
    match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => panic!("vector is not JSON: {error}"),
    }
}

/// The array of a required field of a vector object.
pub fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    match value[key].as_array() {
        Some(items) => items,
        None => panic!("missing array {key} in {value}"),
    }
}

/// The text of a required string field of a vector object.
pub fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    match value[key].as_str() {
        Some(text) => text,
        None => panic!("missing string {key} in {value}"),
    }
}

/// The instant of a vector string.
pub fn instant(text: &str) -> OffsetDateTime {
    match parse_instant(text) {
        Ok(value) => value,
        Err(error) => panic!("vector instant is refused: {error}"),
    }
}

/// The identifier of a report letter of the golden.
pub fn report_id_of(letter: &str) -> Uuid {
    for (name, id) in REPORT_LETTERS {
        if name == letter {
            return uuid(id);
        }
    }
    panic!("unknown report letter {letter}")
}

/// A canonical UUID of a vector string.
pub fn uuid(text: &str) -> Uuid {
    match Uuid::parse_str(text) {
        Ok(value) => value,
        Err(error) => panic!("vector uuid is refused: {error}"),
    }
}

/// The request object of a vector draft.
pub fn object(value: &Value) -> &serde_json::Map<String, Value> {
    match value.as_object() {
        Some(object) => object,
        None => panic!("vector draft is not an object: {value}"),
    }
}

/// Rebuilds a synthetic report from its golden record. The components and evidence go through the
/// same validators the repository uses, so the typed report matches what the scorer would store.
pub fn report_from(value: &Value) -> MatchReport {
    let components = array(value, "components")
        .iter()
        .map(|item| match validate_match_component(item) {
            Ok(component) => component,
            Err(error) => panic!("vector component is refused: {error}"),
        })
        .collect();
    let evidence = array(value, "evidence")
        .iter()
        .map(|item| match validate_match_evidence(item) {
            Ok(evidence) => evidence,
            Err(error) => panic!("vector evidence is refused: {error}"),
        })
        .collect();
    let overall_score = match value["overallScore"].as_i64() {
        Some(score) => score,
        None => panic!("vector score is not an integer"),
    };
    MatchReport {
        id: uuid(text(value, "id")),
        profile_id: Uuid::nil(),
        job_id: Uuid::nil(),
        profile_revision: OffsetDateTime::UNIX_EPOCH,
        job_revision: OffsetDateTime::UNIX_EPOCH,
        algorithm_version: "deterministic-v1".to_owned(),
        overall_score,
        confidence: 0,
        outcome: outcome_of(text(value, "outcome")),
        blocker_mismatch: value["blockerMismatch"].as_bool().unwrap_or(false),
        components,
        evidence,
        created_at: OffsetDateTime::UNIX_EPOCH,
    }
}

/// The outcome named by its wire name (the test-side mirror of the stored text).
pub fn outcome_of(text: &str) -> Outcome {
    match text {
        "STRONG_MATCH" => Outcome::StrongMatch,
        "PARTIAL_MATCH" => Outcome::PartialMatch,
        "WEAK_MATCH" => Outcome::WeakMatch,
        "INSUFFICIENT_EVIDENCE" => Outcome::InsufficientEvidence,
        other => panic!("unknown outcome {other}"),
    }
}

/// The reports of a vector document, by identifier.
pub fn reports_of(document: &Value) -> Vec<MatchReport> {
    array(document, "reports").iter().map(report_from).collect()
}

/// The golden clock instant of a vector document.
pub fn now_of(document: &Value) -> OffsetDateTime {
    instant(text(document, "now"))
}

/// The JSON shape the vectors store for a result: `{"ok": ...}` or `{"error": envelope}`.
pub fn outcome_json<T: Serialize>(result: Result<T, AppError>) -> Value {
    match result {
        Ok(value) => json!({ "ok": value }),
        Err(error) => json!({ "error": error_envelope(&error) }),
    }
}

/// A codec with the golden cursor key.
pub fn codec() -> CursorCodec {
    match CursorKey::from_secret(CURSOR_SECRET) {
        Ok(key) => CursorCodec::new(key),
        Err(error) => panic!("golden cursor key is refused: {error:?}"),
    }
}

/// The service over the in-memory repository, with the golden clock.
pub fn service_over(
    repository: FakeRepository,
    now: OffsetDateTime,
) -> MatchReviewService<FakeRepository> {
    MatchReviewService::new(repository, codec(), Arc::new(FixedClock::new(now)))
}

/// The service over the in-memory repository, with a clock the test sets before each step.
pub fn stepped_service(
    repository: FakeRepository,
    clock: &Arc<SteppedClock>,
) -> MatchReviewService<FakeRepository> {
    let shared: Arc<SteppedClock> = Arc::clone(clock);
    MatchReviewService::new(repository, codec(), shared)
}

/// A clock the test moves forward between service steps, as the golden `now` per step.
#[derive(Debug)]
pub struct SteppedClock(Mutex<OffsetDateTime>);

impl SteppedClock {
    /// A clock at `instant`, truncated to microseconds.
    pub fn new(instant: OffsetDateTime) -> Self {
        Self(Mutex::new(truncate_to_micros(instant)))
    }

    /// Sets the instant the next service call reads.
    pub fn set(&self, instant: OffsetDateTime) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = truncate_to_micros(instant);
    }
}

impl Clock for SteppedClock {
    fn now(&self) -> OffsetDateTime {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The repository calls of a step, in the shape the golden vectors record them.
pub type CallLog = Arc<Mutex<Vec<Value>>>;

/// Takes the calls logged since the last take, leaving the log empty.
pub fn take_calls(log: &CallLog) -> Vec<Value> {
    std::mem::take(&mut *log.lock().unwrap_or_else(PoisonError::into_inner))
}

#[derive(Debug, Default)]
struct State {
    reviews: Vec<MatchReview>,
    disagreements: Vec<MatchDisagreement>,
}

/// In-memory repository whose rules match the adapter: reviews and disagreements are kept in insertion
/// order, a resubmission returns the stored row, and only the status and linked issue of a disagreement
/// change after insert.
#[derive(Debug)]
pub struct FakeRepository {
    reports: Vec<MatchReport>,
    state: Mutex<State>,
    calls: CallLog,
}

impl FakeRepository {
    /// A repository over the reports of the golden.
    pub fn new(reports: Vec<MatchReport>) -> Self {
        Self {
            reports,
            state: Mutex::new(State::default()),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The log of repository calls, shared with the test before the repository moves into a service.
    pub fn log(&self) -> CallLog {
        Arc::clone(&self.calls)
    }

    fn record(&self, call: Value) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
    }
}

#[async_trait]
impl MatchReviewRepository for FakeRepository {
    async fn submit_review(
        &self,
        report_id: Uuid,
        review_factory: &ReviewFactory<'_>,
        disagreement_factory: &DisagreementFactory<'_>,
    ) -> Result<ReviewSubmission, AppError> {
        self.record(json!({ "method": "submit_review", "reportId": report_id.to_string() }));
        let Some(report) = self.reports.iter().find(|report| report.id == report_id) else {
            return Err(AppError::DomainValue(format!(
                "match report not found: {report_id}"
            )));
        };
        let built = review_factory(report)?;
        let review = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            match state.reviews.iter().find(|stored| stored.id == built.id) {
                Some(stored) => stored.clone(),
                None => {
                    state.reviews.push(built.clone());
                    built
                }
            }
        };
        let disagreement = match disagreement_factory(report, &review) {
            None => None,
            Some(built) => {
                let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                match state
                    .disagreements
                    .iter()
                    .find(|stored| stored.id == built.id)
                {
                    Some(stored) => Some(stored.clone()),
                    None => {
                        state.disagreements.push(built.clone());
                        Some(built)
                    }
                }
            }
        };
        Ok(ReviewSubmission {
            review,
            disagreement,
        })
    }

    async fn get_review(&self, review_id: Uuid) -> Result<Option<MatchReview>, AppError> {
        self.record(json!({ "method": "get_review", "reviewId": review_id.to_string() }));
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(state
            .reviews
            .iter()
            .find(|review| review.id == review_id)
            .cloned())
    }

    async fn list_reviews(
        &self,
        report_id: Option<Uuid>,
        request: &PageRequest<'_>,
    ) -> Result<Page<MatchReview>, AppError> {
        self.record(json!({
            "method": "list_reviews",
            "reportId": report_id.map(|id| id.to_string()),
            "limit": request.limit(),
        }));
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let items = state
            .reviews
            .iter()
            .filter(|review| report_id.is_none_or(|id| review.report_id == id))
            .cloned()
            .collect();
        Ok(Page {
            items,
            next_cursor: None,
        })
    }

    async fn list_disagreements(
        &self,
        report_id: Option<Uuid>,
        request: &PageRequest<'_>,
    ) -> Result<Page<MatchDisagreement>, AppError> {
        self.record(json!({
            "method": "list_disagreements",
            "reportId": report_id.map(|id| id.to_string()),
            "limit": request.limit(),
        }));
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let items = state
            .disagreements
            .iter()
            .filter(|item| report_id.is_none_or(|id| item.report_id == id))
            .cloned()
            .collect();
        Ok(Page {
            items,
            next_cursor: None,
        })
    }

    async fn change_disagreement(
        &self,
        disagreement_id: Uuid,
        status: DisagreementStatus,
        issue: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<Option<MatchDisagreement>, AppError> {
        self.record(json!({
            "method": "change_disagreement",
            "disagreementId": disagreement_id.to_string(),
            "issue": issue,
        }));
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(current) = state
            .disagreements
            .iter_mut()
            .find(|item| item.id == disagreement_id)
        else {
            return Ok(None);
        };
        current.status = status;
        if let Some(issue) = issue {
            current.linear_issue_id = Some(issue.to_owned());
        }
        current.updated_at = truncate_to_micros(now);
        Ok(Some(current.clone()))
    }
}
