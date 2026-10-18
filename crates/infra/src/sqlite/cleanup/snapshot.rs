//! Read-only queue aggregates of the cleanup ledger and the legacy resume cleanup queue.
//! Nothing is claimed, created, retried or deleted here.

use jobniac_core::cleanup::CleanupQueueSnapshot;
use jobniac_core::errors::AppError;
use jobniac_core::transactions::TransactionOptions;
use rusqlite::params;
use time::OffsetDateTime;

use crate::sqlite::codec::{instant_text, parse_ts};
use crate::sqlite::error::SqliteError;
use crate::sqlite::generated::run_with;
use crate::sqlite::unit_of_work::SqliteUnitOfWork;

/// The aggregate statement, kept as one statement. Legacy rows are included so that
/// uncertain legacy ownership is reported as uncertain. `?1` is the retention cutoff, `?2` the
/// current instant and `?3` the repeated-failure attempt count.
const SNAPSHOT_SQL: &str = "
    WITH queue AS (
        SELECT status, attempt_count, next_attempt_at, lease_expires_at,
               failure_category, completed_at, 0 AS legacy
        FROM jobniac_generated_asset_cleanups
        UNION ALL
        SELECT status, attempt_count, next_attempt_at, NULL,
               last_failure_type, completed_at, 1 AS legacy
        FROM document_generated_resume_file_cleanups
    )
    SELECT count(*) FILTER (WHERE status IN ('PENDING', 'BLOCKED')) AS pending,
           count(*) FILTER (WHERE status = 'PROCESSING') AS processing,
           count(*) FILTER (
               WHERE status = 'COMPLETED' AND completed_at < ?1
           ) AS expired,
           min(CASE WHEN status = 'PROCESSING' AND NOT legacy
               THEN lease_expires_at ELSE next_attempt_at END) FILTER (
               WHERE (status IN ('PENDING', 'BLOCKED') AND next_attempt_at <= ?2)
                  OR (status = 'PROCESSING' AND
                      coalesce(lease_expires_at, next_attempt_at) <= ?2)
           ) AS oldest_due,
           coalesce(max(status <> 'COMPLETED' AND attempt_count >= ?3
                AND failure_category IS NOT NULL), 0) AS repeated_failure,
           coalesce(max((legacy AND status <> 'COMPLETED')
                        OR status = 'BLOCKED'), 0) AS uncertain
    FROM queue
";

/// Reads the queue snapshot in one read-only transaction.
pub(super) async fn read_queue_snapshot(
    uow: &SqliteUnitOfWork,
    now: OffsetDateTime,
    repeated_failure_attempts: u32,
    completed_before: OffsetDateTime,
) -> Result<CleanupQueueSnapshot, AppError> {
    if repeated_failure_attempts == 0 {
        return Err(AppError::Internal);
    }
    let attempts = i64::from(repeated_failure_attempts);
    uow.transaction(
        TransactionOptions {
            read_only: true,
            ..TransactionOptions::default()
        },
        async |tx| {
            let row = run_with::<_, SqliteError>(tx, |conn| {
                Ok(conn.query_row(
                    SNAPSHOT_SQL,
                    params![instant_text(completed_before), instant_text(now), attempts],
                    |row| {
                        Ok((
                            row.get::<_, i64>("pending")?,
                            row.get::<_, i64>("processing")?,
                            row.get::<_, i64>("expired")?,
                            row.get::<_, Option<String>>("oldest_due")?,
                            row.get::<_, bool>("repeated_failure")?,
                            row.get::<_, bool>("uncertain")?,
                        ))
                    },
                )?)
            })?;
            let count = |value: i64| u64::try_from(value).map_err(|_| AppError::Internal);
            Ok(CleanupQueueSnapshot {
                pending_count: count(row.0)?,
                processing_count: count(row.1)?,
                expired_completed_count: count(row.2)?,
                oldest_due_at: row.3.as_deref().map(parse_ts).transpose()?,
                repeated_failure: row.4,
                ownership_uncertain: row.5,
            })
        },
    )
    .await
}
