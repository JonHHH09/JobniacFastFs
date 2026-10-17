//! PostgreSQL adapter of the classified generated-PDF port: reserve, find, materialize,
//! renew, abandon and expired recovery.
//!
//! As in [`super::repository`], the locks are transaction-scoped and taken in a fixed order: the
//! document advisory lock (`jobniac-owned-document:`) first, then the asset lock. Publication
//! runs inside the materialization transaction, so the asset and document locks span the write and
//! the finalization. A busy document or asset is skipped by the try-lock callers (renewal and
//! recovery) and waited for by materialization and abandonment.

use async_trait::async_trait;
use jobniac_core::clock::{format_instant, truncate_to_micros};
use jobniac_core::documents::DocumentMetadata;
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::identity::generation_instant;
use jobniac_core::generated_assets::service::{
    GeneratedAssetRepository, GeneratedAssetWriter, PreparationOutcome, RETRY_BATCH_SIZE,
};
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedReservation, GenerationDescriptor, GenerationIntent, GenerationPurpose,
    GenerationState, IntentReservation, MaterializationOutcome, PublicationFailureCategory,
};
use jobniac_core::transactions::TransactionOptions;
use time::OffsetDateTime;
use uuid::Uuid;

use super::IntentError;
use super::assets::retire_asset;
use super::bindings::{AssetRow, document_lock_key_input};
use super::intents::{
    abandon_generation_intent, admit_generation_claim, complete_generation_intent,
    find_generation_intent, insert_generation_intent, renew_generation_intent,
};
use super::repository::{
    PDF_MEDIA_TYPE, PgGeneratedAssetRepository, app, failure_record, find_asset, lock_asset,
    referenced, reservation_content, unavailable, validate_input,
};
use super::writer::run_file_phase;
use crate::generated::asset_files::RemovalFailureCategory;
use crate::postgres::documents::save_file_in_transaction;
use crate::postgres::profiles::{col, db};
use crate::postgres::unit_of_work::PgTransaction;

/// Takes the document advisory lock and then the asset lock of one generation. `wait` selects the
/// blocking or the try-lock form. Returns whether both locks are held.
async fn lock_generation(
    tx: &mut PgTransaction,
    document_id: Uuid,
    asset_id: Uuid,
    wait: bool,
) -> Result<bool, AppError> {
    let key = document_lock_key_input(document_id);
    let acquired = if wait {
        db(tx
            .client()
            .execute(
                "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
                &[&key],
            )
            .await)?;
        true
    } else {
        let row = db(tx
            .client()
            .query_one(
                "SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0)) AS acquired",
                &[&key],
            )
            .await)?;
        col::<bool>(&row, "acquired")?
    };
    if !acquired {
        return Ok(false);
    }
    lock_asset(tx, asset_id, wait).await
}

/// Whether a stored row and intent are the generation a claim describes.
fn matches(row: &AssetRow, intent: &GenerationIntent, claim: &IntentReservation) -> bool {
    row.document_id == Some(claim.reservation.document_id)
        && row.identity == claim.reservation.asset
        && intent.asset_id == claim.intent.asset_id
        && intent.descriptor == claim.intent.descriptor
        && intent.created_at == claim.intent.created_at
}

/// Inserts a reserved classified asset, which requires its generation intent.
async fn insert_classified_asset(
    tx: &mut PgTransaction,
    identity: &AssetIdentity,
    document_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let size = i64::try_from(identity.size_bytes()).map_err(|_| AppError::Internal)?;
    let generation_intent_required = true;
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

/// Maps an intent failure of an admission step: an unavailable claim is `None`, anything else is
/// an error.
fn unavailable_as_none<T>(result: Result<T, IntentError>) -> Result<Option<T>, AppError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(IntentError::Unavailable) => Ok(None),
        Err(error) => Err(app(error)),
    }
}

