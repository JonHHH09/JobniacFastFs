//! Generation command adapters.
//!
//! Each command takes the single-flight slot of the generation workspace, calls one facade operation
//! and maps the result to a DTO. The facade does the validation of record (the rules and
//! messages); this module admits the identifiers and the kind-specific
//! shape before the call. A failure keeps its wire code and shows the generic copy. The generated PDF is only referenced by its `documentId`; the path never leaves
//! Rust.

use jobniac_core::errors::ApplicationError;
use jobniac_core::ids::admit_uuid;
use jobniac_text::str_rules::strip_text;
use serde_json::{Value, json};
use tauri::State;
use uuid::Uuid;

use crate::error::CommandError;
use crate::generation_dto::{
    GenerationCapabilitiesDto, GenerationResultDto, GenerationResumeRequest,
    GenerationStandaloneRequest, german_tailored_result, single_result, standalone_value,
};
use crate::state::AppState;

/// Message id of every generation failure other than the busy refusal. The screen shows this copy for
/// any exception of the worker, so the screen has one failure copy.
pub const GENERATION_ERROR: &str = "generation.error.generic";

/// The wire code of a request that is not admitted before the facade call.
const VALIDATION: &str = "validation_error";

/// The wire code of a result the screen cannot show.
const INTERNAL: &str = "internal_error";

/// Maximum length, in characters, of a file name and a title.
const TEXT_MAX_CHARACTERS: usize = 4096;

/// Generates a standalone PDF from free text.
#[tauri::command]
#[specta::specta]
pub async fn generation_standalone(
    state: State<'_, AppState>,
    request: GenerationStandaloneRequest,
) -> Result<GenerationResultDto, CommandError> {
    run_standalone(&state, &request).await
}

/// Generates a resume or a German cover letter of a stored profile.
#[tauri::command]
#[specta::specta]
pub async fn generation_resume(
    state: State<'_, AppState>,
    request: GenerationResumeRequest,
) -> Result<GenerationResultDto, CommandError> {
    run_resume(&state, &request).await
}

/// Reports whether generation is admitted. Read-only: it takes
/// no single-flight slot, so the screen can ask on mount.
#[tauri::command]
#[specta::specta]
pub async fn generation_capabilities_get(
    state: State<'_, AppState>,
) -> Result<GenerationCapabilitiesDto, CommandError> {
    run_capabilities(&state).await
}

async fn run_standalone(
    state: &AppState,
    request: &GenerationStandaloneRequest,
) -> Result<GenerationResultDto, CommandError> {
    let services = state.services()?;
    check_text(&request.file_name)?;
    check_text(&request.title)?;
    let value = standalone_value(request);
    let _guard = state.begin_generation()?;
    let response = services
        .generation
        .facade
        .generate_pdf_file_for_desktop(&value)
        .await
        .map_err(|error| generation_error(&error))?;
    // The standalone response carries its own size and page count at the top level.
    single_result(&response, &response).ok_or_else(internal)
}

