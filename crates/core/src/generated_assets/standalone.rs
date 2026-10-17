//! Standalone generated PDFs: reserve, materialize, create and recover abandoned.
//!
//! A standalone PDF is born without a generation intent: the reservation is the asset, its
//! document and the immutable identity. The service proves a reservation by re-reading the same
//! durable identity, publishes only the reserved bytes through a checked writer, and asks the
//! repository for bounded recovery of stale reservations. Persistence sits behind
//! [`StandaloneAssetRepository`]; the PostgreSQL adapter lives in `jobniac_infra::postgres::generated`.

use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use super::identity::{
    AssetIdentity, GeneratedReservation, MaterializationOutcome, PublicationFailure,
    PublicationFailureCategory, PublicationOutcome, validate_file_name, validate_generated_pdf,
};
use super::service::{
    CLAIM_LEASE, GeneratedAssetUnavailable, GeneratedAssetWriter, RETRY_BATCH_SIZE, sha256_hex,
};
use crate::clock::Clock;
use crate::errors::AppError;

/// Refusal of a standalone call while the lifecycle is disabled.
const NOT_ENABLED: &str = "Generated lifecycle is not enabled";
/// Refusal of a reservation that cannot be proven.
const UNPROVEN: &str = "Generated reservation is unproven";
/// Refusal of a reservation that cannot be read.
const RESERVATION_UNAVAILABLE: &str = "Generated reservation is unavailable";
/// Refusal of a materialization whose state cannot be read back.
const MATERIALIZATION_UNAVAILABLE: &str = "Generated materialization state is unavailable";
/// Refusal of a recovery whose state cannot be read.
const RECOVERY_UNAVAILABLE: &str = "Generated recovery state is unavailable";
/// Refusal of a recovery result that is not a bounded unique set.
const RECOVERY_INVALID: &str = "Generated recovery result is invalid";

/// Persistence port of standalone generated PDFs. Each method is one repository operation; the
/// PostgreSQL adapter holds the locks, the transactions and the clock instant.
#[async_trait]
pub trait StandaloneAssetRepository: Send + Sync {
    /// Commits the verified blob, the owned document and the reserved asset in one transaction,
    /// and returns the reservation as stored.
    async fn reserve_generated_pdf(
        &self,
        identity: &AssetIdentity,
        document_id: Uuid,
        original_file_name: &str,
        content: &[u8],
    ) -> Result<GeneratedReservation, AppError>;

    /// Reads the reservation of one asset, if its document is attached. Never infers proof from
    /// the file system.
    async fn find_reservation(
        &self,
        asset_id: Uuid,
    ) -> Result<Option<GeneratedReservation>, AppError>;

    /// Holds the serialization of one asset, verifies the reservation, publishes its bytes through
    /// `writer` and finalizes the asset as `ACTIVE`, or enqueues retirement after a failure.
    async fn materialize_reserved(
        &self,
        asset_id: Uuid,
        writer: &dyn GeneratedAssetWriter,
    ) -> Result<MaterializationOutcome, AppError>;

    /// Try-locks, rereads and enqueues retirement for reserved assets of `root_id` created at or
    /// before `stale_before` that have no intent and no reference, at most `limit` of them.
    async fn recover_stale_reservations(
        &self,
        root_id: Uuid,
        stale_before: OffsetDateTime,
        limit: usize,
    ) -> Result<Vec<Uuid>, AppError>;
}

/// Failure of a standalone reservation or creation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StandaloneError {
    /// The bytes are not an admitted generated PDF.
    #[error("Generated PDF bytes are invalid")]
    InvalidContent,
    /// The file name is not an admitted generated PDF name.
    #[error("Generated PDF name is invalid")]
    InvalidFileName,
    /// The lifecycle is disabled or the state cannot be proven.
    #[error("{0}")]
    Unavailable(GeneratedAssetUnavailable),
}

/// The outcome of materializing a standalone reservation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandaloneResult {
    /// The reservation that was materialized.
    pub reservation: GeneratedReservation,
    /// The materialization outcome.
    pub outcome: MaterializationOutcome,
}

/// Wraps the file port so that only the reserved identity and its exact bytes can be published.
/// Shared by the
/// standalone and the classified services.
pub(crate) struct CheckedWriter<'a> {
    /// The reserved identity that the publication must carry.
    pub(crate) expected: &'a AssetIdentity,
    /// The file port that performs the verified publication.
    pub(crate) writer: &'a dyn GeneratedAssetWriter,
}

impl GeneratedAssetWriter for CheckedWriter<'_> {
    fn materialize_owned(
        &self,
        identity: &AssetIdentity,
        content: &[u8],
    ) -> Result<PublicationOutcome, PublicationFailure> {
        let mismatch = PublicationFailure {
            category: PublicationFailureCategory::IdentityMismatch,
        };
        if identity != self.expected
            || validate_generated_pdf(content).is_err()
            || content.len() as u64 != self.expected.size_bytes()
            || sha256_hex(content) != self.expected.sha256()
        {
            return Err(mismatch);
        }
        self.writer.materialize_owned(identity, content)
    }
}

/// Lifecycle service for standalone generated PDFs.
pub struct StandaloneAssetService {
    repository: Arc<dyn StandaloneAssetRepository>,
    writer: Arc<dyn GeneratedAssetWriter>,
    root_id: Uuid,
    clock: Arc<dyn Clock>,
    enabled: bool,
}

