//! SQLite adapter of the classified generated-PDF port: reserve, find, materialize, renew, abandon
//! and expired recovery.
//!
//! As in [`super::repository`], the single writer serializes the transactions of a generation: a
//! renewal or recovery that meets another writer runs after that writer committed and rereads its
//! row, so it refuses by the same state checks (a rotated fence, a retired asset, a finished intent).
//! Publication runs inside the materialization transaction, so the writer spans the file write and
//! the finalization. Unlike a cleanup removal, this file phase is not split out of the transaction:
//! a concurrent abandon, expired recovery or cleanup of the same asset could otherwise retire it
//! and delete its (still absent) file while the write is in flight, leaving an orphan file. The
//! PostgreSQL adapter held a per-asset advisory lock here; the single writer is the equivalent. The
//! phase is one PDF write bounded by the owned-file limits, far below the busy timeout of the
//! writers that queue behind it ([`crate::sqlite::pool::BUSY_TIMEOUT`]).

use async_trait::async_trait;
use jobniac_core::clock::truncate_to_micros;
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
use rusqlite::params;
use time::OffsetDateTime;
use uuid::Uuid;

use super::IntentError;
use super::assets::retire_asset;
use super::bindings::AssetRow;
use super::guards::check_birth_intents;
use super::intents::{
    abandon_generation_intent, admit_generation_claim, complete_generation_intent,
    find_generation_intent, insert_generation_intent, renew_generation_intent,
};
use super::repository::{
    SqliteGeneratedAssetRepository, activate_reserved, app, failure_record, find_asset, referenced,
    reservation_content, reserve_rows, unavailable, validate_input,
};
use super::run_with;
use super::writer::run_file_phase;
use crate::generated::asset_files::RemovalFailureCategory;
use crate::sqlite::codec::{get_id, get_opt_id, instant_text, uuid_text};
use crate::sqlite::error::SqliteError;

/// Whether a stored row and intent are the generation a claim describes.
fn matches(row: &AssetRow, intent: &GenerationIntent, claim: &IntentReservation) -> bool {
    row.document_id == Some(claim.reservation.document_id)
        && row.identity == claim.reservation.asset
        && intent.asset_id == claim.intent.asset_id
        && intent.descriptor == claim.intent.descriptor
        && intent.created_at == claim.intent.created_at
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
impl GeneratedAssetRepository for SqliteGeneratedAssetRepository {
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
                run_with(tx, |conn| {
                    reserve_rows(
                        conn,
                        identity,
                        document_id,
                        original_file_name,
                        content,
                        now,
                        true,
                    )
                })?;
                // A classified birth must carry its intent when the transaction commits.
                let asset_id = identity.asset_id();
                tx.before_commit(Box::new(move |conn| check_birth_intents(conn, &[asset_id])));
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
                    let asset_id = claim.asset_id();
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
                    // The instant is read only now, after taking the writer, so a lease that
                    // expired during the wait refuses before any file is written.
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
                    if activate_reserved(tx, asset_id, now).await? != 1 {
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
        let now = truncate_to_micros(self.clock_now());
        let candidates: Vec<Uuid> = self
            .uow
            .transaction::<Vec<Uuid>, AppError>(
                TransactionOptions::default(),
                async |tx| {
                    let found = run_with::<_, SqliteError>(tx, |conn| {
                        let mut statement = conn.prepare(
                            "SELECT asset.asset_id, asset.document_id, asset.relative_path \
                             FROM jobniac_generated_assets asset \
                             JOIN jobniac_generation_intents intent ON intent.asset_id = asset.asset_id \
                             WHERE asset.root_id = ?1 AND asset.lifecycle IN ('RESERVED', 'ACTIVE') \
                               AND intent.state = 'PENDING' AND intent.lease_expires_at <= ?2 \
                             ORDER BY intent.lease_expires_at, asset.asset_id",
                        )?;
                        let mut rows = statement.query(params![uuid_text(root_id), instant_text(now)])?;
                        let mut found = Vec::new();
                        while let Some(row) = rows.next()? {
                            let path: String = row.get("relative_path")?;
                            found.push((
                                get_id(row, "asset_id")?,
                                get_opt_id(row, "document_id")?,
                                path,
                            ));
                        }
                        Ok(found)
                    })?;
                    // The referenced filter and the limit apply after the scan, in order, so a
                    // referenced expired intent does not starve a later eligible one.
                    let mut eligible = Vec::new();
                    for (asset_id, document_id, path) in found {
                        if eligible.len() >= limit {
                            break;
                        }
                        if referenced(tx, document_id, &path).await? {
                            continue;
                        }
                        document_id.ok_or(AppError::Internal)?;
                        eligible.push(asset_id);
                    }
                    Ok(eligible)
                },
            )
            .await?;
        let mut recovered = Vec::new();
        for asset_id in candidates {
            if let Some(asset_id) = self.recover_classified_one(asset_id, root_id).await? {
                recovered.push(asset_id);
            }
        }
        Ok(recovered)
    }
}

impl SqliteGeneratedAssetRepository {
    /// The clock instant before truncation, read at each step.
    fn clock_now(&self) -> OffsetDateTime {
        self.clock.now()
    }

    /// Abandons one expired pending generation, rereading the row and the intent first. `None`
    /// means the candidate is no longer eligible.
    async fn recover_classified_one(
        &self,
        asset_id: Uuid,
        root_id: Uuid,
    ) -> Result<Option<Uuid>, AppError> {
        self.uow
            .transaction::<Option<Uuid>, AppError>(TransactionOptions::default(), async |tx| {
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
                let Some(document_id) = row.document_id else {
                    return Ok(None);
                };
                if row.identity.root_id() != root_id
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
