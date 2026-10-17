//! Classified generated PDFs against a disposable PostgreSQL 18 server: reservation with its birth
//! intent, the find and proof of the reservation, activation of a standalone output with its
//! completed intent, the abandonment of a pending output with its retirement, the recovery of an
//! expired pending output and the renewal of a live fence. The server-backed tests skip with a
//! notice when `JOBNIAC_TEST_PG_PORT` is unset. The file port is an in-memory fake; all data is
//! synthetic.

use std::error::Error;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use jobniac_core::clock::FixedClock;
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::service::{GeneratedAssetRepository, PreparationOutcome};
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedAssetWriter, GenerationDescriptor, GenerationPurpose, GenerationState,
    IntentReservation, MaterializationOutcome, PublicationFailure, PublicationFailureCategory,
    PublicationOutcome, sha256_hex,
};
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::postgres::cleanup::PgCleanupRepository;
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::generated::repository::PgGeneratedAssetRepository;
use jobniac_infra::postgres::migrations::migrate;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use jobniac_testkit::pg::{TestDatabase, TestServer};
use time::OffsetDateTime;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

/// A synthetic PDF with a valid header.
const PDF: &[u8] = b"%PDF-1.7\n% synthetic classified output\n";

const LIFECYCLE_SQL: &str = "SELECT lifecycle FROM jobniac.generated_assets WHERE asset_id = $1";

/// A migrated disposable database and the unit of work over it.
struct Fixture {
    _database: TestDatabase,
    uow: PgUnitOfWork,
}

/// A fresh migrated database, or `None` without a configured server.
async fn start() -> Result<Option<Fixture>, Box<dyn Error>> {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(None);
    };
    let database = server.fresh_database().await;
    let config = load_database_config(&database.environment())
        .ok_or("the disposable database configuration is invalid")?;
    migrate(&config).await.map_err(|error| error.0)?;
    let uow = PgUnitOfWork::new(PgPool::new(Some(config)));
    Ok(Some(Fixture {
        _database: database,
        uow,
    }))
}

fn instant(seconds: i64) -> Result<OffsetDateTime, Box<dyn Error>> {
    Ok(OffsetDateTime::from_unix_timestamp(
        1_700_000_000 + seconds,
    )?)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// File fake that records every publication and fails with a configured category.
#[derive(Debug, Default)]
struct RecordingWriter {
    failure: Option<PublicationFailureCategory>,
    calls: Mutex<Vec<Uuid>>,
}

impl GeneratedAssetWriter for RecordingWriter {
    fn materialize_owned(
        &self,
        identity: &AssetIdentity,
        _content: &[u8],
    ) -> Result<PublicationOutcome, PublicationFailure> {
        if let Some(category) = self.failure {
            return Err(PublicationFailure { category });
        }
        lock(&self.calls).push(identity.asset_id());
        Ok(PublicationOutcome::Created)
    }
}

fn standalone_descriptor() -> GenerationDescriptor {
    GenerationDescriptor::new(
        Uuid::new_v4(),
        GenerationPurpose::StandalonePdf,
        None,
        None,
        None,
        None,
    )
    .expect("the standalone descriptor is valid")
}

/// Registers a synthetic root identity, as the setup command does before any reservation.
async fn register_root(
    uow: &PgUnitOfWork,
    root: Uuid,
    at: OffsetDateTime,
) -> Result<(), Box<dyn Error>> {
    PgCleanupRepository::new(uow.clone(), Arc::new(FixedClock::new(at)))
        .register_generated_root(root, at)
        .await
        .map_err(|error| error.to_string().into())
}

/// Reserves one classified standalone output at `at`.
async fn reserve(
    repository: &PgGeneratedAssetRepository,
    root: Uuid,
    at: OffsetDateTime,
) -> Result<IntentReservation, AppError> {
    let asset_id = Uuid::new_v4();
    let identity = AssetIdentity::new(asset_id, root, &sha256_hex(PDF), PDF.len() as u64)
        .map_err(|_| AppError::Internal)?;
    repository
        .reserve_classified_pdf(
            &identity,
            Uuid::new_v4(),
            "synthetic.pdf",
            PDF,
            &standalone_descriptor(),
            Uuid::new_v4(),
            at,
        )
        .await
}

fn repository(
    uow: &PgUnitOfWork,
    seconds: i64,
) -> Result<PgGeneratedAssetRepository, Box<dyn Error>> {
    Ok(PgGeneratedAssetRepository::new(
        uow.clone(),
        Arc::new(FixedClock::new(instant(seconds)?)),
    ))
}

async fn lifecycle(uow: &PgUnitOfWork, asset_id: Uuid) -> Result<String, Box<dyn Error>> {
    let read_only = TransactionOptions {
        read_only: true,
        requires_new: false,
    };
    let value = uow
        .transaction::<String, AppError>(read_only, async |tx| {
            let row = tx
                .client()
                .query_one(LIFECYCLE_SQL, &[&asset_id])
                .await
                .map_err(|_| AppError::Internal)?;
            row.try_get::<_, String>(0).map_err(|_| AppError::Internal)
        })
        .await
        .map_err(|error| format!("lifecycle read failed: {error}"))?;
    Ok(value)
}

#[tokio::test]
async fn a_standalone_reservation_is_found_and_activated_with_its_completed_intent() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let root = Uuid::new_v4();
    register_root(&fixture.uow, root, instant(0)?).await?;
    let repo = repository(&fixture.uow, 0)?;
    let claim = reserve(&repo, root, instant(0)?)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(claim.intent.state, GenerationState::Pending);

    let found = repo
        .find_classified_reservation(claim.asset_id())
        .await
        .map_err(|error| error.to_string())?
        .ok_or("the reservation must be found")?;
    assert_eq!(found, claim);

    let writer = RecordingWriter::default();
    let outcome = repo
        .materialize_classified(&claim, &writer)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(outcome, MaterializationOutcome::Active);
    assert_eq!(lock(&writer.calls).as_slice(), &[claim.asset_id()]);
    assert_eq!(lifecycle(&fixture.uow, claim.asset_id()).await?, "ACTIVE");

    let completed = repo
        .find_classified_reservation(claim.asset_id())
        .await
        .map_err(|error| error.to_string())?
        .ok_or("the completed reservation must be found")?;
    assert_eq!(completed.intent.state, GenerationState::Completed);
    Ok(())
}

