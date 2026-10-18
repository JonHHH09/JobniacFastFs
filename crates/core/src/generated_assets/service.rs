//! Generated-asset ports and the lifecycle service (classified path).
//!
//! The service owns the reconciliation rules: a reservation is proven only by re-reading the same
//! durable identity, a publication is checked against the reserved identity before any write, and
//! a renewal must move the fence forward. Persistence and file writes sit behind the ports.

use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use super::identity::{
    AssetIdentity, GeneratedReservation, InvalidGeneratedIdentity, MaterializationOutcome,
    PublicationFailure, PublicationOutcome, validate_file_name, validate_generated_pdf,
};
use super::intents::{
    GenerationDescriptor, GenerationIntent, GenerationPurpose, GenerationState, IntentReservation,
};
use super::standalone::CheckedWriter;
use crate::clock::Clock;
use crate::errors::AppError;

/// Number of expired or stale rows one recovery call may process.
pub const RETRY_BATCH_SIZE: usize = 25;

/// Age after which a reservation without a fence is stale (five minutes).
pub const CLAIM_LEASE: time::Duration = time::Duration::minutes(5);

/// Result of abandoning a generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparationOutcome {
    /// The asset was fenced and its retirement was enqueued.
    Prepared,
    /// The asset is referenced, so nothing changed.
    Referenced,
    /// The asset was already retired.
    AlreadyRetired,
    /// The claim is not eligible for abandonment.
    Ineligible,
}

/// Safe refusal of a generated-asset operation. It never carries paths, bytes or database text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct GeneratedAssetUnavailable(pub &'static str);

/// A verified generated asset with its outcome (classified path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedAssetResult {
    /// The re-read claim after materialization.
    pub claim: IntentReservation,
    /// The materialization outcome.
    pub outcome: MaterializationOutcome,
}

/// Persistence port of classified generation. Each method is one repository transaction; the
/// SQLite adapter lives in `jobniac_infra::sqlite::generated`.
#[async_trait]
pub trait GeneratedAssetRepository: Send + Sync {
    /// Commits the verified blob, the owned document, the reserved asset and its birth intent in
    /// one transaction, and returns the reservation as stored.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the repository port signature"
    )]
    async fn reserve_classified_pdf(
        &self,
        identity: &AssetIdentity,
        document_id: Uuid,
        original_file_name: &str,
        content: &[u8],
        descriptor: &GenerationDescriptor,
        lease_token: Uuid,
        now: OffsetDateTime,
    ) -> Result<IntentReservation, AppError>;

    /// Reads the classified reservation of one asset, if it exists.
    async fn find_classified_reservation(
        &self,
        asset_id: Uuid,
    ) -> Result<Option<IntentReservation>, AppError>;

    /// Holds the serialization for one materialization: verifies the claim, publishes through
    /// `writer` and finalizes the asset.
    async fn materialize_classified(
        &self,
        claim: &IntentReservation,
        writer: &dyn GeneratedAssetWriter,
    ) -> Result<MaterializationOutcome, AppError>;

    /// Moves a live fence to `new_token`, or returns `None` when the claim is no longer live.
    async fn renew_generation(
        &self,
        claim: &IntentReservation,
        new_token: Uuid,
    ) -> Result<Option<IntentReservation>, AppError>;

    /// Fences abandonment and enqueues retirement in one transaction.
    async fn abandon_generation(
        &self,
        claim: &IntentReservation,
    ) -> Result<PreparationOutcome, AppError>;

    /// Abandons up to `limit` expired pending generations of one root.
    async fn recover_expired_generations(
        &self,
        root_id: Uuid,
        limit: usize,
    ) -> Result<Vec<Uuid>, AppError>;
}

/// Sync file port: publishes verified bytes under an owned, immutable identity.
pub trait GeneratedAssetWriter: Send + Sync {
    /// Writes the file durably with no replacement or fence bypass.
    ///
    /// # Errors
    ///
    /// Returns a fixed-category [`PublicationFailure`] for every failed publication.
    fn materialize_owned(
        &self,
        identity: &AssetIdentity,
        content: &[u8],
    ) -> Result<PublicationOutcome, PublicationFailure>;
}

/// Lifecycle service for classified generated PDFs.
pub struct GeneratedAssetService {
    repository: Arc<dyn GeneratedAssetRepository>,
    writer: Arc<dyn GeneratedAssetWriter>,
    root_id: Uuid,
    clock: Arc<dyn Clock>,
    enabled: bool,
}

impl std::fmt::Debug for GeneratedAssetService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GeneratedAssetService")
            .field("root_id", &self.root_id)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

/// Refusal of a generated-lifecycle call that is not enabled.
const NOT_ENABLED: &str = "Generated lifecycle is not enabled";
/// Refusal of an unproven classified reservation.
const UNPROVEN: &str = "Classified reservation is unproven";

