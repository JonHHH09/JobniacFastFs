//! Profile command adapters. Each command takes the single-flight
//! slot of its workspace, calls one core use case and maps the result to a DTO. Validation of
//! record stays in the core: the request values are passed through as the JSON the
//! use cases canonicalize, and the Rust side re-validates everything it stores.
//!
//! The search limit is fixed here at 50: the UI never chooses it.

use jobniac_core::errors::{AppError, ErrorCode, error_envelope};
use serde_json::{Value, json};
use tauri::State;

use crate::error::CommandError;
use crate::profile_dto::{
    PersonalDetailsEnvelopeDto, ProfileGraphDto, ProfilePageDto, ProfileSearchDto, ProjectPatchDto,
};
use crate::state::AppState;

/// Wire code of a service that is not composed.
pub const NOT_CONFIGURED: &str = "not_configured";
/// Message id when the profile list has no cursor key (the service is not configured).
pub const PROFILES_UNCONFIGURED: &str = "profiles.unconfigured";
/// Message id of a failed profile list, and of any other list failure without a listed code.
pub const PROFILES_ERROR: &str = "profiles.error";
/// Message id of a validation failure at the command boundary (malformed text, invalid revision).
pub const PROFILE_VALIDATION: &str = "profile.error.validation";
/// Message id of a failed profile load (get, personal get) without a listed code.
pub const PROFILE_LOAD_ERROR: &str = "profile.error.load";
/// Message id of a failed create, update or personal save without a listed code.
pub const PROFILE_SAVE_ERROR: &str = "profile.error.save";
/// Message id of a failed project-only save without a listed code.
pub const PROFILE_PROJECT_ERROR: &str = "profile.error.project";
/// Message id of every failed search, whatever the code.
pub const PROFILE_SEARCH_ERROR: &str = "profileSearch.failed";
/// The fixed search limit of the port's command.
pub const PROFILE_SEARCH_LIMIT: i64 = 50;

/// Maps a core failure to the typed command error. The codes that have a shared copy get their
/// message id here; any other code keeps the operation's own fallback id.
pub fn profile_error(error: &AppError, fallback_message_id: &str) -> CommandError {
    let code = error_envelope(error).code;
    let message_id = match code {
        ErrorCode::Conflict => "profile.error.conflict",
        ErrorCode::NotFound => "profile.error.notFound",
        ErrorCode::ValidationError => "profile.error.validation",
        ErrorCode::UpstreamUnavailable
        | ErrorCode::UpstreamInvalidResponse
        | ErrorCode::UpstreamRateLimited => "profile.error.upstream",
        _ => fallback_message_id,
    };
    CommandError::new(code.code(), message_id)
}

/// Parses the request text the UI sends. The request crosses as its JSON text because
/// specta cannot describe the recursive JSON value type: the use cases then see exactly the keys
/// that were sent (absent and null stay distinct), and the text is parsed here.
fn parse_request(text: &str) -> Result<Value, CommandError> {
    serde_json::from_str(text)
        .map_err(|_| CommandError::new("validation_error", PROFILE_VALIDATION))
}

/// Parses the `expectedRevision` string (an `i64` crosses IPC as decimal text) into
/// the integer the use cases check. Like the job command, the text must be a non-empty run of ASCII
/// digits: a sign, a space or any other character is refused (`"+7"` is not the revision 7, which
/// `i64::from_str` alone would accept). The range checks stay in the core.
fn parse_revision(text: &str) -> Result<Value, CommandError> {
    let digits = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    digits
        .then(|| text.parse::<i64>().ok())
        .flatten()
        .map(|revision| json!(revision))
        .ok_or_else(|| CommandError::new("validation_error", PROFILE_VALIDATION))
}

/// The project-patch request with its `expectedRevision` string parsed into the integer the core
/// reads. Any other shape is passed on unchanged, so the core still decides it.
fn with_parsed_revision(request: &Value) -> Result<Value, CommandError> {
    let text = match request.get("expectedRevision") {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    };
    let mut request = request.clone();
    if let (Some(text), Some(object)) = (text, request.as_object_mut()) {
        object.insert("expectedRevision".to_owned(), parse_revision(&text)?);
    }
    Ok(request)
}

