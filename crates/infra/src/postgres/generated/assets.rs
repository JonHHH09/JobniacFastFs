//! Retirement of a generated asset and enqueueing of its cleanup task. The caller owns the locks,
//! the transaction and the instant.

use time::OffsetDateTime;
use uuid::Uuid;

use super::IntentError;
use super::bindings::AssetRow;
use crate::generated::asset_files::RemovalFailureCategory;
use crate::postgres::profiles::db;
use crate::postgres::unit_of_work::PgTransaction;

/// Moves an unreferenced asset to `RETIRING` and enqueues its cleanup in the same transaction.
///
/// The cleanup task is `BLOCKED` when `blocked` is set, otherwise `PENDING`; `category` records the
/// fixed failure category when one applies.
///
/// # Errors
///
/// Fails with [`IntentError::Database`] when either statement fails (a constraint violation keeps
/// its SQLSTATE class, as the profile link layer expects).
pub async fn retire_asset(
    tx: &mut PgTransaction,
    row: &AssetRow,
    now: OffsetDateTime,
    blocked: bool,
    category: Option<RemovalFailureCategory>,
) -> Result<(), IntentError> {
    let retirement_id = Uuid::new_v4();
    let task_id = Uuid::new_v4();
    let asset_id = row.identity.asset_id();
    db(tx
        .client()
        .execute(
            "UPDATE jobniac.generated_assets \
             SET lifecycle = 'RETIRING', retirement_id = $1, retired_at = $2 WHERE asset_id = $3",
            &[&retirement_id, &now, &asset_id],
        )
        .await)?;
    let status = if blocked { "BLOCKED" } else { "PENDING" };
    let category = category.map(|category| category.as_str());
    db(tx
        .client()
        .execute(
            "INSERT INTO jobniac.generated_asset_cleanups \
             (task_id, asset_id, retirement_id, status, next_attempt_at, failure_category, \
              created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            &[
                &task_id,
                &asset_id,
                &retirement_id,
                &status,
                &now,
                &category,
                &now,
                &now,
            ],
        )
        .await)?;
    Ok(())
}
