//! PostgreSQL adapter of the standalone generated-PDF port: reserve, find, materialize
//! and recover.
//!
//! The asset lock (`jobniac.cleanup_asset_lock_key`) is taken as a transaction-scoped lock inside
//! the transaction that performs the writes. The guard trigger on `generated_assets` takes that key
//! on every write, so a second session holding it would deadlock against the writer. Blocking
//! (`pg_advisory_xact_lock`) serializes materialization; try-locks (`pg_try_advisory_xact_lock`)
//! skip a busy asset in recovery. Both are released when the transaction ends.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::clock::{Clock, format_instant, truncate_to_micros};
use jobniac_core::documents::{DocumentMetadata, DocumentRepository};
use jobniac_core::errors::{AppError, ApplicationError};
use jobniac_core::generated_assets::standalone::StandaloneAssetRepository;
use jobniac_core::generated_assets::{
    AssetIdentity, CLAIM_LEASE, GeneratedAssetWriter, GeneratedReservation, MaterializationOutcome,
    PublicationFailureCategory, RETRY_BATCH_SIZE, sha256_hex, validate_file_name,
    validate_generated_pdf,
};
use jobniac_core::transactions::TransactionOptions;
use time::OffsetDateTime;
use uuid::Uuid;

use super::IntentError;
use super::assets::retire_asset;
use super::bindings::AssetRow;
use super::intents::find_generation_intent;
use super::writer::run_file_phase;
use crate::generated::asset_files::RemovalFailureCategory;
use crate::postgres::documents::{PgDocumentRepository, save_file_in_transaction};
use crate::postgres::profiles::{col, db};
use crate::postgres::unit_of_work::{PgTransaction, PgUnitOfWork};

/// Asset columns read by the standalone path. Unlike admission, it never reads `xmin`, so a read
/// does not assign a transaction identifier.
const READ_COLUMNS: &str = "asset_id, root_id, relative_path, sha256, size_bytes, document_id, \
    lifecycle, created_at, materialized_at IS NOT NULL AS materialized, \
    generation_intent_required, false AS current_transaction";

/// Media type of stored generated PDFs.
pub(super) const PDF_MEDIA_TYPE: &str = "application/pdf";

/// The PostgreSQL adapter of [`StandaloneAssetRepository`]. The clock supplies every instant the
/// adapter writes.
#[derive(Debug, Clone)]
pub struct PgGeneratedAssetRepository {
    pub(super) uow: PgUnitOfWork,
    pub(super) clock: Arc<dyn Clock>,
}

impl PgGeneratedAssetRepository {
    /// A repository over `uow` that stamps its writes with `clock`.
    #[must_use]
    pub fn new(uow: PgUnitOfWork, clock: Arc<dyn Clock>) -> Self {
        Self { uow, clock }
    }

