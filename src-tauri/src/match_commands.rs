//! Matching and advisory-review command adapters.
//!
//! Each command takes the single-flight slot of its workspace, calls one core use case and maps the
//! result to a DTO. Validation of record stays in the core: the requests are the JSON
//! objects the use cases read, and a failure keeps its wire code and gets the copy of its operation.
//! The page size (20) is fixed here; the UI never chooses it.
//!
//! Every command that carries identifiers or text takes one `request_json` string, as `review_submit`
//! does. A lone surrogate escape is not valid JSON text, so it is refused here with the
//! operation's `validation_error` copy; a bare string argument would fail in Tauri's deserializer
//! before the command runs and lose its typed error.
//!
//! The MCP tools of the same operations are served by `jobniac-mcp` and are not affected by the
//! names of these commands.

use jobniac_core::errors::{AppError, ErrorCode, error_envelope};
use jobniac_runtime::services::Services;
use jobniac_text::str_rules::strip_text;
use serde_json::{Value, json};
use tauri::State;

use crate::error::CommandError;
use crate::job_commands::NOT_CONFIGURED;
use crate::match_dto::{
    DisagreementPageDto, DisagreementViewDto, MatchBatchPageDto, MatchCapabilitiesDto,
    MatchHistoryPageDto, MatchReportViewDto, OutOfRange, ReviewPageDto, ReviewSubmitResultDto,
    ReviewViewDto,
};
use crate::state::AppState;

/// Message id of a matching request while no local database is configured (the copy is the
/// unconfigured state of the Matching screen).
pub const MATCH_UNCONFIGURED: &str = "matching.unconfigured";
/// Message id of a matching failure whose code is `not_found` (the saved input is gone).
pub const MATCH_NOT_FOUND: &str = "matching.error.notFound";
/// Message id of every other matching failure, whatever the code (the copy names the IDs).
pub const MATCH_ERROR: &str = "matching.error.generic";
/// Message id of a review request while no local database is configured.
pub const REVIEW_UNCONFIGURED: &str = "reviews.unconfigured";
/// Message id of a review failure whose code is `not_found` (the draft is kept).
pub const REVIEW_NOT_FOUND: &str = "reviews.error.notFound";
/// Message id of every other review failure, whatever the code (the draft is kept).
pub const REVIEW_ERROR: &str = "reviews.error.generic";
/// The fixed page size of the batch pages and the report history (`limit: 20`).
pub const MATCH_PAGE_LIMIT: i64 = 20;
/// The fixed page size of the review and disagreement lists (`limit: 20`).
pub const REVIEW_PAGE_LIMIT: i64 = 20;

/// Refuses a request before any use case runs while no local database is configured. The
/// composition root keeps the services without a database, so the check is made
/// here, as the job commands do.
pub(crate) fn require_database(
    services: &Services,
    unconfigured: &str,
) -> Result<(), CommandError> {
    match services.database {
        Some(_) => Ok(()),
        None => Err(CommandError::new(NOT_CONFIGURED, unconfigured)),
    }
}

/// Maps a matching failure to the typed command error. The wire code is kept. Only a `not_found`
/// code has its own copy; every other failure has the generic copy
/// (a validation refusal of an ID is shown as a failed comparison, not as the input prompt).
fn matching_error(error: &AppError) -> CommandError {
    operation_error(error, MATCH_NOT_FOUND, MATCH_ERROR)
}

/// Maps a review failure to the typed command error, with the same rule as [`matching_error`].
fn review_error(error: &AppError) -> CommandError {
    operation_error(error, REVIEW_NOT_FOUND, REVIEW_ERROR)
}

fn operation_error(error: &AppError, not_found: &str, generic: &str) -> CommandError {
    let envelope = error_envelope(error);
    let message_id = match envelope.code {
        ErrorCode::NotFound => not_found,
        _ => generic,
    };
    CommandError::new(envelope.code.code(), message_id)
}

/// A value of the core that is outside its range becomes an internal error with the operation's
/// generic copy. The whole response is refused; no partial page is shown.
fn projection<T>(result: Result<T, OutOfRange>, generic: &str) -> Result<T, CommandError> {
    result.map_err(|_| CommandError::new("internal_error", generic))
}

