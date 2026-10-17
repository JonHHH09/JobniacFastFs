//! Command adapters of "Add from link". The fetch runs in Rust,
//! behind the link use case; React only sends the link text and shows the mapped copy.
//!
//! The link text is input only. It is trimmed here, passed to the use case, and never copied into a
//! DTO, an error or a message id, so a link with userinfo does not reach the screen. Failures map to
//! fixed copy ids, whatever text the core error carries.

use std::future::Future;

use jobniac_core::errors::{AppError, ErrorCode, error_envelope};
use jobniac_core::job_links::JobLinkFetchFailure;
use jobniac_core::jobs::model::JobCaptureResult;
use jobniac_runtime::job_links::JobLinkUseCase;
use jobniac_text::java::java_strip;
use serde_json::{Value, json};
use tauri::State;

use crate::error::CommandError;
use crate::job_commands::{JOB_UNCONFIGURED, NOT_CONFIGURED};
use crate::job_dto::JobCaptureDto;
use crate::job_link_dto::JobLinkCapabilitiesDto;
use crate::state::AppState;

/// Message id of a blank link (the form's validation copy; no request is sent for it).
pub const JOB_LINK_VALIDATION: &str = "job.validation.link";
/// Message id of a failed link capture without a listed code.
pub const JOB_LINK_ERROR: &str = "job.error.link";
/// Message id of a page that could not be fetched.
pub const JOB_LINK_UNAVAILABLE: &str = "job.linkError.unavailable";
/// Message id of a fetch that was interrupted.
pub const JOB_LINK_INTERRUPTED: &str = "job.linkError.interrupted";
/// Message id of a link that is not an absolute http(s) URL.
pub const JOB_LINK_INVALID_URL: &str = "job.linkError.invalidUrl";
/// Message id of a link that redirects (redirects are not followed).
pub const JOB_LINK_REDIRECT: &str = "job.linkError.redirect";
/// Message id of a link that resolves to a private or unsafe address.
pub const JOB_LINK_UNSAFE_ADDRESS: &str = "job.linkError.unsafeAddress";
/// Message id of a plain-http link whose host is not an IP literal.
pub const JOB_LINK_IP_LITERAL: &str = "job.linkError.ipLiteral";
/// Message id of a page without usable job content (any refusal of the fetched content).
pub const JOB_LINK_CONTENT: &str = "job.linkError.content";

/// The link capture as the command uses it. The composed use case implements it; tests substitute a
/// fake, so the command logic runs without a database or a network.
pub(crate) trait LinkCapture {
    /// Captures or reuses the job behind `request` (`{ "url": ... }`).
    fn add_from_link(
        &self,
        request: &Value,
    ) -> impl Future<Output = Result<JobCaptureResult, AppError>> + Send;
}

impl LinkCapture for JobLinkUseCase {
    fn add_from_link(
        &self,
        request: &Value,
    ) -> impl Future<Output = Result<JobCaptureResult, AppError>> + Send {
        JobLinkUseCase::add_job_from_link(self, request)
    }
}

/// Whether the link capture is composed. Answered by the runtime; a missing link service hides the
/// row in React.
#[tauri::command]
#[specta::specta]
pub async fn job_link_capabilities_get(
    state: State<'_, AppState>,
) -> Result<JobLinkCapabilitiesDto, CommandError> {
    Ok(capabilities(state.job_links().is_some()))
}

fn capabilities(job_link: bool) -> JobLinkCapabilitiesDto {
    JobLinkCapabilitiesDto { job_link }
}

/// Captures the job behind a public job link, or reuses the job that its canonical URL identifies.
/// The fetch runs in Rust on an owned worker; React gets the job or a fixed copy id.
#[tauri::command]
#[specta::specta]
pub async fn job_add_link(
    state: State<'_, AppState>,
    url: String,
) -> Result<JobCaptureDto, CommandError> {
    run_add_link(&state, &url).await
}

