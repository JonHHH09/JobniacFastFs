//! Leased cleanup tasks: claims, renewal, the fenced execution and retention pruning.

use jobniac_core::cleanup::{
    CLAIM_LEASE, COMPLETED_RETENTION, CleanupClaim, CleanupOutcome, OwnedAssetRemover,
    RETENTION_BATCH_SIZE, RETRY_DELAY, RemovalFailure, RemovalFailureCategory, RemovalOutcome,
};
use jobniac_core::clock::{Clock, truncate_to_micros};
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::{AssetIdentity, RETRY_BATCH_SIZE};
use jobniac_core::transactions::TransactionOptions;
use rusqlite::{Connection, OptionalExtension, Row, params};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::sqlite::codec::{get_id, get_opt_id, get_opt_ts, get_ts, instant_text, uuid_text};
use crate::sqlite::error::SqliteError;
use crate::sqlite::generated::guards::{
    generated_asset_referenced, guard_cleanup_delete, guard_cleanup_write, guard_deletion,
};
use crate::sqlite::generated::run_with;
use crate::sqlite::generated::writer::run_file_phase;
use crate::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};

/// Columns of one task joined with its asset, read by [`TaskRow::from_row`].
const TASK_SELECT: &str = "SELECT task.task_id, task.status, task.attempt_count, \
    task.next_attempt_at, task.lease_token, task.lease_expires_at, task.retirement_id, \
    asset.asset_id, asset.root_id, asset.relative_path, asset.sha256, asset.size_bytes, \
    asset.document_id, asset.lifecycle, asset.retirement_id AS asset_retirement_id \
    FROM jobniac_generated_asset_cleanups task \
    JOIN jobniac_generated_assets asset ON asset.asset_id = task.asset_id";