/// Parses the request object of a command. Malformed JSON, a lone surrogate escape or a value
/// that is not an object is refused with the operation's generic copy.
pub(crate) fn parse_request(request_json: &str, generic: &str) -> Result<Value, CommandError> {
    match serde_json::from_str::<Value>(request_json) {
        Ok(value) if value.is_object() => Ok(value),
        _ => Err(CommandError::new("validation_error", generic)),
    }
}

/// A required text field of the request. A missing field or a value that is not text is refused.
pub(crate) fn text_field<'a>(
    request: &'a Value,
    key: &str,
    generic: &str,
) -> Result<&'a str, CommandError> {
    request
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::new("validation_error", generic))
}

/// An optional text field: absent or null is `None`, a text value is kept, any other type is refused.
fn optional_text_field<'a>(
    request: &'a Value,
    key: &str,
    generic: &str,
) -> Result<Option<&'a str>, CommandError> {
    match request.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.as_str())),
        Some(_) => Err(CommandError::new("validation_error", generic)),
    }
}

/// An optional text field as the presenters send it: a blank value is absent (`null`), otherwise
/// the stripped text. The core then reads the field as an ordinary identifier or label.
fn optional_text(value: Option<&str>) -> Option<&str> {
    value.map(strip_text).filter(|text| !text.is_empty())
}

/// The request of one batch page: the profile, the fixed page size and the cursor from the previous
/// page (absent for the first page).
fn page_request(profile_id: &str, cursor: Option<&str>) -> Value {
    json!({"profileId": profile_id, "limit": MATCH_PAGE_LIMIT, "cursor": cursor})
}

/// The request of one history page: the optional profile and job filters, the fixed page size and
/// the cursor. A filter that is blank is absent, so the history is not filtered by it.
fn history_request(profile_id: Option<&str>, job_id: Option<&str>, cursor: Option<&str>) -> Value {
    json!({
        "profileId": optional_text(profile_id),
        "jobId": optional_text(job_id),
        "limit": MATCH_PAGE_LIMIT,
        "cursor": cursor,
    })
}

/// The request of one review page, filtered by report when the filter is not blank.
fn review_page_request(report_id: Option<&str>, cursor: Option<&str>) -> Value {
    json!({"reportId": optional_text(report_id), "limit": REVIEW_PAGE_LIMIT, "cursor": cursor})
}

/// The acknowledge request. The issue reference is optional: a blank reference is sent as null, so
/// the stored reference is kept (a blank field is sent as `None`).
fn acknowledge_request(disagreement_id: &str, issue_reference: Option<&str>) -> Value {
    json!({
        "disagreementId": strip_text(disagreement_id),
        "linearIssueId": optional_text(issue_reference),
    })
}

/// The link request. The reference is required; a blank one is sent as-is, so the core refuses it
/// with the operation's copy.
fn link_request(disagreement_id: &str, issue_reference: &str) -> Value {
    json!({
        "disagreementId": strip_text(disagreement_id),
        "linearIssueId": strip_text(issue_reference),
    })
}

/// Parses the review request the UI sends. A lone surrogate escape is not valid JSON
/// text, so it is refused with the operation's copy.
fn parse_submission(request_json: &str) -> Result<Value, CommandError> {
    serde_json::from_str(request_json)
        .map_err(|_| CommandError::new("validation_error", REVIEW_ERROR))
}

/// Reports which comparison and review services are composed. Read-only: it touches no rows and
/// takes no single-flight slot, so the screens can ask it on mount.
#[tauri::command]
#[specta::specta]
pub async fn match_capabilities_get(
    state: State<'_, AppState>,
) -> Result<MatchCapabilitiesDto, CommandError> {
    run_capabilities(&state)
}

fn run_capabilities(state: &AppState) -> Result<MatchCapabilitiesDto, CommandError> {
    let services = state.services()?;
    let configured = services.database.is_some();
    Ok(MatchCapabilitiesDto {
        matching: configured,
        reviews: configured,
    })
}

/// Records whether the review draft holds unsaved edits, so that the window close gate can refuse
/// while they are not saved. React calls it only when the dirty flag changes.
#[tauri::command]
#[specta::specta]
pub async fn review_set_draft_dirty(
    state: State<'_, AppState>,
    dirty: bool,
) -> Result<(), CommandError> {
    run_set_draft_dirty(&state, dirty);
    Ok(())
}