impl GeneratedAssetService {
    /// Creates the service. `enabled` must be the exact opt-in value resolved by the runtime.
    #[must_use]
    pub fn new(
        repository: Arc<dyn GeneratedAssetRepository>,
        writer: Arc<dyn GeneratedAssetWriter>,
        root_id: Uuid,
        clock: Arc<dyn Clock>,
        enabled: bool,
    ) -> Self {
        Self {
            repository,
            writer,
            root_id,
            clock,
            enabled,
        }
    }

    fn required(&self) -> Result<Uuid, GeneratedAssetUnavailable> {
        if self.enabled {
            Ok(self.root_id)
        } else {
            Err(GeneratedAssetUnavailable(NOT_ENABLED))
        }
    }

    /// Checks that the classified lifecycle is enabled, without reading the database or the
    /// filesystem.
    ///
    /// # Errors
    ///
    /// The refusal of a disabled lifecycle.
    pub fn ensure_classified_available(&self) -> Result<(), GeneratedAssetUnavailable> {
        self.required().map(|_| ())
    }

    /// Reserves a classified PDF and proves the stored reservation matches the one dispatched.
    ///
    /// # Errors
    ///
    /// Fails for invalid input (as [`InvalidGeneratedIdentity`] wrapped by the caller), a disabled
    /// lifecycle, or a reservation that cannot be proven after reconciliation.
    pub async fn reserve_classified_pdf(
        &self,
        content: &[u8],
        file_name: &str,
        descriptor: &GenerationDescriptor,
    ) -> Result<IntentReservation, ReserveError> {
        let root_id = self.required().map_err(ReserveError::Unavailable)?;
        validate_generated_pdf(content).map_err(ReserveError::Invalid)?;
        validate_file_name(file_name).map_err(ReserveError::Invalid)?;
        descriptor.validate().map_err(ReserveError::Invalid)?;
        let now = self.clock.now();
        let asset_id = Uuid::new_v4();
        let identity = AssetIdentity::new(
            asset_id,
            root_id,
            &sha256_hex(content),
            content.len() as u64,
        )
        .map_err(ReserveError::Invalid)?;
        let document_id = Uuid::new_v4();
        let token = Uuid::new_v4();
        let intent = GenerationIntent::pending(asset_id, descriptor.clone(), token, now)
            .map_err(ReserveError::Invalid)?;
        let expected = IntentReservation::new(
            GeneratedReservation {
                asset: identity.clone(),
                document_id,
            },
            intent,
        )
        .map_err(ReserveError::Invalid)?;
        let found = match self
            .repository
            .reserve_classified_pdf(
                &identity,
                document_id,
                file_name,
                content,
                descriptor,
                token,
                now,
            )
            .await
        {
            Ok(found) => found,
            Err(_) => match self.repository.find_classified_reservation(asset_id).await {
                Ok(found) => found.ok_or(ReserveError::Unavailable(GeneratedAssetUnavailable(
                    UNPROVEN,
                )))?,
                Err(_) => {
                    return Err(ReserveError::Unavailable(GeneratedAssetUnavailable(
                        "Classified reservation is unavailable",
                    )));
                }
            },
        };
        if found != expected {
            return Err(ReserveError::Unavailable(GeneratedAssetUnavailable(
                UNPROVEN,
            )));
        }
        Ok(expected)
    }

    /// Reads the classified reservation of one asset of this root.
    ///
    /// # Errors
    ///
    /// Fails when the lifecycle is disabled or the stored row belongs to another asset or root.
    pub async fn find_classified_reservation(
        &self,
        asset_id: Uuid,
    ) -> Result<Option<IntentReservation>, GeneratedAssetUnavailable> {
        let root_id = self.required()?;
        let unavailable = GeneratedAssetUnavailable("Classified reservation is unavailable");
        match self.repository.find_classified_reservation(asset_id).await {
            Ok(None) => Ok(None),
            Ok(Some(found))
                if found.asset_id() == asset_id && found.reservation.asset.root_id() == root_id =>
            {
                Ok(Some(found))
            }
            _ => Err(unavailable),
        }
    }

