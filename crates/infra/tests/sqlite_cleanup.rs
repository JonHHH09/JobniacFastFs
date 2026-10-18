//! Owned-asset cleanup ledger against a migrated SQLite file: durable retirement, leased claims,
//! fenced execution with retryable and blocked failures, lease expiry, retention pruning, the queue
//! snapshot and the registry reads. The file layer is replaced by scripted removers. All data is
//! synthetic.

mod sqlite_generated_support;
mod sqlite_support;

use std::error::Error;
use std::sync::{Arc, Mutex};

use jobniac_core::cleanup::{
    CleanupExecutor, CleanupHealthRepository, CleanupOutcome, CleanupRepository,
    GenerationRootRepository, OwnedAssetRemover, RemovalFailure, RemovalFailureCategory,
    RemovalOutcome, classify,
};
use jobniac_core::clock::Clock;
use jobniac_core::generated_assets::{AssetIdentity, PreparationOutcome, sha256_hex};
use jobniac_core::health::CleanupStatus;
use jobniac_infra::sqlite::cleanup::SqliteCleanupRepository;
use sqlite_generated_support::stored;
use sqlite_support::{SqliteFixture, TestResult};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Synthetic PDF bytes for the owned asset.
const PDF: &[u8] = b"%PDF-1.7\n% synthetic cleanup asset\n";

/// A clock the test moves forward explicitly.
#[derive(Debug)]
struct TestClock(Mutex<OffsetDateTime>);

impl TestClock {
    fn advance(&self, by: Duration) {
        if let Ok(mut current) = self.0.lock() {
            *current += by;
        }
    }
}

impl Clock for TestClock {
    fn now(&self) -> OffsetDateTime {
        self.0
            .lock()
            .map_or(datetime!(2026-10-15 12:00:00 UTC), |current| *current)
    }
}

/// Returns one scripted outcome for every removal.
#[derive(Debug)]
struct ScriptedRemover(Result<RemovalOutcome, RemovalFailureCategory>);

impl OwnedAssetRemover for ScriptedRemover {
    fn remove_owned(&self, _identity: &AssetIdentity) -> Result<RemovalOutcome, RemovalFailure> {
        self.0.map_err(RemovalFailure::new)
    }
}

const REMOVED: ScriptedRemover = ScriptedRemover(Ok(RemovalOutcome::Removed));

/// A migrated database, the repository and the controllable clock.
struct Fixture {
    sqlite: SqliteFixture,
    clock: Arc<TestClock>,
    repository: SqliteCleanupRepository,
}

fn start() -> Fixture {
    let sqlite = SqliteFixture::start();
    let clock = Arc::new(TestClock(Mutex::new(datetime!(2026-10-15 12:00:00 UTC))));
    let repository =
        SqliteCleanupRepository::new(sqlite.unit_of_work(), Arc::clone(&clock) as Arc<dyn Clock>);
    Fixture {
        sqlite,
        clock,
        repository,
    }
}

/// Registers a new root and seeds one RESERVED, unreferenced asset under it.
fn seed_asset(
    fixture: &Fixture,
    root: Uuid,
) -> Result<AssetIdentity, Box<dyn Error + Send + Sync>> {
    let asset_id = Uuid::new_v4();
    let identity = AssetIdentity::new(asset_id, root, &sha256_hex(PDF), u64::try_from(PDF.len())?)?;
    let now = stored(fixture.clock.now());
    let connection = fixture.sqlite.connection();
    connection.execute(
        "INSERT INTO jobniac_generated_asset_roots (id, created_at) VALUES (?1, ?2) \
         ON CONFLICT (id) DO NOTHING",
        rusqlite::params![root.to_string(), now],
    )?;
    connection.execute(
        "INSERT INTO jobniac_generated_assets (asset_id, root_id, relative_path, \
         sha256, size_bytes, document_id, lifecycle, created_at, generation_intent_required) \
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'RESERVED', ?6, 0)",
        rusqlite::params![
            asset_id.to_string(),
            root.to_string(),
            identity.relative_path(),
            identity.sha256(),
            i64::try_from(PDF.len())?,
            now
        ],
    )?;
    Ok(identity)
}

/// Reads one nullable text column of a single row addressed by a uuid parameter.
fn text(
    fixture: &Fixture,
    sql: &str,
    id: Uuid,
) -> Result<Option<String>, Box<dyn Error + Send + Sync>> {
    Ok(fixture
        .sqlite
        .connection()
        .query_row(sql, [id.to_string()], |row| row.get("value"))?)
}

fn repository(fixture: &Fixture) -> Arc<dyn CleanupRepository> {
    Arc::new(fixture.repository.clone())
}