/// Maps a search failure: the copy is the same for every code.
fn search_error(error: &AppError) -> CommandError {
    CommandError::new(error_envelope(error).code.code(), PROFILE_SEARCH_ERROR)
}

/// List one page of profile identities, optionally from a cursor returned by the previous page.
#[tauri::command]
#[specta::specta]
pub async fn list_profiles(
    state: State<'_, AppState>,
    cursor: Option<String>,
) -> Result<ProfilePageDto, CommandError> {
    run_list_profiles(&state, cursor.as_deref()).await
}

async fn run_list_profiles(
    state: &AppState,
    cursor: Option<&str>,
) -> Result<ProfilePageDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_profile_list()?;
    let list = services
        .profiles
        .list
        .as_ref()
        .map_err(|_| CommandError::new(NOT_CONFIGURED, PROFILES_UNCONFIGURED))?;
    let page = list
        .list_profiles(None, cursor)
        .await
        .map_err(|error| profile_error(&error, PROFILES_ERROR))?;
    Ok(ProfilePageDto::from(&page))
}

/// The complete graph of one profile.
#[tauri::command]
#[specta::specta]
pub async fn get_profile(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<ProfileGraphDto, CommandError> {
    run_get_profile(&state, &profile_id).await
}

async fn run_get_profile(
    state: &AppState,
    profile_id: &str,
) -> Result<ProfileGraphDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_profile()?;
    let graph = services
        .profiles
        .details
        .get_profile(profile_id)
        .await
        .map_err(|error| profile_error(&error, PROFILE_LOAD_ERROR))?;
    Ok(ProfileGraphDto::from(&graph))
}

/// Create a profile and its complete child graph from the write request.
#[tauri::command]
#[specta::specta]
pub async fn create_profile(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<ProfileGraphDto, CommandError> {
    let request = parse_request(&request_json)?;
    run_create_profile(&state, &request).await
}

async fn run_create_profile(
    state: &AppState,
    request: &Value,
) -> Result<ProfileGraphDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_profile()?;
    let graph = services
        .profiles
        .details
        .create_profile(request)
        .await
        .map_err(|error| profile_error(&error, PROFILE_SAVE_ERROR))?;
    Ok(ProfileGraphDto::from(&graph))
}

/// Replace a profile graph when `expectedRevision` (a decimal string) matches the stored revision.
#[tauri::command]
#[specta::specta]
pub async fn update_profile(
    state: State<'_, AppState>,
    profile_id: String,
    expected_revision: String,
    request_json: String,
) -> Result<ProfileGraphDto, CommandError> {
    let request = parse_request(&request_json)?;
    run_update_profile(&state, &profile_id, &expected_revision, &request).await
}

async fn run_update_profile(
    state: &AppState,
    profile_id: &str,
    expected_revision: &str,
    request: &Value,
) -> Result<ProfileGraphDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_profile()?;
    let revision = parse_revision(expected_revision)?;
    let graph = services
        .profiles
        .details
        .update_profile(profile_id, &revision, request)
        .await
        .map_err(|error| profile_error(&error, PROFILE_SAVE_ERROR))?;
    Ok(ProfileGraphDto::from(&graph))
}

/// Patch one project. The request carries `profileId`, `projectId`, `expectedRevision` and the
/// changed fields only; the core decides which fields apply.
#[tauri::command]
#[specta::specta]
pub async fn update_project(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<ProjectPatchDto, CommandError> {
    let request = parse_request(&request_json)?;
    run_update_project(&state, &request).await
}

async fn run_update_project(
    state: &AppState,
    request: &Value,
) -> Result<ProjectPatchDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_profile()?;
    let request = with_parsed_revision(request)?;
    let result = services
        .profiles
        .details
        .update_profile_project(&request)
        .await
        .map_err(|error| profile_error(&error, PROFILE_PROJECT_ERROR))?;
    Ok(ProjectPatchDto::from(&result))
}

/// Search the profile postings for a trimmed query. The limit is fixed at 50 by this command.
#[tauri::command]
#[specta::specta]
pub async fn search_profiles(
    state: State<'_, AppState>,
    query: String,
) -> Result<ProfileSearchDto, CommandError> {
    run_search_profiles(&state, &query).await
}

