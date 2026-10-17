//! Profile-from-stored-PDF command adapters.
//!
//! The ingestion of a stored PDF can replace an existing profile, which needs a confirmation. A
//! replacement request is validated here and staged in Rust: the confirmation step returns an opaque
//! token and never reaches the service. Confirm and cancel take the token only, and an ingestion is
//! refused while a replacement waits. The revision and the character budget cross as strings and are
//! parsed here, so an out-of-range value is refused rather than clamped.

use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::ids::admit_uuid;
use jobniac_text::str_rules::strip_text;
use serde_json::{Value, json};
use tauri::State;
use uuid::Uuid;

use crate::error::CommandError;
use crate::match_commands::{parse_request, require_database, text_field};
use crate::profile_pdf_dto::{
    CancelReplacementDto, IngestResultDto, IngestStepDto, ProfilePdfCapabilitiesDto, RestoredState,
    SourceViewDto, ingest_result, source_view,
};
use crate::state::{AppState, PROFILE_PDF_CONFIRM_REQUIRED, PendingReplacement};

/// Message id of a request while no local database is configured.
pub const PROFILE_PDF_UNCONFIGURED: &str = "profilePdf.intro.unavailable";
/// Message id of an ingestion whose document or profile ID is malformed.
pub const PROFILE_PDF_IDS: &str = "profilePdf.input.ids";
/// Message id of an expected revision that is not an ASCII integer in the signed 64-bit range.
pub const PROFILE_PDF_REVISION: &str = "profilePdf.input.revision";
/// Message id of a character budget outside 1 to 250,000.
pub const PROFILE_PDF_MAX_CHARACTERS: &str = "profilePdf.input.maxCharacters";
/// Message id of a replacement without its existing profile ID or revision.
pub const PROFILE_PDF_REPLACEMENT_REQUIRED: &str = "profilePdf.input.replacementRequired";
/// Message id of a lookup whose profile ID is malformed.
pub const PROFILE_PDF_LOOKUP: &str = "profilePdf.input.lookupId";
/// Message id of every other profile-from-PDF failure, whatever the code.
pub const PROFILE_PDF_ERROR: &str = "profilePdf.error.generic";

/// The largest character budget of an ingestion.
pub const MAX_CHARACTERS_LIMIT: i64 = 250_000;

/// The longest decimal integer text of a request: the digits of the signed
/// 64-bit maximum.
const REVISION_MAX_DIGITS: usize = 19;

/// The validated fields of an ingestion request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IngestInput {
    document_id: Uuid,
    existing_profile_id: Option<Uuid>,
    overwrite: bool,
    max_characters: Option<i64>,
    expected_revision: Option<i64>,
}

impl IngestInput {
    /// Validates the request in this order: document, existing profile,
    /// expected revision, character budget, then the overwrite flag and the replacement rule last.
    /// A blank existing profile, revision or budget text is absent, as in the workspace.
    fn parse(request: &Value) -> Result<Self, CommandError> {
        let document_id = uuid_text(text_field(request, "documentId", PROFILE_PDF_IDS)?)
            .ok_or_else(|| CommandError::new("validation_error", PROFILE_PDF_IDS))?;
        let existing_profile_id = match request.get("existingProfileId") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) if strip_text(text).is_empty() => None,
            Some(Value::String(text)) => Some(
                uuid_text(text)
                    .ok_or_else(|| CommandError::new("validation_error", PROFILE_PDF_IDS))?,
            ),
            Some(_) => return Err(CommandError::new("validation_error", PROFILE_PDF_IDS)),
        };
        let expected_revision = match request.get("expectedRevision") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => revision_text(text)?,
            Some(_) => return Err(CommandError::new("validation_error", PROFILE_PDF_REVISION)),
        };
        let max_characters = match request.get("maxCharacters") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => max_characters_text(text)?,
            Some(_) => {
                return Err(CommandError::new(
                    "validation_error",
                    PROFILE_PDF_MAX_CHARACTERS,
                ));
            }
        };
        let overwrite = match request.get("overwriteExistingProfile") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(flag)) => *flag,
            Some(_) => return Err(CommandError::new("validation_error", PROFILE_PDF_IDS)),
        };
        if overwrite && (existing_profile_id.is_none() || expected_revision.is_none()) {
            return Err(CommandError::new(
                "validation_error",
                PROFILE_PDF_REPLACEMENT_REQUIRED,
            ));
        }
        Ok(Self {
            document_id,
            existing_profile_id,
            overwrite,
            max_characters,
            expected_revision,
        })
    }
}

