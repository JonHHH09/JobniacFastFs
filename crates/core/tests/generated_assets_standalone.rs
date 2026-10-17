//! Standalone generated PDFs (reserve, materialize, create and recover abandoned reservations)
//! against in-memory fakes of the repository and the file port. All fixtures are synthetic.

use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use jobniac_core::clock::FixedClock;
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::standalone::{
    StandaloneAssetRepository, StandaloneAssetService, StandaloneError,
};
use jobniac_core::generated_assets::{
    AssetIdentity, CLAIM_LEASE, GeneratedAssetUnavailable, GeneratedAssetWriter,
    GeneratedReservation, MaterializationOutcome, PublicationFailure, PublicationFailureCategory,
    PublicationOutcome, RETRY_BATCH_SIZE, sha256_hex,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

/// A synthetic PDF with a valid header.
const PDF: &[u8] = b"%PDF-1.7\n% synthetic standalone resume\n";

const UNPROVEN: &str = "Generated reservation is unproven";
const RESERVATION_UNAVAILABLE: &str = "Generated reservation is unavailable";
const MATERIALIZATION_UNAVAILABLE: &str = "Generated materialization state is unavailable";
const RECOVERY_UNAVAILABLE: &str = "Generated recovery state is unavailable";
const RECOVERY_INVALID: &str = "Generated recovery result is invalid";
const NOT_ENABLED: &str = "Generated lifecycle is not enabled";

fn now() -> Result<OffsetDateTime, Box<dyn Error>> {
    Ok(OffsetDateTime::from_unix_timestamp(1_700_000_000)?)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One stored asset: its reservation, the bytes of its document and its lifecycle.
#[derive(Debug, Clone)]
struct Row {
    reservation: GeneratedReservation,
    content: Vec<u8>,
    state: State,
    created: OffsetDateTime,
    referenced: bool,
}

/// Lifecycle of a fake row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Reserved,
    Active,
    Retired,
}

/// In-memory repository with switches for the failure paths the service must survive.
#[derive(Debug)]
struct Fake {
    root: Uuid,
    now: OffsetDateTime,
    rows: Mutex<HashMap<Uuid, Row>>,
    /// The reservation is committed, then the call reports an error.
    reserve_fails_after_commit: bool,
    /// The reservation fails and nothing is committed.
    reserve_fails: bool,
    find_fails: bool,
    /// Lookups return a reservation with another document identifier.
    foreign_find: bool,
    /// Materialization hands the file port bytes that differ from the stored content.
    tamper: bool,
    materialize_fails: bool,
    recover_fails: bool,
    recover_override: Option<Vec<Uuid>>,
    recover_cutoffs: Mutex<Vec<OffsetDateTime>>,
}

impl Fake {
    fn new(root: Uuid, now: OffsetDateTime) -> Self {
        Self {
            root,
            now,
            rows: Mutex::new(HashMap::new()),
            reserve_fails_after_commit: false,
            reserve_fails: false,
            find_fails: false,
            foreign_find: false,
            tamper: false,
            materialize_fails: false,
            recover_fails: false,
            recover_override: None,
            recover_cutoffs: Mutex::new(Vec::new()),
        }
    }

    /// Stores a row built from synthetic bytes and returns its asset identifier.
    fn seed(
        &self,
        root: Uuid,
        created: OffsetDateTime,
        referenced: bool,
    ) -> Result<Uuid, Box<dyn Error>> {
        let asset_id = Uuid::new_v4();
        let asset = AssetIdentity::new(asset_id, root, &sha256_hex(PDF), PDF.len() as u64)?;
        let reservation = GeneratedReservation {
            asset,
            document_id: Uuid::new_v4(),
        };
        lock(&self.rows).insert(
            asset_id,
            Row {
                reservation,
                content: PDF.to_vec(),
                state: State::Reserved,
                created,
                referenced,
            },
        );
        Ok(asset_id)
    }
}

#[async_trait]
impl StandaloneAssetRepository for Fake {
    async fn reserve_generated_pdf(
        &self,
        identity: &AssetIdentity,
        document_id: Uuid,
        _original_file_name: &str,
        content: &[u8],
    ) -> Result<GeneratedReservation, AppError> {
        if self.reserve_fails {
            return Err(AppError::Internal);
        }
        let reservation = GeneratedReservation {
            asset: identity.clone(),
            document_id,
        };
        lock(&self.rows).insert(
            identity.asset_id(),
            Row {
                reservation: reservation.clone(),
                content: content.to_vec(),
                state: State::Reserved,
                created: self.now,
                referenced: false,
            },
        );
        if self.reserve_fails_after_commit {
            return Err(AppError::Internal);
        }
        Ok(reservation)
    }

    async fn find_reservation(
        &self,
        asset_id: Uuid,
    ) -> Result<Option<GeneratedReservation>, AppError> {
        if self.find_fails {
            return Err(AppError::Internal);
        }
        let rows = lock(&self.rows);
        Ok(rows.get(&asset_id).map(|row| {
            if self.foreign_find {
                GeneratedReservation {
                    asset: row.reservation.asset.clone(),
                    document_id: Uuid::new_v4(),
                }
            } else {
                row.reservation.clone()
            }
        }))
    }

    async fn materialize_reserved(
        &self,
        asset_id: Uuid,
        writer: &dyn GeneratedAssetWriter,
    ) -> Result<MaterializationOutcome, AppError> {
        if self.materialize_fails {
            return Err(AppError::Internal);
        }
        let row = lock(&self.rows).get(&asset_id).cloned();
        let Some(row) = row else {
            return Ok(MaterializationOutcome::Ineligible);
        };
        match row.state {
            State::Retired => return Ok(MaterializationOutcome::Ineligible),
            State::Active => return Ok(MaterializationOutcome::Active),
            State::Reserved => {}
        }
        if row.referenced {
            return Ok(MaterializationOutcome::Ineligible);
        }
        let bytes = if self.tamper {
            b"%PDF-1.7\n% tampered\n".to_vec()
        } else {
            row.content.clone()
        };
        let mut rows = lock(&self.rows);
        let stored = rows.get_mut(&asset_id).ok_or(AppError::Internal)?;
        match writer.materialize_owned(&row.reservation.asset, &bytes) {
            Ok(_) => {
                stored.state = State::Active;
                Ok(MaterializationOutcome::Active)
            }
            Err(failure) => {
                stored.state = State::Retired;
                Ok(match failure.category {
                    PublicationFailureCategory::FileWriteFailed => {
                        MaterializationOutcome::CleanupPending
                    }
                    _ => MaterializationOutcome::Blocked,
                })
            }
        }
    }

    async fn recover_stale_reservations(
        &self,
        root_id: Uuid,
        stale_before: OffsetDateTime,
        limit: usize,
    ) -> Result<Vec<Uuid>, AppError> {
        lock(&self.recover_cutoffs).push(stale_before);
        if self.recover_fails {
            return Err(AppError::Internal);
        }
        if let Some(override_ids) = &self.recover_override {
            return Ok(override_ids.clone());
        }
        let mut rows = lock(&self.rows);
        let mut candidates: Vec<(OffsetDateTime, Uuid)> = rows
            .iter()
            .filter(|(_, row)| {
                row.reservation.asset.root_id() == root_id
                    && row.state == State::Reserved
                    && row.created <= stale_before
                    && !row.referenced
            })
            .map(|(id, row)| (row.created, *id))
            .collect();
        candidates.sort();
        candidates.truncate(limit);
        let mut recovered = Vec::new();
        for (_, asset_id) in candidates {
            if let Some(row) = rows.get_mut(&asset_id) {
                row.state = State::Retired;
            }
            recovered.push(asset_id);
        }
        Ok(recovered)
    }
}

/// File fake that records every publication and fails with a configured category.
#[derive(Debug, Default)]
struct FakeWriter {
    failure: Option<PublicationFailureCategory>,
    calls: Mutex<Vec<Uuid>>,
}

impl GeneratedAssetWriter for FakeWriter {
    fn materialize_owned(
        &self,
        identity: &AssetIdentity,
        _content: &[u8],
    ) -> Result<PublicationOutcome, PublicationFailure> {
        lock(&self.calls).push(identity.asset_id());
        match self.failure {
            Some(category) => Err(PublicationFailure { category }),
            None => Ok(PublicationOutcome::Created),
        }
    }
}

fn service(
    fake: &Arc<Fake>,
    writer: &Arc<FakeWriter>,
    enabled: bool,
) -> Result<StandaloneAssetService, Box<dyn Error>> {
    Ok(StandaloneAssetService::new(
        Arc::clone(fake) as Arc<dyn StandaloneAssetRepository>,
        Arc::clone(writer) as Arc<dyn GeneratedAssetWriter>,
        fake.root,
        Arc::new(FixedClock::new(now()?)),
        enabled,
    ))
}

fn fixture(
    configure: impl FnOnce(&mut Fake),
) -> Result<(Arc<Fake>, Arc<FakeWriter>), Box<dyn Error>> {
    let mut fake = Fake::new(Uuid::new_v4(), now()?);
    configure(&mut fake);
    Ok((Arc::new(fake), Arc::new(FakeWriter::default())))
}

#[tokio::test]
async fn reserve_stores_the_exact_bytes_and_proves_the_reservation() -> TestResult {
    let (fake, writer) = fixture(|_| {})?;
    let svc = service(&fake, &writer, true)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    assert_eq!(reservation.asset.root_id(), fake.root);
    assert_eq!(reservation.asset.size_bytes(), PDF.len() as u64);
    assert_eq!(reservation.asset.sha256(), sha256_hex(PDF));
    assert!(lock(&fake.rows).contains_key(&reservation.asset.asset_id()));
    assert!(lock(&writer.calls).is_empty(), "reserving never publishes");
    Ok(())
}

#[tokio::test]
async fn reserve_checks_the_bytes_before_the_name() -> TestResult {
    let (fake, writer) = fixture(|_| {})?;
    let svc = service(&fake, &writer, true)?;
    assert_eq!(
        svc.reserve_pdf(b"not a pdf", "bad/name.txt").await,
        Err(StandaloneError::InvalidContent)
    );
    for name in [
        "",
        "resume.txt",
        " resume.pdf",
        "folder/resume.pdf",
        "folder\\resume.pdf",
        "line\nbreak.pdf",
    ] {
        assert_eq!(
            svc.reserve_pdf(PDF, name).await,
            Err(StandaloneError::InvalidFileName),
            "{name:?}"
        );
    }
    assert!(
        lock(&fake.rows).is_empty(),
        "a refused input stores nothing"
    );
    Ok(())
}

#[tokio::test]
async fn a_disabled_lifecycle_refuses_every_standalone_call() -> TestResult {
    let (fake, writer) = fixture(|_| {})?;
    let svc = service(&fake, &writer, false)?;
    let refusal = GeneratedAssetUnavailable(NOT_ENABLED);
    assert_eq!(
        svc.reserve_pdf(PDF, "synthetic-resume.pdf").await,
        Err(StandaloneError::Unavailable(refusal.clone()))
    );
    let reservation = GeneratedReservation {
        asset: AssetIdentity::new(
            Uuid::new_v4(),
            fake.root,
            &sha256_hex(PDF),
            PDF.len() as u64,
        )?,
        document_id: Uuid::new_v4(),
    };
    assert_eq!(svc.materialize(&reservation).await, Err(refusal.clone()));
    assert_eq!(svc.recover_abandoned().await, Err(refusal));
    assert!(lock(&fake.rows).is_empty());
    Ok(())
}

#[tokio::test]
async fn a_committed_reservation_is_reconciled_after_an_error() -> TestResult {
    let (fake, writer) = fixture(|fake| fake.reserve_fails_after_commit = true)?;
    let svc = service(&fake, &writer, true)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    assert_eq!(
        lock(&fake.rows)
            .get(&reservation.asset.asset_id())
            .map(|row| row.reservation.clone()),
        Some(reservation)
    );
    Ok(())
}

#[tokio::test]
async fn a_reconciled_reservation_must_be_the_dispatched_one() -> TestResult {
    let (fake, writer) = fixture(|fake| {
        fake.reserve_fails_after_commit = true;
        fake.foreign_find = true;
    })?;
    let svc = service(&fake, &writer, true)?;
    assert_eq!(
        svc.reserve_pdf(PDF, "synthetic-resume.pdf").await,
        Err(StandaloneError::Unavailable(GeneratedAssetUnavailable(
            UNPROVEN
        )))
    );
    Ok(())
}

#[tokio::test]
async fn reserve_failures_without_a_commit_are_unproven_or_unavailable() -> TestResult {
    let (fake, writer) = fixture(|fake| fake.reserve_fails = true)?;
    let svc = service(&fake, &writer, true)?;
    assert_eq!(
        svc.reserve_pdf(PDF, "synthetic-resume.pdf").await,
        Err(StandaloneError::Unavailable(GeneratedAssetUnavailable(
            UNPROVEN
        )))
    );
    let (fake, writer) = fixture(|fake| {
        fake.reserve_fails = true;
        fake.find_fails = true;
    })?;
    let svc = service(&fake, &writer, true)?;
    assert_eq!(
        svc.reserve_pdf(PDF, "synthetic-resume.pdf").await,
        Err(StandaloneError::Unavailable(GeneratedAssetUnavailable(
            RESERVATION_UNAVAILABLE
        )))
    );
    Ok(())
}

#[tokio::test]
async fn materialize_publishes_once_and_then_reports_active() -> TestResult {
    let (fake, writer) = fixture(|_| {})?;
    let svc = service(&fake, &writer, true)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let first = svc.materialize(&reservation).await?;
    assert_eq!(first.outcome, MaterializationOutcome::Active);
    assert_eq!(first.reservation, reservation);
    let second = svc.materialize(&reservation).await?;
    assert_eq!(second.outcome, MaterializationOutcome::Active);
    assert_eq!(
        lock(&writer.calls).len(),
        1,
        "an active asset is not published again"
    );
    Ok(())
}

#[tokio::test]
async fn create_reserves_and_materializes_in_one_call() -> TestResult {
    let (fake, writer) = fixture(|_| {})?;
    let svc = service(&fake, &writer, true)?;
    let result = svc.create(PDF, "synthetic-resume.pdf").await?;
    assert_eq!(result.outcome, MaterializationOutcome::Active);
    assert_eq!(
        lock(&writer.calls).as_slice(),
        [result.reservation.asset.asset_id()]
    );
    Ok(())
}

#[tokio::test]
async fn materialize_refuses_another_root_and_an_unproven_reservation() -> TestResult {
    let (fake, writer) = fixture(|_| {})?;
    let svc = service(&fake, &writer, true)?;
    let foreign = GeneratedReservation {
        asset: AssetIdentity::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            &sha256_hex(PDF),
            PDF.len() as u64,
        )?,
        document_id: Uuid::new_v4(),
    };
    assert_eq!(
        svc.materialize(&foreign).await,
        Err(GeneratedAssetUnavailable(UNPROVEN))
    );

    let (fake, writer) = fixture(|fake| fake.foreign_find = true)?;
    let svc = service(&fake, &writer, true)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    assert_eq!(
        svc.materialize(&reservation).await,
        Err(GeneratedAssetUnavailable(UNPROVEN))
    );
    assert!(lock(&writer.calls).is_empty());
    Ok(())
}