async fn run_search_profiles(
    state: &AppState,
    query: &str,
) -> Result<ProfileSearchDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_profile()?;
    let request = json!({ "query": query, "limit": PROFILE_SEARCH_LIMIT });
    let response = services
        .profiles
        .search
        .search_profiles(&request)
        .await
        .map_err(|error| search_error(&error))?;
    Ok(ProfileSearchDto::from(&response))
}

/// The personal-details envelope of a profile.
#[tauri::command]
#[specta::specta]
pub async fn get_personal_details(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<PersonalDetailsEnvelopeDto, CommandError> {
    run_get_personal_details(&state, &profile_id).await
}

async fn run_get_personal_details(
    state: &AppState,
    profile_id: &str,
) -> Result<PersonalDetailsEnvelopeDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_profile()?;
    let envelope = services
        .profiles
        .personal_details
        .get_personal_details(profile_id)
        .await
        .map_err(|error| profile_error(&error, PROFILE_LOAD_ERROR))?;
    Ok(PersonalDetailsEnvelopeDto::from(&envelope))
}

/// Replace the personal details under the profile revision check.
#[tauri::command]
#[specta::specta]
pub async fn update_personal_details(
    state: State<'_, AppState>,
    profile_id: String,
    expected_revision: String,
    request_json: String,
) -> Result<PersonalDetailsEnvelopeDto, CommandError> {
    let request = parse_request(&request_json)?;
    run_update_personal_details(&state, &profile_id, &expected_revision, &request).await
}

