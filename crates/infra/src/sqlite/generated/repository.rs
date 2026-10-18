//! SQLite adapter of the standalone generated-PDF port: reserve, find, materialize and recover.
//!
//! PostgreSQL serialized the asset with a transaction-scoped advisory lock: blocking in
//! materialization, a try-lock that skipped a busy asset in recovery. Here every write transaction
//! is `BEGIN IMMEDIATE` on the single writer, so a transaction that holds the writer is the only one
//! that can touch the asset, and a recovery candidate is never "busy": it is either still eligible
//! at the reread inside its own transaction, or it changed since the candidate scan and is skipped
//! for that reason.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::clock::{Clock, format_instant, truncate_to_micros};
use jobniac_core::documents::DocumentMetadata;
use jobniac_core::errors::{AppError, ApplicationError};
use jobniac_core::generated_assets::standalone::StandaloneAssetRepository;
use jobniac_core::generated_assets::{
    AssetIdentity, CLAIM_LEASE, GeneratedAssetWriter, GeneratedReservation, MaterializationOutcome,
    PublicationFailureCategory, RETRY_BATCH_SIZE, sha256_hex, validate_file_name,
    validate_generated_pdf,
};
use jobniac_core::transactions::TransactionOptions;
use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;
use uuid::Uuid;

use super::IntentError;
use super::assets::retire_asset;
use super::bindings::{ASSET_COLUMNS, AssetRow};
use super::codec::{get_id, id, ts};
use super::documents::{find_content, save_file};
use super::guards::{NewAsset, generated_asset_referenced, guard_asset_insert};
use super::intents::find_generation_intent;
use super::run_with;
use super::writer::run_file_phase;
use crate::generated::asset_files::RemovalFailureCategory;
use crate::sqlite::error::SqliteError;
use crate::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};

/// Media type of stored generated PDFs.
pub(super) const PDF_MEDIA_TYPE: &str = "application/pdf";

/// The SQLite adapter of [`StandaloneAssetRepository`]. The clock supplies every instant the
/// adapter writes.
#[derive(Debug, Clone)]
pub struct SqliteGeneratedAssetRepository {
    pub(super) uow: SqliteUnitOfWork,
    pub(super) clock: Arc<dyn Clock>,
}

impl SqliteGeneratedAssetRepository {
    /// A repository over `uow` that stamps its writes with `clock`.
    #[must_use]
    pub fn new(uow: SqliteUnitOfWork, clock: Arc<dyn Clock>) -> Self {
        Self { uow, clock }
    }

    /// The current instant, truncated to the microseconds the schema stores.
    fn now(&self) -> OffsetDateTime {
        truncate_to_micros(self.clock.now())
    }
}

/// The unavailable database error of a refused state.
pub(super) fn unavailable() -> AppError {
    ApplicationError::database_unavailable().into()
}

/// Converts an intent-layer failure to the core error. Only a database failure carries detail.
/// Shared by the generated and cleanup adapters, so both refuse the same states the same way.
pub(crate) fn app(error: IntentError) -> AppError {
    match error {
        IntentError::Database(error) => error,
        IntentError::Unavailable | IntentError::Binding | IntentError::Invalid => unavailable(),
    }
}

/// Reads one asset row on `conn`.
pub(crate) fn find_asset_in(
    conn: &Connection,
    asset_id: Uuid,
) -> Result<Option<AssetRow>, IntentError> {
    conn.query_row(
        &format!("SELECT {ASSET_COLUMNS} FROM jobniac_generated_assets WHERE asset_id = ?1"),
        [id(asset_id)],
        |row| Ok(AssetRow::from_row(row)),
    )
    .optional()?
    .transpose()
}

/// Reads one asset row. `_for_update` is the PostgreSQL row lock, which the single writer makes
/// unnecessary.
pub(crate) async fn find_asset(
    tx: &mut SqliteTransaction,
    asset_id: Uuid,
    _for_update: bool,
) -> Result<Option<AssetRow>, AppError> {
    run_with(tx, |conn| find_asset_in(conn, asset_id)).map_err(app)
}

