//! Profile and job delete command adapters.
//!
//! The delete is two steps. A request issues a confirmation token for one entity id, in Rust, and
//! deletes nothing. A confirm runs the delete only with that token: the token is single use, bound to
//! the entity kind and id, and short lived. The confirm takes the workspace's single-flight slot, so a
//! running delete is busy to the other commands and to the window close gate. The use cases do the
//! validation of record; this module admits the identifier and maps failures to fixed copy.

use std::time::Instant;

use jobniac_core::deletion::{JobDeletionService, ProfileDeletionService};
use jobniac_core::errors::{AppError, ErrorCode, error_envelope};
use jobniac_core::ids::admit_uuid;
use jobniac_text::str_rules::strip_text;
use serde_json::Value;
use tauri::State;
use uuid::Uuid;

use crate::delete_dto::{
    DeleteConfirmationDto, DeletionCapabilitiesDto, JobDeleteResultDto, ProfileDeleteResultDto,
};
use crate::error::CommandError;
use crate::profile_commands::NOT_CONFIGURED;
use crate::state::{AppState, DeleteKind};

/// Fixed copy of a profile delete that failed and cannot be shown as a specific case.
pub const DELETE_PROFILE_FAILED: &str = "delete.failed.profile";

/// Fixed copy of a job delete that failed and cannot be shown as a specific case.
pub const DELETE_JOB_FAILED: &str = "delete.failed.job";

/// Fixed copy of a delete refused while a generation for the entity is pending (wire code `conflict`).
pub const DELETE_CONFLICT: &str = "delete.failed.conflict";

/// Copy of a job delete whose job no longer exists; the job leaves the list (wire code `not_found`).
pub const JOB_DELETED_MISSING: &str = "job.deleted.missing";

/// The wire code of an identifier or token the boundary refuses before the use case runs.
const VALIDATION: &str = "validation_error";

/// The wire code of a result the screen cannot read.
const INTERNAL: &str = "internal_error";

/// The fixed failure copy of one kind. The screen shows it for any code it does not map itself.
fn failed_message(kind: DeleteKind) -> &'static str {
    match kind {
        DeleteKind::Profile => DELETE_PROFILE_FAILED,
        DeleteKind::Job => DELETE_JOB_FAILED,
    }
}

/// Reports whether the delete use cases are composed. Read-only:
/// it takes no single-flight slot, so a screen can ask once an entity is loaded.
#[tauri::command]
#[specta::specta]
pub async fn deletion_capabilities_get(
    state: State<'_, AppState>,
) -> Result<DeletionCapabilitiesDto, CommandError> {
    let services = state.services()?;
    Ok(capabilities(
        services.deletion.profile.is_some() && services.deletion.job.is_some(),
    ))
}

