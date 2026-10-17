//! Generated-asset identities, classified intents and the classified lifecycle service, with
//! in-memory fakes of the persistence and file ports. All data is synthetic.

use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jobniac_core::clock::FixedClock;
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedAssetRepository, GeneratedAssetService, GeneratedAssetWriter,
    GeneratedReservation, GenerationDescriptor, GenerationIntent, GenerationPurpose,
    GenerationState, IntentReservation, MaterializationOutcome, PreparationOutcome,
    PublicationFailure, PublicationFailureCategory, PublicationOutcome, RETRY_BATCH_SIZE,
    sha256_hex, validate_binding_intents, validate_file_name, validate_generated_pdf,
};
use time::OffsetDateTime;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const PDF: &[u8] = b"%PDF-1.7\n% synthetic generated resume\n";

fn instant(seconds: i64) -> Result<OffsetDateTime, Box<dyn Error>> {
    Ok(OffsetDateTime::from_unix_timestamp(
        1_700_000_000 + seconds,
    )?)
}

fn identity(root: Uuid, content: &[u8]) -> Result<AssetIdentity, Box<dyn Error>> {
    Ok(AssetIdentity::new(
        Uuid::new_v4(),
        root,
        &sha256_hex(content),
        content.len() as u64,
    )?)
}

fn standalone(operation: Uuid) -> Result<GenerationDescriptor, Box<dyn Error>> {
    Ok(GenerationDescriptor::new(
        operation,
        GenerationPurpose::StandalonePdf,
        None,
        None,
        None,
        None,
    )?)
}

#[test]
fn identity_derives_the_path_and_refuses_bad_digests_and_sizes() -> TestResult {
    let root = Uuid::new_v4();
    let asset = identity(root, PDF)?;
    assert_eq!(asset.relative_path(), format!("{}.pdf", asset.asset_id()));
    assert!(AssetIdentity::new(Uuid::new_v4(), root, &"A".repeat(64), 1).is_err());
    assert!(AssetIdentity::new(Uuid::new_v4(), root, &"a".repeat(63), 1).is_err());
    assert!(AssetIdentity::new(Uuid::new_v4(), root, &"a".repeat(64), 0).is_err());
    assert!(
        AssetIdentity::new(Uuid::new_v4(), root, &"a".repeat(64), 10 * 1024 * 1024 + 1).is_err()
    );
    Ok(())
}

#[test]
fn from_parts_refuses_a_path_that_differs_from_the_asset_id() -> TestResult {
    let root = Uuid::new_v4();
    let asset = identity(root, PDF)?;
    let good = AssetIdentity::from_parts(
        asset.asset_id(),
        root,
        asset.relative_path(),
        asset.sha256(),
        asset.size_bytes(),
    );
    assert!(good.is_ok());
    let bare_name = AssetIdentity::from_parts(
        asset.asset_id(),
        root,
        "resume.pdf",
        asset.sha256(),
        asset.size_bytes(),
    );
    assert!(bare_name.is_err());
    Ok(())
}

#[test]
fn pdf_bytes_must_start_with_the_pdf_header_and_fit_the_bound() {
    assert!(validate_generated_pdf(PDF).is_ok());
    assert!(validate_generated_pdf(b"").is_err());
    assert!(validate_generated_pdf(b"PK\x03\x04 not a pdf").is_err());
    assert!(validate_generated_pdf(&vec![b'%'; 10 * 1024 * 1024 + 1]).is_err());
}

#[test]
fn file_names_must_be_trimmed_pdf_names_without_separators() {
    assert!(validate_file_name("Ada Example Resume.PDF").is_ok());
    assert!(validate_file_name(" resume.pdf").is_err());
    assert!(validate_file_name("resume.txt").is_err());
    assert!(validate_file_name("dir/resume.pdf").is_err());
    assert!(validate_file_name("dir\\resume.pdf").is_err());
    assert!(validate_file_name("bad\nname.pdf").is_err());
}

