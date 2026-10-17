//! Job command adapters. Each command takes the single-flight slot of the job
//! workspace, calls one core use case and maps the result to a DTO. Validation of record stays in the
//! core; this module only parses the boundary text, maps the editor's `skillsText` to the skill list
//! the core reads (one skill per line), and maps failures to copy ids.
//!
//! The search limit (20) and the list page size (20) are fixed here: the UI
//! never chooses them.

use jobniac_core::errors::{AppError, ErrorCode, error_envelope};
use jobniac_runtime::services::Services;
use jobniac_text::str_rules::{split_lines, strip_text};
use serde_json::{Map, Value};
use tauri::State;

use crate::error::CommandError;
use crate::job_dto::{JobCaptureDto, JobDetailDto, JobPageDto, JobSearchDto};
use crate::state::AppState;

/// Wire code of a request made while the job service has no local database configured.
pub const NOT_CONFIGURED: &str = "not_configured";
/// Message id of a job request while no local database is configured.
pub const JOB_UNCONFIGURED: &str = "jobs.unconfigured";
/// Message id of a blank posting text (the paste form's validation copy).
pub const JOB_PASTE_VALIDATION: &str = "job.validation.paste";
/// Message id of a blank or unsearchable search query (the search form's validation copy).
pub const JOB_SEARCH_VALIDATION: &str = "job.validation.search";
/// Message id of a blank posting date (the editor's validation copy).
pub const JOB_POSTED_AT_VALIDATION: &str = "job.validation.postedAt";
/// Message id of a failed capture (and of a blank or malformed paste).
pub const JOB_ADD_ERROR: &str = "job.error.add";
/// Message id of a failed saved-jobs list.
pub const JOB_LIST_ERROR: &str = "job.error.list";
/// Message id of a failed job load, and of a malformed job id, without a listed code.
pub const JOB_DETAIL_ERROR: &str = "job.error.detail";
/// Message id of a job load whose job no longer exists.
pub const JOB_DETAIL_MISSING: &str = "job.error.detailNotFound";
/// Message id of every failed search, whatever the code (the copy names the query and the database).
pub const JOB_SEARCH_ERROR: &str = "job.error.search";
/// Message id of a failed job update without a listed code.
pub const JOB_UPDATE_ERROR: &str = "job.error.update";
/// Message id of an update whose revision is stale (the edits are kept in the UI).
pub const JOB_CONFLICT: &str = "job.error.conflict";
/// The fixed page size of the saved-jobs list.
pub const JOB_LIST_LIMIT: i64 = 20;
/// The fixed limit of the job search.
pub const JOB_SEARCH_LIMIT: i64 = 20;

/// Refuses a job request before any use case runs while no local database is configured. The
/// composition root keeps the job service without a database, so the check is
/// made here, and the UI shows `jobs.unconfigured` and disables the controls.
fn require_database(services: &Services) -> Result<(), CommandError> {
    match services.database {
        Some(_) => Ok(()),
        None => Err(CommandError::new(NOT_CONFIGURED, JOB_UNCONFIGURED)),
    }
}

/// Maps a core failure to the typed command error. The wire code is kept. A validation refusal of
/// a named field has the copy of that field; every other
/// failure has the operation's copy.
fn job_error(error: &AppError, message_id: &str) -> CommandError {
    let envelope = error_envelope(error);
    let copy = match (
        &envelope.code,
        envelope.details.get("field").map(String::as_str),
    ) {
        (ErrorCode::ValidationError, Some("text")) => JOB_PASTE_VALIDATION,
        (ErrorCode::ValidationError, Some("query")) => JOB_SEARCH_VALIDATION,
        (ErrorCode::ValidationError, Some("postedAt")) => JOB_POSTED_AT_VALIDATION,
        _ => message_id,
    };
    CommandError::new(envelope.code.code(), copy)
}

/// A load failure: a missing job has its own copy, every other code the load copy.
fn detail_error(error: &AppError) -> CommandError {
    match error_envelope(error).code {
        ErrorCode::NotFound => CommandError::new(ErrorCode::NotFound.code(), JOB_DETAIL_MISSING),
        _ => job_error(error, JOB_DETAIL_ERROR),
    }
}

/// An update failure: a stale revision has its own copy, every other code the update copy.
fn update_error(error: &AppError) -> CommandError {
    match error_envelope(error).code {
        ErrorCode::Conflict => CommandError::new(ErrorCode::Conflict.code(), JOB_CONFLICT),
        _ => job_error(error, JOB_UPDATE_ERROR),
    }
}

/// Parses the request text the UI sends. A lone surrogate escape is not valid JSON
/// text, so it is refused here with the operation's validation copy.
fn parse_request(text: &str, message_id: &str) -> Result<Value, CommandError> {
    serde_json::from_str(text).map_err(|_| CommandError::new("validation_error", message_id))
}