/// Step one of a profile delete: issues the confirmation token for this profile. Nothing is deleted.
#[tauri::command]
#[specta::specta]
pub async fn request_profile_delete(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<DeleteConfirmationDto, CommandError> {
    let services = state.services()?;
    request_token(
        &state,
        DeleteKind::Profile,
        &profile_id,
        services.deletion.profile.is_some(),
        Instant::now(),
    )
}

/// Step two of a profile delete: deletes the profile when the token is the one issued for it.
#[tauri::command]
#[specta::specta]
pub async fn delete_profile(
    state: State<'_, AppState>,
    profile_id: String,
    confirmation_token: String,
) -> Result<ProfileDeleteResultDto, CommandError> {
    let services = state.services()?;
    run_delete_profile(
        &state,
        &profile_id,
        &confirmation_token,
        services.deletion.profile.as_deref(),
        Instant::now(),
    )
    .await
}

/// Step one of a job delete: issues the confirmation token for this job. Nothing is deleted.
#[tauri::command]
#[specta::specta]
pub async fn request_job_delete(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<DeleteConfirmationDto, CommandError> {
    let services = state.services()?;
    request_token(
        &state,
        DeleteKind::Job,
        &job_id,
        services.deletion.job.is_some(),
        Instant::now(),
    )
}

/// Step two of a job delete: deletes the job when the token is the one issued for it.
#[tauri::command]
#[specta::specta]
pub async fn delete_job(
    state: State<'_, AppState>,
    job_id: String,
    confirmation_token: String,
) -> Result<JobDeleteResultDto, CommandError> {
    let services = state.services()?;
    run_delete_job(
        &state,
        &job_id,
        &confirmation_token,
        services.deletion.job.as_deref(),
        Instant::now(),
    )
    .await
}

/// The capability answer: one flag, because both use cases are composed together.
fn capabilities(deletion: bool) -> DeletionCapabilitiesDto {
    DeletionCapabilitiesDto { deletion }
}

/// Issues the token of a request. A missing use case refuses before any token exists.
fn request_token(
    state: &AppState,
    kind: DeleteKind,
    entity_text: &str,
    available: bool,
    now: Instant,
) -> Result<DeleteConfirmationDto, CommandError> {
    let entity_id = admit_id(entity_text, kind)?;
    if !available {
        return Err(unavailable(kind));
    }
    Ok(DeleteConfirmationDto {
        confirmation_token: state.issue_deletion_token(kind, entity_id, now),
    })
}

/// The profile confirm: admit, check the use case, take the slot, consume the token, then delete.
async fn run_delete_profile(
    state: &AppState,
    profile_id: &str,
    token: &str,
    service: Option<&ProfileDeletionService>,
    now: Instant,
) -> Result<ProfileDeleteResultDto, CommandError> {
    let kind = DeleteKind::Profile;
    let entity_id = admit_id(profile_id, kind)?;
    let service = service.ok_or_else(|| unavailable(kind))?;
    // The slot comes before the token, so a busy refusal leaves the token usable for a retry.
    let _slot = state.begin_profile()?;
    confirm_token(state, kind, entity_id, token, now)?;
    let outcome = service
        .delete_profile(&entity_id.to_string())
        .await
        .map_err(|error| delete_error(kind, &error))?;
    Ok(ProfileDeleteResultDto {
        profile_id: entity_id.to_string(),
        deleted: deleted_flag(kind, &outcome)?,
    })
}

/// The job confirm: the same order as the profile confirm.
async fn run_delete_job(
    state: &AppState,
    job_id: &str,
    token: &str,
    service: Option<&JobDeletionService>,
    now: Instant,
) -> Result<JobDeleteResultDto, CommandError> {
    let kind = DeleteKind::Job;
    let entity_id = admit_id(job_id, kind)?;
    let service = service.ok_or_else(|| unavailable(kind))?;
    let _slot = state.begin_job()?;
    confirm_token(state, kind, entity_id, token, now)?;
    let outcome = service
        .delete_job(&entity_id.to_string())
        .await
        .map_err(|error| delete_error(kind, &error))?;
    Ok(JobDeleteResultDto {
        job_id: entity_id.to_string(),
        deleted: deleted_flag(kind, &outcome)?,
    })
}

/// Admits an entity identifier: trimmed, lowercased, then a canonical UUID. A malformed one is
/// a validation error with the fixed failure copy, before any use case runs.
fn admit_id(text: &str, kind: DeleteKind) -> Result<Uuid, CommandError> {
    admit_uuid(&strip_text(text).to_ascii_lowercase())
        .map_err(|_| CommandError::new(VALIDATION, failed_message(kind)))
}

/// Consumes the pending confirmation. A missing, reused, expired, mismatched or foreign token is the
/// same validation error, and the delete never runs without it.
fn confirm_token(
    state: &AppState,
    kind: DeleteKind,
    entity_id: Uuid,
    token: &str,
    now: Instant,
) -> Result<(), CommandError> {
    if state.take_deletion_token(kind, entity_id, token, now) {
        Ok(())
    } else {
        Err(CommandError::new(VALIDATION, failed_message(kind)))
    }
}

/// The use case reports `deleted` as a boolean; anything else is an internal failure.
fn deleted_flag(kind: DeleteKind, outcome: &Value) -> Result<bool, CommandError> {
    outcome
        .get("deleted")
        .and_then(Value::as_bool)
        .ok_or_else(|| CommandError::new(INTERNAL, failed_message(kind)))
}

/// The copy of a delete whose use case is not composed. The screen hides the control in that case.
fn unavailable(kind: DeleteKind) -> CommandError {
    CommandError::new(NOT_CONFIGURED, failed_message(kind))
}

/// Maps a use-case failure to the wire code and a fixed copy id. The service text is never sent.
/// A generation in progress maps to its own copy; a job that is already gone maps to its copy.
fn delete_error(kind: DeleteKind, error: &AppError) -> CommandError {
    let code = error_envelope(error).code;
    let message_id = match (code, kind) {
        (ErrorCode::Conflict, _) => DELETE_CONFLICT,
        (ErrorCode::NotFound, DeleteKind::Job) => JOB_DELETED_MISSING,
        _ => failed_message(kind),
    };
    CommandError::new(code.code(), message_id)
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use jobniac_core::deletion::{
        EntityDeletion, JobDeletionRepository, JobDeletionService, ProfileDeletionRepository,
        ProfileDeletionService,
    };
    use jobniac_core::env::Environment;
    use jobniac_core::errors::ApplicationError;
    use jobniac_runtime::services::compose;

    use super::*;
    use crate::state::{CloseRefusal, DELETE_TOKEN_TTL, JOB_CLOSE, PROFILE_CLOSE};

    type TestResult = Result<(), String>;

    const PROFILE_ID: &str = "00000000-0000-4000-8000-0000000000a1";
    const OTHER_PROFILE_ID: &str = "00000000-0000-4000-8000-0000000000a2";
    const JOB_ID: &str = "00000000-0000-4000-8000-0000000000d3";

    /// A repository double: counts the calls and answers the configured outcome. `Ok(deleted)` is a
    /// result, `Err(code)` a use-case failure with that wire code.
    #[derive(Debug)]
    struct FakeRepository {
        outcome: Result<bool, ErrorCode>,
        calls: Arc<AtomicUsize>,
    }

    impl FakeRepository {
        fn respond(&self) -> Result<EntityDeletion, AppError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.outcome {
                Ok(deleted) => Ok(EntityDeletion {
                    deleted,
                    retired_asset_ids: Vec::new(),
                }),
                Err(code) => Err(AppError::Application(ApplicationError::of(code))),
            }
        }
    }

    // The trait methods are `async fn`s in the core; this is the same signature without the macro.
    impl ProfileDeletionRepository for FakeRepository {
        fn delete_profile<'life0, 'async_trait>(
            &'life0 self,
            _profile_id: Uuid,
        ) -> Pin<Box<dyn Future<Output = Result<EntityDeletion, AppError>> + Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            let outcome = self.respond();
            Box::pin(async move { outcome })
        }
    }

    impl JobDeletionRepository for FakeRepository {
        fn delete_job<'life0, 'async_trait>(
            &'life0 self,
            _job_id: Uuid,
        ) -> Pin<Box<dyn Future<Output = Result<EntityDeletion, AppError>> + Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            let outcome = self.respond();
            Box::pin(async move { outcome })
        }
    }

    fn state() -> AppState {
        AppState::new(compose(&Environment::default()))
    }

    fn profile_service(
        outcome: Result<bool, ErrorCode>,
    ) -> (ProfileDeletionService, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let repository = FakeRepository {
            outcome,
            calls: Arc::clone(&calls),
        };
        (ProfileDeletionService::new(Arc::new(repository)), calls)
    }

    fn job_service(outcome: Result<bool, ErrorCode>) -> (JobDeletionService, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let repository = FakeRepository {
            outcome,
            calls: Arc::clone(&calls),
        };
        (JobDeletionService::new(Arc::new(repository)), calls)
    }

    fn issue(state: &AppState, kind: DeleteKind, id: &str) -> Result<String, String> {
        let token = request_token(state, kind, id, true, Instant::now())
            .map_err(|error| format!("{error:?}"))?;
        Ok(token.confirmation_token)
    }

    fn refused(error: CommandError, code: &str, message_id: &str) -> TestResult {
        if error.code == code && error.message_id == message_id {
            Ok(())
        } else {
            Err(format!("unexpected refusal {error:?}"))
        }
    }

    #[tokio::test]
    async fn a_token_issued_for_a_profile_deletes_it_once() -> TestResult {
        let state = state();
        let (service, calls) = profile_service(Ok(true));
        let token = issue(&state, DeleteKind::Profile, PROFILE_ID)?;
        let result = run_delete_profile(&state, PROFILE_ID, &token, Some(&service), Instant::now())
            .await
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(
            result,
            ProfileDeleteResultDto {
                profile_id: PROFILE_ID.to_owned(),
                deleted: true,
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn a_reused_token_is_refused_and_the_delete_does_not_run_again() -> TestResult {
        let state = state();
        let (service, calls) = profile_service(Ok(true));
        let token = issue(&state, DeleteKind::Profile, PROFILE_ID)?;
        run_delete_profile(&state, PROFILE_ID, &token, Some(&service), Instant::now())
            .await
            .map_err(|error| format!("{error:?}"))?;
        let second =
            run_delete_profile(&state, PROFILE_ID, &token, Some(&service), Instant::now()).await;
        refused(
            second.err().ok_or("a reused token must be refused")?,
            VALIDATION,
            DELETE_PROFILE_FAILED,
        )?;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn a_token_for_another_profile_is_refused_and_consumed() -> TestResult {
        let state = state();
        let (service, calls) = profile_service(Ok(true));
        let token = issue(&state, DeleteKind::Profile, PROFILE_ID)?;
        let other = run_delete_profile(
            &state,
            OTHER_PROFILE_ID,
            &token,
            Some(&service),
            Instant::now(),
        )
        .await;
        refused(
            other
                .err()
                .ok_or("a token for another id must be refused")?,
            VALIDATION,
            DELETE_PROFILE_FAILED,
        )?;
        // The mismatch consumed the pending token, so the right id no longer works either.
        let own =
            run_delete_profile(&state, PROFILE_ID, &token, Some(&service), Instant::now()).await;
        assert!(own.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn a_profile_token_cannot_delete_a_job() -> TestResult {
        let state = state();
        let (service, calls) = job_service(Ok(true));
        let token = issue(&state, DeleteKind::Profile, PROFILE_ID)?;
        let result =
            run_delete_job(&state, PROFILE_ID, &token, Some(&service), Instant::now()).await;
        refused(
            result
                .err()
                .ok_or("a profile token must not delete a job")?,
            VALIDATION,
            DELETE_JOB_FAILED,
        )?;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn an_expired_token_is_refused() -> TestResult {
        let state = state();
        let (service, calls) = profile_service(Ok(true));
        let issued = Instant::now();
        let entity = admit_id(PROFILE_ID, DeleteKind::Profile).map_err(|e| format!("{e:?}"))?;
        let token = state.issue_deletion_token(DeleteKind::Profile, entity, issued);
        let later = issued + DELETE_TOKEN_TTL + Duration::from_secs(1);
        let result = run_delete_profile(&state, PROFILE_ID, &token, Some(&service), later).await;
        refused(
            result.err().ok_or("an expired token must be refused")?,
            VALIDATION,
            DELETE_PROFILE_FAILED,
        )?;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn a_token_is_accepted_up_to_its_lifetime() -> TestResult {
        let state = state();
        let (service, _calls) = profile_service(Ok(true));
        let issued = Instant::now();
        let entity = admit_id(PROFILE_ID, DeleteKind::Profile).map_err(|e| format!("{e:?}"))?;
        let token = state.issue_deletion_token(DeleteKind::Profile, entity, issued);
        let at_limit = issued + DELETE_TOKEN_TTL;
        let result = run_delete_profile(&state, PROFILE_ID, &token, Some(&service), at_limit).await;
        assert!(result.is_ok(), "{result:?}");
        Ok(())
    }

    #[tokio::test]
    async fn an_unknown_token_is_refused_without_a_request() -> TestResult {
        let state = state();
        let (service, calls) = profile_service(Ok(true));
        issue(&state, DeleteKind::Profile, PROFILE_ID)?;
        let result = run_delete_profile(
            &state,
            PROFILE_ID,
            "not-issued",
            Some(&service),
            Instant::now(),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn a_delete_without_the_use_case_is_disabled() -> TestResult {
        let state = state();
        let entity = admit_id(PROFILE_ID, DeleteKind::Profile).map_err(|e| format!("{e:?}"))?;
        let token = state.issue_deletion_token(DeleteKind::Profile, entity, Instant::now());
        let result = run_delete_profile(&state, PROFILE_ID, &token, None, Instant::now()).await;
        refused(
            result.err().ok_or("a missing use case must refuse")?,
            NOT_CONFIGURED,
            DELETE_PROFILE_FAILED,
        )?;
        let request = request_token(&state, DeleteKind::Job, JOB_ID, false, Instant::now());
        refused(
            request
                .err()
                .ok_or("a missing use case must refuse the request")?,
            NOT_CONFIGURED,
            DELETE_JOB_FAILED,
        )
    }

    #[test]
    fn the_capability_is_on_only_when_the_use_cases_are_composed() {
        assert!(!capabilities(false).deletion);
        assert!(capabilities(true).deletion);
        let composed = compose(&Environment::default())
            .map(|services| services.deletion.profile.is_some() || services.deletion.job.is_some());
        assert_eq!(composed.ok(), Some(false));
    }

    #[test]
    fn a_malformed_identifier_is_refused_before_any_token_or_call() -> TestResult {
        let state = state();
        let request = request_token(
            &state,
            DeleteKind::Profile,
            "not-a-uuid",
            true,
            Instant::now(),
        );
        refused(
            request.err().ok_or("a malformed id must be refused")?,
            VALIDATION,
            DELETE_PROFILE_FAILED,
        )?;
        assert_eq!(
            admit_id(
                " 00000000-0000-4000-8000-0000000000A1 ",
                DeleteKind::Profile
            )
            .map_err(|error| format!("{error:?}"))?
            .to_string(),
            PROFILE_ID
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_conflict_keeps_its_code_and_maps_to_the_generation_copy() -> TestResult {
        let state = state();
        let (service, _calls) = profile_service(Err(ErrorCode::Conflict));
        let token = issue(&state, DeleteKind::Profile, PROFILE_ID)?;
        let result =
            run_delete_profile(&state, PROFILE_ID, &token, Some(&service), Instant::now()).await;
        refused(
            result.err().ok_or("a conflict must fail the delete")?,
            "conflict",
            DELETE_CONFLICT,
        )
    }

    #[tokio::test]
    async fn a_missing_job_maps_to_its_own_copy_and_other_codes_to_the_failure_copy() -> TestResult
    {
        let state = state();
        let (service, _calls) = job_service(Err(ErrorCode::NotFound));
        let token = issue(&state, DeleteKind::Job, JOB_ID)?;
        let result = run_delete_job(&state, JOB_ID, &token, Some(&service), Instant::now()).await;
        refused(
            result.err().ok_or("a missing job must fail the delete")?,
            "not_found",
            JOB_DELETED_MISSING,
        )?;
        let (service, _calls) = job_service(Err(ErrorCode::InternalError));
        let token = issue(&state, DeleteKind::Job, JOB_ID)?;
        let other = run_delete_job(&state, JOB_ID, &token, Some(&service), Instant::now()).await;
        refused(
            other
                .err()
                .ok_or("an internal failure must fail the delete")?,
            "internal_error",
            DELETE_JOB_FAILED,
        )
    }

    #[tokio::test]
    async fn a_profile_that_was_already_gone_is_not_an_error() -> TestResult {
        let state = state();
        let (service, _calls) = profile_service(Ok(false));
        let token = issue(&state, DeleteKind::Profile, PROFILE_ID)?;
        let result = run_delete_profile(&state, PROFILE_ID, &token, Some(&service), Instant::now())
            .await
            .map_err(|error| format!("{error:?}"))?;
        assert!(!result.deleted);
        Ok(())
    }

    #[tokio::test]
    async fn a_busy_delete_is_refused_and_keeps_the_token_for_a_retry() -> TestResult {
        let state = state();
        let (service, calls) = profile_service(Ok(true));
        let token = issue(&state, DeleteKind::Profile, PROFILE_ID)?;
        {
            let _held = state
                .begin_profile()
                .map_err(|error| format!("{error:?}"))?;
            let busy =
                run_delete_profile(&state, PROFILE_ID, &token, Some(&service), Instant::now())
                    .await;
            refused(
                busy.err()
                    .ok_or("a running profile operation must refuse the delete")?,
                "busy",
                "profile.busy",
            )?;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let retried =
            run_delete_profile(&state, PROFILE_ID, &token, Some(&service), Instant::now()).await;
        assert!(retried.is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn a_running_job_delete_refuses_the_job_close_and_releases_it() -> TestResult {
        let state = state();
        let (service, _calls) = job_service(Ok(true));
        let token = issue(&state, DeleteKind::Job, JOB_ID)?;
        let closing = {
            let _slot = state.begin_job().map_err(|error| format!("{error:?}"))?;
            state.close_refusal()
        };
        assert_eq!(
            closing,
            Some(CloseRefusal {
                workspace: "job",
                reason: "busy",
                message_id: JOB_CLOSE,
            })
        );
        let result = run_delete_job(&state, JOB_ID, &token, Some(&service), Instant::now()).await;
        assert!(result.is_ok());
        assert_eq!(state.close_refusal(), None);
        Ok(())
    }

    #[test]
    fn a_running_profile_operation_refuses_the_profile_close() -> TestResult {
        let state = state();
        let _slot = state
            .begin_profile()
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "profile",
                reason: "busy",
                message_id: PROFILE_CLOSE,
            })
        );
        Ok(())
    }
}