async fn run_add_link(state: &AppState, url: &str) -> Result<JobCaptureDto, CommandError> {
    state.services()?;
    // The link capture shares the job slot and the job close gate.
    let _slot = state.begin_job()?;
    let Some(links) = state.job_links() else {
        // Without the link service (or its database) the capture is not composed at all.
        return Err(CommandError::new(NOT_CONFIGURED, JOB_UNCONFIGURED));
    };
    capture_link(links.as_ref(), url).await
}

/// Refuses a blank link before any request, then captures the job of the trimmed link.
async fn capture_link<S: LinkCapture + ?Sized>(
    service: &S,
    url: &str,
) -> Result<JobCaptureDto, CommandError> {
    let link = java_strip(url);
    if link.is_empty() {
        return Err(CommandError::new("validation_error", JOB_LINK_VALIDATION));
    }
    let request = json!({ "url": link });
    let result = service
        .add_from_link(&request)
        .await
        .map_err(|error| link_error(&error))?;
    Ok(JobCaptureDto::from(&result))
}

/// Maps a core failure to the copy of the link screen. A refusal of the `url` field is classified by
/// its reason (the fetch failure message); a refusal of the fetched content has the content copy;
/// anything else has the generic copy. The wire code is kept.
fn link_error(error: &AppError) -> CommandError {
    let envelope = error_envelope(error);
    let copy = match envelope.code {
        ErrorCode::ValidationError => match envelope.details.get("field").map(String::as_str) {
            Some("fetchedContent") => JOB_LINK_CONTENT,
            Some("url") => envelope
                .details
                .get("reason")
                .map_or(JOB_LINK_ERROR, |reason| reason_copy(reason)),
            _ => JOB_LINK_ERROR,
        },
        _ => JOB_LINK_ERROR,
    };
    CommandError::new(envelope.code.code(), copy)
}