/// Maps the editor's `skillsText` (raw text, one skill per line) to the `skills` list the core reads.
/// Lines are split on line breaks, stripped and empty lines dropped, so an empty text
/// clears the skills. An absent or null `skillsText` leaves the `skills` key alone (the stored skills
/// are kept).
fn map_skills_text(object: &mut Map<String, Value>) -> Result<(), CommandError> {
    let Some(text) = object.remove("skillsText") else {
        return Ok(());
    };
    match text {
        Value::Null => Ok(()),
        Value::String(text) => {
            let lines = split_lines(&text)
                .into_iter()
                .map(strip_text)
                .filter(|line| !line.is_empty())
                .map(|line| Value::String(line.to_owned()))
                .collect();
            object.insert("skills".to_owned(), Value::Array(lines));
            Ok(())
        }
        _ => Err(CommandError::new("validation_error", JOB_UPDATE_ERROR)),
    }
}

/// Parses the revision text: a non-empty run of ASCII digits, as a decimal integer. A sign, a space
/// or any other character is refused (`"+7"` is not the revision 7).
fn parse_revision(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>().ok()
}

/// The update request the core reads: the changed fields, the job id from the path, the revision as
/// an integer and the skills from `skillsText`. A request that is not an object is passed on
/// unchanged, so the core refuses it.
fn update_request(
    job_id: &str,
    expected_revision: &str,
    request: &Value,
) -> Result<Value, CommandError> {
    let revision = parse_revision(expected_revision)
        .ok_or_else(|| CommandError::new("validation_error", JOB_UPDATE_ERROR))?;
    let mut request = request.clone();
    if let Some(object) = request.as_object_mut() {
        map_skills_text(object)?;
        object.insert("jobId".to_owned(), Value::String(job_id.to_owned()));
        object.insert(
            "expectedRevision".to_owned(),
            Value::Number(revision.into()),
        );
    }
    Ok(request)
}

/// Captures pasted text as a job, or reuses the job that the text hash or the fingerprint identifies.
#[tauri::command]
#[specta::specta]
pub async fn add_job_from_text(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<JobCaptureDto, CommandError> {
    let request = parse_request(&request_json, JOB_ADD_ERROR)?;
    run_add_job_from_text(&state, &request).await
}

async fn run_add_job_from_text(
    state: &AppState,
    request: &Value,
) -> Result<JobCaptureDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_job()?;
    require_database(services)?;
    let result = services
        .jobs
        .service
        .add_job_from_text(request)
        .await
        .map_err(|error| job_error(&error, JOB_ADD_ERROR))?;
    Ok(JobCaptureDto::from(&result))
}

/// One page of the saved jobs, optionally from the cursor of the previous page.
#[tauri::command]
#[specta::specta]
pub async fn list_jobs(
    state: State<'_, AppState>,
    cursor: Option<String>,
) -> Result<JobPageDto, CommandError> {
    run_list_jobs(&state, cursor.as_deref()).await
}

async fn run_list_jobs(state: &AppState, cursor: Option<&str>) -> Result<JobPageDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_job()?;
    require_database(services)?;
    let page = services
        .jobs
        .service
        .list_jobs(Some(JOB_LIST_LIMIT), cursor)
        .await
        .map_err(|error| job_error(&error, JOB_LIST_ERROR))?;
    Ok(JobPageDto::from(&page))
}

/// The complete job, by canonical UUID text.
#[tauri::command]
#[specta::specta]
pub async fn get_job(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<JobDetailDto, CommandError> {
    run_get_job(&state, &job_id).await
}

async fn run_get_job(state: &AppState, job_id: &str) -> Result<JobDetailDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_job()?;
    require_database(services)?;
    let job = services
        .jobs
        .service
        .get_job(job_id)
        .await
        .map_err(|error| detail_error(&error))?;
    Ok(JobDetailDto::from(&job))
}

/// Search the saved jobs for a query. The limit is fixed at 20 by this command.
#[tauri::command]
#[specta::specta]
pub async fn search_jobs(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<JobSearchDto, CommandError> {
    let request = parse_request(&request_json, JOB_SEARCH_ERROR)?;
    run_search_jobs(&state, request).await
}

async fn run_search_jobs(
    state: &AppState,
    mut request: Value,
) -> Result<JobSearchDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_job()?;
    require_database(services)?;
    if let Some(object) = request.as_object_mut() {
        object.insert("limit".to_owned(), Value::Number(JOB_SEARCH_LIMIT.into()));
    }
    let response = services
        .jobs
        .search
        .search_jobs(&request)
        .await
        .map_err(|error| job_error(&error, JOB_SEARCH_ERROR))?;
    Ok(JobSearchDto::from(&response))
}

