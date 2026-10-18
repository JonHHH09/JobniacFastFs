//! Bounded cleanup executor and its repository and remover ports.
//!
//! The executor claims due tasks and hands each one to the repository together with a guard
//! that only removes the identity the claim was issued for. The repository re-reads the identity,
//! checks the fence and every reference, and keeps its serialization held through the removal
//! and the token-checked finalization. The executor never sees a row, a token or a path.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use super::health::CleanupHealthRepository;
use super::model::{
    CleanupClaim, CleanupOutcome, RETENTION_BATCH_SIZE, RETENTION_MAX_BATCHES, RemovalFailure,
    RemovalFailureCategory, RemovalOutcome,
};
use crate::errors::AppError;
use crate::generated_assets::{AssetIdentity, PreparationOutcome, RETRY_BATCH_SIZE};

/// Port of the owned file namespace. Runs only inside [`CleanupRepository::execute_claim`]; it
/// fails closed on any identity or ownership mismatch.
pub trait OwnedAssetRemover: Send + Sync {
    /// Removes the owned file of `identity`.
    ///
    /// # Errors
    ///
    /// Returns a fixed-category [`RemovalFailure`]; no path or OS text is carried.
    fn remove_owned(&self, identity: &AssetIdentity) -> Result<RemovalOutcome, RemovalFailure>;
}

/// Persistence port of the cleanup ledger. Each method is one repository operation; the
/// SQLite adapter lives in `jobniac_infra::sqlite::cleanup`.
#[async_trait]
pub trait CleanupRepository: CleanupHealthRepository {
    /// Commits retirement and task intent of an unreferenced asset under the common serialization.
    async fn prepare_cleanup(&self, asset_id: Uuid) -> Result<PreparationOutcome, AppError>;

    /// Leases up to `limit` due tasks of `root_id`, skipping any asset whose serialization is busy.
    async fn claim_due(&self, root_id: Uuid, limit: usize) -> Result<Vec<CleanupClaim>, AppError>;

    /// Extends a live lease, or returns `None` when the claim no longer holds.
    async fn renew_claim(&self, claim: &CleanupClaim) -> Result<Option<CleanupClaim>, AppError>;

    /// Fences the callback and the finalization under the common asset serialization.
    ///
    /// The repository reconstructs the identity from storage, rechecks the current token, lease,
    /// retirement and references, calls `remover` with that identity, and then completes the task
    /// or records a failure, all with the token check. A retryable failure reschedules the task;
    /// any other failure blocks it.
    async fn execute_claim(
        &self,
        claim: &CleanupClaim,
        remover: &dyn OwnedAssetRemover,
    ) -> Result<CleanupOutcome, AppError>;

    /// Deletes up to `limit` completed tasks past retention, keeping identity and open intent.
    async fn prune_completed(&self, limit: usize) -> Result<usize, AppError>;
}

/// Refusal of a cleanup run. The message is fixed and never carries database or path text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CleanupUnavailable(pub &'static str);

/// Counts of one bounded cleanup run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanupRunSummary {
    /// Tasks leased by this run.
    pub claimed: usize,
    /// Tasks completed.
    pub completed: usize,
    /// Tasks rescheduled after a retryable failure.
    pub retry_scheduled: usize,
    /// Tasks whose lease was lost before finalization.
    pub lease_lost: usize,
    /// Tasks blocked on a non-retryable failure.
    pub blocked: usize,
    /// Claims or executions that failed at the state layer.
    pub state_failures: usize,
}

/// Guard handed to the repository: it accepts only the identity the claim was issued for.
struct ExpectedIdentity<'a> {
    expected: &'a AssetIdentity,
    remover: &'a dyn OwnedAssetRemover,
}

impl OwnedAssetRemover for ExpectedIdentity<'_> {
    fn remove_owned(&self, identity: &AssetIdentity) -> Result<RemovalOutcome, RemovalFailure> {
        if identity != self.expected {
            return Err(RemovalFailure::new(
                RemovalFailureCategory::IdentityMismatch,
            ));
        }
        self.remover.remove_owned(identity)
    }
}