/// The copy of a fetch failure reason (the failure's message), or the generic copy.
fn reason_copy(reason: &str) -> &'static str {
    [
        (JobLinkFetchFailure::Unavailable, JOB_LINK_UNAVAILABLE),
        (JobLinkFetchFailure::Interrupted, JOB_LINK_INTERRUPTED),
        (JobLinkFetchFailure::InvalidUrl, JOB_LINK_INVALID_URL),
        (JobLinkFetchFailure::RedirectNotFollowed, JOB_LINK_REDIRECT),
        (JobLinkFetchFailure::UnsafeAddress, JOB_LINK_UNSAFE_ADDRESS),
        (JobLinkFetchFailure::IpLiteralRequired, JOB_LINK_IP_LITERAL),
    ]
    .into_iter()
    .find(|(failure, _)| failure.message() == reason)
    .map_or(JOB_LINK_ERROR, |(_, copy)| copy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::JOB_BUSY;
    use jobniac_core::env::Environment;
    use jobniac_core::errors::{ApplicationError, validation_error};
    use jobniac_core::jobs::model::{CaptureStatus, JobAggregate, JobRecord};
    use jobniac_runtime::services::compose;
    use std::sync::Mutex;
    use time::OffsetDateTime;
    use uuid::Uuid;

    /// A synthetic link that carries userinfo: it must never reach a DTO, an error or a copy id.
    const SECRET_LINK: &str = "https://synthetic-user:synthetic-secret@example.com/jobs/1";
    const JOB_ID: &str = "00000000-0000-4000-8000-0000000000c1";

    /// A fake capture: records every request and answers with a fixed result.
    struct FakeLinks {
        answer: fn() -> Result<JobCaptureResult, AppError>,
        requests: Mutex<Vec<Value>>,
    }

    impl FakeLinks {
        fn new(answer: fn() -> Result<JobCaptureResult, AppError>) -> Self {
            Self {
                answer,
                requests: Mutex::new(Vec::new()),
            }
        }

        fn requests(&self) -> Vec<Value> {
            self.requests
                .lock()
                .map(|requests| requests.clone())
                .unwrap_or_default()
        }
    }

    impl LinkCapture for FakeLinks {
        fn add_from_link(
            &self,
            request: &Value,
        ) -> impl Future<Output = Result<JobCaptureResult, AppError>> + Send {
            if let Ok(mut requests) = self.requests.lock() {
                requests.push(request.clone());
            }
            std::future::ready((self.answer)())
        }
    }

    fn record() -> JobRecord {
        let when = OffsetDateTime::UNIX_EPOCH;
        JobRecord {
            id: Uuid::parse_str(JOB_ID).unwrap_or(Uuid::nil()),
            source_method: "link".to_owned(),
            source_label: None,
            title: "Synthetic Engineer".to_owned(),
            company: Some("Example Labs".to_owned()),
            location: None,
            description: "Synthetic description".to_owned(),
            experience_requirement: None,
            employment_type: None,
            seniority: None,
            posted_at: None,
            canonical_fingerprint: "fingerprint".to_owned(),
            created_at: when,
            updated_at: when,
            revision: 3,
        }
    }

    fn capture(status: CaptureStatus) -> Result<JobCaptureResult, AppError> {
        Ok(JobCaptureResult {
            status,
            job: JobAggregate {
                job: record(),
                skills: Vec::new(),
                link_ingestion: None,
                text_ingestion: None,
            },
        })
    }

    fn created() -> Result<JobCaptureResult, AppError> {
        capture(CaptureStatus::CreatedJob)
    }

    fn reused() -> Result<JobCaptureResult, AppError> {
        capture(CaptureStatus::ReusedExistingJob)
    }

    /// A refusal of the `url` field whose reason is the fetch failure message, with the link in the
    /// text of the error (as a careless core error could carry it).
    fn url_refusal(reason: &str) -> AppError {
        AppError::Application(validation_error(
            &format!("Invalid job link ingestion request {SECRET_LINK}"),
            Some("url"),
            Some(reason),
        ))
    }

    #[test]
    fn every_fetch_failure_has_its_own_copy() {
        for (failure, copy) in [
            (JobLinkFetchFailure::Unavailable, JOB_LINK_UNAVAILABLE),
            (JobLinkFetchFailure::Interrupted, JOB_LINK_INTERRUPTED),
            (JobLinkFetchFailure::InvalidUrl, JOB_LINK_INVALID_URL),
            (JobLinkFetchFailure::RedirectNotFollowed, JOB_LINK_REDIRECT),
            (JobLinkFetchFailure::UnsafeAddress, JOB_LINK_UNSAFE_ADDRESS),
            (JobLinkFetchFailure::IpLiteralRequired, JOB_LINK_IP_LITERAL),
        ] {
            assert_eq!(reason_copy(failure.message()), copy);
            assert_eq!(
                link_error(&url_refusal(failure.message())),
                CommandError::new("validation_error", copy)
            );
        }
    }

    #[test]
    fn a_refused_fetched_content_has_the_content_copy_whatever_its_text() {
        let refusal = AppError::Application(validation_error(
            "Invalid job request",
            Some("fetchedContent"),
            Some("job page fetch returned HTTP 403"),
        ));
        assert_eq!(
            link_error(&refusal),
            CommandError::new("validation_error", JOB_LINK_CONTENT)
        );
    }

    #[test]
    fn an_unlisted_refusal_and_a_failure_have_the_generic_copy() {
        assert_eq!(
            link_error(&url_refusal("must not be blank")),
            CommandError::new("validation_error", JOB_LINK_ERROR)
        );
        assert_eq!(
            link_error(&AppError::Internal),
            CommandError::new("internal_error", JOB_LINK_ERROR)
        );
        let other_field = AppError::Application(validation_error(
            "Invalid job request",
            Some("postedAt"),
            Some("must be a timestamp"),
        ));
        assert_eq!(
            link_error(&other_field),
            CommandError::new("validation_error", JOB_LINK_ERROR)
        );
    }

    #[test]
    fn no_copy_id_or_code_carries_the_link_text() {
        for error in [
            url_refusal(JobLinkFetchFailure::Unavailable.message()),
            url_refusal("unknown"),
            AppError::Application(ApplicationError::of(ErrorCode::InternalError)),
        ] {
            let mapped = link_error(&error);
            assert!(!mapped.message_id.contains("synthetic"));
            assert!(!mapped.code.contains("synthetic"));
            assert!(!format!("{mapped:?}").contains("synthetic-secret"));
        }
    }

    #[tokio::test]
    async fn a_blank_link_is_refused_before_any_request() {
        let fake = FakeLinks::new(created);
        assert_eq!(
            capture_link(&fake, "  \t ").await.err(),
            Some(CommandError::new("validation_error", JOB_LINK_VALIDATION))
        );
        assert!(fake.requests().is_empty());
    }

    #[tokio::test]
    async fn the_trimmed_link_is_the_only_request_value() -> Result<(), CommandError> {
        let fake = FakeLinks::new(created);
        capture_link(&fake, &format!("  {SECRET_LINK}  ")).await?;
        assert_eq!(fake.requests(), vec![json!({ "url": SECRET_LINK })]);
        Ok(())
    }

    #[tokio::test]
    async fn a_new_job_is_reported_as_created_and_a_known_link_as_reused()
    -> Result<(), Box<dyn std::error::Error>> {
        let created_dto = capture_link(&FakeLinks::new(created), SECRET_LINK).await?;
        let created_json = serde_json::to_value(&created_dto)?;
        assert_eq!(created_json["status"], "created_job");
        let reused_dto = capture_link(&FakeLinks::new(reused), SECRET_LINK).await?;
        let reused_json = serde_json::to_value(&reused_dto)?;
        assert_eq!(reused_json["status"], "reused_existing_job");
        assert_eq!(reused_json["job"]["job"]["id"], JOB_ID);
        assert_eq!(reused_json["job"]["job"]["revision"], "3");
        assert_eq!(reused_json["job"]["job"]["sourceMethod"], "link");
        Ok(())
    }

    #[tokio::test]
    async fn a_refusal_of_the_capture_keeps_no_link_text() {
        let fake =
            FakeLinks::new(|| Err(url_refusal(JobLinkFetchFailure::UnsafeAddress.message())));
        let refusal = capture_link(&fake, SECRET_LINK).await.err();
        assert_eq!(
            refusal,
            Some(CommandError::new(
                "validation_error",
                JOB_LINK_UNSAFE_ADDRESS
            ))
        );
    }

    #[tokio::test]
    async fn without_a_composed_link_service_the_capture_is_not_configured() {
        let state = AppState::new(compose(&Environment::default()));
        assert_eq!(
            run_add_link(&state, SECRET_LINK).await.err(),
            Some(CommandError::new(NOT_CONFIGURED, JOB_UNCONFIGURED))
        );
        assert!(!capabilities(state.job_links().is_some()).job_link);
    }

    #[tokio::test]
    async fn a_finished_capture_releases_the_job_slot_on_every_path() {
        // The refusal without a link service is an early return: the slot must still be released.
        let state = AppState::new(compose(&Environment::default()));
        assert!(run_add_link(&state, SECRET_LINK).await.is_err());
        assert!(state.begin_job().is_ok(), "the job slot must be free again");
    }

    #[tokio::test]
    async fn a_capture_while_a_job_operation_runs_is_refused_as_busy() {
        let state = AppState::new(compose(&Environment::default()));
        let _held = state.begin_job();
        assert_eq!(
            run_add_link(&state, SECRET_LINK).await.err(),
            Some(CommandError::busy(JOB_BUSY))
        );
    }

    #[test]
    fn a_running_link_capture_refuses_the_window_close_as_a_busy_job() {
        let state = AppState::new(compose(&Environment::default()));
        let held = state.begin_job();
        assert_eq!(
            state.close_refusal().map(|refusal| refusal.message_id),
            Some(crate::state::JOB_CLOSE)
        );
        drop(held);
        assert_eq!(state.close_refusal(), None);
    }

    #[tokio::test]
    async fn a_startup_failure_is_returned_for_the_link_commands() {
        let state = AppState::new(Err(jobniac_runtime::services::StartupError));
        assert_eq!(
            run_add_link(&state, SECRET_LINK).await.err(),
            Some(CommandError::startup_failed())
        );
    }

    #[test]
    fn the_capability_follows_the_composed_link_service() {
        assert!(capabilities(true).job_link);
        assert!(!capabilities(false).job_link);
    }
}