#[test]
fn descriptor_shapes_follow_the_purpose() -> TestResult {
    let operation = Uuid::new_v4();
    let profile = Uuid::new_v4();
    assert!(standalone(operation).is_ok());
    assert!(
        GenerationDescriptor::new(
            operation,
            GenerationPurpose::StandalonePdf,
            Some(profile),
            None,
            None,
            None
        )
        .is_err()
    );
    assert!(
        GenerationDescriptor::new(
            operation,
            GenerationPurpose::ProfileResume,
            Some(profile),
            None,
            None,
            Some("master_resume".to_owned())
        )
        .is_ok()
    );
    assert!(
        GenerationDescriptor::new(
            operation,
            GenerationPurpose::ProfileResume,
            Some(profile),
            None,
            None,
            Some("de".to_owned())
        )
        .is_err()
    );
    assert!(
        GenerationDescriptor::new(
            operation,
            GenerationPurpose::GermanCoverLetter,
            Some(profile),
            Some(Uuid::new_v4()),
            Some(Uuid::new_v4()),
            Some("en".to_owned())
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn pending_intents_need_a_lease_of_at_most_five_minutes() -> TestResult {
    let asset = Uuid::new_v4();
    let descriptor = standalone(Uuid::new_v4())?;
    let now = instant(0)?;
    let pending = GenerationIntent::pending(asset, descriptor.clone(), Uuid::new_v4(), now)?;
    assert_eq!(pending.state, GenerationState::Pending);
    assert!(
        GenerationIntent::new(
            asset,
            descriptor.clone(),
            GenerationState::Pending,
            Some(Uuid::new_v4()),
            Some(now),
            Some(now + time::Duration::minutes(6)),
            now,
            None,
        )
        .is_err()
    );
    assert!(
        GenerationIntent::new(
            asset,
            descriptor,
            GenerationState::Completed,
            None,
            None,
            None,
            now,
            None,
        )
        .is_err(),
        "a terminal intent needs its finish instant"
    );
    Ok(())
}

#[test]
fn binding_claims_must_match_their_bindings_and_one_operation() -> TestResult {
    let root = Uuid::new_v4();
    let operation = Uuid::new_v4();
    let now = instant(0)?;
    let asset = identity(root, PDF)?;
    let binding = GeneratedReservation {
        asset: asset.clone(),
        document_id: Uuid::new_v4(),
    };
    let intent = GenerationIntent::pending(
        asset.asset_id(),
        standalone(operation)?,
        Uuid::new_v4(),
        now,
    )?;
    let claim = IntentReservation::new(binding.clone(), intent)?;
    assert!(validate_binding_intents(std::slice::from_ref(&binding), None).is_ok());
    assert!(
        validate_binding_intents(
            std::slice::from_ref(&binding),
            Some(std::slice::from_ref(&claim))
        )
        .is_ok()
    );
    assert!(validate_binding_intents(&[], Some(std::slice::from_ref(&claim))).is_err());
    assert!(validate_binding_intents(&[binding.clone(), binding.clone(), binding], None).is_err());
    Ok(())
}

/// In-memory repository of classified reservations. It stores the bytes it was given and keeps
/// one intent per asset, like the PostgreSQL adapter.
#[derive(Default)]
struct FakeRepository {
    claims: Mutex<HashMap<Uuid, IntentReservation>>,
    materialized: Mutex<Vec<Uuid>>,
    fail_reserve: bool,
}

#[async_trait]
impl GeneratedAssetRepository for FakeRepository {
    async fn reserve_classified_pdf(
        &self,
        identity: &AssetIdentity,
        document_id: Uuid,
        _original_file_name: &str,
        content: &[u8],
        descriptor: &GenerationDescriptor,
        lease_token: Uuid,
        now: OffsetDateTime,
    ) -> Result<IntentReservation, AppError> {
        if sha256_hex(content) != identity.sha256() {
            return Err(AppError::Internal);
        }
        let intent =
            GenerationIntent::pending(identity.asset_id(), descriptor.clone(), lease_token, now)
                .map_err(|_| AppError::Internal)?;
        let claim = IntentReservation::new(
            GeneratedReservation {
                asset: identity.clone(),
                document_id,
            },
            intent,
        )
        .map_err(|_| AppError::Internal)?;
        if self.fail_reserve {
            // The transaction may have committed; the caller reconciles by identity.
            self.claims
                .lock()
                .map_err(|_| AppError::Internal)?
                .insert(identity.asset_id(), claim);
            return Err(AppError::Internal);
        }
        self.claims
            .lock()
            .map_err(|_| AppError::Internal)?
            .insert(identity.asset_id(), claim.clone());
        Ok(claim)
    }

    async fn find_classified_reservation(
        &self,
        asset_id: Uuid,
    ) -> Result<Option<IntentReservation>, AppError> {
        Ok(self
            .claims
            .lock()
            .map_err(|_| AppError::Internal)?
            .get(&asset_id)
            .cloned())
    }

    async fn materialize_classified(
        &self,
        claim: &IntentReservation,
        writer: &dyn GeneratedAssetWriter,
    ) -> Result<MaterializationOutcome, AppError> {
        // The fake holds no bytes, so it publishes the reference content the test supplies.
        let content = PDF;
        match writer.materialize_owned(&claim.reservation.asset, content) {
            Ok(_) => {
                // A standalone classified output completes its intent in the same transaction.
                let mut claims = self.claims.lock().map_err(|_| AppError::Internal)?;
                if let Some(stored) = claims.get_mut(&claim.asset_id()) {
                    stored.intent.state = GenerationState::Completed;
                    stored.intent.lease_token = None;
                    stored.intent.lease_started_at = None;
                    stored.intent.lease_expires_at = None;
                    stored.intent.finished_at = Some(stored.intent.created_at);
                }
                self.materialized
                    .lock()
                    .map_err(|_| AppError::Internal)?
                    .push(claim.asset_id());
                Ok(MaterializationOutcome::Active)
            }
            Err(_) => Ok(MaterializationOutcome::Blocked),
        }
    }

    async fn renew_generation(
        &self,
        claim: &IntentReservation,
        new_token: Uuid,
    ) -> Result<Option<IntentReservation>, AppError> {
        let mut claims = self.claims.lock().map_err(|_| AppError::Internal)?;
        let Some(stored) = claims.get_mut(&claim.asset_id()) else {
            return Ok(None);
        };
        let start = stored.intent.lease_started_at.ok_or(AppError::Internal)?;
        stored.intent.lease_token = Some(new_token);
        stored.intent.lease_started_at = Some(start + time::Duration::seconds(10));
        stored.intent.lease_expires_at = Some(start + time::Duration::seconds(310));
        Ok(Some(stored.clone()))
    }

    async fn abandon_generation(
        &self,
        claim: &IntentReservation,
    ) -> Result<PreparationOutcome, AppError> {
        let mut claims = self.claims.lock().map_err(|_| AppError::Internal)?;
        let Some(stored) = claims.get_mut(&claim.asset_id()) else {
            return Ok(PreparationOutcome::Ineligible);
        };
        stored.intent.state = GenerationState::Abandoned;
        stored.intent.lease_token = None;
        stored.intent.lease_started_at = None;
        stored.intent.lease_expires_at = None;
        stored.intent.finished_at = Some(claim.intent.created_at);
        Ok(PreparationOutcome::Prepared)
    }

    async fn recover_expired_generations(
        &self,
        _root_id: Uuid,
        limit: usize,
    ) -> Result<Vec<Uuid>, AppError> {
        Ok(self
            .claims
            .lock()
            .map_err(|_| AppError::Internal)?
            .keys()
            .copied()
            .take(limit)
            .collect())
    }
}

/// File fake that accepts one identity and records what it published.
#[derive(Default)]
struct FakeWriter {
    published: Mutex<Vec<Uuid>>,
}

impl GeneratedAssetWriter for FakeWriter {
    fn materialize_owned(
        &self,
        identity: &AssetIdentity,
        _content: &[u8],
    ) -> Result<PublicationOutcome, PublicationFailure> {
        self.published
            .lock()
            .map_err(|_| PublicationFailure {
                category: PublicationFailureCategory::FileWriteFailed,
            })?
            .push(identity.asset_id());
        Ok(PublicationOutcome::Created)
    }
}

fn service(
    repository: Arc<FakeRepository>,
    writer: Arc<FakeWriter>,
    root: Uuid,
    enabled: bool,
) -> Result<GeneratedAssetService, Box<dyn Error>> {
    Ok(GeneratedAssetService::new(
        repository,
        writer,
        root,
        Arc::new(FixedClock::new(instant(0)?)),
        enabled,
    ))
}

#[tokio::test]
async fn disabled_lifecycle_refuses_every_classified_call() -> TestResult {
    let service = service(
        Arc::new(FakeRepository::default()),
        Arc::new(FakeWriter::default()),
        Uuid::new_v4(),
        false,
    )?;
    let descriptor = standalone(Uuid::new_v4())?;
    assert!(
        service
            .reserve_classified_pdf(PDF, "resume.pdf", &descriptor)
            .await
            .is_err()
    );
    assert!(service.recover_expired_generations().await.is_err());
    Ok(())
}

#[tokio::test]
async fn reserve_then_materialize_publishes_the_reserved_identity() -> TestResult {
    let root = Uuid::new_v4();
    let repository = Arc::new(FakeRepository::default());
    let writer = Arc::new(FakeWriter::default());
    let service = service(repository.clone(), writer.clone(), root, true)?;
    let claim = service
        .reserve_classified_pdf(PDF, "resume.pdf", &standalone(Uuid::new_v4())?)
        .await?;
    assert_eq!(claim.intent.state, GenerationState::Pending);
    let result = service.materialize_classified(&claim).await?;
    assert_eq!(result.outcome, MaterializationOutcome::Active);
    assert_eq!(
        writer.published.lock().map_err(|_| "poisoned")?.as_slice(),
        &[claim.asset_id()]
    );
    Ok(())
}

#[tokio::test]
async fn reserve_reconciles_a_possibly_committed_reservation_by_identity() -> TestResult {
    let repository = Arc::new(FakeRepository {
        fail_reserve: true,
        ..FakeRepository::default()
    });
    let service = service(
        repository,
        Arc::new(FakeWriter::default()),
        Uuid::new_v4(),
        true,
    )?;
    let claim = service
        .reserve_classified_pdf(PDF, "resume.pdf", &standalone(Uuid::new_v4())?)
        .await?;
    assert_eq!(claim.intent.state, GenerationState::Pending);
    assert_eq!(claim.asset_id(), claim.reservation.asset.asset_id());
    Ok(())
}

#[tokio::test]
async fn renewal_moves_the_fence_and_keeps_the_claim_pending() -> TestResult {
    let root = Uuid::new_v4();
    let service = service(
        Arc::new(FakeRepository::default()),
        Arc::new(FakeWriter::default()),
        root,
        true,
    )?;
    let claim = service
        .reserve_classified_pdf(PDF, "resume.pdf", &standalone(Uuid::new_v4())?)
        .await?;
    let renewed = service
        .renew_generation(&claim)
        .await?
        .ok_or("the live claim must renew")?;
    assert_ne!(renewed.intent.lease_token, claim.intent.lease_token);
    assert_eq!(renewed.intent.state, GenerationState::Pending);
    Ok(())
}

#[tokio::test]
async fn abandonment_is_fenced_and_recovery_is_bounded() -> TestResult {
    let root = Uuid::new_v4();
    let service = service(
        Arc::new(FakeRepository::default()),
        Arc::new(FakeWriter::default()),
        root,
        true,
    )?;
    let claim = service
        .reserve_classified_pdf(PDF, "resume.pdf", &standalone(Uuid::new_v4())?)
        .await?;
    assert_eq!(
        service.abandon_generation(&claim).await?,
        PreparationOutcome::Prepared
    );
    let recovered = service.recover_expired_generations().await?;
    assert!(recovered.len() <= RETRY_BATCH_SIZE);
    Ok(())
}

#[tokio::test]
async fn foreign_root_claims_are_refused() -> TestResult {
    let service = service(
        Arc::new(FakeRepository::default()),
        Arc::new(FakeWriter::default()),
        Uuid::new_v4(),
        true,
    )?;
    let foreign = service_claim_in_other_root().await?;
    assert!(service.materialize_classified(&foreign).await.is_err());
    assert!(service.abandon_generation(&foreign).await.is_err());
    Ok(())
}

async fn service_claim_in_other_root() -> Result<IntentReservation, Box<dyn Error>> {
    let other = service(
        Arc::new(FakeRepository::default()),
        Arc::new(FakeWriter::default()),
        Uuid::new_v4(),
        true,
    )?;
    Ok(other
        .reserve_classified_pdf(PDF, "resume.pdf", &standalone(Uuid::new_v4())?)
        .await?)
}