#[async_trait]
impl GeneratedAssetRepository for PgGeneratedAssetRepository {
    async fn reserve_classified_pdf(
        &self,
        identity: &AssetIdentity,
        document_id: Uuid,
        original_file_name: &str,
        content: &[u8],
        descriptor: &GenerationDescriptor,
        lease_token: Uuid,
        now: OffsetDateTime,
    ) -> Result<IntentReservation, AppError> {
        run_file_phase(|| validate_input(identity, original_file_name, content))?;
        descriptor.validate().map_err(|_| AppError::Internal)?;
        let now = generation_instant(now).map_err(|_| AppError::Internal)?;
        let reservation = GeneratedReservation {
            asset: identity.clone(),
            document_id,
        };
        self.uow
            .transaction::<IntentReservation, AppError>(TransactionOptions::default(), async |tx| {
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
                insert_classified_asset(tx, identity, document_id, now).await?;
                insert_generation_intent(tx, &reservation, descriptor, lease_token, now)
                    .await
                    .map_err(app)
            })
            .await
    }

    async fn find_classified_reservation(
        &self,
        asset_id: Uuid,
    ) -> Result<Option<IntentReservation>, AppError> {
        let read_only = TransactionOptions {
            read_only: true,
            ..TransactionOptions::default()
        };
        self.uow
            .transaction::<Option<IntentReservation>, AppError>(read_only, async |tx| {
                let Some(asset) = find_asset(tx, asset_id, false).await? else {
                    return Ok(None);
                };
                let intent = find_generation_intent(tx, asset_id, false)
                    .await
                    .map_err(app)?;
                let (Some(intent), Some(_)) = (intent, asset.document_id) else {
                    return Ok(None);
                };
                let (reservation, _) = reservation_content(tx, &asset).await?;
                IntentReservation::new(reservation, intent)
                    .map(Some)
                    .map_err(|_| AppError::Internal)
            })
            .await
    }

    async fn materialize_classified(
        &self,
        claim: &IntentReservation,
        writer: &dyn GeneratedAssetWriter,
    ) -> Result<MaterializationOutcome, AppError> {
        self.uow
            .transaction::<MaterializationOutcome, AppError>(
                TransactionOptions::default(),
                async |tx| {
                    let document_id = claim.reservation.document_id;
                    let asset_id = claim.asset_id();
                    if !lock_generation(tx, document_id, asset_id, true).await? {
                        return Ok(MaterializationOutcome::Ineligible);
                    }
                    let Some(row) = find_asset(tx, asset_id, true).await? else {
                        return Ok(MaterializationOutcome::Ineligible);
                    };
                    let Some(intent) = find_generation_intent(tx, asset_id, true)
                        .await
                        .map_err(app)?
                    else {
                        return Ok(MaterializationOutcome::Ineligible);
                    };
                    if !matches(&row, &intent, claim) {
                        return Ok(MaterializationOutcome::Ineligible);
                    }
                    if intent.state == GenerationState::Completed {
                        return Ok(if row.lifecycle == "ACTIVE" {
                            MaterializationOutcome::Active
                        } else {
                            MaterializationOutcome::Ineligible
                        });
                    }
                    if intent.state != GenerationState::Pending {
                        return Ok(MaterializationOutcome::Ineligible);
                    }
                    // The instant is read only now, after the possibly long wait for the locks, so a
                    // lease that expired during the wait refuses before any file is written.
                    let now = truncate_to_micros(self.clock_now());
                    let admitted = unavailable_as_none(
                        admit_generation_claim(tx, claim, &claim.intent.descriptor, now).await,
                    )?;
                    if admitted.is_none() {
                        return Ok(MaterializationOutcome::Ineligible);
                    }
                    if row.lifecycle == "ACTIVE" {
                        return Ok(MaterializationOutcome::Active);
                    }
                    if row.lifecycle != "RESERVED"
                        || referenced(tx, row.document_id, row.identity.relative_path()).await?
                    {
                        return Ok(MaterializationOutcome::Ineligible);
                    }
                    let (reservation, content) = reservation_content(tx, &row).await?;
                    // A publication failure is recorded and finalized, not raised.
                    let failure = writer
                        .materialize_owned(&reservation.asset, &content)
                        .err()
                        .map(|failure| failure.category);
                    let Some(current) = find_asset(tx, asset_id, true).await? else {
                        return Ok(MaterializationOutcome::Ineligible);
                    };
                    let Some(current_intent) = find_generation_intent(tx, asset_id, true)
                        .await
                        .map_err(app)?
                    else {
                        return Ok(MaterializationOutcome::Ineligible);
                    };
                    if !matches(&current, &current_intent, claim)
                        || current.lifecycle != "RESERVED"
                        || current_intent.state != GenerationState::Pending
                    {
                        return Ok(MaterializationOutcome::Ineligible);
                    }
                    let now = truncate_to_micros(self.clock_now());
                    let admitted = unavailable_as_none(
                        admit_generation_claim(tx, claim, &claim.intent.descriptor, now).await,
                    )?;
                    // A callback begun with a live lease cannot publish completion after expiry;
                    // the same immutable incoming identity remains compensatable.
                    let failure = if admitted.is_none() {
                        Some(PublicationFailureCategory::FileWriteFailed)
                    } else {
                        failure
                    };
                    if let Some(category) = failure {
                        let abandoned = abandon_generation_intent(tx, claim, now)
                            .await
                            .map_err(app)?;
                        if abandoned.is_none() {
                            return Ok(MaterializationOutcome::Ineligible);
                        }
                        let (blocked, removal): (bool, Option<RemovalFailureCategory>) =
                            failure_record(category);
                        retire_asset(tx, &current, now, blocked, removal)
                            .await
                            .map_err(app)?;
                        return Ok(if blocked {
                            MaterializationOutcome::Blocked
                        } else {
                            MaterializationOutcome::CleanupPending
                        });
                    }
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
                    if current_intent.descriptor.purpose == GenerationPurpose::StandalonePdf {
                        complete_generation_intent(tx, claim, &claim.intent.descriptor, now)
                            .await
                            .map_err(app)?;
                    }
                    Ok(MaterializationOutcome::Active)
                },
            )
            .await
    }