fn run_set_draft_dirty(state: &AppState, dirty: bool) {
    state.set_review_draft_dirty(dirty);
}

/// Compares one saved profile with one saved job, or returns the report that the same identity has.
/// The request is `{ profileId, jobId }` as JSON text.
#[tauri::command]
#[specta::specta]
pub async fn match_analyze_pair(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<MatchReportViewDto, CommandError> {
    run_analyze_pair(&state, &request_json).await
}

async fn run_analyze_pair(
    state: &AppState,
    request_json: &str,
) -> Result<MatchReportViewDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, MATCH_ERROR)?;
    let profile_id = text_field(&request, "profileId", MATCH_ERROR)?;
    let job_id = text_field(&request, "jobId", MATCH_ERROR)?;
    let _slot = state.begin_matching()?;
    require_database(services, MATCH_UNCONFIGURED)?;
    let view = services
        .matching
        .analysis
        .analyze_job_match(&json!({"profileId": profile_id, "jobId": job_id}))
        .await
        .map_err(|error| matching_error(&error))?;
    projection(MatchReportViewDto::try_from(&view), MATCH_ERROR)
}

/// Compares one bounded page of a profile's saved jobs. A job that fails is listed in `failed`; the
/// rest of the page continues. Pages are never traversed automatically. The request is
/// `{ profileId, cursor? }` as JSON text.
#[tauri::command]
#[specta::specta]
pub async fn match_analyze_page(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<MatchBatchPageDto, CommandError> {
    run_analyze_page(&state, &request_json).await
}

async fn run_analyze_page(
    state: &AppState,
    request_json: &str,
) -> Result<MatchBatchPageDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, MATCH_ERROR)?;
    let profile_id = text_field(&request, "profileId", MATCH_ERROR)?;
    let cursor = optional_text_field(&request, "cursor", MATCH_ERROR)?;
    let _slot = state.begin_matching()?;
    require_database(services, MATCH_UNCONFIGURED)?;
    let batch = services
        .matching
        .analysis
        .analyze_all_job_matches(&page_request(profile_id, cursor))
        .await
        .map_err(|error| matching_error(&error))?;
    projection(MatchBatchPageDto::try_from(&batch), MATCH_ERROR)
}

/// One page of stored reports, newest first, optionally filtered by profile and by job. The request
/// is `{ profileId?, jobId?, cursor? }` as JSON text.
#[tauri::command]
#[specta::specta]
pub async fn match_list_reports(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<MatchHistoryPageDto, CommandError> {
    run_list_reports(&state, &request_json).await
}

async fn run_list_reports(
    state: &AppState,
    request_json: &str,
) -> Result<MatchHistoryPageDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, MATCH_ERROR)?;
    let profile_id = optional_text_field(&request, "profileId", MATCH_ERROR)?;
    let job_id = optional_text_field(&request, "jobId", MATCH_ERROR)?;
    let cursor = optional_text_field(&request, "cursor", MATCH_ERROR)?;
    let _slot = state.begin_matching()?;
    require_database(services, MATCH_UNCONFIGURED)?;
    let list = services
        .matching
        .analysis
        .list_match_reports(&history_request(profile_id, job_id, cursor))
        .await
        .map_err(|error| matching_error(&error))?;
    projection(MatchHistoryPageDto::try_from(&list), MATCH_ERROR)
}

/// One stored report by its identifier, with its staleness against the current source revisions.
/// The request is `{ id }` as JSON text.
#[tauri::command]
#[specta::specta]
pub async fn match_get_report(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<MatchReportViewDto, CommandError> {
    run_get_report(&state, &request_json).await
}

async fn run_get_report(
    state: &AppState,
    request_json: &str,
) -> Result<MatchReportViewDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, MATCH_ERROR)?;
    let id = text_field(&request, "id", MATCH_ERROR)?;
    let _slot = state.begin_matching()?;
    require_database(services, MATCH_UNCONFIGURED)?;
    let view = services
        .matching
        .analysis
        .get_match_report(&json!({"id": id}))
        .await
        .map_err(|error| matching_error(&error))?;
    projection(MatchReportViewDto::try_from(&view), MATCH_ERROR)
}

