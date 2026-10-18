//! A classified claim whose lease runs out while its writer waits for the SQLite write lock: the
//! clock is read after the lock is granted, so the materialization is refused before any file is
//! written or any row changes. A control with a live lease shows that the wait alone refuses
//! nothing. The wait is a raw connection that holds `BEGIN IMMEDIATE`; the test never sleeps to
//! synchronize. All data is synthetic.

mod sqlite_generated_support;
mod sqlite_scenarios_support;
mod sqlite_support;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use jobniac_core::clock::{Clock, truncate_to_micros};
use jobniac_core::generated_assets::service::{GeneratedAssetRepository, GeneratedAssetService};
use jobniac_core::generated_assets::{
    AssetIdentity, GENERATION_LEASE, GeneratedAssetWriter, GenerationDescriptor, GenerationPurpose,
    MaterializationOutcome, PublicationFailure, PublicationOutcome,
};
use jobniac_infra::sqlite::generated::repository::SqliteGeneratedAssetRepository;
use rusqlite::params;
use sqlite_scenarios_support::{WriteHold, assert_still_waiting};
use sqlite_support::{SqliteFixture, TestResult};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// A synthetic PDF with a valid header.
const PDF: &[u8] = b"%PDF-1.7\n% synthetic classified resume\n";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A clock that moves only when a test advances it.
#[derive(Debug)]
struct TestClock(Mutex<OffsetDateTime>);

impl TestClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(truncate_to_micros(
            OffsetDateTime::from_unix_timestamp(1_700_000_000)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH),
        ))))
    }

    fn advance(&self, by: Duration) {
        *lock(&self.0) += by;
    }
}

impl Clock for TestClock {
    fn now(&self) -> OffsetDateTime {
        *lock(&self.0)
    }
}

/// File fake that records every publication.
#[derive(Debug, Default)]
struct Writer {
    calls: Mutex<Vec<Uuid>>,
}

impl Writer {
    fn calls(&self) -> Vec<Uuid> {
        lock(&self.calls).clone()
    }
}

impl GeneratedAssetWriter for Writer {
    fn materialize_owned(
        &self,
        identity: &AssetIdentity,
        _content: &[u8],
    ) -> Result<PublicationOutcome, PublicationFailure> {
        lock(&self.calls).push(identity.asset_id());
        Ok(PublicationOutcome::Created)
    }
}

struct Env {
    sqlite: SqliteFixture,
    clock: Arc<TestClock>,
    writer: Arc<Writer>,
    repository: Arc<SqliteGeneratedAssetRepository>,
    service: GeneratedAssetService,
}

fn environment() -> Result<Env, Box<dyn std::error::Error + Send + Sync>> {
    let sqlite = SqliteFixture::start();
    let clock = TestClock::new();
    let writer = Arc::new(Writer::default());
    let root = Uuid::new_v4();
    sqlite.connection().execute(
        "INSERT INTO jobniac_generated_asset_roots (id, created_at) VALUES (?1, ?2)",
        params![
            root.to_string(),
            sqlite_generated_support::stored(clock.now())
        ],
    )?;
    let repository = Arc::new(SqliteGeneratedAssetRepository::new(
        sqlite.unit_of_work(),
        clock.clone(),
    ));
    let service = GeneratedAssetService::new(
        repository.clone() as Arc<dyn GeneratedAssetRepository>,
        writer.clone() as Arc<dyn GeneratedAssetWriter>,
        root,
        clock.clone(),
        true,
    );
    Ok(Env {
        sqlite,
        clock,
        writer,
        repository,
        service,
    })
}

fn descriptor() -> Result<GenerationDescriptor, Box<dyn std::error::Error + Send + Sync>> {
    Ok(GenerationDescriptor::new(
        Uuid::new_v4(),
        GenerationPurpose::ProfileResume,
        Some(Uuid::new_v4()),
        None,
        None,
        Some("master_resume".to_owned()),
    )?)
}

/// The lifecycle of the asset and the state of its intent.
fn stored_state(
    env: &Env,
    asset: Uuid,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    Ok(env.sqlite.connection().query_row(
        "SELECT asset.lifecycle, intent.state FROM jobniac_generated_assets asset \
         JOIN jobniac_generation_intents intent ON intent.asset_id = asset.asset_id \
         WHERE asset.asset_id = ?1",
        [asset.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lease_that_expires_while_waiting_for_the_locks_refuses_before_any_write() -> TestResult {
    let env = environment()?;
    let claim = env
        .service
        .reserve_classified_pdf(PDF, "waiting.pdf", &descriptor()?)
        .await?;
    let before = stored_state(&env, claim.asset_id())?;
    assert_eq!(before, ("RESERVED".to_owned(), "PENDING".to_owned()));

    // Another writer holds the write lock, so the materialization waits to begin its transaction.
    let hold = WriteHold::begin(env.sqlite.path());
    let waiting = tokio::spawn({
        let repository = env.repository.clone();
        let writer = env.writer.clone();
        let claim = claim.clone();
        async move {
            repository
                .materialize_classified(&claim, writer.as_ref())
                .await
        }
    });
    assert_still_waiting(&waiting, "the materialization").await;

    // The lease runs out during the wait; the instant must be read after the lock is granted.
    env.clock.advance(GENERATION_LEASE + Duration::seconds(1));
    hold.commit();
    assert_eq!(waiting.await??, MaterializationOutcome::Ineligible);
    assert!(
        env.writer.calls().is_empty(),
        "no file is written for an expired lease"
    );
    assert_eq!(stored_state(&env, claim.asset_id())?, before);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_lease_that_waited_for_the_lock_still_materializes() -> TestResult {
    let env = environment()?;
    let claim = env
        .service
        .reserve_classified_pdf(PDF, "control.pdf", &descriptor()?)
        .await?;
    let hold = WriteHold::begin(env.sqlite.path());
    let waiting = tokio::spawn({
        let repository = env.repository.clone();
        let writer = env.writer.clone();
        let claim = claim.clone();
        async move {
            repository
                .materialize_classified(&claim, writer.as_ref())
                .await
        }
    });
    assert_still_waiting(&waiting, "the materialization").await;
    hold.commit();
    assert_eq!(waiting.await??, MaterializationOutcome::Active);
    assert_eq!(env.writer.calls(), vec![claim.asset_id()]);
    assert_eq!(
        stored_state(&env, claim.asset_id())?,
        ("ACTIVE".to_owned(), "PENDING".to_owned())
    );
    Ok(())
}