    async fn renew_generation(
        &self,
        claim: &IntentReservation,
        new_token: Uuid,
    ) -> Result<Option<IntentReservation>, AppError> {
        let now = truncate_to_micros(self.clock_now());
        self.uow
            .transaction::<Option<IntentReservation>, AppError>(
                TransactionOptions::default(),
                async |tx| {
                    if !lock_generation(tx, claim.reservation.document_id, claim.asset_id(), false)
                        .await?
                    {
                        return Ok(None);
                    }
                    unavailable_as_none(renew_generation_intent(tx, claim, new_token, now).await)
                },
            )
            .await
    }

    async fn abandon_generation(
        &self,
        claim: &IntentReservation,
    ) -> Result<PreparationOutcome, AppError> {
        self.uow
            .transaction::<PreparationOutcome, AppError>(
                TransactionOptions::default(),
                async |tx| {
                    let asset_id = claim.asset_id();
                    if !lock_generation(tx, claim.reservation.document_id, asset_id, true).await? {
                        return Ok(PreparationOutcome::Ineligible);
                    }
                    let Some(row) = find_asset(tx, asset_id, true).await? else {
                        return Ok(PreparationOutcome::Ineligible);
                    };
                    let Some(intent) = find_generation_intent(tx, asset_id, true)
                        .await
                        .map_err(app)?
                    else {
                        return Ok(PreparationOutcome::Ineligible);
                    };
                    if !matches(&row, &intent, claim) {
                        return Ok(PreparationOutcome::Ineligible);
                    }
                    if row.lifecycle == "RETIRING" || row.lifecycle == "DELETED" {
                        return Ok(PreparationOutcome::AlreadyRetired);
                    }
                    if intent.state == GenerationState::Completed {
                        return Ok(
                            if referenced(tx, row.document_id, row.identity.relative_path()).await?
                            {
                                PreparationOutcome::Referenced
                            } else {
                                PreparationOutcome::Ineligible
                            },
                        );
                    }
                    if intent.state != GenerationState::Pending || intent != claim.intent {
                        return Ok(PreparationOutcome::Ineligible);
                    }
                    let now = truncate_to_micros(self.clock_now());
                    let abandoned = abandon_generation_intent(tx, claim, now)
                        .await
                        .map_err(app)?;
                    if abandoned.is_none() {
                        return Ok(PreparationOutcome::Referenced);
                    }
                    retire_asset(tx, &row, now, false, None)
                        .await
                        .map_err(app)?;
                    Ok(PreparationOutcome::Prepared)
                },
            )
            .await
    }