/// Stores an advisory review of a report and, when it diverges, its disagreement. The request is the
/// whole draft as a JSON object; Rust validates every field and sets the source type
/// of the evidence rows.
#[tauri::command]
#[specta::specta]
pub async fn review_submit(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<ReviewSubmitResultDto, CommandError> {
    let request = parse_submission(&request_json)?;
    run_submit_review(&state, &request).await
}

async fn run_submit_review(
    state: &AppState,
    request: &Value,
) -> Result<ReviewSubmitResultDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_review()?;
    require_database(services, REVIEW_UNCONFIGURED)?;
    let submission = services
        .matching
        .reviews
        .submit_match_review(request)
        .await
        .map_err(|error| review_error(&error))?;
    projection(ReviewSubmitResultDto::try_from(&submission), REVIEW_ERROR)
}

/// One saved review by its identifier. The request is `{ id }` as JSON text.
#[tauri::command]
#[specta::specta]
pub async fn review_get(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<ReviewViewDto, CommandError> {
    run_get_review(&state, &request_json).await
}

async fn run_get_review(
    state: &AppState,
    request_json: &str,
) -> Result<ReviewViewDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, REVIEW_ERROR)?;
    let id = text_field(&request, "id", REVIEW_ERROR)?;
    let _slot = state.begin_review()?;
    require_database(services, REVIEW_UNCONFIGURED)?;
    let review = services
        .matching
        .reviews
        .get_match_review(&json!({"id": id}))
        .await
        .map_err(|error| review_error(&error))?;
    projection(ReviewViewDto::try_from(&review), REVIEW_ERROR)
}

/// One page of saved reviews, newest first, optionally filtered by report. The request is
/// `{ reportId?, cursor? }` as JSON text.
#[tauri::command]
#[specta::specta]
pub async fn review_list(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<ReviewPageDto, CommandError> {
    run_list_reviews(&state, &request_json).await
}

async fn run_list_reviews(
    state: &AppState,
    request_json: &str,
) -> Result<ReviewPageDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, REVIEW_ERROR)?;
    let report_id = optional_text_field(&request, "reportId", REVIEW_ERROR)?;
    let cursor = optional_text_field(&request, "cursor", REVIEW_ERROR)?;
    let _slot = state.begin_review()?;
    require_database(services, REVIEW_UNCONFIGURED)?;
    let list = services
        .matching
        .reviews
        .list_match_reviews(&review_page_request(report_id, cursor))
        .await
        .map_err(|error| review_error(&error))?;
    projection(ReviewPageDto::try_from(&list), REVIEW_ERROR)
}

/// One page of disagreements, newest first, optionally filtered by report. The request is
/// `{ reportId?, cursor? }` as JSON text.
#[tauri::command]
#[specta::specta]
pub async fn review_list_disagreements(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<DisagreementPageDto, CommandError> {
    run_list_disagreements(&state, &request_json).await
}

async fn run_list_disagreements(
    state: &AppState,
    request_json: &str,
) -> Result<DisagreementPageDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, REVIEW_ERROR)?;
    let report_id = optional_text_field(&request, "reportId", REVIEW_ERROR)?;
    let cursor = optional_text_field(&request, "cursor", REVIEW_ERROR)?;
    let _slot = state.begin_review()?;
    require_database(services, REVIEW_UNCONFIGURED)?;
    let list = services
        .matching
        .reviews
        .list_match_disagreements(&review_page_request(report_id, cursor))
        .await
        .map_err(|error| review_error(&error))?;
    Ok(DisagreementPageDto::from(&list))
}

/// Marks a disagreement as acknowledged locally. The issue reference is optional; a blank one keeps
/// the stored reference. No external service is contacted. The request is
/// `{ disagreementId, issueReference? }` as JSON text.
#[tauri::command]
#[specta::specta]
pub async fn review_acknowledge_disagreement(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<DisagreementViewDto, CommandError> {
    run_acknowledge(&state, &request_json).await
}

async fn run_acknowledge(
    state: &AppState,
    request_json: &str,
) -> Result<DisagreementViewDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, REVIEW_ERROR)?;
    let disagreement_id = text_field(&request, "disagreementId", REVIEW_ERROR)?;
    let issue_reference = optional_text_field(&request, "issueReference", REVIEW_ERROR)?;
    let _slot = state.begin_review()?;
    require_database(services, REVIEW_UNCONFIGURED)?;
    let disagreement = services
        .matching
        .reviews
        .acknowledge_match_disagreement(&acknowledge_request(disagreement_id, issue_reference))
        .await
        .map_err(|error| review_error(&error))?;
    Ok(DisagreementViewDto::from(&disagreement))
}