    /// The current instant, truncated to the microseconds PostgreSQL stores.
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

/// Takes the asset lock of the current transaction: blocking (`wait = true`) or a try-lock
/// (`wait = false`). Returns whether the lock is held.
pub(crate) async fn lock_asset(
    tx: &mut PgTransaction,
    asset_id: Uuid,
    wait: bool,
) -> Result<bool, AppError> {
    if wait {
        db(tx
            .client()
            .execute(
                "SELECT pg_advisory_xact_lock(jobniac.cleanup_asset_lock_key($1))",
                &[&asset_id],
            )
            .await)?;
        return Ok(true);
    }
    let row = db(tx
        .client()
        .query_one(
            "SELECT pg_try_advisory_xact_lock(jobniac.cleanup_asset_lock_key($1)) AS acquired",
            &[&asset_id],
        )
        .await)?;
    col(&row, "acquired")
}

/// Reads one asset row, optionally with a row lock.
pub(crate) async fn find_asset(
    tx: &mut PgTransaction,
    asset_id: Uuid,
    for_update: bool,
) -> Result<Option<AssetRow>, AppError> {
    let sql = format!(
        "SELECT {READ_COLUMNS} FROM jobniac.generated_assets WHERE asset_id = $1{}",
        if for_update { " FOR UPDATE" } else { "" }
    );
    let row = db(tx.client().query_opt(&sql, &[&asset_id]).await)?;
    row.as_ref()
        .map(AssetRow::from_row)
        .transpose()
        .map_err(app)
}

/// Whether the asset's document or relative path is still referenced.
pub(crate) async fn referenced(
    tx: &mut PgTransaction,
    document_id: Option<Uuid>,
    relative_path: &str,
) -> Result<bool, AppError> {
    let row = db(tx
        .client()
        .query_one(
            "SELECT jobniac.generated_asset_referenced($1, $2) AS referenced",
            &[&document_id, &relative_path],
        )
        .await)?;
    col(&row, "referenced")
}

/// Reads the verified bytes of an asset's document and checks them against the asset row. Every
/// mismatch is the unavailable database error.
pub(super) async fn reservation_content(
    tx: &mut PgTransaction,
    asset: &AssetRow,
) -> Result<(GeneratedReservation, Vec<u8>), AppError> {
    let document_id = asset.document_id.ok_or_else(unavailable)?;
    let (metadata, content) = PgDocumentRepository
        .find_content(tx, document_id)
        .await?
        .ok_or_else(unavailable)?;
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

/// Inserts a reserved standalone asset.
async fn insert_asset(
    tx: &mut PgTransaction,
    identity: &AssetIdentity,
    document_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let size = i64::try_from(identity.size_bytes()).map_err(|_| AppError::Internal)?;
    let generation_intent_required = false;
    db(tx
        .client()
        .execute(
            "INSERT INTO jobniac.generated_assets \
             (asset_id, root_id, relative_path, sha256, size_bytes, \
              document_id, lifecycle, created_at, generation_intent_required) \
             VALUES ($1, $2, $3, $4, $5, $6, 'RESERVED', $7, $8)",
            &[
                &identity.asset_id(),
                &identity.root_id(),
                &identity.relative_path(),
                &identity.sha256(),
                &size,
                &document_id,
                &now,
                &generation_intent_required,
            ],
        )
        .await)?;
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

/// The finalize step of a materialization, in the transaction that read the asset, after
/// publication: activate the asset after a successful write, or retire it with its cleanup task
/// after a failed one.
async fn finalize(
    tx: &mut PgTransaction,
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
            let activated = db(tx
                .client()
                .execute(
                    "UPDATE jobniac.generated_assets \
                     SET lifecycle = 'ACTIVE', materialized_at = $1 \
                     WHERE asset_id = $2 AND lifecycle = 'RESERVED'",
                    &[&now, &asset_id],
                )
                .await)?;
            if activated != 1 {
                return Err(unavailable());
            }
            Ok(MaterializationOutcome::Active)
        }
    }
}

#[async_trait]
impl StandaloneAssetRepository for PgGeneratedAssetRepository {
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
                    save_file_in_transaction(tx.client(), &metadata, content).await?;
                    insert_asset(tx, identity, document_id, now).await?;
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
                    // The blocking asset lock is taken before the row is read.
                    lock_asset(tx, asset_id, true).await?;
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
                let rows = db(tx
                    .client()
                    .query(
                        "SELECT asset_id FROM jobniac.generated_assets asset \
                         WHERE root_id = $1 AND lifecycle = 'RESERVED' AND created_at <= $2 \
                           AND NOT EXISTS (SELECT 1 FROM jobniac.generation_intents intent \
                                           WHERE intent.asset_id = asset.asset_id) \
                         ORDER BY created_at, asset_id LIMIT $3",
                        &[&root_id, &stale_before, &limit],
                    )
                    .await)?;
                rows.iter()
                    .map(|row| col::<Uuid>(row, "asset_id"))
                    .collect::<Result<Vec<_>, _>>()
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

impl PgGeneratedAssetRepository {
    /// Retires one stale candidate under its try-locked asset lock. `None` means the candidate was
    /// busy or no longer eligible at the locked reread.
    async fn recover_one(
        &self,
        asset_id: Uuid,
        root_id: Uuid,
        stale_before: OffsetDateTime,
    ) -> Result<Option<Uuid>, AppError> {
        self.uow
            .transaction::<Option<Uuid>, AppError>(TransactionOptions::default(), async |tx| {
                if !lock_asset(tx, asset_id, false).await? {
                    return Ok(None);
                }
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
    /// unavailable error as a busy lock.
    #[test]
    fn binding_refusal_on_the_standalone_path_is_the_unavailable_error() {
        assert_eq!(app(IntentError::Binding), unavailable());
        assert_eq!(app(IntentError::Unavailable), unavailable());
    }
}