/// The ports and root of an enabled executor.
type Required = (Arc<dyn CleanupRepository>, Arc<dyn OwnedAssetRemover>, Uuid);

/// One bounded, explicit cleanup run. Composition supplies the real ports and enables it.
#[derive(Clone)]
pub struct CleanupExecutor {
    repository: Option<Arc<dyn CleanupRepository>>,
    remover: Option<Arc<dyn OwnedAssetRemover>>,
    root_id: Option<Uuid>,
    enabled: bool,
}

impl std::fmt::Debug for CleanupExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CleanupExecutor")
            .field("enabled", &self.enabled)
            .field("configured", &self.root_id.is_some())
            .finish_non_exhaustive()
    }
}

impl CleanupExecutor {
    /// Builds an executor. Nothing runs until both ports, the root and `enabled` are present.
    pub fn new(
        repository: Option<Arc<dyn CleanupRepository>>,
        remover: Option<Arc<dyn OwnedAssetRemover>>,
        root_id: Option<Uuid>,
        enabled: bool,
    ) -> Self {
        Self {
            repository,
            remover,
            root_id,
            enabled,
        }
    }

    fn required(&self) -> Result<Required, CleanupUnavailable> {
        match (
            self.enabled,
            self.repository.as_ref(),
            self.remover.as_ref(),
            self.root_id,
        ) {
            (true, Some(repository), Some(remover), Some(root_id)) => {
                Ok((Arc::clone(repository), Arc::clone(remover), root_id))
            }
            _ => Err(CleanupUnavailable("Cleanup lifecycle is not enabled")),
        }
    }

    /// Claims up to one batch of due tasks of the configured root and executes each one.
    ///
    /// # Errors
    ///
    /// Fails with [`CleanupUnavailable`] when the lifecycle is not enabled or the claim batch
    /// cannot be read. A failure of one execution is counted, not returned.
    pub async fn run_due_tasks(&self) -> Result<CleanupRunSummary, CleanupUnavailable> {
        let (repository, remover, root_id) = self.required()?;
        let claims = repository
            .claim_due(root_id, RETRY_BATCH_SIZE)
            .await
            .map_err(|_| CleanupUnavailable("Cleanup state is unavailable"))?;
        if claims.len() > RETRY_BATCH_SIZE {
            return Err(CleanupUnavailable("Cleanup claim batch is invalid"));
        }
        let mut summary = CleanupRunSummary {
            claimed: claims.len(),
            ..CleanupRunSummary::default()
        };
        for claim in &claims {
            if claim.asset().root_id() != root_id {
                summary.state_failures += 1;
                continue;
            }
            let guard = ExpectedIdentity {
                expected: claim.asset(),
                remover: remover.as_ref(),
            };
            match repository.execute_claim(claim, &guard).await {
                Ok(CleanupOutcome::Completed) => summary.completed += 1,
                Ok(CleanupOutcome::RetryScheduled) => summary.retry_scheduled += 1,
                Ok(CleanupOutcome::LeaseLost) => summary.lease_lost += 1,
                Ok(CleanupOutcome::Blocked) => summary.blocked += 1,
                Err(_) => summary.state_failures += 1,
            }
        }
        Ok(summary)
    }

    /// Deletes completed tasks past retention, at most [`RETENTION_MAX_BATCHES`] batches.
    ///
    /// # Errors
    ///
    /// Fails with [`CleanupUnavailable`] when the lifecycle is not enabled or a prune fails.
    pub async fn purge_completed_tasks(&self) -> Result<usize, CleanupUnavailable> {
        let (repository, _, _) = self.required()?;
        let mut total = 0;
        for _ in 0..RETENTION_MAX_BATCHES {
            let deleted = repository
                .prune_completed(RETENTION_BATCH_SIZE)
                .await
                .map_err(|_| CleanupUnavailable("Cleanup retention state is unavailable"))?;
            if deleted > RETENTION_BATCH_SIZE {
                return Err(CleanupUnavailable("Cleanup retention count is invalid"));
            }
            total += deleted;
            if deleted < RETENTION_BATCH_SIZE {
                break;
            }
        }
        Ok(total)
    }
}
