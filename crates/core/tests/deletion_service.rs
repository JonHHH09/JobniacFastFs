//! The framework-free deletion use cases. All data is synthetic: the repositories are in-memory
//! recorders.

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::deletion::{
    EntityDeletion, JobDeletionRepository, JobDeletionService, ProfileDeletionRepository,
    ProfileDeletionService,
};
use jobniac_core::errors::{AppError, ApplicationError, ErrorCode, conflict, error_envelope};
use serde_json::json;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

/// Records the ids it is called with and answers a fixed outcome.
#[derive(Debug)]
struct Repository {
    deleted: bool,
    calls: Mutex<Vec<Uuid>>,
    failure: Option<ApplicationError>,
}

impl Repository {
    fn new(deleted: bool) -> Arc<Self> {
        Arc::new(Self {
            deleted,
            calls: Mutex::new(Vec::new()),
            failure: None,
        })
    }

    fn failing(error: ApplicationError) -> Arc<Self> {
        Arc::new(Self {
            deleted: true,
            calls: Mutex::new(Vec::new()),
            failure: Some(error),
        })
    }

    fn calls(&self) -> Vec<Uuid> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn outcome(&self, id: Uuid) -> Result<EntityDeletion, AppError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(id);
        if let Some(error) = &self.failure {
            return Err(error.clone().into());
        }
        Ok(EntityDeletion {
            deleted: self.deleted,
            retired_asset_ids: if self.deleted {
                vec![Uuid::from_u128(7)]
            } else {
                Vec::new()
            },
        })
    }
}

#[async_trait]
impl ProfileDeletionRepository for Repository {
    async fn delete_profile(&self, profile_id: Uuid) -> Result<EntityDeletion, AppError> {
        self.outcome(profile_id)
    }
}

#[async_trait]
impl JobDeletionRepository for Repository {
    async fn delete_job(&self, job_id: Uuid) -> Result<EntityDeletion, AppError> {
        self.outcome(job_id)
    }
}

fn profile_service(repository: &Arc<Repository>) -> ProfileDeletionService {
    ProfileDeletionService::new(Arc::clone(repository) as Arc<dyn ProfileDeletionRepository>)
}

fn job_service(repository: &Arc<Repository>) -> JobDeletionService {
    JobDeletionService::new(Arc::clone(repository) as Arc<dyn JobDeletionRepository>)
}

#[tokio::test]
async fn delete_profile_reports_the_flag_without_error() -> TestResult {
    for deleted in [true, false] {
        let repository = Repository::new(deleted);
        let profile_id = Uuid::new_v4();
        let text = profile_id.to_string().to_uppercase();
        let result = profile_service(&repository).delete_profile(&text).await?;
        assert_eq!(
            result,
            json!({"profileId": profile_id.to_string(), "deleted": deleted})
        );
        assert_eq!(repository.calls(), vec![profile_id]);
    }
    Ok(())
}

#[tokio::test]
async fn delete_profile_never_puts_retired_asset_ids_on_the_wire() -> TestResult {
    let repository = Repository::new(true);
    let result = profile_service(&repository)
        .delete_profile(&Uuid::new_v4().to_string())
        .await?;
    assert_eq!(result.as_object().map(|object| object.len()), Some(2));
    assert!(!result.to_string().contains(&Uuid::from_u128(7).to_string()));
    Ok(())
}

#[tokio::test]
async fn delete_job_returns_deleted_true() -> TestResult {
    let repository = Repository::new(true);
    let job_id = Uuid::new_v4();
    let result = job_service(&repository)
        .delete_job(&job_id.to_string())
        .await?;
    assert_eq!(
        result,
        json!({"jobId": job_id.to_string(), "deleted": true})
    );
    assert_eq!(repository.calls(), vec![job_id]);
    Ok(())
}

#[tokio::test]
async fn delete_job_missing_is_not_found_with_the_fixed_details() -> TestResult {
    let job_id = Uuid::new_v4();
    let repository = Repository::new(false);
    let Err(error) = job_service(&repository)
        .delete_job(&job_id.to_string())
        .await
    else {
        return Err("a missing job must not succeed".into());
    };
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::NotFound);
    assert_eq!(envelope.message, format!("Job not found: {job_id}"));
    let details: Vec<(&str, &str)> = envelope
        .details
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        details,
        vec![("resource", "job"), ("jobId", job_id.to_string().as_str())]
    );
    Ok(())
}

#[tokio::test]
async fn invalid_ids_never_reach_the_repository() -> TestResult {
    let simple = Uuid::new_v4().simple().to_string();
    let spaced = format!(" {}", Uuid::new_v4());
    let braced = format!("{{{}}}", Uuid::new_v4());
    let values = [
        "",
        "not-a-uuid",
        spaced.as_str(),
        simple.as_str(),
        braced.as_str(),
    ];
    for value in values {
        let repository = Repository::new(true);
        let Err(error) = profile_service(&repository).delete_profile(value).await else {
            return Err(format!("profile id {value:?} must be refused").into());
        };
        assert_validation(&error, "profileId", "Invalid profile request");
        let Err(error) = job_service(&repository).delete_job(value).await else {
            return Err(format!("job id {value:?} must be refused").into());
        };
        assert_validation(&error, "jobId", "Invalid job request");
        assert!(repository.calls().is_empty());
    }
    Ok(())
}

fn assert_validation(error: &AppError, field: &str, message: &str) {
    let envelope = error_envelope(error);
    assert_eq!(envelope.code, ErrorCode::ValidationError);
    assert_eq!(envelope.message, message);
    assert_eq!(
        envelope.details.get("field").map(String::as_str),
        Some(field)
    );
    assert_eq!(
        envelope.details.get("reason").map(String::as_str),
        Some("must be a valid UUID")
    );
}

#[tokio::test]
async fn repository_conflict_propagates() -> TestResult {
    let repository = Repository::failing(conflict(
        "Profile has a pending generation",
        [("reason", "generation_pending")],
    ));
    let Err(error) = profile_service(&repository)
        .delete_profile(&Uuid::new_v4().to_string())
        .await
    else {
        return Err("a pending generation must refuse the delete".into());
    };
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::Conflict);
    assert_eq!(envelope.message, "Profile has a pending generation");
    assert_eq!(
        envelope.details.get("reason").map(String::as_str),
        Some("generation_pending")
    );
    Ok(())
}