/// Parses a stored or profile UUID after normalization (white space removed, lowercase).
fn uuid_text(text: &str) -> Option<Uuid> {
    admit_uuid(&strip_text(text).to_ascii_lowercase()).ok()
}

/// The decimal integer of a request text, after stripping: `Ok(None)`
/// for a blank text, `Ok(Some(number))` for ASCII digits of at most 19 characters within the
/// signed 64-bit range, and `Err(())` for anything else (a sign, a fraction, a longer text).
fn decimal_integer(text: &str) -> Result<Option<i64>, ()> {
    let stripped = strip_text(text);
    if stripped.is_empty() {
        return Ok(None);
    }
    if stripped.len() > REVISION_MAX_DIGITS || !stripped.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    stripped.parse::<i64>().map(Some).map_err(|_| ())
}

/// A character budget text: a blank text is absent, otherwise an integer from 1 to 250,000.
/// Anything else is refused, never clamped.
fn max_characters_text(text: &str) -> Result<Option<i64>, CommandError> {
    match decimal_integer(text) {
        Ok(None) => Ok(None),
        Ok(Some(number)) if (1..=MAX_CHARACTERS_LIMIT).contains(&number) => Ok(Some(number)),
        _ => Err(CommandError::new(
            "validation_error",
            PROFILE_PDF_MAX_CHARACTERS,
        )),
    }
}

/// An expected revision text: a blank text is absent, otherwise a non-negative integer within the
/// signed 64-bit range.
fn revision_text(text: &str) -> Result<Option<i64>, CommandError> {
    decimal_integer(text).map_err(|()| CommandError::new("validation_error", PROFILE_PDF_REVISION))
}

/// The request of the core ingestion for the given overwrite decision.
fn ingest_body(input: &IngestInput) -> Value {
    json!({
        "documentId": input.document_id.to_string(),
        "existingProfileId": input.existing_profile_id.map(|id| id.to_string()),
        "overwriteExistingProfile": input.overwrite,
        "maxCharacters": input.max_characters,
        "expectedRevision": input.expected_revision,
    })
}

/// Maps a core failure to the typed command error. The wire code is kept; the copy is the generic one.
fn profile_pdf_error(error: &AppError) -> CommandError {
    CommandError::new(error_envelope(error).code.code(), PROFILE_PDF_ERROR)
}

/// Reports whether the profile-from-PDF screen is usable, so it can show the unavailable intro and
/// disable its actions before any click. Read-only: it takes no slot, so the screen can ask on mount.
#[tauri::command]
#[specta::specta]
pub async fn profile_pdf_capabilities_get(
    state: State<'_, AppState>,
) -> Result<ProfilePdfCapabilitiesDto, CommandError> {
    run_capabilities(&state)
}

fn run_capabilities(state: &AppState) -> Result<ProfilePdfCapabilitiesDto, CommandError> {
    let services = state.services()?;
    // The same gate as `require_database` of every action of the screen.
    Ok(ProfilePdfCapabilitiesDto {
        profile_pdf: services.database.is_some(),
    })
}

