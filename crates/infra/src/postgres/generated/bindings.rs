//! Shared PDF binding locks and integrity checks. The caller
//! owns admission and the transaction; the lock order is documents first, then assets, both sorted.

use jobniac_core::documents::DocumentRepository;
use jobniac_core::generated_assets::{AssetIdentity, GeneratedReservation, validate_generated_pdf};
use time::OffsetDateTime;
use tokio_postgres::Row;
use uuid::Uuid;

use super::IntentError;
use crate::postgres::documents::PgDocumentRepository;
use crate::postgres::profiles::{col, db};
use crate::postgres::unit_of_work::PgTransaction;

/// The text that `hashtextextended(text, 0)` hashes into the owned-document advisory lock key.
#[must_use]
pub fn document_lock_key_input(document_id: Uuid) -> String {
    format!("jobniac-owned-document:{document_id}")
}

/// The text that `jobniac.cleanup_asset_lock_key` hashes into the owned-asset advisory lock key
/// (packaged migration V0005).
#[must_use]
pub fn asset_lock_key_input(asset_id: Uuid) -> String {
    format!("jobniac-owned-asset:{asset_id}")
}

/// Columns of a generated asset row read for admission. `current_transaction` is true when the
/// row was inserted by the current transaction.
pub const ASSET_COLUMNS: &str = "asset_id, root_id, relative_path, sha256, size_bytes, \
    document_id, lifecycle, created_at, materialized_at IS NOT NULL AS materialized, \
    generation_intent_required, xmin = pg_current_xact_id()::text::xid AS current_transaction";

/// A generated asset row as admission reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRow {
    /// The immutable identity stored in the row.
    pub identity: AssetIdentity,
    /// The owning document, when attached.
    pub document_id: Option<Uuid>,
    /// `RESERVED`, `ACTIVE`, `RETIRING` or `DELETED`.
    pub lifecycle: String,
    /// Whether a materialization timestamp exists.
    pub materialized: bool,
    /// Whether this asset was born classified.
    pub generation_intent_required: bool,
    /// Whether the row was inserted by the current transaction.
    pub current_transaction: bool,
    /// Creation instant.
    pub created_at: OffsetDateTime,
}

impl AssetRow {
    /// Reads a row selected with [`ASSET_COLUMNS`].
    ///
    /// # Errors
    ///
    /// Fails with [`IntentError::Database`] for a missing column or an identity the schema would
    /// not allow (corrupted data is internal, never caller-visible detail).
    pub fn from_row(row: &Row) -> Result<Self, IntentError> {
        let size: i64 = col(row, "size_bytes")?;
        let size = u64::try_from(size)
            .map_err(|_| IntentError::Database(jobniac_core::errors::AppError::Internal))?;
        let identity = AssetIdentity::from_parts(
            col(row, "asset_id")?,
            col(row, "root_id")?,
            &col::<String>(row, "relative_path")?,
            col::<String>(row, "sha256")?.as_str(),
            size,
        )?;
        Ok(Self {
            identity,
            document_id: col(row, "document_id")?,
            lifecycle: col(row, "lifecycle")?,
            materialized: col(row, "materialized")?,
            generation_intent_required: col(row, "generation_intent_required")?,
            current_transaction: col(row, "current_transaction")?,
            created_at: col(row, "created_at")?,
        })
    }
}

/// Selects the asset rows that a removed profile resume link or a replacement may retire.
///
/// # Errors
///
/// Fails with [`IntentError::Database`] when the statement fails.
pub async fn removed_assets(
    tx: &mut PgTransaction,
    document_ids: &[Uuid],
    paths: &[String],
) -> Result<Vec<AssetRow>, IntentError> {
    if document_ids.is_empty() && paths.is_empty() {
        return Ok(Vec::new());
    }
    let rows = db(tx
        .client()
        .query(
            &format!(
                "SELECT {ASSET_COLUMNS} FROM jobniac.generated_assets \
                 WHERE document_id = ANY($1) OR relative_path = ANY($2) ORDER BY asset_id"
            ),
            &[&document_ids, &paths],
        )
        .await)?;
    rows.iter()
        .map(AssetRow::from_row)
        .collect::<Result<Vec<_>, _>>()
}

/// Takes the sorted document and asset locks of one binding set.
///
/// # Errors
///
/// Fails with [`IntentError::Database`] when a lock statement fails.
pub async fn lock_pdf_bindings(
    tx: &mut PgTransaction,
    candidates: &[AssetRow],
    bindings: &[GeneratedReservation],
    removed_documents: &[Option<Uuid>],
) -> Result<(), IntentError> {
    let mut documents: Vec<Uuid> = bindings.iter().map(|binding| binding.document_id).collect();
    documents.extend(removed_documents.iter().flatten().copied());
    documents.extend(candidates.iter().filter_map(|row| row.document_id));
    documents.sort_unstable();
    documents.dedup();
    for document_id in documents {
        db(tx
            .client()
            .execute(
                "SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))",
                &[&document_lock_key_input(document_id)],
            )
            .await)?;
    }
    let mut assets: Vec<Uuid> = bindings
        .iter()
        .map(|binding| binding.asset.asset_id())
        .collect();
    assets.extend(candidates.iter().map(|row| row.identity.asset_id()));
    assets.sort_unstable();
    assets.dedup();
    for asset_id in assets {
        db(tx
            .client()
            .execute(
                "SELECT pg_advisory_xact_lock(jobniac.cleanup_asset_lock_key($1))",
                &[&asset_id],
            )
            .await)?;
    }
    Ok(())
}

/// Verifies that a binding is an ACTIVE owned asset whose stored bytes match its identity
///. Every mismatch is [`IntentError::Binding`].
///
/// # Errors
///
/// Fails with [`IntentError::Binding`] for any mismatch, or [`IntentError::Database`] when a
/// statement fails.
pub async fn verify_pdf_binding(
    tx: &mut PgTransaction,
    binding: &GeneratedReservation,
) -> Result<(), IntentError> {
    let row = db(tx
        .client()
        .query_opt(
            &format!("SELECT {ASSET_COLUMNS} FROM jobniac.generated_assets WHERE asset_id = $1 FOR UPDATE"),
            &[&binding.asset.asset_id()],
        )
        .await)?;
    let Some(row) = row else {
        return Err(IntentError::Binding);
    };
    let row = AssetRow::from_row(&row).map_err(|_| IntentError::Binding)?;
    if row.lifecycle != "ACTIVE"
        || !row.materialized
        || row.document_id != Some(binding.document_id)
        || row.identity != binding.asset
    {
        return Err(IntentError::Binding);
    }
    let found = PgDocumentRepository
        .find_content(tx, binding.document_id)
        .await
        .map_err(IntentError::Database)?;
    let Some((metadata, content)) = found else {
        return Err(IntentError::Binding);
    };
    validate_generated_pdf(&content).map_err(|_| IntentError::Binding)?;
    if metadata.media_type != "application/pdf"
        || metadata.sha256 != binding.asset.sha256()
        || u64::try_from(metadata.byte_size).ok() != Some(binding.asset.size_bytes())
    {
        return Err(IntentError::Binding);
    }
    Ok(())
}