#[tokio::test]
async fn a_prepared_asset_is_removed_once_and_its_fence_is_gone() -> TestResult {
    let fixture = start();
    let root = Uuid::new_v4();
    let identity = seed_asset(&fixture, root)?;
    let repo = &fixture.repository;

    assert_eq!(
        repo.prepare_cleanup(identity.asset_id()).await?,
        PreparationOutcome::Prepared
    );
    assert_eq!(
        repo.prepare_cleanup(identity.asset_id()).await?,
        PreparationOutcome::AlreadyRetired
    );
    assert_eq!(
        repo.prepare_cleanup(Uuid::new_v4()).await?,
        PreparationOutcome::Ineligible
    );

    let claims = repo.claim_due(root, 25).await?;
    let [claim] = claims.as_slice() else {
        return Err("exactly one claim is due".into());
    };
    assert_eq!(claim.attempt_count(), 1);
    assert_eq!(claim.asset(), &identity);
    assert_eq!(
        claim.expires_at(),
        fixture.clock.now() + Duration::minutes(5)
    );

    assert_eq!(
        repo.execute_claim(claim, &REMOVED).await?,
        CleanupOutcome::Completed
    );
    assert_eq!(
        repo.execute_claim(claim, &REMOVED).await?,
        CleanupOutcome::LeaseLost
    );
    let lifecycle = text(
        &fixture,
        "SELECT lifecycle AS value FROM jobniac_generated_assets WHERE asset_id = ?1",
        identity.asset_id(),
    )?;
    assert_eq!(lifecycle.as_deref(), Some("DELETED"));
    let status = text(
        &fixture,
        "SELECT status AS value FROM jobniac_generated_asset_cleanups WHERE asset_id = ?1",
        identity.asset_id(),
    )?;
    assert_eq!(status.as_deref(), Some("COMPLETED"));
    Ok(())
}

#[tokio::test]
async fn a_busy_file_is_retried_and_never_blocks_the_task() -> TestResult {
    let fixture = start();
    let root = Uuid::new_v4();
    let identity = seed_asset(&fixture, root)?;
    let repo = &fixture.repository;
    repo.prepare_cleanup(identity.asset_id()).await?;

    for attempt in 1..=3_u32 {
        let claims = repo.claim_due(root, 25).await?;
        let [claim] = claims.as_slice() else {
            return Err("the retry is due".into());
        };
        assert_eq!(claim.attempt_count(), attempt);
        let busy = ScriptedRemover(Err(RemovalFailureCategory::FileDeleteFailed));
        assert_eq!(
            repo.execute_claim(claim, &busy).await?,
            CleanupOutcome::RetryScheduled
        );
        assert!(repo.claim_due(root, 25).await?.is_empty());
        fixture.clock.advance(Duration::minutes(1));
    }

    let status = text(
        &fixture,
        "SELECT status AS value FROM jobniac_generated_asset_cleanups WHERE asset_id = ?1",
        identity.asset_id(),
    )?;
    assert_eq!(status.as_deref(), Some("PENDING"));
    let snapshot = repo
        .read_queue_snapshot(
            fixture.clock.now(),
            3,
            fixture.clock.now() - Duration::days(30),
        )
        .await?;
    assert!(snapshot.repeated_failure);
    assert!(!snapshot.ownership_uncertain);
    assert_eq!(snapshot.pending_count, 1);
    assert_eq!(
        classify(&snapshot, fixture.clock.now()).status,
        CleanupStatus::Degraded
    );

    let claims = repo.claim_due(root, 25).await?;
    let [claim] = claims.as_slice() else {
        return Err("the fourth attempt is due".into());
    };
    assert_eq!(
        repo.execute_claim(claim, &REMOVED).await?,
        CleanupOutcome::Completed
    );
    Ok(())
}

#[tokio::test]
async fn an_ownership_failure_blocks_the_task_and_marks_health_unknown() -> TestResult {
    let fixture = start();
    let root = Uuid::new_v4();
    let identity = seed_asset(&fixture, root)?;
    let repo = &fixture.repository;
    repo.prepare_cleanup(identity.asset_id()).await?;
    let claims = repo.claim_due(root, 25).await?;
    let [claim] = claims.as_slice() else {
        return Err("one claim is due".into());
    };
    let unproven = ScriptedRemover(Err(RemovalFailureCategory::OwnershipUnproven));
    assert_eq!(
        repo.execute_claim(claim, &unproven).await?,
        CleanupOutcome::Blocked
    );

    fixture.clock.advance(Duration::days(1));
    assert!(repo.claim_due(root, 25).await?.is_empty());
    let snapshot = repo
        .read_queue_snapshot(
            fixture.clock.now(),
            3,
            fixture.clock.now() - Duration::days(30),
        )
        .await?;
    assert!(snapshot.ownership_uncertain);
    assert_eq!(
        classify(&snapshot, fixture.clock.now()).status,
        CleanupStatus::Unknown
    );
    Ok(())
}

