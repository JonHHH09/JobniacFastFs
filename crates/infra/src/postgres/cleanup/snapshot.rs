//! Read-only queue aggregates of the cleanup ledger and the legacy resume cleanup queue.
//! Nothing is claimed, created, retried or deleted here.

use jobniac_core::cleanup::CleanupQueueSnapshot;
use jobniac_core::errors::AppError;
use jobniac_core::transactions::TransactionOptions;
use time::OffsetDateTime;

use crate::postgres::profiles::{col, db};
use crate::postgres::unit_of_work::PgUnitOfWork;

/// The aggregate statement, kept as one statement. Legacy rows are included so that
/// uncertain legacy ownership is reported as uncertain.
const SNAPSHOT_SQL: &str = "
    WITH queue AS (
        SELECT status, attempt_count, next_attempt_at, lease_expires_at,
               failure_category, completed_at, false AS legacy
        FROM jobniac.generated_asset_cleanups
        UNION ALL
        SELECT status, attempt_count, next_attempt_at, NULL,
               last_failure_type, completed_at, true AS legacy
        FROM document.generated_resume_file_cleanups
    )
    SELECT count(*) FILTER (WHERE status IN ('PENDING', 'BLOCKED')) AS pending,
           count(*) FILTER (WHERE status = 'PROCESSING') AS processing,
           count(*) FILTER (
               WHERE status = 'COMPLETED' AND completed_at < $1
           ) AS expired,
           min(CASE WHEN status = 'PROCESSING' AND NOT legacy
               THEN lease_expires_at ELSE next_attempt_at END) FILTER (
               WHERE (status IN ('PENDING', 'BLOCKED') AND next_attempt_at <= $2)
                  OR (status = 'PROCESSING' AND
                      coalesce(lease_expires_at, next_attempt_at) <= $3)
           ) AS oldest_due,
           coalesce(bool_or(status <> 'COMPLETED' AND attempt_count >= $4
                AND failure_category IS NOT NULL), false) AS repeated_failure,
           coalesce(bool_or((legacy AND status <> 'COMPLETED')
                            OR status = 'BLOCKED'), false) AS uncertain
    FROM queue
";

/// Reads the queue snapshot in one read-only transaction.
pub(super) async fn read_queue_snapshot(
    uow: &PgUnitOfWork,
    now: OffsetDateTime,
    repeated_failure_attempts: u32,
    completed_before: OffsetDateTime,
) -> Result<CleanupQueueSnapshot, AppError> {
    if repeated_failure_attempts == 0 {
        return Err(AppError::Internal);
    }
    let attempts = i32::try_from(repeated_failure_attempts).map_err(|_| AppError::Internal)?;
    uow.transaction(
        TransactionOptions {
            read_only: true,
            ..TransactionOptions::default()
        },
        async |tx| {
            let row = db(tx
                .client()
                .query_one(SNAPSHOT_SQL, &[&completed_before, &now, &now, &attempts])
                .await)?;
            let count = |name: &str| -> Result<u64, AppError> {
                let value: i64 = col(&row, name)?;
                u64::try_from(value).map_err(|_| AppError::Internal)
            };
            Ok(CleanupQueueSnapshot {
                pending_count: count("pending")?,
                processing_count: count("processing")?,
                expired_completed_count: count("expired")?,
                oldest_due_at: col(&row, "oldest_due")?,
                repeated_failure: col(&row, "repeated_failure")?,
                ownership_uncertain: col(&row, "uncertain")?,
            })
        },
    )
    .await
}