async fn run_update_personal_details(
    state: &AppState,
    profile_id: &str,
    expected_revision: &str,
    request: &Value,
) -> Result<PersonalDetailsEnvelopeDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_profile()?;
    let revision = parse_revision(expected_revision)?;
    let envelope = services
        .profiles
        .personal_details
        .update_personal_details(profile_id, &revision, request)
        .await
        .map_err(|error| profile_error(&error, PROFILE_SAVE_ERROR))?;
    Ok(PersonalDetailsEnvelopeDto::from(&envelope))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{PROFILE_BUSY, PROFILE_LIST_BUSY};
    use jobniac_core::env::Environment;
    use jobniac_core::errors::ApplicationError;
    use jobniac_core::profiles::model::ProfileRecord;
    use jobniac_runtime::services::compose;
    use time::OffsetDateTime;
    use uuid::Uuid;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const PROFILE_ID: &str = "00000000-0000-4000-8000-000000000001";

    fn state_without_database() -> AppState {
        AppState::new(compose(&Environment::default()))
    }

    fn state_with_short_cursor_key() -> AppState {
        let env = Environment::from_pairs([("JOBNIAC_CURSOR_HMAC_SECRET", "short")]);
        AppState::new(compose(&env))
    }

    fn state_with_cursor_key() -> AppState {
        let env = Environment::from_pairs([(
            "JOBNIAC_CURSOR_HMAC_SECRET",
            "cursor-secret-0123456789abcdefghijklmnop",
        )]);
        AppState::new(compose(&env))
    }

    fn application_error(code: ErrorCode) -> AppError {
        AppError::Application(ApplicationError::of(code))
    }

    #[test]
    fn profile_errors_keep_the_wire_code_and_map_the_shared_copy() {
        assert_eq!(
            profile_error(&application_error(ErrorCode::Conflict), PROFILE_SAVE_ERROR),
            CommandError::new("conflict", "profile.error.conflict")
        );
        assert_eq!(
            profile_error(&application_error(ErrorCode::NotFound), PROFILE_LOAD_ERROR),
            CommandError::new("not_found", "profile.error.notFound")
        );
        assert_eq!(
            profile_error(
                &application_error(ErrorCode::ValidationError),
                PROFILE_SAVE_ERROR
            ),
            CommandError::new("validation_error", "profile.error.validation")
        );
        assert_eq!(
            profile_error(
                &application_error(ErrorCode::UpstreamInvalidResponse),
                PROFILE_LOAD_ERROR
            ),
            CommandError::new("upstream_invalid_response", "profile.error.upstream")
        );
    }

    #[test]
    fn other_codes_keep_the_operation_fallback_copy() {
        assert_eq!(
            profile_error(
                &application_error(ErrorCode::InternalError),
                PROFILE_PROJECT_ERROR
            ),
            CommandError::new("internal_error", "profile.error.project")
        );
        assert_eq!(
            profile_error(&AppError::Internal, PROFILE_LOAD_ERROR),
            CommandError::new("internal_error", "profile.error.load")
        );
    }

    #[test]
    fn every_search_failure_has_the_same_copy() {
        assert_eq!(
            search_error(&application_error(ErrorCode::ValidationError)),
            CommandError::new("validation_error", PROFILE_SEARCH_ERROR)
        );
        assert_eq!(
            search_error(&application_error(ErrorCode::Conflict)),
            CommandError::new("conflict", PROFILE_SEARCH_ERROR)
        );
    }

    #[tokio::test]
    async fn a_short_cursor_key_stops_startup_so_the_list_reports_the_startup_failure() {
        // Startup refuses a too-short cursor secret, so no list service exists.
        let state = state_with_short_cursor_key();
        assert_eq!(
            run_list_profiles(&state, None).await.err(),
            Some(CommandError::startup_failed())
        );
    }

    #[tokio::test]
    async fn list_without_a_database_fails_with_the_list_copy() {
        let state = state_with_cursor_key();
        assert_eq!(
            run_list_profiles(&state, None).await.err(),
            Some(CommandError::new("internal_error", PROFILES_ERROR))
        );
    }

    #[tokio::test]
    async fn a_second_list_while_one_runs_is_refused_as_busy() {
        let state = state_with_cursor_key();
        let _held = state.begin_profile_list();
        assert_eq!(
            run_list_profiles(&state, None).await.err(),
            Some(CommandError::busy(PROFILE_LIST_BUSY))
        );
    }

    #[tokio::test]
    async fn a_profile_request_while_one_runs_is_refused_as_busy() {
        let state = state_without_database();
        let _held = state.begin_profile();
        assert_eq!(
            run_get_profile(&state, PROFILE_ID).await.err(),
            Some(CommandError::busy(PROFILE_BUSY))
        );
    }

    #[tokio::test]
    async fn a_malformed_profile_id_is_a_validation_error_before_any_read() {
        let state = state_without_database();
        assert_eq!(
            run_get_profile(&state, "not-a-uuid").await.err(),
            Some(CommandError::new(
                "validation_error",
                "profile.error.validation"
            ))
        );
    }

    #[tokio::test]
    async fn a_malformed_expected_revision_is_refused_before_any_write() {
        let state = state_without_database();
        let request = json!({"fullName": "Ada", "email": "ada@example.test", "summary": null});
        assert_eq!(
            run_update_profile(&state, PROFILE_ID, "seven", &request)
                .await
                .err()
                .map(|error| error.code),
            Some("validation_error".to_owned())
        );
    }

    #[test]
    fn the_revision_text_is_ascii_digits_only_like_the_job_command() {
        assert_eq!(parse_revision("7").ok(), Some(json!(7)));
        for refused in ["+7", " 7", "7 ", "-7", "", "7.0", "99999999999999999999"] {
            assert_eq!(
                parse_revision(refused).err(),
                Some(CommandError::new("validation_error", PROFILE_VALIDATION)),
                "{refused:?}"
            );
        }
        let patch = json!({"expectedRevision": "+7", "name": "Synthetic project"});
        assert!(with_parsed_revision(&patch).is_err());
    }

    #[tokio::test]
    async fn a_decimal_revision_passes_the_boundary_and_reaches_the_read() {
        // "7" is the wire form of the revision: it is parsed here and checked by the core, which
        // then fails only because no database is configured (an internal error, not validation).
        let state = state_without_database();
        let request = json!({"fullName": "Ada", "email": "ada@example.test", "summary": null});
        assert_eq!(
            run_update_profile(&state, PROFILE_ID, "7", &request)
                .await
                .err()
                .map(|error| (error.code, error.message_id)),
            Some(("internal_error".to_owned(), PROFILE_SAVE_ERROR.to_owned()))
        );
    }

    #[tokio::test]
    async fn a_decimal_project_revision_is_parsed_before_the_patch() {
        let state = state_without_database();
        let request = json!({
            "profileId": PROFILE_ID,
            "projectId": PROFILE_ID,
            "expectedRevision": "7",
            "name": "Synthetic project"
        });
        assert_eq!(
            run_update_project(&state, &request)
                .await
                .err()
                .map(|error| (error.code, error.message_id)),
            Some((
                "internal_error".to_owned(),
                PROFILE_PROJECT_ERROR.to_owned()
            ))
        );
    }

    #[test]
    fn the_project_request_carries_the_revision_as_an_integer() -> TestResult {
        let parsed = with_parsed_revision(&json!({"expectedRevision": "7", "name": "x"}))
            .map_err(|error| error.code)?;
        assert_eq!(parsed, json!({"expectedRevision": 7, "name": "x"}));
        assert!(with_parsed_revision(&json!({"expectedRevision": "seven"})).is_err());
        Ok(())
    }

    #[test]
    fn a_malformed_or_lone_surrogate_request_is_a_validation_error_with_the_validation_copy() {
        // A lone surrogate escape is not valid JSON text: the boundary refuses it.
        let lone_surrogate = r#"{"fullName":"\ud800"}"#;
        assert_eq!(
            parse_request(lone_surrogate).err(),
            Some(CommandError::new(
                "validation_error",
                "profile.error.validation"
            ))
        );
        assert!(parse_request("{").is_err());
        assert_eq!(
            parse_request(r#"{"fullName":"x"}"#).ok(),
            Some(json!({"fullName": "x"}))
        );
    }

    #[tokio::test]
    async fn a_blank_search_is_a_validation_error_with_the_search_copy() {
        let state = state_without_database();
        assert_eq!(
            run_search_profiles(&state, "   ").await.err(),
            Some(CommandError::new("validation_error", PROFILE_SEARCH_ERROR))
        );
    }

    #[tokio::test]
    async fn personal_details_without_a_database_fail_with_the_load_copy() {
        let state = state_without_database();
        assert_eq!(
            run_get_personal_details(&state, PROFILE_ID).await.err(),
            Some(CommandError::new("internal_error", PROFILE_LOAD_ERROR))
        );
    }

    #[tokio::test]
    async fn startup_failure_is_returned_for_profile_commands() {
        let state = AppState::new(Err(jobniac_runtime::services::StartupError));
        assert_eq!(
            run_search_profiles(&state, "ada").await.err(),
            Some(CommandError::startup_failed())
        );
    }

    #[test]
    fn a_graph_maps_revisions_and_dates_to_their_wire_text() -> Result<(), serde_json::Error> {
        let when = OffsetDateTime::UNIX_EPOCH;
        let id = Uuid::parse_str(PROFILE_ID).unwrap_or(Uuid::nil());
        let record = ProfileRecord {
            id,
            full_name: "Ada Example".to_owned(),
            email: "ada@example.test".to_owned(),
            summary: None,
            raw_resume_text: None,
            created_at: when,
            updated_at: when,
            embedding: None,
            revision: 7,
        };
        let graph = jobniac_core::profiles::ProfileGraph {
            profile: record,
            contacts: Vec::new(),
            links: Vec::new(),
            skills: Vec::new(),
            languages: Vec::new(),
            education: Vec::new(),
            experiences: Vec::new(),
            projects: Vec::new(),
            project_technologies: Vec::new(),
        };
        let dto = ProfileGraphDto::from(&graph);
        assert_eq!(dto.profile.revision, "7");
        assert_eq!(dto.profile.id, PROFILE_ID);
        let json = serde_json::to_value(&dto)?;
        assert!(json.get("projectTechnologies").is_none());
        assert_eq!(json["profile"]["fullName"], "Ada Example");
        assert_eq!(json["profile"]["revision"], "7");
        assert_eq!(json["projects"], json!([]));
        Ok(())
    }

    #[test]
    fn a_search_response_keeps_totals_as_strings_and_null_when_unknown() -> TestResult {
        let response = jobniac_core::profiles::ProfileSearchResponse {
            query: "ada".to_owned(),
            query_tokens: vec!["ada".to_owned()],
            total_matches: None,
            matched_count: 3,
            has_more: true,
            returned_count: 0,
            profiles: Vec::new(),
        };
        let json = serde_json::to_value(ProfileSearchDto::from(&response))?;
        assert_eq!(json["totalMatches"], Value::Null);
        assert_eq!(json["matchedCount"], "3");
        assert_eq!(json["hasMore"], true);
        assert_eq!(json["returnedCount"], 0);
        Ok(())
    }
}