#[tokio::test]
async fn a_file_write_failure_schedules_cleanup_and_retires_the_asset() -> TestResult {
    let (fake, _) = fixture(|_| {})?;
    let writer = Arc::new(FakeWriter {
        failure: Some(PublicationFailureCategory::FileWriteFailed),
        ..FakeWriter::default()
    });
    let svc = service(&fake, &writer, true)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::CleanupPending
    );
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::Ineligible,
        "a retired asset is not materialized again"
    );
    assert_eq!(lock(&writer.calls).len(), 1);
    Ok(())
}

#[tokio::test]
async fn an_ownership_failure_blocks_the_asset() -> TestResult {
    let (fake, _) = fixture(|_| {})?;
    let writer = Arc::new(FakeWriter {
        failure: Some(PublicationFailureCategory::RetiredAsset),
        ..FakeWriter::default()
    });
    let svc = service(&fake, &writer, true)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::Blocked
    );
    Ok(())
}

#[tokio::test]
async fn tampered_bytes_never_reach_the_file_port() -> TestResult {
    let (fake, writer) = fixture(|fake| fake.tamper = true)?;
    let svc = service(&fake, &writer, true)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::Blocked
    );
    assert!(
        lock(&writer.calls).is_empty(),
        "the checked writer refuses bytes that differ from the reserved identity"
    );
    Ok(())
}

