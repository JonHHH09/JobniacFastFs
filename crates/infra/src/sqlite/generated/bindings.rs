//! Shared PDF binding reads and integrity checks. The caller owns admission and the transaction.
//!
//! PostgreSQL took the sorted document and asset advisory locks here. SQLite has one writer, so
//! [`lock_pdf_bindings`] keeps its place in the call sequence but takes nothing.

use jobniac_core::generated_assets::{AssetIdentity, GeneratedReservation, validate_generated_pdf};
use rusqlite::{Row, params_from_iter};
use time::OffsetDateTime;
use uuid::Uuid;

use super::IntentError;
use super::codec::{get_id, get_opt_id, get_ts, id};
use super::documents::find_content;
use super::run_with;
use crate::sqlite::error::SqliteError;
use crate::sqlite::unit_of_work::SqliteTransaction;

/// Columns of a generated asset row read for admission.
///
/// `current_transaction` is true for a classified asset that has no intent yet and was never
/// updated, which only the transaction that inserted it can observe (see
/// [`super::guards`]); the PostgreSQL adapter read `xmin` for this.
pub const ASSET_COLUMNS: &str = "asset_id, root_id, relative_path, sha256, size_bytes, \
    document_id, lifecycle, created_at, materialized_at IS NOT NULL AS materialized, \
    generation_intent_required, \
    (generation_intent_required = 1 AND row_version = 0 AND NOT EXISTS ( \
        SELECT 1 FROM jobniac_generation_intents intent \
        WHERE intent.asset_id = jobniac_generated_assets.asset_id)) AS current_transaction";

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
    pub fn from_row(row: &Row<'_>) -> Result<Self, IntentError> {
        let size: i64 = row.get("size_bytes")?;
        let size = u64::try_from(size)
            .map_err(|_| IntentError::Database(jobniac_core::errors::AppError::Internal))?;
        let identity = AssetIdentity::from_parts(
            get_id(row, "asset_id")?,
            get_id(row, "root_id")?,
            &row.get::<_, String>("relative_path")?,
            row.get::<_, String>("sha256")?.as_str(),
            size,
        )?;
        Ok(Self {
            identity,
            document_id: get_opt_id(row, "document_id")?,
            lifecycle: row.get("lifecycle")?,
            materialized: row.get("materialized")?,
            generation_intent_required: row.get("generation_intent_required")?,
            current_transaction: row.get("current_transaction")?,
            created_at: get_ts(row, "created_at")?,
        })
    }
}

/// Selects the asset rows that a removed profile resume link or a replacement may retire.
///
/// # Errors
///
/// Fails with [`IntentError::Database`] when the statement fails.
pub async fn removed_assets(
    tx: &mut SqliteTransaction,
    document_ids: &[Uuid],
    paths: &[String],
) -> Result<Vec<AssetRow>, IntentError> {
    if document_ids.is_empty() && paths.is_empty() {
        return Ok(Vec::new());
    }
    run_with(tx, |conn| {
        let documents: Vec<String> = document_ids.iter().copied().map(id).collect();
        let marks = |count: usize, start: usize| -> String {
            (start..start + count)
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let sql = format!(
            "SELECT {ASSET_COLUMNS} FROM jobniac_generated_assets \
             WHERE document_id IN ({}) OR relative_path IN ({}) ORDER BY asset_id",
            marks(documents.len(), 1),
            marks(paths.len(), documents.len() + 1)
        );
        let mut statement = conn.prepare(&sql).map_err(SqliteError::from)?;
        let parameters = documents.iter().chain(paths.iter());
        let mut rows = statement
            .query(params_from_iter(parameters))
            .map_err(SqliteError::from)?;
        let mut found = Vec::new();
        while let Some(row) = rows.next().map_err(SqliteError::from)? {
            found.push(AssetRow::from_row(row)?);
        }
        Ok(found)
    })
}

/// Takes the document and asset locks of one binding set. Nothing is locked: the writer of a write
/// transaction is already exclusive, so this exists only to keep the call sequence of the
/// PostgreSQL adapter.
///
/// # Errors
///
/// Never fails.
#[allow(
    clippy::unused_async,
    reason = "keeps the signature of the call sequence"
)]
pub async fn lock_pdf_bindings(
    _tx: &mut SqliteTransaction,
    _candidates: &[AssetRow],
    _bindings: &[GeneratedReservation],
    _removed_documents: &[Option<Uuid>],
) -> Result<(), IntentError> {
    Ok(())
}

/// Verifies that a binding is an ACTIVE owned asset whose stored bytes match its identity. Every
/// mismatch is [`IntentError::Binding`].
///
/// # Errors
///
/// Fails with [`IntentError::Binding`] for any mismatch, or [`IntentError::Database`] when a
/// statement fails.
pub async fn verify_pdf_binding(
    tx: &mut SqliteTransaction,
    binding: &GeneratedReservation,
) -> Result<(), IntentError> {
    run_with(tx, |conn| {
        let row = conn.query_row(
            &format!("SELECT {ASSET_COLUMNS} FROM jobniac_generated_assets WHERE asset_id = ?1"),
            [id(binding.asset.asset_id())],
            |row| Ok(AssetRow::from_row(row)),
        );
        let row = match row {
            Ok(row) => row.map_err(|_| IntentError::Binding)?,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Err(IntentError::Binding),
            Err(error) => return Err(IntentError::from(error)),
        };
        if row.lifecycle != "ACTIVE"
            || !row.materialized
            || row.document_id != Some(binding.document_id)
            || row.identity != binding.asset
        {
            return Err(IntentError::Binding);
        }
        let found = find_content(conn, binding.document_id).map_err(IntentError::Database)?;
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
    })
}