/// Starts an ingestion, or stages a replacement for confirmation.
#[tauri::command]
#[specta::specta]
pub async fn profile_pdf_ingest(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<IngestStepDto, CommandError> {
    run_ingest(&state, &request_json).await
}

async fn run_ingest(state: &AppState, request_json: &str) -> Result<IngestStepDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, PROFILE_PDF_IDS)?;
    let input = IngestInput::parse(&request)?;
    require_database(services, PROFILE_PDF_UNCONFIGURED)?;
    let _guard = state.begin_profile_pdf()?;
    if state.has_pending_replacement() {
        return Err(CommandError::new(
            "pending_confirmation",
            PROFILE_PDF_CONFIRM_REQUIRED,
        ));
    }
    if input.overwrite {
        // Both are present: `IngestInput::parse` refuses a replacement without them.
        let (Some(existing), Some(revision)) = (input.existing_profile_id, input.expected_revision)
        else {
            return Err(CommandError::new(
                "validation_error",
                PROFILE_PDF_REPLACEMENT_REQUIRED,
            ));
        };
        let token = Uuid::new_v4().to_string();
        state.stage_replacement(PendingReplacement {
            token: token.clone(),
            document_id: input.document_id,
            existing_profile_id: existing,
            expected_revision: revision,
            max_characters: input.max_characters,
        });
        return Ok(IngestStepDto::ConfirmationRequired {
            token,
            existing_profile_id: existing.to_string(),
            expected_revision: revision.to_string(),
            document_id: input.document_id.to_string(),
        });
    }
    let value = services
        .profile_pdf
        .ingest_profile_from_stored_pdf(&ingest_body(&input))
        .await
        .map_err(|error| profile_pdf_error(&error))?;
    let result = ingest_result(&value).map_err(|_| internal())?;
    state.set_profile_pdf_ready(true);
    Ok(IngestStepDto::Result { result })
}

/// Confirms the staged replacement and runs it.
#[tauri::command]
#[specta::specta]
pub async fn profile_pdf_confirm_replacement(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<IngestResultDto, CommandError> {
    run_confirm(&state, &request_json).await
}

async fn run_confirm(
    state: &AppState,
    request_json: &str,
) -> Result<IngestResultDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, PROFILE_PDF_ERROR)?;
    let token = text_field(&request, "token", PROFILE_PDF_ERROR)?;
    require_database(services, PROFILE_PDF_UNCONFIGURED)?;
    let _guard = state.begin_profile_pdf()?;
    let pending = state
        .take_replacement(token)
        .ok_or_else(|| CommandError::new("not_found", PROFILE_PDF_ERROR))?;
    let body = json!({
        "documentId": pending.document_id.to_string(),
        "existingProfileId": pending.existing_profile_id.to_string(),
        "overwriteExistingProfile": true,
        "maxCharacters": pending.max_characters,
        "expectedRevision": pending.expected_revision,
    });
    let value = services
        .profile_pdf
        .ingest_profile_from_stored_pdf(&body)
        .await
        .map_err(|error| profile_pdf_error(&error))?;
    let result = ingest_result(&value).map_err(|_| internal())?;
    state.set_profile_pdf_ready(true);
    Ok(result)
}

/// Drops the staged replacement named by the token and reports the state the screen returns to.
/// A different token leaves the staged one in place.
#[tauri::command]
#[specta::specta]
pub async fn profile_pdf_cancel_replacement(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<CancelReplacementDto, CommandError> {
    let request = parse_request(&request_json, PROFILE_PDF_ERROR)?;
    let token = text_field(&request, "token", PROFILE_PDF_ERROR)?;
    let _ = state.take_replacement(token);
    let restored_state = if state.profile_pdf_ready() {
        RestoredState::Ready
    } else {
        RestoredState::Idle
    };
    Ok(CancelReplacementDto { restored_state })
}

/// Reads the PDF source of a profile.
#[tauri::command]
#[specta::specta]
pub async fn profile_pdf_lookup_source(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<SourceViewDto, CommandError> {
    run_lookup(&state, &request_json).await
}

async fn run_lookup(state: &AppState, request_json: &str) -> Result<SourceViewDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, PROFILE_PDF_LOOKUP)?;
    let profile_id = uuid_text(text_field(&request, "profileId", PROFILE_PDF_LOOKUP)?)
        .ok_or_else(|| CommandError::new("validation_error", PROFILE_PDF_LOOKUP))?;
    require_database(services, PROFILE_PDF_UNCONFIGURED)?;
    let _guard = state.begin_profile_pdf()?;
    // Every ingest and lookup is refused while a replacement waits.
    if state.has_pending_replacement() {
        return Err(CommandError::new(
            "pending_confirmation",
            PROFILE_PDF_CONFIRM_REQUIRED,
        ));
    }
    let value = services
        .profile_pdf
        .get_profile_pdf_source(&profile_id.to_string())
        .await
        .map_err(|error| profile_pdf_error(&error))?;
    let source = source_view(&value).map_err(|_| internal())?;
    state.set_profile_pdf_ready(true);
    Ok(source)
}