#[tokio::test]
async fn a_repository_failure_during_materialization_is_a_safe_refusal() -> TestResult {
    let (fake, writer) = fixture(|fake| fake.materialize_fails = true)?;
    let svc = service(&fake, &writer, true)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    assert_eq!(
        svc.materialize(&reservation).await,
        Err(GeneratedAssetUnavailable(MATERIALIZATION_UNAVAILABLE))
    );
    Ok(())
}

#[tokio::test]
async fn recovery_retires_only_stale_unreferenced_reservations_of_this_root() -> TestResult {
    let (fake, writer) = fixture(|_| {})?;
    let stale = now()? - Duration::minutes(6);
    let fresh = now()? - Duration::minutes(1);
    let stale_free = fake.seed(fake.root, stale, false)?;
    let fresh_free = fake.seed(fake.root, fresh, false)?;
    let stale_referenced = fake.seed(fake.root, stale, true)?;
    let other_root = fake.seed(Uuid::new_v4(), stale, false)?;
    let svc = service(&fake, &writer, true)?;

    assert_eq!(svc.recover_abandoned().await?, vec![stale_free]);
    assert_eq!(lock(&fake.rows)[&fresh_free].state, State::Reserved);
    assert_eq!(lock(&fake.rows)[&stale_referenced].state, State::Reserved);
    assert_eq!(lock(&fake.rows)[&other_root].state, State::Reserved);
    assert_eq!(
        lock(&fake.recover_cutoffs).as_slice(),
        [now()? - CLAIM_LEASE]
    );
    Ok(())
}

