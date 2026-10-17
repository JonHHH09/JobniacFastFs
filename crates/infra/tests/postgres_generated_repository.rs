//! Standalone generated PDFs against a disposable PostgreSQL 18 server: reservation, reconciliation,
//! activation, retirement with cleanup rows, abandoned-reservation recovery, the busy-lock skip and
//! the serialization of concurrent materializations. The server-backed tests skip with a notice
//! when `JOBNIAC_TEST_PG_PORT` is unset. The file port is an in-memory fake; all data is synthetic.

use std::error::Error;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use jobniac_core::clock::FixedClock;
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::standalone::{
    StandaloneAssetRepository, StandaloneAssetService,
};
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedAssetWriter, MaterializationOutcome, PublicationFailure,
    PublicationFailureCategory, PublicationOutcome,
};
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::generated::repository::PgGeneratedAssetRepository;
use jobniac_infra::postgres::migrations::migrate;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use jobniac_testkit::pg::{TestDatabase, TestServer};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

/// A synthetic PDF with a valid header.
const PDF: &[u8] = b"%PDF-1.7\n% synthetic standalone resume\n";

const LIFECYCLE_SQL: &str = "SELECT lifecycle FROM jobniac.generated_assets WHERE asset_id = $1";
const MATERIALIZED_SQL: &str =
    "SELECT (materialized_at IS NOT NULL)::text FROM jobniac.generated_assets WHERE asset_id = $1";
const CLEANUP_SQL: &str = "SELECT status || ':' || COALESCE(failure_category, '-') \
    FROM jobniac.generated_asset_cleanups WHERE asset_id = $1";

/// A migrated disposable database and the unit of work over it.
struct Fixture {
    _database: TestDatabase,
    uow: PgUnitOfWork,
}

/// A fresh migrated database, or `None` without a configured server.
async fn start() -> Result<Option<Fixture>, Box<dyn Error>> {
    let Some(server) = TestServer::from_env() else {
        assert!(
            std::env::var_os("CI").is_none(),
            "JOBNIAC_TEST_PG_PORT must be set in CI so that the generated PostgreSQL tests run"
        );
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

fn now() -> Result<OffsetDateTime, Box<dyn Error>> {
    Ok(OffsetDateTime::from_unix_timestamp(1_700_000_000)?)
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
        lock(&self.calls).push(identity.asset_id());
        match self.failure {
            Some(category) => Err(PublicationFailure { category }),
            None => Ok(PublicationOutcome::Created),
        }
    }
}

/// Registers a generated root and returns its identifier.
async fn register_root(uow: &PgUnitOfWork, now: OffsetDateTime) -> Result<Uuid, AppError> {
    let root = Uuid::new_v4();
    uow.transaction::<(), AppError>(TransactionOptions::default(), async |tx| {
        tx.client()
            .execute(
                "INSERT INTO jobniac.generated_asset_roots (id, created_at) VALUES ($1, $2)",
                &[&root, &now],
            )
            .await
            .map_err(|_| AppError::Internal)?;
        Ok(())
    })
    .await?;
    Ok(root)
}

/// The service over the PostgreSQL repository, the recording file fake and a fixed clock.
fn service(
    uow: &PgUnitOfWork,
    writer: &Arc<RecordingWriter>,
    root: Uuid,
    now: OffsetDateTime,
) -> Result<StandaloneAssetService, Box<dyn Error>> {
    let clock = Arc::new(FixedClock::new(now));
    let repository = PgGeneratedAssetRepository::new(uow.clone(), clock.clone());
    Ok(StandaloneAssetService::new(
        Arc::new(repository) as Arc<dyn StandaloneAssetRepository>,
        Arc::clone(writer) as Arc<dyn GeneratedAssetWriter>,
        root,
        clock,
        true,
    ))
}

/// One text value of one asset, read in its own transaction.
async fn text(uow: &PgUnitOfWork, sql: &'static str, asset: Uuid) -> Result<String, AppError> {
    uow.transaction(TransactionOptions::default(), async |tx| {
        let row = tx
            .client()
            .query_one(sql, &[&asset])
            .await
            .map_err(|_| AppError::Internal)?;
        row.try_get::<_, String>(0).map_err(|_| AppError::Internal)
    })
    .await
}

/// Moves the creation instant of an asset into the past, to make it stale. The guard trigger keeps
/// `created_at` immutable, so the test disables user triggers for this one transaction only.
async fn backdate(
    uow: &PgUnitOfWork,
    asset: Uuid,
    created: OffsetDateTime,
) -> Result<(), AppError> {
    uow.transaction::<(), AppError>(TransactionOptions::default(), async |tx| {
        tx.client()
            .batch_execute("SET LOCAL session_replication_role = replica")
            .await
            .map_err(|_| AppError::Internal)?;
        tx.client()
            .execute(
                "UPDATE jobniac.generated_assets SET created_at = $2 WHERE asset_id = $1",
                &[&asset, &created],
            )
            .await
            .map_err(|_| AppError::Internal)?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn reserve_round_trips_through_the_repository() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let now = now()?;
    let root = register_root(&fixture.uow, now).await?;
    let writer = Arc::new(RecordingWriter::default());
    let svc = service(&fixture.uow, &writer, root, now)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let repository =
        PgGeneratedAssetRepository::new(fixture.uow.clone(), Arc::new(FixedClock::new(now)));
    assert_eq!(
        repository
            .find_reservation(reservation.asset.asset_id())
            .await?,
        Some(reservation.clone())
    );
    assert_eq!(
        text(&fixture.uow, LIFECYCLE_SQL, reservation.asset.asset_id()).await?,
        "RESERVED"
    );
    assert!(lock(&writer.calls).is_empty(), "reserving never publishes");
    Ok(())
}

#[tokio::test]
async fn materialize_activates_the_asset_once() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let now = now()?;
    let root = register_root(&fixture.uow, now).await?;
    let writer = Arc::new(RecordingWriter::default());
    let svc = service(&fixture.uow, &writer, root, now)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let asset = reservation.asset.asset_id();
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::Active
    );
    assert_eq!(text(&fixture.uow, LIFECYCLE_SQL, asset).await?, "ACTIVE");
    assert_eq!(text(&fixture.uow, MATERIALIZED_SQL, asset).await?, "true");
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::Active
    );
    assert_eq!(lock(&writer.calls).len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_file_write_failure_retires_the_asset_with_a_pending_cleanup() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let now = now()?;
    let root = register_root(&fixture.uow, now).await?;
    let writer = Arc::new(RecordingWriter {
        failure: Some(PublicationFailureCategory::FileWriteFailed),
        ..RecordingWriter::default()
    });
    let svc = service(&fixture.uow, &writer, root, now)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let asset = reservation.asset.asset_id();
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::CleanupPending
    );
    assert_eq!(text(&fixture.uow, LIFECYCLE_SQL, asset).await?, "RETIRING");
    assert_eq!(text(&fixture.uow, CLEANUP_SQL, asset).await?, "PENDING:-");
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::Ineligible
    );
    Ok(())
}