    /// Materializes a proven claim: publishes through the checked writer and re-reads the state.
    ///
    /// # Errors
    ///
    /// Fails when the claim is not proven, the lifecycle is disabled, or the final state differs.
    pub async fn materialize_classified(
        &self,
        claim: &IntentReservation,
    ) -> Result<ClassifiedAssetResult, GeneratedAssetUnavailable> {
        let root_id = self.required()?;
        if claim.reservation.asset.root_id() != root_id {
            return Err(GeneratedAssetUnavailable(UNPROVEN));
        }
        let found = self
            .repository
            .find_classified_reservation(claim.asset_id())
            .await
            .map_err(|_| GeneratedAssetUnavailable(UNPROVEN))?
            .ok_or(GeneratedAssetUnavailable(UNPROVEN))?;
        if !same_generation(&found, claim)
            || (found.intent.state == GenerationState::Pending && found.intent != claim.intent)
        {
            return Err(GeneratedAssetUnavailable(UNPROVEN));
        }
        let checked = CheckedWriter {
            expected: &claim.reservation.asset,
            writer: self.writer.as_ref(),
        };
        let unavailable =
            GeneratedAssetUnavailable("Classified materialization state is unavailable");
        let outcome = self
            .repository
            .materialize_classified(claim, &checked)
            .await
            .map_err(|_| unavailable.clone())?;
        let current = self
            .repository
            .find_classified_reservation(claim.asset_id())
            .await
            .map_err(|_| unavailable.clone())?
            .ok_or_else(|| unavailable.clone())?;
        if !same_generation(&current, claim) {
            return Err(unavailable);
        }
        if outcome == MaterializationOutcome::Active {
            let standalone_not_completed = current.intent.descriptor.purpose
                == GenerationPurpose::StandalonePdf
                && current.intent.state != GenerationState::Completed;
            if current.intent.state == GenerationState::Abandoned
                || standalone_not_completed
                || (current.intent.state == GenerationState::Pending
                    && current.intent != claim.intent)
            {
                return Err(unavailable);
            }
        }
        Ok(ClassifiedAssetResult {
            claim: current,
            outcome,
        })
    }

    /// Renews a pending claim with a fresh fence token.
    ///
    /// # Errors
    ///
    /// Fails when the claim is not pending in this root, or the renewal cannot be proven.
    pub async fn renew_generation(
        &self,
        claim: &IntentReservation,
    ) -> Result<Option<IntentReservation>, GeneratedAssetUnavailable> {
        let root_id = self.required()?;
        if claim.reservation.asset.root_id() != root_id
            || claim.intent.state != GenerationState::Pending
        {
            return Err(GeneratedAssetUnavailable(
                "Generation continuation is unavailable",
            ));
        }
        let token = Uuid::new_v4();
        let found = self
            .repository
            .renew_generation(claim, token)
            .await
            .map_err(|_| GeneratedAssetUnavailable("Generation renewal is unavailable"))?;
        let Some(found) = found else {
            return Ok(None);
        };
        let proven = same_generation(&found, claim)
            && found.intent.state == GenerationState::Pending
            && found.intent.lease_token == Some(token)
            && found.intent.lease_started_at >= claim.intent.lease_started_at
            && found.intent.lease_expires_at >= claim.intent.lease_expires_at;
        if !proven {
            return Err(GeneratedAssetUnavailable("Generation renewal is unproven"));
        }
        Ok(Some(found))
    }

    /// Abandons a claim; the repository fences it and enqueues retirement atomically.
    ///
    /// # Errors
    ///
    /// Fails when the claim belongs to another root or the repository state is unavailable.
    pub async fn abandon_generation(
        &self,
        claim: &IntentReservation,
    ) -> Result<PreparationOutcome, GeneratedAssetUnavailable> {
        let root_id = self.required()?;
        if claim.reservation.asset.root_id() != root_id {
            return Err(GeneratedAssetUnavailable(UNPROVEN));
        }
        self.repository
            .abandon_generation(claim)
            .await
            .map_err(|_| GeneratedAssetUnavailable("Generation abandonment state is unavailable"))
    }

    /// Abandons expired pending generations of this root, at most [`RETRY_BATCH_SIZE`].
    ///
    /// # Errors
    ///
    /// Fails when the lifecycle is disabled or the batch result is not a bounded unique set.
    pub async fn recover_expired_generations(
        &self,
    ) -> Result<Vec<Uuid>, GeneratedAssetUnavailable> {
        let root_id = self.required()?;
        let recovered = self
            .repository
            .recover_expired_generations(root_id, RETRY_BATCH_SIZE)
            .await
            .map_err(|_| GeneratedAssetUnavailable("Classified recovery state is unavailable"))?;
        let mut unique = recovered.clone();
        unique.sort_unstable();
        unique.dedup();
        if recovered.len() > RETRY_BATCH_SIZE || unique.len() != recovered.len() {
            return Err(GeneratedAssetUnavailable(
                "Classified recovery state is unavailable",
            ));
        }
        Ok(recovered)
    }
}

/// Failure of [`GeneratedAssetService::reserve_classified_pdf`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReserveError {
    /// The input or descriptor is invalid.
    #[error("{0}")]
    Invalid(InvalidGeneratedIdentity),
    /// The lifecycle is unavailable or the reservation cannot be proven.
    #[error("{0}")]
    Unavailable(GeneratedAssetUnavailable),
}

fn same_generation(left: &IntentReservation, right: &IntentReservation) -> bool {
    left.reservation == right.reservation
        && left.intent.asset_id == right.intent.asset_id
        && left.intent.descriptor == right.intent.descriptor
        && left.intent.created_at == right.intent.created_at
}

/// Lowercase hexadecimal SHA-256 of `content`.
#[must_use]
pub fn sha256_hex(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(content);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