/// Whether the asset's document or relative path is still referenced.
pub(crate) async fn referenced(
    tx: &mut SqliteTransaction,
    document_id: Option<Uuid>,
    relative_path: &str,
) -> Result<bool, AppError> {
    Ok(run_with::<_, SqliteError>(tx, |conn| {
        generated_asset_referenced(conn, document_id, relative_path)
    })?)
}

/// Reads the verified bytes of an asset's document and checks them against the asset row. Every
/// mismatch is the unavailable database error.
pub(super) async fn reservation_content(
    tx: &mut SqliteTransaction,
    asset: &AssetRow,
) -> Result<(GeneratedReservation, Vec<u8>), AppError> {
    let document_id = asset.document_id.ok_or_else(unavailable)?;
    let (metadata, content) =
        run_with(tx, |conn| find_content(conn, document_id))?.ok_or_else(unavailable)?;
    if metadata.id != document_id
        || metadata.media_type != PDF_MEDIA_TYPE
        || metadata.sha256 != asset.identity.sha256()
        || u64::try_from(metadata.byte_size).ok() != Some(asset.identity.size_bytes())
        || validate_generated_pdf(&content).is_err()
    {
        return Err(unavailable());
    }
    Ok((
        GeneratedReservation {
            asset: asset.identity.clone(),
            document_id,
        },
        content,
    ))
}

/// Stores the document of a reservation and inserts its reserved asset, guarded against adopting
/// existing references.
pub(super) fn reserve_rows(
    conn: &Connection,
    identity: &AssetIdentity,
    document_id: Uuid,
    original_file_name: &str,
    content: &[u8],
    now: OffsetDateTime,
    generation_intent_required: bool,
) -> Result<(), AppError> {
    let size = i64::try_from(content.len()).map_err(|_| AppError::Internal)?;
    let metadata = DocumentMetadata {
        id: document_id,
        original_file_name: original_file_name.to_owned(),
        media_type: PDF_MEDIA_TYPE.to_owned(),
        byte_size: size,
        sha256: identity.sha256().to_owned(),
        created_at: format_instant(now),
        updated_at: format_instant(now),
    };
    save_file(conn, &metadata, content)?;
    let size = i64::try_from(identity.size_bytes()).map_err(|_| AppError::Internal)?;
    guard_asset_insert(
        conn,
        &NewAsset {
            asset_id: identity.asset_id(),
            relative_path: identity.relative_path(),
            document_id: Some(document_id),
            sha256: identity.sha256(),
            size_bytes: size,
        },
    )?;
    conn.execute(
        "INSERT INTO jobniac_generated_assets \
         (asset_id, root_id, relative_path, sha256, size_bytes, \
          document_id, lifecycle, created_at, generation_intent_required) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'RESERVED', ?7, ?8)",
        params![
            id(identity.asset_id()),
            id(identity.root_id()),
            identity.relative_path(),
            identity.sha256(),
            size,
            id(document_id),
            ts(now),
            generation_intent_required,
        ],
    )
    .map_err(SqliteError::from)?;
    Ok(())
}

/// Checks the inputs of a reservation before any write.
pub(super) fn validate_input(
    identity: &AssetIdentity,
    original_file_name: &str,
    content: &[u8],
) -> Result<(), AppError> {
    validate_generated_pdf(content).map_err(|_| AppError::Internal)?;
    if content.len() as u64 != identity.size_bytes() || sha256_hex(content) != identity.sha256() {
        return Err(AppError::Internal);
    }
    validate_file_name(original_file_name).map_err(|_| AppError::Internal)
}

/// The blocked flag and the removal category recorded for a failed publication: a
/// file write failure is retried as cleanup; every other category blocks the asset, and
/// `RETIRED_ASSET` is recorded as an ownership failure.
pub(super) const fn failure_record(
    category: PublicationFailureCategory,
) -> (bool, Option<RemovalFailureCategory>) {
    match category {
        PublicationFailureCategory::FileWriteFailed => (false, None),
        PublicationFailureCategory::IdentityMismatch => {
            (true, Some(RemovalFailureCategory::IdentityMismatch))
        }
        PublicationFailureCategory::OwnershipUnproven
        | PublicationFailureCategory::RetiredAsset => {
            (true, Some(RemovalFailureCategory::OwnershipUnproven))
        }
        PublicationFailureCategory::UnsupportedPlatform => {
            (true, Some(RemovalFailureCategory::UnsupportedPlatform))
        }
    }
}

