//! PostgreSQL ledger of owned-asset cleanup.
//!
//! The adapter implements the core ports: durable retirement
//! ([`CleanupRepository::prepare_cleanup`]), leased claims, the fenced execution, retention
//! pruning, the read-only queue snapshot and the registry reads of the generated root. Every
//! operation is one transaction of the unit of work.
//!
//! Serialization: the asset lock is a transaction-level advisory lock (`pg_advisory_xact_lock` or
//! the try form) held by a single transaction, so the file callback runs inside that transaction.
//! The key is the shared `jobniac.cleanup_asset_lock_key`, so the lifecycle, the other writers and
//! the cleanup exclude each other.

mod remover;
mod snapshot;
mod tasks;

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::cleanup::{
    CleanupClaim, CleanupHealthRepository, CleanupOutcome, CleanupQueueSnapshot, CleanupRepository,
    GenerationRootRepository, GenerationRootState, OwnedAssetRemover,
};
use jobniac_core::clock::Clock;
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::{GenerationState, PreparationOutcome};
use jobniac_core::generated_root_setup::SetupRegistry;
use jobniac_core::transactions::TransactionOptions;
use time::OffsetDateTime;
use uuid::Uuid;

use super::generated::assets::retire_asset;
use super::generated::intents::find_generation_intent;
use super::generated::repository::{app, find_asset, lock_asset, referenced};
use super::profiles::{col, db};
use super::unit_of_work::PgUnitOfWork;

pub use remover::OwnedFileAdapter;

/// The PostgreSQL adapter of the cleanup ports. Cheap to clone; clones share the pool.
#[derive(Clone)]
pub struct PgCleanupRepository {
    uow: PgUnitOfWork,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for PgCleanupRepository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PgCleanupRepository")
            .finish_non_exhaustive()
    }
}

impl PgCleanupRepository {
    /// A repository over `uow`, reading time from `clock`.
    pub fn new(uow: PgUnitOfWork, clock: Arc<dyn Clock>) -> Self {
        Self { uow, clock }
    }

    /// Inserts the registration of one owned-root identity if absent. Never updates or deletes a
    /// registration; only the explicit setup command calls it.
    ///
    /// # Errors
    ///
    /// Fails with a caller-safe database error; the message never carries SQL text.
    pub async fn register_generated_root(
        &self,
        root_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<(), AppError> {
        self.uow
            .transaction(TransactionOptions::default(), async |tx| {
                db(tx
                    .client()
                    .execute(
                        "INSERT INTO jobniac.generated_asset_roots (id, created_at) \
                         VALUES ($1, $2) ON CONFLICT (id) DO NOTHING",
                        &[&root_id, &now],
                    )
                    .await)?;
                Ok(())
            })
            .await
    }
}

/// The registry of the explicit setup over the cleanup repository: the registered check reads
/// the root state, the insert is `register_generated_root`.
#[derive(Clone)]
pub struct PgSetupRegistry {
    repository: PgCleanupRepository,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for PgSetupRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PgSetupRegistry")
            .finish_non_exhaustive()
    }
}

impl PgSetupRegistry {
    /// A registry over `repository`, stamping registrations with `clock`.
    #[must_use]
    pub fn new(repository: PgCleanupRepository, clock: Arc<dyn Clock>) -> Self {
        Self { repository, clock }
    }
}

#[async_trait]
impl SetupRegistry for PgSetupRegistry {
    async fn is_registered(&self, root_id: Uuid) -> Result<bool, AppError> {
        let state = self
            .repository
            .read_generation_root_state(root_id, self.clock.now())
            .await?;
        Ok(state.registered)
    }

    async fn register(&self, root_id: Uuid) -> Result<(), AppError> {
        self.repository
            .register_generated_root(root_id, self.clock.now())
            .await
    }
}

#[async_trait]
impl GenerationRootRepository for PgCleanupRepository {
    async fn read_generation_root_state(
        &self,
        root_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<GenerationRootState, AppError> {
        self.uow
            .transaction(
                TransactionOptions {
                    read_only: true,
                    ..TransactionOptions::default()
                },
                async |tx| {
                    let row = db(tx
                        .client()
                        .query_one(
                            "SELECT EXISTS (
                               SELECT 1 FROM jobniac.generated_asset_roots WHERE id=$1
                             ) AS registered, EXISTS (
                               SELECT 1 FROM jobniac.generation_intents intent
                               JOIN jobniac.generated_assets asset ON asset.asset_id=intent.asset_id
                               WHERE asset.root_id=$2 AND asset.lifecycle IN ('RESERVED','ACTIVE')
                                 AND intent.state='PENDING' AND intent.lease_expires_at<=$3
                             ) AS expired_pending",
                            &[&root_id, &root_id, &now],
                        )
                        .await)?;
                    Ok(GenerationRootState {
                        registered: col(&row, "registered")?,
                        expired_pending: col(&row, "expired_pending")?,
                    })
                },
            )
            .await
    }
}

#[async_trait]
impl CleanupHealthRepository for PgCleanupRepository {
    async fn read_queue_snapshot(
        &self,
        now: OffsetDateTime,
        repeated_failure_attempts: u32,
        completed_before: OffsetDateTime,
    ) -> Result<CleanupQueueSnapshot, AppError> {
        snapshot::read_queue_snapshot(&self.uow, now, repeated_failure_attempts, completed_before)
            .await
    }
}

#[async_trait]
impl CleanupRepository for PgCleanupRepository {
    async fn prepare_cleanup(&self, asset_id: Uuid) -> Result<PreparationOutcome, AppError> {
        let clock = Arc::clone(&self.clock);
        self.uow
            .transaction(TransactionOptions::default(), async |tx| {
                lock_asset(tx, asset_id, true).await?;
                let Some(row) = find_asset(tx, asset_id, true).await? else {
                    return Ok(PreparationOutcome::Ineligible);
                };
                if matches!(row.lifecycle.as_str(), "RETIRING" | "DELETED") {
                    return Ok(PreparationOutcome::AlreadyRetired);
                }
                if referenced(tx, row.document_id, row.identity.relative_path()).await? {
                    return Ok(PreparationOutcome::Referenced);
                }
                let intent = find_generation_intent(tx, asset_id, false)
                    .await
                    .map_err(app)?;
                if intent.is_some_and(|intent| intent.state == GenerationState::Pending) {
                    return Ok(PreparationOutcome::Ineligible);
                }
                retire_asset(tx, &row, clock.now(), false, None)
                    .await
                    .map_err(app)?;
                Ok(PreparationOutcome::Prepared)
            })
            .await
    }

    async fn claim_due(&self, root_id: Uuid, limit: usize) -> Result<Vec<CleanupClaim>, AppError> {
        tasks::claim_due(&self.uow, self.clock.as_ref(), root_id, limit).await
    }

    async fn renew_claim(&self, claim: &CleanupClaim) -> Result<Option<CleanupClaim>, AppError> {
        tasks::renew_claim(&self.uow, self.clock.as_ref(), claim).await
    }

    async fn execute_claim(
        &self,
        claim: &CleanupClaim,
        remover: &dyn OwnedAssetRemover,
    ) -> Result<CleanupOutcome, AppError> {
        tasks::execute_claim(&self.uow, self.clock.as_ref(), claim, remover).await
    }

    async fn prune_completed(&self, limit: usize) -> Result<usize, AppError> {
        tasks::prune_completed(&self.uow, self.clock.as_ref(), limit).await
    }
}