/// The internal failure of a core value outside its range. The copy is the generic one.
fn internal() -> CommandError {
    CommandError::new("internal_error", PROFILE_PDF_ERROR)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const DOCUMENT: &str = "00000000-0000-4000-8000-000000000001";
    const PROFILE: &str = "00000000-0000-4000-8000-000000000002";

    #[test]
    fn the_capabilities_follow_the_database_and_the_startup() {
        let without_database = AppState::new(jobniac_runtime::services::compose(
            &jobniac_core::env::Environment::default(),
        ));
        assert_eq!(
            run_capabilities(&without_database).ok(),
            Some(ProfilePdfCapabilitiesDto { profile_pdf: false })
        );
        let failed = AppState::new(Err(jobniac_runtime::services::StartupError));
        assert_eq!(
            run_capabilities(&failed).err(),
            Some(CommandError::startup_failed())
        );
    }

    #[test]
    fn a_plain_ingestion_request_parses_with_defaults() {
        let input = IngestInput::parse(&json!({"documentId": DOCUMENT}));
        assert_eq!(
            input.ok(),
            Some(IngestInput {
                document_id: Uuid::parse_str(DOCUMENT).unwrap_or_else(|_| Uuid::nil()),
                existing_profile_id: None,
                overwrite: false,
                max_characters: None,
                expected_revision: None,
            })
        );
    }

    #[test]
    fn a_replacement_needs_the_profile_and_its_revision() {
        let without_revision = IngestInput::parse(&json!({
            "documentId": DOCUMENT,
            "existingProfileId": PROFILE,
            "overwriteExistingProfile": true,
        }));
        assert_eq!(
            without_revision.err(),
            Some(CommandError::new(
                "validation_error",
                PROFILE_PDF_REPLACEMENT_REQUIRED
            ))
        );
        let complete = IngestInput::parse(&json!({
            "documentId": DOCUMENT,
            "existingProfileId": PROFILE,
            "overwriteExistingProfile": true,
            "expectedRevision": "7",
        }));
        assert_eq!(
            complete.ok().and_then(|input| input.expected_revision),
            Some(7)
        );
    }

    #[test]
    fn the_revision_is_digits_within_the_signed_range() {
        assert_eq!(revision_text("0").ok(), Some(Some(0)));
        assert_eq!(
            revision_text("9223372036854775807").ok(),
            Some(Some(i64::MAX))
        );
        assert!(revision_text("9223372036854775808").is_err());
        assert!(revision_text("-1").is_err());
        assert_eq!(
            revision_text("1 ").ok(),
            Some(Some(1)),
            "white space is stripped"
        );
        assert_eq!(
            revision_text(" ").ok(),
            Some(None),
            "a blank text is absent"
        );
        assert!(revision_text("00000000000000000000").is_err());
    }

    #[test]
    fn the_character_budget_is_one_to_two_hundred_fifty_thousand_and_never_clamped() {
        assert_eq!(max_characters_text("1").ok(), Some(Some(1)));
        assert_eq!(max_characters_text("250000").ok(), Some(Some(250_000)));
        assert_eq!(
            max_characters_text("").ok(),
            Some(None),
            "a blank text is absent"
        );
        assert!(max_characters_text("0").is_err());
        assert!(max_characters_text("250001").is_err());
        assert!(max_characters_text("12a").is_err());
    }

    #[test]
    fn a_numeric_revision_or_budget_is_refused_because_the_wire_form_is_text() {
        let numeric = IngestInput::parse(&json!({"documentId": DOCUMENT, "expectedRevision": 3}));
        assert_eq!(
            numeric.err(),
            Some(CommandError::new("validation_error", PROFILE_PDF_REVISION))
        );
    }

    #[test]
    fn the_core_body_carries_the_overwrite_decision_and_the_numbers() {
        let input = IngestInput {
            document_id: Uuid::nil(),
            existing_profile_id: None,
            overwrite: false,
            max_characters: Some(5000),
            expected_revision: Some(2),
        };
        let body = ingest_body(&input);
        assert_eq!(body["overwriteExistingProfile"], false);
        assert_eq!(body["maxCharacters"], 5000);
        assert_eq!(body["expectedRevision"], 2);
    }

    #[test]
    fn an_ingestion_is_refused_while_a_replacement_waits() {
        use jobniac_runtime::services::StartupError;
        let state = AppState::new(Err(StartupError));
        state.stage_replacement(PendingReplacement {
            token: "t".to_owned(),
            document_id: Uuid::nil(),
            existing_profile_id: Uuid::nil(),
            expected_revision: 1,
            max_characters: None,
        });
        assert!(state.has_pending_replacement());
        // A different token does not take the staged replacement.
        assert!(state.take_replacement("other").is_none());
        assert!(state.take_replacement("t").is_some());
        assert!(!state.has_pending_replacement());
    }

    /// The parsed request, or the message id of the refusal.
    fn parse_message(request: &Value) -> Result<IngestInput, String> {
        IngestInput::parse(request).map_err(|error| error.message_id)
    }

    fn document() -> &'static str {
        "00000000-0000-4000-8000-000000000001"
    }

    #[test]
    fn blank_existing_profile_and_blank_numbers_are_absent_as_in_the_workspace() {
        let parsed = parse_message(&json!({
            "documentId": document(),
            "existingProfileId": "   ",
            "expectedRevision": " \t ",
            "maxCharacters": "",
        }))
        .ok();
        let parsed = parsed.unwrap_or_else(|| panic!("blank values are absent"));
        assert_eq!(parsed.existing_profile_id, None);
        assert_eq!(parsed.expected_revision, None);
        assert_eq!(parsed.max_characters, None);
    }

    #[test]
    fn padded_revision_and_budget_are_stripped_before_the_digit_rules() {
        let parsed = parse_message(&json!({
            "documentId": document(),
            "expectedRevision": " 7 ",
            "maxCharacters": "\u{3000}250\u{3000}",
        }))
        .ok();
        let parsed = parsed.unwrap_or_else(|| panic!("padded integers are accepted"));
        assert_eq!(parsed.expected_revision, Some(7));
        assert_eq!(parsed.max_characters, Some(250));
    }

    #[test]
    fn a_revision_is_checked_before_the_budget() {
        let refusal = parse_message(&json!({
            "documentId": document(),
            "expectedRevision": "x",
            "maxCharacters": "0",
        }));
        assert_eq!(refusal.err().as_deref(), Some(PROFILE_PDF_REVISION));
    }

    #[test]
    fn signs_and_long_digit_runs_are_refused_as_in_the_workspace() {
        for budget in ["+7", "7.0", "0000000000000000000007", "250001"] {
            let refusal = parse_message(&json!({
                "documentId": document(),
                "maxCharacters": budget,
            }));
            assert_eq!(
                refusal.err().as_deref(),
                Some(PROFILE_PDF_MAX_CHARACTERS),
                "budget {budget:?}"
            );
        }
        for revision in ["+7", "-1", "9223372036854775808", "00000000000000000007"] {
            let refusal = parse_message(&json!({
                "documentId": document(),
                "expectedRevision": revision,
            }));
            assert_eq!(
                refusal.err().as_deref(),
                Some(PROFILE_PDF_REVISION),
                "revision {revision:?}"
            );
        }
    }
}
