//! Retirement of a generated asset and enqueueing of its cleanup task. The caller owns the
//! transaction and the instant.

use rusqlite::params;
use time::OffsetDateTime;
use uuid::Uuid;

use super::IntentError;
use super::bindings::AssetRow;
use super::guards::{guard_cleanup_write, guard_retirement};
use super::run_with;
use crate::generated::asset_files::RemovalFailureCategory;
use crate::sqlite::codec::{instant_text, uuid_text};
use crate::sqlite::unit_of_work::SqliteTransaction;

/// Moves an unreferenced asset to `RETIRING` and enqueues its cleanup in the same transaction.
///
/// The cleanup task is `BLOCKED` when `blocked` is set, otherwise `PENDING`; `category` records the
/// fixed failure category when one applies. The retirement guards (a referenced asset cannot
/// retire, a pending generation must be fenced first) run before the update.
///
/// # Errors
///
/// Fails with [`IntentError::Database`] when a guard or either statement fails (a constraint
/// violation keeps its SQLSTATE class, as the profile link layer expects).
pub async fn retire_asset(
    tx: &mut SqliteTransaction,
    row: &AssetRow,
    now: OffsetDateTime,
    blocked: bool,
    category: Option<RemovalFailureCategory>,
) -> Result<(), IntentError> {
    let retirement_id = Uuid::new_v4();
    let task_id = Uuid::new_v4();
    let asset_id = row.identity.asset_id();
    let status = if blocked { "BLOCKED" } else { "PENDING" };
    let category = category.map(|category| category.as_str());
    let now = instant_text(now);
    run_with(tx, |conn| {
        guard_retirement(conn, asset_id)?;
        conn.execute(
            "UPDATE jobniac_generated_assets \
             SET lifecycle = 'RETIRING', retirement_id = ?1, retired_at = ?2 WHERE asset_id = ?3",
            params![uuid_text(retirement_id), now, uuid_text(asset_id)],
        )?;
        guard_cleanup_write(conn, asset_id, retirement_id)?;
        conn.execute(
            "INSERT INTO jobniac_generated_asset_cleanups \
             (task_id, asset_id, retirement_id, status, next_attempt_at, failure_category, \
              created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                uuid_text(task_id),
                uuid_text(asset_id),
                uuid_text(retirement_id),
                status,
                now,
                category,
                now,
                now
            ],
        )?;
        Ok(())
    })
}