#[tokio::test]
async fn recovery_is_bounded_to_one_batch() -> TestResult {
    let (fake, writer) = fixture(|_| {})?;
    let stale = now()? - Duration::minutes(6);
    for _ in 0..RETRY_BATCH_SIZE + 5 {
        fake.seed(fake.root, stale, false)?;
    }
    let svc = service(&fake, &writer, true)?;
    assert_eq!(svc.recover_abandoned().await?.len(), RETRY_BATCH_SIZE);
    Ok(())
}

#[tokio::test]
async fn recovery_refuses_duplicate_and_oversized_results() -> TestResult {
    let id = Uuid::new_v4();
    let (fake, writer) = fixture(|fake| fake.recover_override = Some(vec![id, id]))?;
    let svc = service(&fake, &writer, true)?;
    assert_eq!(
        svc.recover_abandoned().await,
        Err(GeneratedAssetUnavailable(RECOVERY_INVALID))
    );

    let oversized: Vec<Uuid> = (0..=RETRY_BATCH_SIZE).map(|_| Uuid::new_v4()).collect();
    let (fake, writer) = fixture(|fake| fake.recover_override = Some(oversized))?;
    let svc = service(&fake, &writer, true)?;
    assert_eq!(
        svc.recover_abandoned().await,
        Err(GeneratedAssetUnavailable(RECOVERY_INVALID))
    );
    Ok(())
}

#[tokio::test]
async fn recovery_repository_failure_is_a_safe_refusal() -> TestResult {
    let (fake, writer) = fixture(|fake| fake.recover_fails = true)?;
    let svc = service(&fake, &writer, true)?;
    assert_eq!(
        svc.recover_abandoned().await,
        Err(GeneratedAssetUnavailable(RECOVERY_UNAVAILABLE))
    );
    Ok(())
}