/// One task and its asset as the ledger transactions read them.
struct TaskRow {
    task_id: Uuid,
    status: String,
    attempt_count: i64,
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
    fn from_row(row: &Row<'_>) -> Result<Self, AppError> {
        Ok(Self {
            task_id: get_id(row, "task_id")?,
            status: row.get("status").map_err(SqliteError::from)?,
            attempt_count: row.get("attempt_count").map_err(SqliteError::from)?,
            next_attempt_at: get_ts(row, "next_attempt_at")?,
            lease_token: get_opt_id(row, "lease_token")?,
            lease_expires_at: get_opt_ts(row, "lease_expires_at")?,
            retirement_id: get_id(row, "retirement_id")?,
            asset_retirement_id: get_id(row, "asset_retirement_id")?,
            asset_id: get_id(row, "asset_id")?,
            root_id: get_id(row, "root_id")?,
            relative_path: row.get("relative_path").map_err(SqliteError::from)?,
            sha256: row.get("sha256").map_err(SqliteError::from)?,
            size_bytes: row.get("size_bytes").map_err(SqliteError::from)?,
            document_id: get_opt_id(row, "document_id")?,
            lifecycle: row.get("lifecycle").map_err(SqliteError::from)?,
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
            && self.attempt_count == i64::from(claim.attempt_count())
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

/// Reads one task with its asset.
fn task_row(conn: &Connection, task_id: Uuid) -> Result<Option<TaskRow>, AppError> {
    conn.query_row(
        &format!("{TASK_SELECT} WHERE task.task_id = ?1"),
        [uuid_text(task_id)],
        |row| Ok(TaskRow::from_row(row)),
    )
    .optional()
    .map_err(SqliteError::from)?
    .transpose()
}

/// Records a failed attempt: retryable failures go back to `PENDING` after the
/// retry delay, every other failure is `BLOCKED` until an operator decides.
fn fail(
    conn: &Connection,
    row: &TaskRow,
    lease_token: Uuid,
    category: RemovalFailureCategory,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let status = if category.is_retryable() {
        "PENDING"
    } else {
        "BLOCKED"
    };
    guard_cleanup_write(conn, row.asset_id, row.retirement_id)?;
    conn.execute(
        "UPDATE jobniac_generated_asset_cleanups \
         SET status = ?1, failure_category = ?2, next_attempt_at = ?3, \
             updated_at = ?4, lease_token = NULL, lease_expires_at = NULL \
         WHERE task_id = ?5 AND lease_token = ?6",
        params![
            status,
            category.as_str(),
            instant_text(now + RETRY_DELAY),
            instant_text(now),
            uuid_text(row.task_id),
            uuid_text(lease_token),
        ],
    )
    .map_err(SqliteError::from)?;
    Ok(())
}

/// Leases up to `limit` due tasks of `root_id`. There is no busy asset to skip: the transaction
/// holds the single writer.
pub(super) async fn claim_due(
    uow: &SqliteUnitOfWork,
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
        run_with(tx, |conn| claim_in(conn, clock, root_id, now, limit))
    })
    .await
}

fn claim_in(
    conn: &Connection,
    clock: &dyn Clock,
    root_id: Uuid,
    now: OffsetDateTime,
    limit: i64,
) -> Result<Vec<CleanupClaim>, AppError> {
    let candidates: Vec<Uuid> = {
        let mut statement = conn
            .prepare(
                "SELECT task.task_id \
                 FROM jobniac_generated_asset_cleanups task \
                 JOIN jobniac_generated_assets asset ON asset.asset_id = task.asset_id \
                 WHERE asset.root_id = ?1 AND asset.lifecycle = 'RETIRING' \
                   AND ((task.status = 'PENDING' AND task.next_attempt_at <= ?2) \
                     OR (task.status = 'PROCESSING' AND task.lease_expires_at <= ?2)) \
                 ORDER BY task.next_attempt_at, task.task_id LIMIT ?3",
            )
            .map_err(SqliteError::from)?;
        let mut rows = statement
            .query(params![uuid_text(root_id), instant_text(now), limit])
            .map_err(SqliteError::from)?;
        let mut found = Vec::new();
        while let Some(row) = rows.next().map_err(SqliteError::from)? {
            found.push(get_id(row, "task_id")?);
        }
        found
    };
    let mut claims = Vec::new();
    for task_id in candidates {
        let Some(row) = task_row(conn, task_id)? else {
            continue;
        };
        let now = truncate_to_micros(clock.now());
        if row.lifecycle != "RETIRING" || !row.is_due(now) {
            continue;
        }
        let token = Uuid::new_v4();
        let expiry = now + CLAIM_LEASE;
        guard_cleanup_write(conn, row.asset_id, row.retirement_id)?;
        conn.execute(
            "UPDATE jobniac_generated_asset_cleanups \
             SET status = 'PROCESSING', attempt_count = attempt_count + 1, \
                 lease_token = ?1, lease_expires_at = ?2, updated_at = ?3 \
             WHERE task_id = ?4",
            params![
                uuid_text(token),
                instant_text(expiry),
                instant_text(now),
                uuid_text(row.task_id)
            ],
        )
        .map_err(SqliteError::from)?;
        let attempt =
            u32::try_from(row.attempt_count.saturating_add(1)).map_err(|_| AppError::Internal)?;
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
}

/// Extends a live lease. A lost fence is `None`.
pub(super) async fn renew_claim(
    uow: &SqliteUnitOfWork,
    clock: &dyn Clock,
    claim: &CleanupClaim,
) -> Result<Option<CleanupClaim>, AppError> {
    uow.transaction(TransactionOptions::default(), async |tx| {
        run_with(tx, |conn| {
            let Some(row) = task_row(conn, claim.task_id())? else {
                return Ok(None);
            };
            let now = truncate_to_micros(clock.now());
            if !row.matches(claim, Some(now))? {
                return Ok(None);
            }
            let expiry = now + CLAIM_LEASE;
            guard_cleanup_write(conn, row.asset_id, row.retirement_id)?;
            conn.execute(
                "UPDATE jobniac_generated_asset_cleanups \
                 SET lease_expires_at = ?1, updated_at = ?2 WHERE task_id = ?3",
                params![
                    instant_text(expiry),
                    instant_text(now),
                    uuid_text(claim.task_id())
                ],
            )
            .map_err(SqliteError::from)?;
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
    })
    .await
}

/// Runs the file phase of a removal (hashing and fsync) without parking an async worker (see
/// [`run_file_phase`]).
fn remove_on_worker(
    remover: &dyn OwnedAssetRemover,
    asset: &AssetIdentity,
) -> Result<RemovalOutcome, RemovalFailure> {
    run_file_phase(|| remover.remove_owned(asset))
}

/// Reads a task for a claim on the transaction's connection.
fn read_task(tx: &SqliteTransaction, task_id: Uuid) -> Result<Option<TaskRow>, AppError> {
    run_with(tx, |conn| task_row(conn, task_id))
}

/// What the first transaction of [`execute_claim`] decided.
enum Admission {
    /// The claim is live and the asset unreferenced: run the file phase.
    Proceed,
    /// The claim ended here (lost fence, or the asset is referenced and the task was blocked).
    Done(CleanupOutcome),
}

/// Runs the fenced removal of one claim and finalizes it, in two short transactions around a file
/// phase that holds no transaction.
///
/// The file phase (hashing and fsync) must not run under `BEGIN IMMEDIATE`, which would block every
/// writer of every process for its length. The claim protocol makes the split safe:
///
/// * The first transaction checks the fence (`PROCESSING`, this lease token, attempt count,
///   retirement identity, lease not expired) and that the asset is unreferenced.
/// * While the file phase runs, the asset is `RETIRING`. The lifecycle only moves forward, and an
///   owned document attaches only to an `ACTIVE` asset, so no new reference can appear. Another
///   claim of the task needs an expired lease and changes the attempt count and token.
/// * The second transaction rereads the task and finalizes only under the same fence (the lease
///   expiry is not checked again, as before: an expired but untouched claim may still finish). A
///   rotated fence records nothing. The deletion guard rechecks references and the completed task.
///   A file removed for a claim that lost its fence is harmless: the next claim finds it absent.
///
/// A removal failure is recorded with the same fence.
pub(super) async fn execute_claim(
    uow: &SqliteUnitOfWork,
    clock: &dyn Clock,
    claim: &CleanupClaim,
    remover: &dyn OwnedAssetRemover,
) -> Result<CleanupOutcome, AppError> {
    let admission = uow
        .transaction(TransactionOptions::default(), async |tx| {
            let Some(row) = read_task(tx, claim.task_id())? else {
                return Ok(Admission::Done(CleanupOutcome::LeaseLost));
            };
            if !row.matches(claim, Some(truncate_to_micros(clock.now())))? {
                return Ok(Admission::Done(CleanupOutcome::LeaseLost));
            }
            let still_referenced = run_with::<_, SqliteError>(tx, |conn| {
                generated_asset_referenced(conn, row.document_id, &row.relative_path)
            })?;
            if still_referenced {
                let now = truncate_to_micros(clock.now());
                run_with(tx, |conn| {
                    fail(
                        conn,
                        &row,
                        claim.lease_token(),
                        RemovalFailureCategory::OwnershipUnproven,
                        now,
                    )
                })?;
                return Ok(Admission::Done(CleanupOutcome::Blocked));
            }
            Ok::<_, AppError>(Admission::Proceed)
        })
        .await?;
    if let Admission::Done(outcome) = admission {
        return Ok(outcome);
    }
    // No transaction is open here: other writers commit while the file work runs.
    let removal = remove_on_worker(remover, claim.asset());
    uow.transaction(TransactionOptions::default(), async |tx| {
        let Some(current) = read_task(tx, claim.task_id())? else {
            return Ok(CleanupOutcome::LeaseLost);
        };
        if !current.matches(claim, None)? {
            return Ok(CleanupOutcome::LeaseLost);
        }
        let now = truncate_to_micros(clock.now());
        if let Err(failure) = removal {
            let category = failure.category();
            run_with(tx, |conn| {
                fail(conn, &current, claim.lease_token(), category, now)
            })?;
            return Ok(if category.is_retryable() {
                CleanupOutcome::RetryScheduled
            } else {
                CleanupOutcome::Blocked
            });
        }
        run_with(tx, |conn| {
            guard_cleanup_write(conn, current.asset_id, current.retirement_id)?;
            conn.execute(
                "UPDATE jobniac_generated_asset_cleanups \
                 SET status = 'COMPLETED', completed_at = ?1, updated_at = ?1, \
                     lease_token = NULL, lease_expires_at = NULL, failure_category = NULL \
                 WHERE task_id = ?2 AND lease_token = ?3",
                params![
                    instant_text(now),
                    uuid_text(claim.task_id()),
                    uuid_text(claim.lease_token())
                ],
            )
            .map_err(SqliteError::from)?;
            // The asset becomes DELETED only after its completed task exists.
            guard_deletion(conn, claim.asset().asset_id(), claim.retirement_id())?;
            conn.execute(
                "UPDATE jobniac_generated_assets SET lifecycle = 'DELETED', deleted_at = ?1 \
                 WHERE asset_id = ?2 AND retirement_id = ?3",
                params![
                    instant_text(now),
                    uuid_text(claim.asset().asset_id()),
                    uuid_text(claim.retirement_id())
                ],
            )
            .map_err(SqliteError::from)?;
            Ok::<_, AppError>(())
        })?;
        Ok(CleanupOutcome::Completed)
    })
    .await
}

/// Deletes up to `limit` completed tasks past retention.
pub(super) async fn prune_completed(
    uow: &SqliteUnitOfWork,
    clock: &dyn Clock,
    limit: usize,
) -> Result<usize, AppError> {
    if !(1..=RETENTION_BATCH_SIZE).contains(&limit) {
        return Err(AppError::Internal);
    }
    let limit = i64::try_from(limit).map_err(|_| AppError::Internal)?;
    uow.transaction(TransactionOptions::default(), async |tx| {
        let completed_before = clock.now() - COMPLETED_RETENTION;
        run_with(tx, |conn| {
            let due: Vec<Uuid> = {
                let mut statement = conn
                    .prepare(
                        "SELECT task_id FROM jobniac_generated_asset_cleanups \
                         WHERE status = 'COMPLETED' AND completed_at < ?1 \
                         ORDER BY completed_at, task_id LIMIT ?2",
                    )
                    .map_err(SqliteError::from)?;
                let mut rows = statement
                    .query(params![instant_text(completed_before), limit])
                    .map_err(SqliteError::from)?;
                let mut found = Vec::new();
                while let Some(row) = rows.next().map_err(SqliteError::from)? {
                    found.push(get_id(row, "task_id")?);
                }
                found
            };
            for task_id in &due {
                guard_cleanup_delete(conn, *task_id)?;
                conn.execute(
                    "DELETE FROM jobniac_generated_asset_cleanups WHERE task_id = ?1",
                    [uuid_text(*task_id)],
                )
                .map_err(SqliteError::from)?;
            }
            Ok::<_, AppError>(due.len())
        })
    })
    .await
}
