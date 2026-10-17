//! The matching use cases: one pair analysis, a batch of one bounded page of jobs with per-pair
//! failure isolation, one report read and the keyset history.
//!
//! The requests arrive as JSON objects. Each use case reads its fields in a fixed order, so the first failing field and its message are deterministic.
//! Validation runs before any repository call, so an invalid request loads and writes nothing.

use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::clock::Clock;
use crate::errors::AppError;
use crate::ids::{UuidRefusal, admit_uuid};
use crate::pagination::{CursorCodec, MAX_LIMIT, PageRequest, invalid_cursor};

use super::graph::{JobGraph, ProfileGraph};
use super::report::{MatchReport, MatchView};
use super::repository::MatchingRepository;
use super::scorer::{ProfileSkillIndex, score_match, score_match_with_index};

/// The message of a pair that failed inside a batch. The cause is never reported.
pub const ANALYSIS_FAILED: &str = "analysis failed";

const REQUEST_MESSAGE: &str = "request must not be null";
const SCOPE_BATCH: &str = "analyze-all-job-matches";
const SCOPE_HISTORY: &str = "match-reports";

/// A job that could not be analyzed inside a batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairFailure {
    /// The profile of the batch, in canonical lowercase form.
    pub profile_id: String,
    /// The job identifier text as the repository returned it.
    pub job_id: String,
    /// Always [`ANALYSIS_FAILED`].
    pub error: &'static str,
}

/// The result of one batch page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchAnalysis {
    /// The analyzed jobs of the page, in page order.
    pub succeeded: Vec<MatchView>,
    /// The jobs of the page that failed, in page order.
    pub failed: Vec<PairFailure>,
    /// The cursor of the next page, if any.
    pub next_cursor: Option<String>,
}

/// One page of stored reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportList {
    /// The reports of the page, newest first.
    pub reports: Vec<MatchView>,
    /// The cursor of the next page, if any.
    pub next_cursor: Option<String>,
}

/// The matching use cases over a repository, a cursor codec and a clock.
#[derive(Debug)]
pub struct MatchAnalysisService<R> {
    repository: R,
    codec: CursorCodec,
    clock: Arc<dyn Clock>,
}

impl<R: MatchingRepository> MatchAnalysisService<R> {
    /// A service over `repository`, signing cursors with `codec` and stamping reports with `clock`.
    pub fn new(repository: R, codec: CursorCodec, clock: Arc<dyn Clock>) -> Self {
        Self {
            repository,
            codec,
            clock,
        }
    }

    /// Analyzes one profile and one job, or returns the report that the same identity already has.
    /// The result is never stale: the report is computed now.
    ///
    /// # Errors
    ///
    /// `DomainValue` for a missing or malformed `profileId` or `jobId`, a scorer refusal, or a
    /// repository error. Nothing is loaded or written for an invalid request.
    pub async fn analyze_job_match(&self, request: &Value) -> Result<MatchView, AppError> {
        let values = request_object(request)?;
        let profile_id = required_id(values.get("profileId"), "profileId")?;
        let job_id = required_id(values.get("jobId"), "jobId")?;
        let scorer = |profile: &ProfileGraph, job: &JobGraph| -> Result<MatchReport, AppError> {
            score_match(profile, job, self.clock.now())
        };
        let report = self
            .repository
            .analyze_pair(profile_id, job_id, &scorer)
            .await?;
        Ok(MatchView {
            report,
            stale: false,
        })
    }

    /// Analyzes one bounded page of a profile's jobs. The profile is loaded once. A job whose
    /// analysis or save fails is reported in `failed`, and the remaining jobs of the page continue.
    ///
    /// # Errors
    ///
    /// `DomainValue` for an invalid `profileId`, `limit` or the request object, the cursor error for
    /// a malformed or foreign cursor, or a repository error that loads the page.
    pub async fn analyze_all_job_matches(
        &self,
        request: &Value,
    ) -> Result<BatchAnalysis, AppError> {
        let values = request_object(request)?;
        let profile_id = required_id(values.get("profileId"), "profileId")?;
        let page_request = page_request_for(
            &self.codec,
            values,
            SCOPE_BATCH,
            &format!("profile={profile_id}"),
        )?;
        let (profile, page) = self
            .repository
            .load_batch(profile_id, &page_request)
            .await?;
        let mut succeeded = Vec::new();
        let mut failed = Vec::new();
        let index = ProfileSkillIndex::new(&profile);
        for job in &page.items {
            match self.score_and_save(&profile, &index, job).await {
                Ok(report) => succeeded.push(MatchView {
                    report,
                    stale: false,
                }),
                Err(_) => failed.push(PairFailure {
                    profile_id: profile_id.to_string(),
                    job_id: job.id.clone(),
                    error: ANALYSIS_FAILED,
                }),
            }
        }
        Ok(BatchAnalysis {
            succeeded,
            failed,
            next_cursor: page.next_cursor,
        })
    }