/// Moves a `RESERVED` asset to `ACTIVE`. Returns the number of rows changed.
pub(super) async fn activate_reserved(
    tx: &mut SqliteTransaction,
    asset_id: Uuid,
    now: OffsetDateTime,
) -> Result<usize, AppError> {
    Ok(run_with::<_, SqliteError>(tx, |conn| {
        Ok(conn.execute(
            "UPDATE jobniac_generated_assets \
             SET lifecycle = 'ACTIVE', materialized_at = ?1 \
             WHERE asset_id = ?2 AND lifecycle = 'RESERVED'",
            params![ts(now), id(asset_id)],
        )?)
    })?)
}

/// The finalize step of a materialization, in the transaction that read the asset, after
/// publication: activate the asset after a successful write, or retire it with its cleanup task
/// after a failed one.
async fn finalize(
    tx: &mut SqliteTransaction,
    asset_id: Uuid,
    reservation: &GeneratedReservation,
    failure: Option<PublicationFailureCategory>,
    now: OffsetDateTime,
) -> Result<MaterializationOutcome, AppError> {
    let Some(current) = find_asset(tx, asset_id, true).await? else {
        return Ok(MaterializationOutcome::Ineligible);
    };
    if current.lifecycle != "RESERVED"
        || current.document_id != Some(reservation.document_id)
        || current.identity != reservation.asset
    {
        return Ok(MaterializationOutcome::Ineligible);
    }
    match failure {
        Some(category) => {
            if referenced(tx, current.document_id, current.identity.relative_path()).await? {
                return Ok(MaterializationOutcome::Ineligible);
            }
            let (blocked, removal) = failure_record(category);
            retire_asset(tx, &current, now, blocked, removal)
                .await
                .map_err(app)?;
            Ok(if blocked {
                MaterializationOutcome::Blocked
            } else {
                MaterializationOutcome::CleanupPending
            })
        }
        None => {
            if activate_reserved(tx, asset_id, now).await? != 1 {
                return Err(unavailable());
            }
            Ok(MaterializationOutcome::Active)
        }
    }
}

#[async_trait]
impl StandaloneAssetRepository for SqliteGeneratedAssetRepository {
    async fn reserve_generated_pdf(
        &self,
        identity: &AssetIdentity,
        document_id: Uuid,
        original_file_name: &str,
        content: &[u8],
    ) -> Result<GeneratedReservation, AppError> {
        run_file_phase(|| validate_input(identity, original_file_name, content))?;
        let now = self.now();
        self.uow
            .transaction::<GeneratedReservation, AppError>(
                TransactionOptions::default(),
                async |tx| {
                    run_with(tx, |conn| {
                        reserve_rows(
                            conn,
                            identity,
                            document_id,
                            original_file_name,
                            content,
                            now,
                            false,
                        )
                    })?;
                    Ok(GeneratedReservation {
                        asset: identity.clone(),
                        document_id,
                    })
                },
            )
            .await
    }

    async fn find_reservation(
        &self,
        asset_id: Uuid,
    ) -> Result<Option<GeneratedReservation>, AppError> {
        let read_only = TransactionOptions {
            read_only: true,
            ..TransactionOptions::default()
        };
        self.uow
            .transaction::<Option<GeneratedReservation>, AppError>(read_only, async |tx| {
                let Some(asset) = find_asset(tx, asset_id, false).await? else {
                    return Ok(None);
                };
                if asset.document_id.is_none() {
                    return Ok(None);
                }
                let (reservation, _) = reservation_content(tx, &asset).await?;
                Ok(Some(reservation))
            })
            .await
    }