    async fn recover_expired_generations(
        &self,
        root_id: Uuid,
        limit: usize,
    ) -> Result<Vec<Uuid>, AppError> {
        if limit == 0 || limit > RETRY_BATCH_SIZE {
            return Err(AppError::Internal);
        }
        let limit = i64::try_from(limit).map_err(|_| AppError::Internal)?;
        let now = truncate_to_micros(self.clock_now());
        let candidates: Vec<(Uuid, Uuid)> = self
            .uow
            .transaction::<Vec<(Uuid, Uuid)>, AppError>(
                TransactionOptions::default(),
                async |tx| {
                    let rows = db(tx
                        .client()
                        .query(
                            "SELECT asset.asset_id, asset.document_id \
                             FROM jobniac.generated_assets asset \
                             JOIN jobniac.generation_intents intent ON intent.asset_id = asset.asset_id \
                             WHERE asset.root_id = $1 AND asset.lifecycle IN ('RESERVED', 'ACTIVE') \
                               AND intent.state = 'PENDING' AND intent.lease_expires_at <= $2 \
                               AND NOT jobniac.generated_asset_referenced(\
                                   asset.document_id, asset.relative_path) \
                             ORDER BY intent.lease_expires_at, asset.asset_id LIMIT $3",
                            &[&root_id, &now, &limit],
                        )
                        .await)?;
                    rows.iter()
                        .map(|row| {
                            let asset: Uuid = col(row, "asset_id")?;
                            let document: Option<Uuid> = col(row, "document_id")?;
                            Ok((asset, document.ok_or(AppError::Internal)?))
                        })
                        .collect::<Result<Vec<_>, AppError>>()
                },
            )
            .await?;
        let mut recovered = Vec::new();
        for (asset_id, document_id) in candidates {
            if let Some(asset_id) = self
                .recover_classified_one(asset_id, document_id, root_id)
                .await?
            {
                recovered.push(asset_id);
            }
        }
        Ok(recovered)
    }
}

impl PgGeneratedAssetRepository {
    /// The clock instant before truncation, read at each step.
    fn clock_now(&self) -> OffsetDateTime {
        self.clock.now()
    }

    /// Abandons one expired pending generation under its try-locked generation, rereading the row
    /// and the intent first. `None` means the candidate was busy or no longer eligible.
    async fn recover_classified_one(
        &self,
        asset_id: Uuid,
        document_id: Uuid,
        root_id: Uuid,
    ) -> Result<Option<Uuid>, AppError> {
        self.uow
            .transaction::<Option<Uuid>, AppError>(TransactionOptions::default(), async |tx| {
                if !lock_generation(tx, document_id, asset_id, false).await? {
                    return Ok(None);
                }
                let Some(row) = find_asset(tx, asset_id, true).await? else {
                    return Ok(None);
                };
                let Some(intent) = find_generation_intent(tx, asset_id, true)
                    .await
                    .map_err(app)?
                else {
                    return Ok(None);
                };
                let now = truncate_to_micros(self.clock_now());
                let expired = intent
                    .lease_expires_at
                    .is_some_and(|expires| expires <= now);
                if row.identity.root_id() != root_id
                    || row.document_id.is_none()
                    || !matches!(row.lifecycle.as_str(), "RESERVED" | "ACTIVE")
                    || intent.state != GenerationState::Pending
                    || !expired
                    || referenced(tx, row.document_id, row.identity.relative_path()).await?
                {
                    return Ok(None);
                }
                let claim = IntentReservation::new(
                    GeneratedReservation {
                        asset: row.identity.clone(),
                        document_id,
                    },
                    intent,
                )
                .map_err(|_| AppError::Internal)?;
                let abandoned = abandon_generation_intent(tx, &claim, now)
                    .await
                    .map_err(app)?;
                if abandoned.is_none() {
                    return Ok(None);
                }
                retire_asset(tx, &row, now, false, None)
                    .await
                    .map_err(app)?;
                Ok(Some(asset_id))
            })
            .await
    }
}