/// Saves the changed fields of a job under the revision it was loaded at. The revision is a decimal
/// string; the request carries only the changed fields (`skillsText` when the skills changed).
#[tauri::command]
#[specta::specta]
pub async fn update_job(
    state: State<'_, AppState>,
    job_id: String,
    expected_revision: String,
    request_json: String,
) -> Result<JobDetailDto, CommandError> {
    let request = parse_request(&request_json, JOB_UPDATE_ERROR)?;
    run_update_job(&state, &job_id, &expected_revision, &request).await
}

async fn run_update_job(
    state: &AppState,
    job_id: &str,
    expected_revision: &str,
    request: &Value,
) -> Result<JobDetailDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_job()?;
    require_database(services)?;
    let request = update_request(job_id, expected_revision, request)?;
    let job = services
        .jobs
        .service
        .update_job(&request)
        .await
        .map_err(|error| update_error(&error))?;
    Ok(JobDetailDto::from(&job))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::JOB_BUSY;
    use jobniac_core::env::Environment;
    use jobniac_core::errors::{ApplicationError, validation_error};
    use jobniac_runtime::services::compose;
    use serde_json::json;

    const JOB_ID: &str = "00000000-0000-4000-8000-0000000000c1";

    fn state_without_database() -> AppState {
        AppState::new(compose(&Environment::default()))
    }

    fn application_error(code: ErrorCode) -> AppError {
        AppError::Application(ApplicationError::of(code))
    }

    #[test]
    fn a_missing_job_and_a_stale_revision_have_their_own_copy() {
        assert_eq!(
            detail_error(&application_error(ErrorCode::NotFound)),
            CommandError::new("not_found", JOB_DETAIL_MISSING)
        );
        assert_eq!(
            detail_error(&application_error(ErrorCode::InternalError)),
            CommandError::new("internal_error", JOB_DETAIL_ERROR)
        );
        assert_eq!(
            update_error(&application_error(ErrorCode::Conflict)),
            CommandError::new("conflict", JOB_CONFLICT)
        );
        assert_eq!(
            update_error(&application_error(ErrorCode::ValidationError)),
            CommandError::new("validation_error", JOB_UPDATE_ERROR)
        );
    }

    #[test]
    fn list_search_and_add_failures_keep_the_code_and_their_copy() {
        assert_eq!(
            job_error(&AppError::Internal, JOB_LIST_ERROR),
            CommandError::new("internal_error", JOB_LIST_ERROR)
        );
        assert_eq!(
            job_error(
                &application_error(ErrorCode::ValidationError),
                JOB_SEARCH_ERROR
            ),
            CommandError::new("validation_error", JOB_SEARCH_ERROR)
        );
        assert_eq!(
            job_error(
                &application_error(ErrorCode::ValidationError),
                JOB_ADD_ERROR
            ),
            CommandError::new("validation_error", JOB_ADD_ERROR)
        );
    }

    #[test]
    fn a_malformed_or_lone_surrogate_request_is_refused_with_the_operation_copy() {
        assert_eq!(
            parse_request(r#"{"text":"\ud800"}"#, JOB_ADD_ERROR).err(),
            Some(CommandError::new("validation_error", JOB_ADD_ERROR))
        );
        assert!(parse_request("{", JOB_SEARCH_ERROR).is_err());
        assert_eq!(
            parse_request(r#"{"text":"x"}"#, JOB_ADD_ERROR).ok(),
            Some(json!({"text": "x"}))
        );
    }

    #[test]
    fn skills_text_becomes_one_stripped_skill_per_line() -> Result<(), CommandError> {
        let request =
            json!({"title": "Synthetic", "skillsText": " Rust \r\n\n  SQL  \u{2028}Go\n   "});
        let update = update_request(JOB_ID, "7", &request)?;
        assert_eq!(update["skills"], json!(["Rust", "SQL", "Go"]));
        assert!(update.get("skillsText").is_none());
        Ok(())
    }

    #[test]
    fn an_empty_skills_text_clears_and_an_absent_one_keeps_the_skills() -> Result<(), CommandError>
    {
        let cleared = update_request(JOB_ID, "7", &json!({"skillsText": ""}))?;
        assert_eq!(cleared["skills"], json!([]));
        let kept = update_request(JOB_ID, "7", &json!({"title": "x", "skillsText": null}))?;
        assert!(kept.get("skills").is_none());
        assert!(kept.get("skillsText").is_none());
        Ok(())
    }

    #[test]
    fn the_update_request_carries_the_job_id_and_the_revision_as_an_integer()
    -> Result<(), CommandError> {
        let update = update_request(JOB_ID, "12", &json!({"title": "Synthetic"}))?;
        assert_eq!(update["jobId"], JOB_ID);
        assert_eq!(update["expectedRevision"], json!(12));
        assert_eq!(update["title"], "Synthetic");
        assert!(update_request(JOB_ID, "twelve", &json!({})).is_err());
        assert!(update_request(JOB_ID, "+7", &json!({})).is_err());
        assert!(update_request(JOB_ID, "-1", &json!({})).is_err());
        assert!(update_request(JOB_ID, "", &json!({})).is_err());
        assert!(update_request(JOB_ID, "7", &json!({"skillsText": 3})).is_err());
        Ok(())
    }

    #[test]
    fn a_malformed_revision_keeps_the_update_copy() {
        // An invalid revision has no copy of its own: nothing is saved without one.
        assert_eq!(
            update_request(JOB_ID, "+7", &json!({})).err(),
            Some(CommandError::new("validation_error", JOB_UPDATE_ERROR))
        );
    }

    #[test]
    fn a_non_object_update_is_passed_on_for_the_core_to_refuse() -> Result<(), CommandError> {
        assert_eq!(update_request(JOB_ID, "7", &json!("text"))?, json!("text"));
        Ok(())
    }

    #[tokio::test]
    async fn list_without_a_database_is_not_configured() {
        let state = state_without_database();
        assert_eq!(
            run_list_jobs(&state, None).await.err(),
            Some(CommandError::new(NOT_CONFIGURED, JOB_UNCONFIGURED))
        );
    }

    #[tokio::test]
    async fn a_job_request_while_one_runs_is_refused_as_busy() {
        let state = state_without_database();
        let _held = state.begin_job();
        assert_eq!(
            run_get_job(&state, JOB_ID).await.err(),
            Some(CommandError::busy(JOB_BUSY))
        );
        assert_eq!(
            run_list_jobs(&state, None).await.err(),
            Some(CommandError::busy(JOB_BUSY))
        );
    }

    #[test]
    fn a_malformed_job_id_keeps_the_load_copy() {
        let refusal = AppError::Application(validation_error(
            "Invalid job request",
            Some("jobId"),
            Some("must be a valid UUID"),
        ));
        assert_eq!(
            detail_error(&refusal),
            CommandError::new("validation_error", JOB_DETAIL_ERROR)
        );
    }

    #[tokio::test]
    async fn get_without_a_database_is_not_configured() {
        let state = state_without_database();
        assert_eq!(
            run_get_job(&state, JOB_ID).await.err(),
            Some(CommandError::new(NOT_CONFIGURED, JOB_UNCONFIGURED))
        );
    }

    #[tokio::test]
    async fn a_capture_without_a_database_is_not_configured() {
        let state = state_without_database();
        assert_eq!(
            run_add_job_from_text(&state, &json!({"text": "Synthetic posting"}))
                .await
                .err(),
            Some(CommandError::new(NOT_CONFIGURED, JOB_UNCONFIGURED))
        );
    }

    #[tokio::test]
    async fn a_search_without_a_database_is_not_configured() {
        let state = state_without_database();
        assert_eq!(
            run_search_jobs(&state, json!({"query": "rust"}))
                .await
                .err(),
            Some(CommandError::new(NOT_CONFIGURED, JOB_UNCONFIGURED))
        );
    }

    #[tokio::test]
    async fn an_update_without_a_database_is_not_configured() {
        let state = state_without_database();
        assert_eq!(
            run_update_job(&state, JOB_ID, "7", &json!({"title": "Synthetic"}))
                .await
                .err(),
            Some(CommandError::new(NOT_CONFIGURED, JOB_UNCONFIGURED))
        );
    }

    #[test]
    fn a_refusal_of_a_named_field_has_the_copy_of_that_field() {
        let refusal = |field: &str| {
            AppError::Application(validation_error(
                "Invalid job request",
                Some(field),
                Some("must not be blank"),
            ))
        };
        assert_eq!(
            job_error(&refusal("text"), JOB_ADD_ERROR),
            CommandError::new("validation_error", JOB_PASTE_VALIDATION)
        );
        assert_eq!(
            job_error(&refusal("query"), JOB_SEARCH_ERROR),
            CommandError::new("validation_error", JOB_SEARCH_VALIDATION)
        );
        assert_eq!(
            update_error(&refusal("postedAt")),
            CommandError::new("validation_error", JOB_POSTED_AT_VALIDATION)
        );
        // Other fields keep the operation copy, so a lone-surrogate or skills refusal is unchanged.
        assert_eq!(
            update_error(&refusal("skills")),
            CommandError::new("validation_error", JOB_UPDATE_ERROR)
        );
    }

    #[tokio::test]
    async fn startup_failure_is_returned_for_job_commands() {
        let state = AppState::new(Err(jobniac_runtime::services::StartupError));
        assert_eq!(
            run_list_jobs(&state, None).await.err(),
            Some(CommandError::startup_failed())
        );
    }
}
