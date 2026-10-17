//! Profile and job hard-delete use cases.
//!
//! The wire contract: `delete_profile` reports `deleted: false` for a
//! missing profile, `delete_job` answers `not_found`. The repository owns one transaction in which
//! the parent and its dependent history are removed together with the durable retirement of every
//! owned, now-unreferenced generated asset. It answers `conflict` while a generation for the entity
//! is pending. Documents, blobs and shared files are never deleted here.
//!
//! The retired asset ids are returned to the caller, but they never reach the wire.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::errors::{AppError, not_found};
use crate::ids::admit_uuid;

/// The outcome of one atomic deletion. `retired_asset_ids` are sorted and cleanup-pending.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EntityDeletion {
    /// Whether the root row existed and was removed.
    pub deleted: bool,
    /// The owned generated assets retired by this deletion, sorted by UUID.
    pub retired_asset_ids: Vec<Uuid>,
}

/// The profile hard-delete port.
#[async_trait]
pub trait ProfileDeletionRepository: std::fmt::Debug + Send + Sync {
    /// Delete the profile and its history atomically.
    async fn delete_profile(&self, profile_id: Uuid) -> Result<EntityDeletion, AppError>;
}

/// The job hard-delete port.
#[async_trait]
pub trait JobDeletionRepository: std::fmt::Debug + Send + Sync {
    /// Delete the job and its history atomically.
    async fn delete_job(&self, job_id: Uuid) -> Result<EntityDeletion, AppError>;
}

/// Profile deletion use case (`ProfileDeletionService`).
#[derive(Debug, Clone)]
pub struct ProfileDeletionService {
    repository: Arc<dyn ProfileDeletionRepository>,
}

impl ProfileDeletionService {
    /// A service over `repository`.
    #[must_use]
    pub fn new(repository: Arc<dyn ProfileDeletionRepository>) -> Self {
        Self { repository }
    }

    /// Delete a profile by its UUID text. Returns `{profileId, deleted}`; a missing profile is not
    /// an error.
    ///
    /// # Errors
    ///
    /// `validation_error` for a malformed identifier (before any repository call), or the
    /// repository's error (`conflict` while a generation is pending).
    pub async fn delete_profile(&self, profile_id: &str) -> Result<Value, AppError> {
        let parsed = parse_id(profile_id, "profileId", "Invalid profile request")?;
        let outcome = self.repository.delete_profile(parsed).await?;
        Ok(json!({"profileId": parsed.to_string(), "deleted": outcome.deleted}))
    }
}

/// Job deletion use case (`JobDeletionService`).
#[derive(Debug, Clone)]
pub struct JobDeletionService {
    repository: Arc<dyn JobDeletionRepository>,
}

impl JobDeletionService {
    /// A service over `repository`.
    #[must_use]
    pub fn new(repository: Arc<dyn JobDeletionRepository>) -> Self {
        Self { repository }
    }

    /// Delete a job by its UUID text. Returns `{jobId, deleted: true}`; a missing job is
    /// `not_found`.
    ///
    /// # Errors
    ///
    /// `validation_error` for a malformed identifier, `not_found` for a missing job, or the
    /// repository's error.
    pub async fn delete_job(&self, job_id: &str) -> Result<Value, AppError> {
        let parsed = parse_id(job_id, "jobId", "Invalid job request")?;
        if !self.repository.delete_job(parsed).await?.deleted {
            let message = format!("Job not found: {parsed}");
            let details = [
                ("resource".to_owned(), "job".to_owned()),
                ("jobId".to_owned(), parsed.to_string()),
            ];
            return Err(not_found(&message, details).into());
        }
        Ok(json!({"jobId": parsed.to_string(), "deleted": true}))
    }
}

/// Admits an identifier with the boundary's field and message.
fn parse_id(text: &str, field: &str, message: &str) -> Result<Uuid, AppError> {
    admit_uuid(text).map_err(|refusal| refusal.validation(message, field))
}