async fn run_resume(
    state: &AppState,
    request: &GenerationResumeRequest,
) -> Result<GenerationResultDto, CommandError> {
    let services = state.services()?;
    match request {
        GenerationResumeRequest::MasterResume { profile_id } => {
            let value = profile_value(&admit_id(profile_id)?);
            let _guard = state.begin_generation()?;
            let response = services
                .generation
                .facade
                .generate_pdf_resume(&value)
                .await
                .map_err(|error| generation_error(&error))?;
            resume_result(&response)
        }
        GenerationResumeRequest::CanadianResume { profile_id } => {
            let value = profile_value(&admit_id(profile_id)?);
            let _guard = state.begin_generation()?;
            let response = services
                .generation
                .facade
                .generate_canadian_pdf_resume(&value)
                .await
                .map_err(|error| generation_error(&error))?;
            resume_result(&response)
        }
        GenerationResumeRequest::CanadianFrenchResume { profile_id } => {
            let value = profile_value(&admit_id(profile_id)?);
            let _guard = state.begin_generation()?;
            let response = services
                .generation
                .facade
                .generate_canadian_french_pdf_resume(&value)
                .await
                .map_err(|error| generation_error(&error))?;
            resume_result(&response)
        }
        GenerationResumeRequest::GermanyResume {
            profile_id,
            job_id,
            include_projects,
        } => {
            let value = german_tailored_value(
                &admit_id(profile_id)?,
                &admit_id(job_id)?,
                *include_projects,
            );
            let _guard = state.begin_generation()?;
            let response = services
                .generation
                .facade
                .generate_german_tailored_resume(&value)
                .await
                .map_err(|error| generation_error(&error))?;
            german_tailored_result(&response).ok_or_else(internal)
        }
        GenerationResumeRequest::GermanCoverLetter {
            profile_id,
            job_id,
            resume_id,
        } => {
            let value = german_cover_letter_value(
                &admit_id(profile_id)?,
                &admit_id(job_id)?,
                &admit_id(resume_id)?,
            );
            let _guard = state.begin_generation()?;
            let response = services
                .generation
                .facade
                .generate_german_cover_letter(&value)
                .await
                .map_err(|error| generation_error(&error))?;
            // The cover-letter response carries its document and its size and page count at the top level.
            single_result(&response, &response).ok_or_else(internal)
        }
    }
}

async fn run_capabilities(state: &AppState) -> Result<GenerationCapabilitiesDto, CommandError> {
    let services = state.services()?;
    // The runtime decides: generation is on only with a database, the exact flag and a qualified root.
    Ok(GenerationCapabilitiesDto {
        generation: services.generation.facade.is_available().await,
    })
}

/// Admits a request identifier: trimmed, lowercased, then a canonical UUID. The copy is the
/// generic one, because the screen shows one failure copy for generation.
fn admit_id(text: &str) -> Result<Uuid, CommandError> {
    admit_uuid(&strip_text(text).to_ascii_lowercase())
        .map_err(|_| CommandError::new(VALIDATION, GENERATION_ERROR))
}

/// The facade request of a profile resume operation.
fn profile_value(profile_id: &Uuid) -> Value {
    json!({ "profileId": profile_id.to_string() })
}

/// The facade request of a German tailored resume (`generate_german_tailored_resume`):
/// `includeProjects` is always a boolean, as the checkbox sends it.
fn german_tailored_value(profile_id: &Uuid, job_id: &Uuid, include_projects: bool) -> Value {
    json!({
        "profileId": profile_id.to_string(),
        "jobId": job_id.to_string(),
        "includeProjects": include_projects,
    })
}

/// The facade request of a German cover letter (`generate_german_cover_letter`).
fn german_cover_letter_value(profile_id: &Uuid, job_id: &Uuid, resume_id: &Uuid) -> Value {
    json!({
        "profileId": profile_id.to_string(),
        "jobId": job_id.to_string(),
        "resumeId": resume_id.to_string(),
    })
}

/// The file-name and title fields are text of at most 4096 characters.
fn check_text(text: &str) -> Result<(), CommandError> {
    if text.chars().count() > TEXT_MAX_CHARACTERS {
        return Err(CommandError::new(VALIDATION, GENERATION_ERROR));
    }
    Ok(())
}

/// Maps a profile resume response to the single-PDF result: the document is `documentId` and the
/// size and page count are in `generatedFile`.
fn resume_result(response: &Value) -> Result<GenerationResultDto, CommandError> {
    let file = response.get("generatedFile").ok_or_else(internal)?;
    single_result(response, file).ok_or_else(internal)
}

/// The result the screen cannot show, as the generic internal failure.
fn internal() -> CommandError {
    CommandError::new(INTERNAL, GENERATION_ERROR)
}

