//! SQLite ledger of owned-asset cleanup.
//!
//! The adapter implements the core ports: durable retirement
//! ([`CleanupRepository::prepare_cleanup`]), leased claims, the fenced execution, retention
//! pruning, the read-only queue snapshot and the registry reads of the generated root. Every
//! operation is one transaction of the unit of work.
//!
//! Serialization: the transaction holds the single writer, so the file callback runs inside the
//! transaction and no other writer touches the asset meanwhile; a claim never finds a busy asset.
//! The cleanup guards run in [`crate::sqlite::generated::guards`] before each write to the ledger.

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
use rusqlite::params;
use time::OffsetDateTime;
use uuid::Uuid;

use super::error::SqliteError;
use super::generated::assets::retire_asset;
use super::generated::intents::find_generation_intent;
use super::generated::repository::{app, find_asset, referenced};
use super::generated::run_with;
use super::unit_of_work::SqliteUnitOfWork;
use crate::sqlite::codec::{instant_text, uuid_text};

pub use remover::OwnedFileAdapter;

/// The SQLite adapter of the cleanup ports. Cheap to clone; clones share the pool.
#[derive(Clone)]
pub struct SqliteCleanupRepository {
    uow: SqliteUnitOfWork,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for SqliteCleanupRepository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteCleanupRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteCleanupRepository {
    /// A repository over `uow`, reading time from `clock`.
    pub fn new(uow: SqliteUnitOfWork, clock: Arc<dyn Clock>) -> Self {
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
                run_with::<_, SqliteError>(tx, |conn| {
                    conn.execute(
                        "INSERT INTO jobniac_generated_asset_roots (id, created_at) \
                         VALUES (?1, ?2) ON CONFLICT (id) DO NOTHING",
                        params![uuid_text(root_id), instant_text(now)],
                    )?;
                    Ok(())
                })?;
                Ok(())
            })
            .await
    }
}

/// The registry of the explicit setup over the cleanup repository: the registered check reads
/// the root state, the insert is `register_generated_root`.
#[derive(Clone)]
pub struct SqliteSetupRegistry {
    repository: SqliteCleanupRepository,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for SqliteSetupRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteSetupRegistry")
            .finish_non_exhaustive()
    }
}

impl SqliteSetupRegistry {
    /// A registry over `repository`, stamping registrations with `clock`.
    #[must_use]
    pub fn new(repository: SqliteCleanupRepository, clock: Arc<dyn Clock>) -> Self {
        Self { repository, clock }
    }
}

#[async_trait]
impl SetupRegistry for SqliteSetupRegistry {
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
impl GenerationRootRepository for SqliteCleanupRepository {
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
                    Ok(run_with::<_, SqliteError>(tx, |conn| {
                        let (registered, expired_pending) = conn.query_row(
                            "SELECT EXISTS (
                               SELECT 1 FROM jobniac_generated_asset_roots WHERE id = ?1
                             ), EXISTS (
                               SELECT 1 FROM jobniac_generation_intents intent
                               JOIN jobniac_generated_assets asset ON asset.asset_id = intent.asset_id
                               WHERE asset.root_id = ?1 AND asset.lifecycle IN ('RESERVED', 'ACTIVE')
                                 AND intent.state = 'PENDING' AND intent.lease_expires_at <= ?2
                             )",
                            params![uuid_text(root_id), instant_text(now)],
                            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
                        )?;
                        Ok(GenerationRootState {
                            registered,
                            expired_pending,
                        })
                    })?)
                },
            )
            .await
    }
}

#[async_trait]
impl CleanupHealthRepository for SqliteCleanupRepository {
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
impl CleanupRepository for SqliteCleanupRepository {
    async fn prepare_cleanup(&self, asset_id: Uuid) -> Result<PreparationOutcome, AppError> {
        let clock = Arc::clone(&self.clock);
        self.uow
            .transaction(TransactionOptions::default(), async |tx| {
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