impl std::fmt::Debug for StandaloneAssetService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StandaloneAssetService")
            .field("root_id", &self.root_id)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl StandaloneAssetService {
    /// Creates the service. `enabled` must be the exact opt-in value resolved by the runtime.
    #[must_use]
    pub fn new(
        repository: Arc<dyn StandaloneAssetRepository>,
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

    /// Reserves a standalone PDF and proves the stored reservation matches the one dispatched.
    ///
    /// Validation runs in order: the bytes, then the file name. A failed reservation is
    /// reconciled by re-reading the same asset identifier before it is refused.
    ///
    /// # Errors
    ///
    /// Fails with [`StandaloneError::Unavailable`] when the lifecycle is disabled or the reservation
    /// cannot be proven, and with the invalid-input variants for bad bytes or names.
    pub async fn reserve_pdf(
        &self,
        content: &[u8],
        file_name: &str,
    ) -> Result<GeneratedReservation, StandaloneError> {
        let root_id = self.required().map_err(StandaloneError::Unavailable)?;
        validate_generated_pdf(content).map_err(|_| StandaloneError::InvalidContent)?;
        validate_file_name(file_name).map_err(|_| StandaloneError::InvalidFileName)?;
        let asset_id = Uuid::new_v4();
        let expected = GeneratedReservation {
            asset: AssetIdentity::new(
                asset_id,
                root_id,
                &sha256_hex(content),
                content.len() as u64,
            )
            .map_err(|_| StandaloneError::InvalidContent)?,
            document_id: Uuid::new_v4(),
        };
        let found = match self
            .repository
            .reserve_generated_pdf(&expected.asset, expected.document_id, file_name, content)
            .await
        {
            Ok(found) => found,
            Err(_) => match self.repository.find_reservation(asset_id).await {
                Ok(Some(found)) => found,
                Ok(None) => {
                    return Err(StandaloneError::Unavailable(GeneratedAssetUnavailable(
                        UNPROVEN,
                    )));
                }
                Err(_) => {
                    return Err(StandaloneError::Unavailable(GeneratedAssetUnavailable(
                        RESERVATION_UNAVAILABLE,
                    )));
                }
            },
        };
        if found != expected {
            return Err(StandaloneError::Unavailable(GeneratedAssetUnavailable(
                UNPROVEN,
            )));
        }
        Ok(expected)
    }

    /// Materializes a reservation: re-proves it, publishes its exact bytes through the checked
    /// writer and returns the outcome.
    ///
    /// # Errors
    ///
    /// Fails with [`GeneratedAssetUnavailable`] when the lifecycle is disabled, the reservation
    /// belongs to another root or cannot be proven, or the materialization state is unavailable.
    pub async fn materialize(
        &self,
        reservation: &GeneratedReservation,
    ) -> Result<StandaloneResult, GeneratedAssetUnavailable> {
        let root_id = self.required()?;
        if reservation.asset.root_id() != root_id {
            return Err(GeneratedAssetUnavailable(UNPROVEN));
        }
        let found = self
            .repository
            .find_reservation(reservation.asset.asset_id())
            .await
            .map_err(|_| GeneratedAssetUnavailable(RESERVATION_UNAVAILABLE))?;
        if found.as_ref() != Some(reservation) {
            return Err(GeneratedAssetUnavailable(UNPROVEN));
        }
        let checked = CheckedWriter {
            expected: &reservation.asset,
            writer: self.writer.as_ref(),
        };
        let outcome = self
            .repository
            .materialize_reserved(reservation.asset.asset_id(), &checked)
            .await
            .map_err(|_| GeneratedAssetUnavailable(MATERIALIZATION_UNAVAILABLE))?;
        Ok(StandaloneResult {
            reservation: reservation.clone(),
            outcome,
        })
    }

    /// Reserves and materializes a standalone PDF in one call.
    ///
    /// # Errors
    ///
    /// Fails with the same refusals as [`StandaloneAssetService::reserve_pdf`] or
    /// [`StandaloneAssetService::materialize`].
    pub async fn create(
        &self,
        content: &[u8],
        file_name: &str,
    ) -> Result<StandaloneResult, StandaloneError> {
        let reservation = self.reserve_pdf(content, file_name).await?;
        self.materialize(&reservation)
            .await
            .map_err(StandaloneError::Unavailable)
    }

    /// Retires up to [`RETRY_BATCH_SIZE`] stale reservations of this root whose claim is older than
    /// [`CLAIM_LEASE`].
    ///
    /// # Errors
    ///
    /// Fails when the lifecycle is disabled, the repository state is unavailable, or the result is
    /// not a bounded unique set.
    pub async fn recover_abandoned(&self) -> Result<Vec<Uuid>, GeneratedAssetUnavailable> {
        let root_id = self.required()?;
        let cutoff = self.clock.now() - CLAIM_LEASE;
        let recovered = self
            .repository
            .recover_stale_reservations(root_id, cutoff, RETRY_BATCH_SIZE)
            .await
            .map_err(|_| GeneratedAssetUnavailable(RECOVERY_UNAVAILABLE))?;
        let mut unique = recovered.clone();
        unique.sort_unstable();
        unique.dedup();
        if recovered.len() > RETRY_BATCH_SIZE || unique.len() != recovered.len() {
            return Err(GeneratedAssetUnavailable(RECOVERY_INVALID));
        }
        Ok(recovered)
    }
}