    /// One stored report and its staleness.
    ///
    /// # Errors
    ///
    /// `DomainValue` `reportId ...` for a missing or malformed `id`, `match report not found: <id>`
    /// when the report does not exist, or a repository error.
    pub async fn get_match_report(&self, request: &Value) -> Result<MatchView, AppError> {
        let report_id = required_id(request_object(request)?.get("id"), "reportId")?;
        self.repository
            .get_report(report_id)
            .await?
            .ok_or_else(|| AppError::DomainValue(format!("match report not found: {report_id}")))
    }

    /// One page of stored reports, optionally filtered by profile and job.
    ///
    /// # Errors
    ///
    /// `DomainValue` for an invalid `profileId`, `jobId` or `limit`, the cursor error for a malformed
    /// or foreign cursor, or a repository error.
    pub async fn list_match_reports(&self, request: &Value) -> Result<ReportList, AppError> {
        let values = request_object(request)?;
        let profile_id = optional_id(values.get("profileId"), "profileId")?;
        let job_id = optional_id(values.get("jobId"), "jobId")?;
        let profile_text = profile_id.map_or_else(|| "null".to_owned(), |id| id.to_string());
        let job_text = job_id.map_or_else(|| "null".to_owned(), |id| id.to_string());
        let page_request = page_request_for(
            &self.codec,
            values,
            SCOPE_HISTORY,
            &format!("profile={profile_text};job={job_text}"),
        )?;
        let page = self
            .repository
            .list_reports(profile_id, job_id, &page_request)
            .await?;
        Ok(ReportList {
            reports: page.items,
            next_cursor: page.next_cursor,
        })
    }

    async fn score_and_save(
        &self,
        profile: &ProfileGraph,
        index: &ProfileSkillIndex,
        job: &JobGraph,
    ) -> Result<MatchReport, AppError> {
        let report = score_match_with_index(profile, index, job, self.clock.now())?;
        self.repository.save_report(report).await
    }
}

/// The request object; any other JSON value is refused.
fn request_object(request: &Value) -> Result<&Map<String, Value>, AppError> {
    request
        .as_object()
        .ok_or_else(|| AppError::DomainValue(REQUEST_MESSAGE.to_owned()))
}

/// A required identifier: absent or null, a non-string, or a non-canonical UUID are refused with the
/// `DomainValue` messages `<field> must not be null` and `<field> must be a valid UUID`.
fn required_id(value: Option<&Value>, field: &str) -> Result<Uuid, AppError> {
    let text = match value {
        None | Some(Value::Null) => return Err(UuidRefusal::Null.domain_value(field)),
        Some(Value::String(text)) => text,
        Some(_) => return Err(UuidRefusal::Malformed.domain_value(field)),
    };
    admit_uuid(text).map_err(|refusal| refusal.domain_value(field))
}

fn optional_id(value: Option<&Value>, field: &str) -> Result<Option<Uuid>, AppError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(_) => required_id(value, field).map(Some),
    }
}

/// Validates the page request in this order: the cursor is decoded first (so a bad cursor wins
/// over a bad limit), then the limit is checked, then the scope and the filter fingerprint.
///
/// A limit must be a JSON integer. Booleans, strings and floats are refused.
fn page_request_for<'a>(
    codec: &'a CursorCodec,
    values: &Map<String, Value>,
    scope: &str,
    filter_identity: &str,
) -> Result<PageRequest<'a>, AppError> {
    let cursor = match values.get("cursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.as_str()),
        Some(_) => return Err(invalid_cursor()),
    };
    codec.decode(cursor)?;
    let limit = match values.get("limit") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => Some(number.as_i64().ok_or_else(limit_error)?),
        Some(_) => return Err(limit_error()),
    };
    codec.page_request(limit, cursor, scope, filter_identity)
}

fn limit_error() -> AppError {
    AppError::DomainValue(format!("limit must be between 1 and {MAX_LIMIT}"))
}