/// Records an existing issue reference on a disagreement. The reference is stored as an opaque
/// value and is never looked up. The request is `{ disagreementId, issueReference }` as JSON text.
#[tauri::command]
#[specta::specta]
pub async fn review_link_disagreement(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<DisagreementViewDto, CommandError> {
    run_link(&state, &request_json).await
}

async fn run_link(
    state: &AppState,
    request_json: &str,
) -> Result<DisagreementViewDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, REVIEW_ERROR)?;
    let disagreement_id = text_field(&request, "disagreementId", REVIEW_ERROR)?;
    let issue_reference = text_field(&request, "issueReference", REVIEW_ERROR)?;
    let _slot = state.begin_review()?;
    require_database(services, REVIEW_UNCONFIGURED)?;
    let disagreement = services
        .matching
        .reviews
        .link_match_disagreement(&link_request(disagreement_id, issue_reference))
        .await
        .map_err(|error| review_error(&error))?;
    Ok(DisagreementViewDto::from(&disagreement))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{MATCH_BUSY, REVIEW_BUSY};
    use jobniac_core::env::Environment;
    use jobniac_core::errors::{ApplicationError, validation_error};
    use jobniac_runtime::services::compose;

    const PROFILE_ID: &str = "00000000-0000-4000-8000-0000000000a1";
    const JOB_ID: &str = "00000000-0000-4000-8000-0000000000b1";
    const REPORT_ID: &str = "00000000-0000-4000-8000-0000000000d1";

    fn state_without_database() -> AppState {
        AppState::new(compose(&Environment::default()))
    }

    fn application_error(code: ErrorCode) -> AppError {
        AppError::Application(ApplicationError::of(code))
    }

    /// The JSON text the UI sends for one request object.
    fn text(request: Value) -> String {
        request.to_string()
    }

    #[test]
    fn a_not_found_has_its_own_copy_and_everything_else_the_generic_copy() {
        assert_eq!(
            matching_error(&application_error(ErrorCode::NotFound)),
            CommandError::new("not_found", MATCH_NOT_FOUND)
        );
        assert_eq!(
            review_error(&application_error(ErrorCode::NotFound)),
            CommandError::new("not_found", REVIEW_NOT_FOUND)
        );
        // A missing report is a domain refusal (validation_error) in the core, so it
        // keeps the generic copy.
        let missing = AppError::DomainValue("match report not found: x".to_owned());
        assert_eq!(
            matching_error(&missing),
            CommandError::new("validation_error", MATCH_ERROR)
        );
        assert_eq!(
            review_error(&AppError::Internal),
            CommandError::new("internal_error", REVIEW_ERROR)
        );
        let refusal = AppError::Application(validation_error(
            "Invalid match request",
            Some("profileId"),
            Some("must be a valid UUID"),
        ));
        assert_eq!(
            matching_error(&refusal),
            CommandError::new("validation_error", MATCH_ERROR)
        );
    }

    #[test]
    fn a_page_request_carries_the_fixed_limit_and_the_cursor_or_null() {
        assert_eq!(
            page_request(PROFILE_ID, None),
            json!({"profileId": PROFILE_ID, "limit": 20, "cursor": null})
        );
        assert_eq!(page_request(PROFILE_ID, Some("c2"))["cursor"], "c2");
    }

    #[test]
    fn a_blank_history_filter_is_absent_and_a_filled_one_is_trimmed() {
        let padded_job = format!(" {JOB_ID} ");
        let request = history_request(Some("   "), Some(padded_job.as_str()), None);
        assert_eq!(request["profileId"], Value::Null);
        assert_eq!(request["jobId"], JOB_ID);
        assert_eq!(request["limit"], 20);
        assert_eq!(request["cursor"], Value::Null);
    }

    #[test]
    fn a_blank_acknowledge_reference_is_null_and_a_link_keeps_the_text() {
        assert_eq!(
            acknowledge_request(REPORT_ID, Some("  \t "))["linearIssueId"],
            Value::Null
        );
        assert_eq!(
            acknowledge_request(REPORT_ID, None)["linearIssueId"],
            Value::Null
        );
        assert_eq!(
            acknowledge_request(REPORT_ID, Some(" SYN-7 "))["linearIssueId"],
            "SYN-7"
        );
        let link = link_request(&format!(" {REPORT_ID} "), " SYN-8 ");
        assert_eq!(link["disagreementId"], REPORT_ID);
        assert_eq!(link["linearIssueId"], "SYN-8");
    }

    #[test]
    fn a_review_list_request_is_filtered_by_report_when_given() {
        let request = review_page_request(Some(REPORT_ID), Some("c3"));
        assert_eq!(request["reportId"], REPORT_ID);
        assert_eq!(request["limit"], 20);
        assert_eq!(request["cursor"], "c3");
        assert_eq!(review_page_request(Some(""), None)["reportId"], Value::Null);
    }

    #[test]
    fn a_malformed_or_lone_surrogate_submission_is_refused_with_the_review_copy() {
        assert_eq!(
            parse_submission(r#"{"reviewer":"\ud800"}"#).err(),
            Some(CommandError::new("validation_error", REVIEW_ERROR))
        );
        assert!(parse_submission("{").is_err());
        assert_eq!(
            parse_submission(r#"{"reviewer":"synthetic"}"#).ok(),
            Some(json!({"reviewer": "synthetic"}))
        );
    }

    #[test]
    fn a_request_must_be_an_object_and_its_text_fields_must_be_text() {
        assert_eq!(
            parse_request("[]", MATCH_ERROR).err(),
            Some(CommandError::new("validation_error", MATCH_ERROR))
        );
        // A lone surrogate in an identifier is refused with the operation copy, not a plain error.
        let lone = parse_request(r#"{"profileId":"\ud800","jobId":"x"}"#, MATCH_ERROR);
        assert_eq!(
            lone.err(),
            Some(CommandError::new("validation_error", MATCH_ERROR))
        );
        let request = json!({"profileId": PROFILE_ID, "cursor": null, "jobId": 7});
        assert_eq!(
            text_field(&request, "profileId", MATCH_ERROR).ok(),
            Some(PROFILE_ID)
        );
        assert_eq!(
            text_field(&request, "jobId", MATCH_ERROR).err(),
            Some(CommandError::new("validation_error", MATCH_ERROR))
        );
        assert_eq!(
            optional_text_field(&request, "cursor", MATCH_ERROR).ok(),
            Some(None)
        );
        assert_eq!(
            optional_text_field(&request, "jobId", MATCH_ERROR).err(),
            Some(CommandError::new("validation_error", MATCH_ERROR))
        );
    }

    #[test]
    fn a_projection_outside_its_range_is_an_internal_error_with_the_generic_copy() {
        assert_eq!(
            projection::<u32>(Err(OutOfRange), REVIEW_ERROR).err(),
            Some(CommandError::new("internal_error", REVIEW_ERROR))
        );
        assert_eq!(
            projection(Ok::<u32, OutOfRange>(3), REVIEW_ERROR).ok(),
            Some(3)
        );
    }

    #[tokio::test]
    async fn every_matching_command_reports_not_configured_without_a_database() {
        let state = state_without_database();
        let unconfigured = CommandError::new(NOT_CONFIGURED, MATCH_UNCONFIGURED);
        let pair = text(json!({"profileId": PROFILE_ID, "jobId": JOB_ID}));
        assert_eq!(
            run_analyze_pair(&state, &pair).await.err(),
            Some(unconfigured.clone())
        );
        let page = text(json!({"profileId": PROFILE_ID}));
        assert_eq!(
            run_analyze_page(&state, &page).await.err(),
            Some(unconfigured.clone())
        );
        assert_eq!(
            run_list_reports(&state, "{}").await.err(),
            Some(unconfigured.clone())
        );
        let get = text(json!({"id": REPORT_ID}));
        assert_eq!(run_get_report(&state, &get).await.err(), Some(unconfigured));
    }

    #[tokio::test]
    async fn every_review_command_reports_not_configured_without_a_database() {
        let state = state_without_database();
        let unconfigured = CommandError::new(NOT_CONFIGURED, REVIEW_UNCONFIGURED);
        assert_eq!(
            run_submit_review(&state, &json!({"reportId": REPORT_ID}))
                .await
                .err(),
            Some(unconfigured.clone())
        );
        let by_id = text(json!({"id": REPORT_ID}));
        assert_eq!(
            run_get_review(&state, &by_id).await.err(),
            Some(unconfigured.clone())
        );
        assert_eq!(
            run_list_reviews(&state, "{}").await.err(),
            Some(unconfigured.clone())
        );
        assert_eq!(
            run_list_disagreements(&state, "{}").await.err(),
            Some(unconfigured.clone())
        );
        let acknowledge = text(json!({"disagreementId": REPORT_ID}));
        assert_eq!(
            run_acknowledge(&state, &acknowledge).await.err(),
            Some(unconfigured.clone())
        );
        let link = text(json!({"disagreementId": REPORT_ID, "issueReference": "SYN-1"}));
        assert_eq!(run_link(&state, &link).await.err(), Some(unconfigured));
    }

    #[tokio::test]
    async fn a_request_missing_its_identifier_is_refused_before_the_database_is_asked() {
        let state = state_without_database();
        assert_eq!(
            run_analyze_pair(&state, "{}").await.err(),
            Some(CommandError::new("validation_error", MATCH_ERROR))
        );
        assert_eq!(
            run_link(&state, r#"{"disagreementId":"x"}"#).await.err(),
            Some(CommandError::new("validation_error", REVIEW_ERROR))
        );
    }

    #[tokio::test]
    async fn a_request_while_the_workspace_runs_is_refused_as_busy() {
        let state = state_without_database();
        let _held = state.begin_matching();
        let pair = text(json!({"profileId": PROFILE_ID, "jobId": JOB_ID}));
        assert_eq!(
            run_analyze_pair(&state, &pair).await.err(),
            Some(CommandError::busy(MATCH_BUSY))
        );
        assert_eq!(
            run_list_reports(&state, "{}").await.err(),
            Some(CommandError::busy(MATCH_BUSY))
        );
        // The review workspace has its own slot.
        assert_eq!(
            run_list_reviews(&state, "{}").await.err(),
            Some(CommandError::new(NOT_CONFIGURED, REVIEW_UNCONFIGURED))
        );
    }

    #[tokio::test]
    async fn a_review_request_while_one_runs_is_refused_as_busy() {
        let state = state_without_database();
        let _held = state.begin_review();
        assert_eq!(
            run_list_disagreements(&state, "{}").await.err(),
            Some(CommandError::busy(REVIEW_BUSY))
        );
    }

    #[tokio::test]
    async fn startup_failure_is_returned_for_matching_and_review_commands() {
        let state = AppState::new(Err(jobniac_runtime::services::StartupError));
        let pair = text(json!({"profileId": PROFILE_ID, "jobId": JOB_ID}));
        assert_eq!(
            run_analyze_pair(&state, &pair).await.err(),
            Some(CommandError::startup_failed())
        );
        assert_eq!(
            run_submit_review(&state, &json!({})).await.err(),
            Some(CommandError::startup_failed())
        );
        assert_eq!(
            run_capabilities(&state).err(),
            Some(CommandError::startup_failed())
        );
    }

    #[test]
    fn the_capabilities_report_no_service_without_a_database() -> Result<(), CommandError> {
        let state = state_without_database();
        assert_eq!(
            run_capabilities(&state)?,
            MatchCapabilitiesDto {
                matching: false,
                reviews: false,
            }
        );
        Ok(())
    }

    #[test]
    fn the_draft_dirty_flag_reaches_the_close_gate() {
        let state = state_without_database();
        run_set_draft_dirty(&state, true);
        assert_eq!(
            state.close_refusal().map(|refusal| refusal.message_id),
            Some(crate::state::REVIEW_CLOSE)
        );
        run_set_draft_dirty(&state, false);
        assert_eq!(state.close_refusal(), None);
    }
}