/// Maps a facade failure to the typed command error. The wire code is kept; the copy is the generic one.
fn generation_error(error: &ApplicationError) -> CommandError {
    CommandError::new(error.code().code(), GENERATION_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobniac_core::errors::{ApplicationError, ErrorCode};
    use serde_json::json;

    const PROFILE_ID: &str = "00000000-0000-4000-8000-0000000000a1";

    #[test]
    fn identifiers_are_trimmed_and_lowercased_before_the_facade() -> Result<(), CommandError> {
        let upper = format!("  {}  ", PROFILE_ID.to_ascii_uppercase());
        assert_eq!(admit_id(&upper)?.to_string(), PROFILE_ID);
        Ok(())
    }

    #[test]
    fn a_malformed_identifier_is_refused_with_the_generic_copy() {
        assert_eq!(
            admit_id("not-a-uuid").err(),
            Some(CommandError::new(VALIDATION, GENERATION_ERROR))
        );
        assert_eq!(
            admit_id("").err(),
            Some(CommandError::new(VALIDATION, GENERATION_ERROR))
        );
    }

    #[test]
    fn a_title_longer_than_the_ui_limit_is_refused_before_the_facade() {
        let long = "x".repeat(TEXT_MAX_CHARACTERS + 1);
        assert!(check_text(&long).is_err());
        assert!(check_text(&"x".repeat(TEXT_MAX_CHARACTERS)).is_ok());
    }

    #[test]
    fn the_profile_request_is_the_canonical_identifier() -> Result<(), CommandError> {
        let id = admit_id(PROFILE_ID)?;
        assert_eq!(profile_value(&id), json!({"profileId": PROFILE_ID}));
        Ok(())
    }

    #[test]
    fn a_resume_response_without_its_generated_file_is_the_internal_failure() {
        let response = json!({"documentId": PROFILE_ID});
        assert_eq!(resume_result(&response).err(), Some(internal()));
    }

    #[test]
    fn a_resume_response_maps_to_one_pdf_with_no_resume_id() -> Result<(), CommandError> {
        let response = json!({
            "documentId": PROFILE_ID,
            "filePath": "generated/x.pdf",
            "generatedFile": {"byteSize": 100, "pageCount": 1, "path": "generated/x.pdf"},
        });
        let result = resume_result(&response)?;
        assert_eq!(result.resume_id, None);
        assert_eq!(result.outputs.len(), 1);
        assert_eq!(result.outputs[0].byte_size, 100);
        assert_eq!(result.outputs[0].page_count, 1);
        Ok(())
    }

    #[test]
    fn a_facade_failure_keeps_its_wire_code_and_the_generic_copy() {
        let error = ApplicationError::of(ErrorCode::NotFound);
        assert_eq!(
            generation_error(&error),
            CommandError::new("not_found", GENERATION_ERROR)
        );
    }

    const RESUME_ID: &str = "00000000-0000-4000-8000-0000000000b2";
    const JOB_ID: &str = "00000000-0000-4000-8000-0000000000d3";
    const EN_DOCUMENT_ID: &str = "00000000-0000-4000-8000-0000000000e4";
    const DE_DOCUMENT_ID: &str = "00000000-0000-4000-8000-0000000000e5";

    fn parse(text: &str) -> Uuid {
        Uuid::parse_str(text).expect("synthetic UUID")
    }

    #[test]
    fn the_german_tailored_request_carries_the_three_facade_keys() {
        let value = german_tailored_value(&parse(PROFILE_ID), &parse(JOB_ID), true);
        assert_eq!(
            value,
            json!({"profileId": PROFILE_ID, "jobId": JOB_ID, "includeProjects": true})
        );
    }

    #[test]
    fn the_german_cover_letter_request_names_its_three_identifiers() {
        let value =
            german_cover_letter_value(&parse(PROFILE_ID), &parse(JOB_ID), &parse(RESUME_ID));
        assert_eq!(
            value,
            json!({"profileId": PROFILE_ID, "jobId": JOB_ID, "resumeId": RESUME_ID})
        );
    }

    #[test]
    fn a_german_tailored_response_maps_to_en_and_de_with_no_path() -> Result<(), serde_json::Error>
    {
        let response = json!({
            "resumeId": RESUME_ID,
            "profileId": PROFILE_ID,
            "jobId": JOB_ID,
            "format": "germany",
            "variants": [
                {"language": "en", "documentId": EN_DOCUMENT_ID, "filePath": "generated/en.pdf",
                 "generatedFile": {"byteSize": 900, "pageCount": 2, "path": "generated/en.pdf"}},
                {"language": "de", "documentId": DE_DOCUMENT_ID, "filePath": "generated/de.pdf",
                 "generatedFile": {"byteSize": 950, "pageCount": 2, "path": "generated/de.pdf"}},
            ],
            "replacedExisting": false,
            "generatedAt": "2026-10-15T00:00:00Z",
        });
        let result = german_tailored_result(&response)
            .ok_or_else(|| serde::de::Error::custom("the German response must map to a result"))?;
        assert_eq!(
            serde_json::to_value(&result)?,
            json!({
                "variant": "germanTailored",
                "resumeId": RESUME_ID,
                "outputs": [
                    {"documentId": EN_DOCUMENT_ID, "labelPrefix": "EN", "byteSize": 900, "pageCount": 2},
                    {"documentId": DE_DOCUMENT_ID, "labelPrefix": "DE", "byteSize": 950, "pageCount": 2},
                ],
            })
        );
        assert!(!serde_json::to_string(&result)?.contains("generated/"));
        Ok(())
    }

    #[test]
    fn a_german_tailored_response_that_is_not_en_then_de_is_the_internal_failure() {
        let variant = |language: &str, document: &str| {
            json!({"language": language, "documentId": document,
                   "generatedFile": {"byteSize": 900, "pageCount": 2}})
        };
        let reversed = json!({
            "resumeId": RESUME_ID,
            "variants": [variant("de", DE_DOCUMENT_ID), variant("en", EN_DOCUMENT_ID)],
        });
        let single = json!({"resumeId": RESUME_ID, "variants": [variant("en", EN_DOCUMENT_ID)]});
        let bad_resume = json!({
            "resumeId": "not-a-uuid",
            "variants": [variant("en", EN_DOCUMENT_ID), variant("de", DE_DOCUMENT_ID)],
        });
        assert!(german_tailored_result(&reversed).is_none());
        assert!(german_tailored_result(&single).is_none());
        assert!(german_tailored_result(&bad_resume).is_none());
        assert_eq!(
            german_tailored_result(&reversed).ok_or_else(internal).err(),
            Some(internal())
        );
    }

    #[test]
    fn a_german_cover_letter_response_maps_to_one_pdf_with_no_path() -> Result<(), serde_json::Error>
    {
        let response = json!({
            "coverLetterId": EN_DOCUMENT_ID,
            "resumeId": RESUME_ID,
            "documentId": DE_DOCUMENT_ID,
            "filePath": "generated/de/letter.pdf",
            "byteSize": 1200,
            "pageCount": 1,
            "language": "de",
        });
        let result = single_result(&response, &response)
            .ok_or_else(|| serde::de::Error::custom("the cover letter must map to a result"))?;
        assert_eq!(result.resume_id, None);
        assert_eq!(result.outputs[0].document_id, DE_DOCUMENT_ID);
        assert!(!serde_json::to_string(&result)?.contains("generated/"));
        Ok(())
    }

    #[test]
    fn a_german_refusal_keeps_its_wire_code_and_the_generic_copy() {
        let refusal = ApplicationError::of(ErrorCode::NotFound);
        assert_eq!(
            generation_error(&refusal),
            CommandError::new("not_found", GENERATION_ERROR)
        );
        let unavailable = ApplicationError::of(ErrorCode::ValidationError);
        assert_eq!(
            generation_error(&unavailable),
            CommandError::new("validation_error", GENERATION_ERROR)
        );
    }

    #[tokio::test]
    async fn capabilities_fail_with_the_startup_error_when_services_did_not_start() {
        let state = AppState::new(Err(jobniac_runtime::services::StartupError));
        assert_eq!(
            run_capabilities(&state).await.err(),
            Some(CommandError::startup_failed())
        );
    }
}
