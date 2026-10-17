//! Leased cleanup tasks: claims, renewal, the fenced execution and retention pruning.

use jobniac_core::cleanup::{
    CLAIM_LEASE, COMPLETED_RETENTION, CleanupClaim, CleanupOutcome, OwnedAssetRemover,
    RETENTION_BATCH_SIZE, RETRY_DELAY, RemovalFailure, RemovalFailureCategory, RemovalOutcome,
};
use jobniac_core::clock::{Clock, truncate_to_micros};
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::{AssetIdentity, RETRY_BATCH_SIZE};
use jobniac_core::transactions::TransactionOptions;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::postgres::generated::repository::{lock_asset, referenced};
use crate::postgres::generated::writer::run_file_phase;
use crate::postgres::profiles::{col, db};
use crate::postgres::unit_of_work::{PgTransaction, PgUnitOfWork};

/// Columns of one task joined with its asset, read by [`TaskRow::from_row`].
const TASK_SELECT: &str = "SELECT task.task_id, task.status, task.attempt_count, \
    task.next_attempt_at, task.lease_token, task.lease_expires_at, task.retirement_id, \
    asset.asset_id, asset.root_id, asset.relative_path, asset.sha256, asset.size_bytes, \
    asset.document_id, asset.lifecycle, asset.retirement_id AS asset_retirement_id \
    FROM jobniac.generated_asset_cleanups task \
    JOIN jobniac.generated_assets asset ON asset.asset_id = task.asset_id";

/// One task and its asset as the ledger transactions read them.
struct TaskRow {
    task_id: Uuid,
    status: String,
    attempt_count: i32,
    next_attempt_at: OffsetDateTime,
    lease_token: Option<Uuid>,
    lease_expires_at: Option<OffsetDateTime>,
    retirement_id: Uuid,
    asset_retirement_id: Uuid,
    asset_id: Uuid,
    root_id: Uuid,
    relative_path: String,
    sha256: String,
    size_bytes: i64,
    document_id: Option<Uuid>,
    lifecycle: String,
}

impl TaskRow {
    fn from_row(row: &tokio_postgres::Row) -> Result<Self, AppError> {
        Ok(Self {
            task_id: col(row, "task_id")?,
            status: col(row, "status")?,
            attempt_count: col(row, "attempt_count")?,
            next_attempt_at: col(row, "next_attempt_at")?,
            lease_token: col(row, "lease_token")?,
            lease_expires_at: col(row, "lease_expires_at")?,
            retirement_id: col(row, "retirement_id")?,
            asset_retirement_id: col(row, "asset_retirement_id")?,
            asset_id: col(row, "asset_id")?,
            root_id: col(row, "root_id")?,
            relative_path: col(row, "relative_path")?,
            sha256: col(row, "sha256")?,
            size_bytes: col(row, "size_bytes")?,
            document_id: col(row, "document_id")?,
            lifecycle: col(row, "lifecycle")?,
        })
    }

    /// The immutable identity stored with the task. A stored value that is not a valid identity is
    /// corrupted data, so it is internal and never caller-visible.
    fn identity(&self) -> Result<AssetIdentity, AppError> {
        let size = u64::try_from(self.size_bytes).map_err(|_| AppError::Internal)?;
        AssetIdentity::from_parts(
            self.asset_id,
            self.root_id,
            &self.relative_path,
            &self.sha256,
            size,
        )
        .map_err(|_| AppError::Internal)
    }

    /// Whether the task is still leased under this fence, retired with this identity,
    /// and (when `now` is given) the lease has not expired.
    fn matches(&self, claim: &CleanupClaim, now: Option<OffsetDateTime>) -> Result<bool, AppError> {
        Ok(self.status == "PROCESSING"
            && self.lifecycle == "RETIRING"
            && self.lease_token == Some(claim.lease_token())
            && self.retirement_id == claim.retirement_id()
            && self.asset_retirement_id == claim.retirement_id()
            && self.attempt_count == i32::try_from(claim.attempt_count()).unwrap_or(-1)
            && self.lease_expires_at == Some(claim.expires_at())
            && now.is_none_or(|now| self.lease_expires_at.is_some_and(|expiry| expiry > now))
            && self.identity()? == *claim.asset())
    }

    /// Whether the task is pending and due, or leased with an expired lease.
    fn is_due(&self, now: OffsetDateTime) -> bool {
        (self.status == "PENDING" && self.next_attempt_at <= now)
            || (self.status == "PROCESSING" && self.lease_expires_at.is_some_and(|e| e <= now))
    }
}