#[tokio::test]
async fn an_identity_failure_blocks_the_asset_with_its_category() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let now = now()?;
    let root = register_root(&fixture.uow, now).await?;
    let writer = Arc::new(RecordingWriter {
        failure: Some(PublicationFailureCategory::IdentityMismatch),
        ..RecordingWriter::default()
    });
    let svc = service(&fixture.uow, &writer, root, now)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let asset = reservation.asset.asset_id();
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::Blocked
    );
    assert_eq!(
        text(&fixture.uow, CLEANUP_SQL, asset).await?,
        "BLOCKED:IDENTITY_MISMATCH"
    );
    Ok(())
}

#[tokio::test]
async fn concurrent_materializations_publish_once() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let now = now()?;
    let root = register_root(&fixture.uow, now).await?;
    let writer = Arc::new(RecordingWriter::default());
    let svc = service(&fixture.uow, &writer, root, now)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let (first, second) =
        tokio::join!(svc.materialize(&reservation), svc.materialize(&reservation));
    assert_eq!(first?.outcome, MaterializationOutcome::Active);
    assert_eq!(second?.outcome, MaterializationOutcome::Active);
    assert_eq!(
        lock(&writer.calls).len(),
        1,
        "the second caller waits on the asset lock and sees the active row"
    );
    Ok(())
}

#[tokio::test]
async fn recovery_retires_only_stale_reservations() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let now = now()?;
    let root = register_root(&fixture.uow, now).await?;
    let writer = Arc::new(RecordingWriter::default());
    let svc = service(&fixture.uow, &writer, root, now)?;
    let stale = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let fresh = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let active = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    backdate(
        &fixture.uow,
        stale.asset.asset_id(),
        now - Duration::minutes(10),
    )
    .await?;
    backdate(
        &fixture.uow,
        active.asset.asset_id(),
        now - Duration::minutes(10),
    )
    .await?;
    svc.materialize(&active).await?;

    let recovered = svc.recover_abandoned().await?;
    assert_eq!(recovered, vec![stale.asset.asset_id()]);
    assert_eq!(
        text(&fixture.uow, LIFECYCLE_SQL, stale.asset.asset_id()).await?,
        "RETIRING"
    );
    assert_eq!(
        text(&fixture.uow, CLEANUP_SQL, stale.asset.asset_id()).await?,
        "PENDING:-"
    );
    assert_eq!(
        text(&fixture.uow, LIFECYCLE_SQL, fresh.asset.asset_id()).await?,
        "RESERVED"
    );
    assert_eq!(
        text(&fixture.uow, LIFECYCLE_SQL, active.asset.asset_id()).await?,
        "ACTIVE"
    );
    Ok(())
}

#[tokio::test]
async fn recovery_skips_a_stale_reservation_whose_lock_is_busy() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let now = now()?;
    let root = register_root(&fixture.uow, now).await?;
    let writer = Arc::new(RecordingWriter::default());
    let svc = service(&fixture.uow, &writer, root, now)?;
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let asset = reservation.asset.asset_id();
    backdate(&fixture.uow, asset, now - Duration::minutes(10)).await?;

    let holder = fixture.uow.pool().acquire().await?;
    holder
        .client()
        .execute(
            "SELECT pg_advisory_lock(jobniac.cleanup_asset_lock_key($1))",
            &[&asset],
        )
        .await?;
    let busy = svc.recover_abandoned().await?;
    assert!(busy.is_empty(), "a try-lock never waits for another holder");
    holder
        .client()
        .execute(
            "SELECT pg_advisory_unlock(jobniac.cleanup_asset_lock_key($1))",
            &[&asset],
        )
        .await?;
    drop(holder);

    assert_eq!(svc.recover_abandoned().await?, vec![asset]);
    Ok(())
}
