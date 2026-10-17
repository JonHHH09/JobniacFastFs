//! Explicit generation namespace qualification.
//!
//! `inspect` succeeds only when generation is enabled, the root is registered and its owned
//! namespace proves its controls. Any other condition is the same refusal, so callers cannot
//! tell which check failed. The registry read and the file proof are ports; nothing here touches
//! a database or the filesystem.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use super::model::RemovalFailure;
use crate::errors::AppError;
use crate::generated_assets::GeneratedAssetUnavailable;

/// Registered identity and expired classified work of one root, observed without mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationRootState {
    /// Whether the root identity is registered.
    pub registered: bool,
    /// Whether a pending classified generation of the root has an expired lease.
    pub expired_pending: bool,
}

/// Read-only registry port of the generated root.
#[async_trait]
pub trait GenerationRootRepository: Send + Sync + fmt::Debug {
    /// Reads the registration and the expired pending work of `root_id` as of `now`.
    async fn read_generation_root_state(
        &self,
        root_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<GenerationRootState, AppError>;
}

/// Port of the owned namespace proof. Reads and locks existing controls; never creates one.
pub trait GenerationRootFiles: Send + Sync + fmt::Debug {
    /// Proves the existing controls of the configured root.
    ///
    /// # Errors
    ///
    /// Returns a fixed-category [`RemovalFailure`] when ownership cannot be proven.
    fn qualify_owned_root(&self) -> Result<(), RemovalFailure>;
}

/// Qualifies the generation namespace before any lifecycle operation.
#[derive(Clone)]
pub struct GenerationRootQualification {
    repository: Option<Arc<dyn GenerationRootRepository>>,
    files: Option<Arc<dyn GenerationRootFiles>>,
    root_id: Option<Uuid>,
    enabled: bool,
}

impl fmt::Debug for GenerationRootQualification {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GenerationRootQualification")
            .field("enabled", &self.enabled)
            .field("configured", &self.root_id.is_some())
            .finish_non_exhaustive()
    }
}

impl GenerationRootQualification {
    /// Builds the qualification. Nothing is read until [`Self::inspect`] is called.
    pub fn new(
        repository: Option<Arc<dyn GenerationRootRepository>>,
        files: Option<Arc<dyn GenerationRootFiles>>,
        root_id: Option<Uuid>,
        enabled: bool,
    ) -> Self {
        Self {
            repository,
            files,
            root_id,
            enabled,
        }
    }

    /// Observes the root and proves its namespace.
    ///
    /// # Errors
    ///
    /// Fails with [`GeneratedAssetUnavailable`] when generation is disabled or unconfigured, the
    /// registry cannot be read, the root is not registered, or its namespace is not qualified.
    pub async fn inspect(
        &self,
        now: OffsetDateTime,
    ) -> Result<GenerationRootState, GeneratedAssetUnavailable> {
        let unavailable = || GeneratedAssetUnavailable("Generation is not available");
        let (true, Some(repository), Some(files), Some(root_id)) = (
            self.enabled,
            self.repository.as_ref(),
            self.files.as_ref(),
            self.root_id,
        ) else {
            return Err(unavailable());
        };
        let state = repository
            .read_generation_root_state(root_id, now)
            .await
            .map_err(|_| unavailable())?;
        if !state.registered {
            return Err(unavailable());
        }
        files.qualify_owned_root().map_err(|_| unavailable())?;
        Ok(state)
    }
}