/// Reads one task with its asset, optionally locking the task row.
async fn task_row(
    tx: &mut PgTransaction,
    task_id: Uuid,
    for_update: bool,
) -> Result<Option<TaskRow>, AppError> {
    let sql = format!(
        "{TASK_SELECT} WHERE task.task_id = $1{}",
        if for_update {
            " FOR UPDATE OF task"
        } else {
            ""
        }
    );
    let row = db(tx.client().query_opt(&sql, &[&task_id]).await)?;
    row.as_ref().map(TaskRow::from_row).transpose()
}

/// Records a failed attempt: retryable failures go back to `PENDING` after the
/// retry delay, every other failure is `BLOCKED` until an operator decides.
async fn fail(
    tx: &mut PgTransaction,
    task_id: Uuid,
    lease_token: Uuid,
    category: RemovalFailureCategory,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let status = if category.is_retryable() {
        "PENDING"
    } else {
        "BLOCKED"
    };
    db(tx
        .client()
        .execute(
            "UPDATE jobniac.generated_asset_cleanups \
             SET status = $1, failure_category = $2, next_attempt_at = $3, \
                 updated_at = $4, lease_token = NULL, lease_expires_at = NULL \
             WHERE task_id = $5 AND lease_token = $6",
            &[
                &status,
                &category.as_str(),
                &(now + RETRY_DELAY),
                &now,
                &task_id,
                &lease_token,
            ],
        )
        .await)?;
    Ok(())
}

/// Leases up to `limit` due tasks of `root_id`. An asset whose lock is busy
/// is skipped, never waited for.
pub(super) async fn claim_due(
    uow: &PgUnitOfWork,
    clock: &dyn Clock,
    root_id: Uuid,
    limit: usize,
) -> Result<Vec<CleanupClaim>, AppError> {
    if !(1..=RETRY_BATCH_SIZE).contains(&limit) {
        return Err(AppError::Internal);
    }
    let limit = i64::try_from(limit).map_err(|_| AppError::Internal)?;
    uow.transaction(TransactionOptions::default(), async |tx| {
        let now = truncate_to_micros(clock.now());
        let candidates = db(tx
            .client()
            .query(
                "SELECT task.task_id, asset.asset_id \
                 FROM jobniac.generated_asset_cleanups task \
                 JOIN jobniac.generated_assets asset ON asset.asset_id = task.asset_id \
                 WHERE asset.root_id = $1 AND asset.lifecycle = 'RETIRING' \
                   AND ((task.status = 'PENDING' AND task.next_attempt_at <= $2) \
                     OR (task.status = 'PROCESSING' AND task.lease_expires_at <= $3)) \
                 ORDER BY task.next_attempt_at, task.task_id LIMIT $4",
                &[&root_id, &now, &now, &limit],
            )
            .await)?;
        let mut claims = Vec::new();
        for candidate in candidates {
            let task_id: Uuid = col(&candidate, "task_id")?;
            let asset_id: Uuid = col(&candidate, "asset_id")?;
            if !lock_asset(tx, asset_id, false).await? {
                continue;
            }
            let Some(row) = task_row(tx, task_id, true).await? else {
                continue;
            };
            let now = truncate_to_micros(clock.now());
            if row.lifecycle != "RETIRING" || !row.is_due(now) {
                continue;
            }
            let token = Uuid::new_v4();
            let expiry = now + CLAIM_LEASE;
            db(tx
                .client()
                .execute(
                    "UPDATE jobniac.generated_asset_cleanups \
                     SET status = 'PROCESSING', attempt_count = attempt_count + 1, \
                         lease_token = $1, lease_expires_at = $2, updated_at = $3 \
                     WHERE task_id = $4",
                    &[&token, &expiry, &now, &row.task_id],
                )
                .await)?;
            let attempt = u32::try_from(row.attempt_count.saturating_add(1))
                .map_err(|_| AppError::Internal)?;
            claims.push(
                CleanupClaim::new(
                    row.task_id,
                    row.identity()?,
                    token,
                    attempt,
                    expiry,
                    row.retirement_id,
                )
                .map_err(|_| AppError::Internal)?,
            );
        }
        Ok(claims)
    })
    .await
}

/// Extends a live lease. A busy lock or a lost fence is `None`.
pub(super) async fn renew_claim(
    uow: &PgUnitOfWork,
    clock: &dyn Clock,
    claim: &CleanupClaim,
) -> Result<Option<CleanupClaim>, AppError> {
    uow.transaction(TransactionOptions::default(), async |tx| {
        if !lock_asset(tx, claim.asset().asset_id(), false).await? {
            return Ok(None);
        }
        let Some(row) = task_row(tx, claim.task_id(), true).await? else {
            return Ok(None);
        };
        let now = truncate_to_micros(clock.now());
        if !row.matches(claim, Some(now))? {
            return Ok(None);
        }
        let expiry = now + CLAIM_LEASE;
        db(tx
            .client()
            .execute(
                "UPDATE jobniac.generated_asset_cleanups \
                 SET lease_expires_at = $1, updated_at = $2 WHERE task_id = $3",
                &[&expiry, &now, &claim.task_id()],
            )
            .await)?;
        Ok(Some(
            CleanupClaim::new(
                claim.task_id(),
                claim.asset().clone(),
                claim.lease_token(),
                claim.attempt_count(),
                expiry,
                claim.retirement_id(),
            )
            .map_err(|_| AppError::Internal)?,
        ))
    })
    .await
}