#[tokio::test]
async fn an_expired_lease_is_reclaimed_and_the_old_fence_is_lost() -> TestResult {
    let fixture = start();
    let root = Uuid::new_v4();
    let identity = seed_asset(&fixture, root)?;
    let repo = &fixture.repository;
    repo.prepare_cleanup(identity.asset_id()).await?;

    let first = repo.claim_due(root, 25).await?;
    let [first] = first.as_slice() else {
        return Err("one claim is due".into());
    };
    let renewed = repo
        .renew_claim(first)
        .await?
        .ok_or("a live lease renews")?;
    assert_eq!(
        renewed.expires_at(),
        fixture.clock.now() + Duration::minutes(5)
    );

    fixture.clock.advance(Duration::minutes(6));
    assert!(repo.renew_claim(first).await?.is_none());
    let second = repo.claim_due(root, 25).await?;
    let [second] = second.as_slice() else {
        return Err("the expired lease is due again".into());
    };
    assert_eq!(second.attempt_count(), 2);
    assert_eq!(
        repo.execute_claim(first, &REMOVED).await?,
        CleanupOutcome::LeaseLost
    );
    assert_eq!(
        repo.execute_claim(second, &REMOVED).await?,
        CleanupOutcome::Completed
    );
    Ok(())
}

#[tokio::test]
async fn the_executor_runs_the_due_batch_end_to_end_and_prunes_after_retention() -> TestResult {
    let fixture = start();
    let root = Uuid::new_v4();
    let first = seed_asset(&fixture, root)?;
    let second = seed_asset(&fixture, root)?;
    fixture.repository.prepare_cleanup(first.asset_id()).await?;
    fixture
        .repository
        .prepare_cleanup(second.asset_id())
        .await?;

    let executor = CleanupExecutor::new(
        Some(repository(&fixture)),
        Some(Arc::new(REMOVED)),
        Some(root),
        true,
    );
    let summary = executor.run_due_tasks().await?;
    assert_eq!(summary.claimed, 2);
    assert_eq!(summary.completed, 2);
    assert_eq!(summary.state_failures, 0);

    assert_eq!(executor.purge_completed_tasks().await?, 0);
    fixture.clock.advance(Duration::days(31));
    assert_eq!(executor.purge_completed_tasks().await?, 2);
    Ok(())
}

#[tokio::test]
async fn retention_keeps_unfinished_work_and_legacy_rows_mark_ownership_uncertain() -> TestResult {
    let fixture = start();
    let root = Uuid::new_v4();
    let identity = seed_asset(&fixture, root)?;
    fixture
        .repository
        .prepare_cleanup(identity.asset_id())
        .await?;
    fixture.clock.advance(Duration::days(31));
    assert_eq!(fixture.repository.prune_completed(1000).await?, 0);

    let now = fixture.clock.now();
    fixture.sqlite.connection().execute(
        "INSERT INTO document_generated_resume_file_cleanups \
         (id, file_path, status, attempt_count, next_attempt_at, created_at, updated_at) \
         VALUES (?1, 'legacy-synthetic.pdf', 'PENDING', 0, ?2, ?2, ?2)",
        rusqlite::params![Uuid::new_v4().to_string(), stored(now)],
    )?;
    let snapshot = fixture
        .repository
        .read_queue_snapshot(now, 3, now - Duration::days(30))
        .await?;
    assert!(snapshot.ownership_uncertain);
    assert_eq!(snapshot.pending_count, 2);
    Ok(())
}

#[tokio::test]
async fn the_registry_reads_and_registers_one_root_idempotently() -> TestResult {
    let fixture = start();
    let root = Uuid::new_v4();
    let now = fixture.clock.now();
    let state = fixture
        .repository
        .read_generation_root_state(root, now)
        .await?;
    assert!(!state.registered);
    fixture
        .repository
        .register_generated_root(root, now)
        .await?;
    fixture
        .repository
        .register_generated_root(root, now)
        .await?;
    let state = fixture
        .repository
        .read_generation_root_state(root, now)
        .await?;
    assert!(state.registered);
    assert!(!state.expired_pending);
    Ok(())
}

#[tokio::test]
async fn invalid_claim_limits_are_refused_and_an_empty_root_has_no_claims() -> TestResult {
    let fixture = start();
    assert!(
        fixture
            .repository
            .claim_due(Uuid::new_v4(), 0)
            .await
            .is_err()
    );
    assert!(
        fixture
            .repository
            .claim_due(Uuid::new_v4(), 26)
            .await
            .is_err()
    );
    assert!(
        fixture
            .repository
            .claim_due(Uuid::new_v4(), 25)
            .await?
            .is_empty()
    );
    Ok(())
}