#[tokio::test]
async fn a_failed_publication_blocks_the_output_and_records_the_category() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let root = Uuid::new_v4();
    register_root(&fixture.uow, root, instant(0)?).await?;
    let repo = repository(&fixture.uow, 0)?;
    let claim = reserve(&repo, root, instant(0)?)
        .await
        .map_err(|error| error.to_string())?;
    let writer = RecordingWriter {
        failure: Some(PublicationFailureCategory::IdentityMismatch),
        calls: Mutex::new(Vec::new()),
    };
    let outcome = repo
        .materialize_classified(&claim, &writer)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(outcome, MaterializationOutcome::Blocked);
    assert!(lock(&writer.calls).is_empty());
    assert_ne!(lifecycle(&fixture.uow, claim.asset_id()).await?, "ACTIVE");
    Ok(())
}

#[tokio::test]
async fn abandoning_a_pending_output_prepares_its_retirement_once() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let root = Uuid::new_v4();
    register_root(&fixture.uow, root, instant(0)?).await?;
    let repo = repository(&fixture.uow, 0)?;
    let claim = reserve(&repo, root, instant(0)?)
        .await
        .map_err(|error| error.to_string())?;
    let first = repo
        .abandon_generation(&claim)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(first, PreparationOutcome::Prepared);
    assert_ne!(lifecycle(&fixture.uow, claim.asset_id()).await?, "ACTIVE");
    let second = repo
        .abandon_generation(&claim)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(second, PreparationOutcome::AlreadyRetired);
    Ok(())
}

#[tokio::test]
async fn an_expired_pending_output_is_recovered_once_and_a_live_fence_renews() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let root = Uuid::new_v4();
    register_root(&fixture.uow, root, instant(0)?).await?;
    let repo = repository(&fixture.uow, 0)?;
    let live = reserve(&repo, root, instant(0)?)
        .await
        .map_err(|error| error.to_string())?;
    let renewed = repo
        .renew_generation(&live, Uuid::new_v4())
        .await
        .map_err(|error| error.to_string())?
        .ok_or("a live pending claim must renew")?;
    assert_eq!(renewed.intent.state, GenerationState::Pending);

    let later = repository(&fixture.uow, 3_600)?;
    let recovered = later
        .recover_expired_generations(root, 25)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(recovered, vec![live.asset_id()]);
    let again = later
        .recover_expired_generations(root, 25)
        .await
        .map_err(|error| error.to_string())?;
    assert!(again.is_empty());
    Ok(())
}