    async fn materialize_reserved(
        &self,
        asset_id: Uuid,
        writer: &dyn GeneratedAssetWriter,
    ) -> Result<MaterializationOutcome, AppError> {
        let now = self.now();
        self.uow
            .transaction::<MaterializationOutcome, AppError>(
                TransactionOptions::default(),
                async |tx| {
                    let Some(asset) = find_asset(tx, asset_id, true).await? else {
                        return Ok(MaterializationOutcome::Ineligible);
                    };
                    if asset.document_id.is_none()
                        || find_generation_intent(tx, asset_id, false)
                            .await
                            .map_err(app)?
                            .is_some()
                    {
                        return Ok(MaterializationOutcome::Ineligible);
                    }
                    if asset.lifecycle == "ACTIVE" {
                        return Ok(MaterializationOutcome::Active);
                    }
                    if asset.lifecycle != "RESERVED"
                        || referenced(tx, asset.document_id, asset.identity.relative_path()).await?
                    {
                        return Ok(MaterializationOutcome::Ineligible);
                    }
                    let (reservation, content) = reservation_content(tx, &asset).await?;
                    // A publication failure is recorded and finalized, not raised.
                    let failure = writer
                        .materialize_owned(&reservation.asset, &content)
                        .err()
                        .map(|failure| failure.category);
                    finalize(tx, asset_id, &reservation, failure, now).await
                },
            )
            .await
    }

    async fn recover_stale_reservations(
        &self,
        root_id: Uuid,
        stale_before: OffsetDateTime,
        limit: usize,
    ) -> Result<Vec<Uuid>, AppError> {
        if limit == 0 || limit > RETRY_BATCH_SIZE {
            return Err(AppError::Internal);
        }
        let limit = i64::try_from(limit).map_err(|_| AppError::Internal)?;
        let stale_before = truncate_to_micros(stale_before);
        if stale_before > self.now() - CLAIM_LEASE {
            return Err(AppError::Internal);
        }
        let candidates: Vec<Uuid> = self
            .uow
            .transaction::<Vec<Uuid>, AppError>(TransactionOptions::default(), async |tx| {
                Ok(run_with::<_, SqliteError>(tx, |conn| {
                    let mut statement = conn.prepare(
                        "SELECT asset_id FROM jobniac_generated_assets asset \
                         WHERE root_id = ?1 AND lifecycle = 'RESERVED' AND created_at <= ?2 \
                           AND NOT EXISTS (SELECT 1 FROM jobniac_generation_intents intent \
                                           WHERE intent.asset_id = asset.asset_id) \
                         ORDER BY created_at, asset_id LIMIT ?3",
                    )?;
                    let mut rows =
                        statement.query(params![id(root_id), ts(stale_before), limit])?;
                    let mut found = Vec::new();
                    while let Some(row) = rows.next()? {
                        found.push(get_id(row, "asset_id")?);
                    }
                    Ok(found)
                })?)
            })
            .await?;
        let mut recovered = Vec::new();
        for asset_id in candidates {
            if let Some(asset_id) = self.recover_one(asset_id, root_id, stale_before).await? {
                recovered.push(asset_id);
            }
        }
        Ok(recovered)
    }
}

impl SqliteGeneratedAssetRepository {
    /// Retires one stale candidate. `None` means the candidate is no longer eligible at the reread
    /// inside this transaction.
    async fn recover_one(
        &self,
        asset_id: Uuid,
        root_id: Uuid,
        stale_before: OffsetDateTime,
    ) -> Result<Option<Uuid>, AppError> {
        self.uow
            .transaction::<Option<Uuid>, AppError>(TransactionOptions::default(), async |tx| {
                let Some(asset) = find_asset(tx, asset_id, true).await? else {
                    return Ok(None);
                };
                let now = self.now();
                let cutoff = stale_before.min(now - CLAIM_LEASE);
                if asset.identity.root_id() != root_id
                    || asset.lifecycle != "RESERVED"
                    || asset.created_at > cutoff
                    || find_generation_intent(tx, asset_id, false)
                        .await
                        .map_err(app)?
                        .is_some()
                    || referenced(tx, asset.document_id, asset.identity.relative_path()).await?
                {
                    return Ok(None);
                }
                retire_asset(tx, &asset, now, false, None)
                    .await
                    .map_err(app)?;
                Ok(Some(asset_id))
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The standalone path has no `invalid_input` callback, so a binding refusal is the same
    /// unavailable error as a stale claim.
    #[test]
    fn binding_refusal_on_the_standalone_path_is_the_unavailable_error() {
        assert_eq!(app(IntentError::Binding), unavailable());
        assert_eq!(app(IntentError::Unavailable), unavailable());
    }
}