/// Runs the file phase of a removal (hashing and fsync) on the transaction's task without
/// parking an async worker (see [`run_file_phase`]).
fn remove_on_worker(
    remover: &dyn OwnedAssetRemover,
    asset: &AssetIdentity,
) -> Result<RemovalOutcome, RemovalFailure> {
    run_file_phase(|| remover.remove_owned(asset))
}

/// Runs the fenced removal of one claim and finalizes it.
///
/// The asset serialization is held by this transaction from the first check through the removal
/// and the token-checked finalization. A removal failure is recorded with the same fence; a lost
/// fence records nothing.
pub(super) async fn execute_claim(
    uow: &PgUnitOfWork,
    clock: &dyn Clock,
    claim: &CleanupClaim,
    remover: &dyn OwnedAssetRemover,
) -> Result<CleanupOutcome, AppError> {
    uow.transaction(TransactionOptions::default(), async |tx| {
        lock_asset(tx, claim.asset().asset_id(), true).await?;
        let Some(row) = task_row(tx, claim.task_id(), true).await? else {
            return Ok(CleanupOutcome::LeaseLost);
        };
        if !row.matches(claim, Some(truncate_to_micros(clock.now())))? {
            return Ok(CleanupOutcome::LeaseLost);
        }
        if referenced(tx, row.document_id, &row.relative_path).await? {
            fail(
                tx,
                claim.task_id(),
                claim.lease_token(),
                RemovalFailureCategory::OwnershipUnproven,
                truncate_to_micros(clock.now()),
            )
            .await?;
            return Ok(CleanupOutcome::Blocked);
        }
        let removal = remove_on_worker(remover, claim.asset());

        let Some(current) = task_row(tx, claim.task_id(), true).await? else {
            return Ok(CleanupOutcome::LeaseLost);
        };
        if !current.matches(claim, None)? {
            return Ok(CleanupOutcome::LeaseLost);
        }
        let now = truncate_to_micros(clock.now());
        if let Err(failure) = removal {
            let category = failure.category();
            fail(tx, claim.task_id(), claim.lease_token(), category, now).await?;
            return Ok(if category.is_retryable() {
                CleanupOutcome::RetryScheduled
            } else {
                CleanupOutcome::Blocked
            });
        }
        db(tx
            .client()
            .execute(
                "UPDATE jobniac.generated_asset_cleanups \
                 SET status = 'COMPLETED', completed_at = $1, updated_at = $1, \
                     lease_token = NULL, lease_expires_at = NULL, failure_category = NULL \
                 WHERE task_id = $2 AND lease_token = $3",
                &[&now, &claim.task_id(), &claim.lease_token()],
            )
            .await)?;
        db(tx
            .client()
            .execute(
                "UPDATE jobniac.generated_assets SET lifecycle = 'DELETED', deleted_at = $1 \
                 WHERE asset_id = $2 AND retirement_id = $3",
                &[&now, &claim.asset().asset_id(), &claim.retirement_id()],
            )
            .await)?;
        Ok(CleanupOutcome::Completed)
    })
    .await
}

/// Deletes up to `limit` completed tasks past retention.
pub(super) async fn prune_completed(
    uow: &PgUnitOfWork,
    clock: &dyn Clock,
    limit: usize,
) -> Result<usize, AppError> {
    if !(1..=RETENTION_BATCH_SIZE).contains(&limit) {
        return Err(AppError::Internal);
    }
    let limit = i64::try_from(limit).map_err(|_| AppError::Internal)?;
    uow.transaction(TransactionOptions::default(), async |tx| {
        let completed_before = clock.now() - COMPLETED_RETENTION;
        let removed = db(tx
            .client()
            .query(
                "DELETE FROM jobniac.generated_asset_cleanups \
                 WHERE task_id IN ( \
                     SELECT task_id FROM jobniac.generated_asset_cleanups \
                     WHERE status = 'COMPLETED' AND completed_at < $1 \
                     ORDER BY completed_at, task_id LIMIT $2 FOR UPDATE SKIP LOCKED \
                 ) RETURNING task_id",
                &[&completed_before, &limit],
            )
            .await)?;
        Ok(removed.len())
    })
    .await
}
