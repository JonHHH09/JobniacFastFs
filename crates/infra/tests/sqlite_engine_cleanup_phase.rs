//! The file phase of a cleanup removal holds no write transaction: another writer commits while it
//! runs, and a claim whose fence rotated meanwhile records nothing. All data is synthetic.

mod sqlite_support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use jobniac_core::cleanup::{
    CleanupOutcome, CleanupRepository, OwnedAssetRemover, RemovalFailure, RemovalOutcome,
};
use jobniac_core::clock::Clock;
use jobniac_core::generated_assets::{AssetIdentity, sha256_hex};
use jobniac_infra::sqlite::cleanup::SqliteCleanupRepository;
use jobniac_infra::sqlite::codec::instant_text;
use rusqlite::Connection;
use sqlite_support::{SqliteFixture, TestResult};
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

const PDF: &[u8] = b"%PDF-1.7\n% synthetic cleanup phase asset\n";
const START: OffsetDateTime = datetime!(2026-10-15 12:00:00 UTC);

#[derive(Debug)]
struct FixedClock(Mutex<OffsetDateTime>);

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// While "removing", writes through another connection with no busy wait and reports whether the
/// write lock was free.
struct WritingRemover {
    other: Mutex<Connection>,
    committed: Mutex<Option<bool>>,
}

impl OwnedAssetRemover for WritingRemover {
    fn remove_owned(&self, _identity: &AssetIdentity) -> Result<RemovalOutcome, RemovalFailure> {
        let other = self
            .other
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let wrote = other
            .execute_batch("BEGIN IMMEDIATE; INSERT INTO engine_probe VALUES (1); COMMIT")
            .is_ok();
        if !wrote {
            let _ = other.execute_batch("ROLLBACK");
        }
        *self
            .committed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(wrote);
        Ok(RemovalOutcome::Removed)
    }
}

fn seed_retiring_asset(
    fixture: &SqliteFixture,
    root: Uuid,
) -> Result<AssetIdentity, Box<dyn std::error::Error + Send + Sync>> {
    let identity = AssetIdentity::new(
        Uuid::new_v4(),
        root,
        &sha256_hex(PDF),
        u64::try_from(PDF.len())?,
    )?;
    let now = instant_text(START);
    let connection = fixture.connection();
    connection.execute(
        "INSERT INTO jobniac_generated_asset_roots (id, created_at) VALUES (?1, ?2)",
        rusqlite::params![root.to_string(), now],
    )?;
    connection.execute(
        "INSERT INTO jobniac_generated_assets (asset_id, root_id, relative_path, sha256, \
         size_bytes, document_id, lifecycle, created_at, generation_intent_required) \
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'RESERVED', ?6, 0)",
        rusqlite::params![
            identity.asset_id().to_string(),
            root.to_string(),
            identity.relative_path(),
            identity.sha256(),
            i64::try_from(PDF.len())?,
            now
        ],
    )?;
    Ok(identity)
}

#[tokio::test]
async fn a_second_writer_commits_while_the_removal_runs() -> TestResult {
    let fixture = SqliteFixture::start();
    fixture
        .connection()
        .execute_batch("CREATE TABLE engine_probe (x INTEGER)")?;
    let clock = Arc::new(FixedClock(Mutex::new(START)));
    let repository =
        SqliteCleanupRepository::new(fixture.unit_of_work(), Arc::clone(&clock) as Arc<dyn Clock>);
    let root = Uuid::new_v4();
    let identity = seed_retiring_asset(&fixture, root)?;
    repository.prepare_cleanup(identity.asset_id()).await?;
    let claims = repository.claim_due(root, 25).await?;
    let [claim] = claims.as_slice() else {
        return Err("one claim is due".into());
    };

    let other = fixture.connection();
    other.busy_timeout(Duration::ZERO)?;
    let remover = WritingRemover {
        other: Mutex::new(other),
        committed: Mutex::new(None),
    };
    assert_eq!(
        repository.execute_claim(claim, &remover).await?,
        CleanupOutcome::Completed
    );
    assert_eq!(
        *remover.committed.lock().map_err(|_| "poisoned")?,
        Some(true),
        "the other writer committed during the file phase"
    );
    let lifecycle: String = fixture.connection().query_row(
        "SELECT lifecycle FROM jobniac_generated_assets WHERE asset_id = ?1",
        [identity.asset_id().to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(lifecycle, "DELETED");
    Ok(())
}

/// Rotates the fence (as a later claim would) while the file phase runs.
struct RotatingRemover {
    other: Mutex<Connection>,
    task_id: Uuid,
}

impl OwnedAssetRemover for RotatingRemover {
    fn remove_owned(&self, _identity: &AssetIdentity) -> Result<RemovalOutcome, RemovalFailure> {
        let other = self
            .other
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = other.execute(
            "UPDATE jobniac_generated_asset_cleanups SET lease_token = ?1, attempt_count = attempt_count + 1 \
             WHERE task_id = ?2",
            rusqlite::params![Uuid::new_v4().to_string(), self.task_id.to_string()],
        );
        Ok(RemovalOutcome::Removed)
    }
}

#[tokio::test]
async fn a_rotated_fence_during_the_removal_records_nothing() -> TestResult {
    let fixture = SqliteFixture::start();
    let clock = Arc::new(FixedClock(Mutex::new(START)));
    let repository =
        SqliteCleanupRepository::new(fixture.unit_of_work(), Arc::clone(&clock) as Arc<dyn Clock>);
    let root = Uuid::new_v4();
    let identity = seed_retiring_asset(&fixture, root)?;
    repository.prepare_cleanup(identity.asset_id()).await?;
    let claims = repository.claim_due(root, 25).await?;
    let [claim] = claims.as_slice() else {
        return Err("one claim is due".into());
    };
    let other = fixture.connection();
    let remover = RotatingRemover {
        other: Mutex::new(other),
        task_id: claim.task_id(),
    };
    assert_eq!(
        repository.execute_claim(claim, &remover).await?,
        CleanupOutcome::LeaseLost
    );
    let status: String = fixture.connection().query_row(
        "SELECT status FROM jobniac_generated_asset_cleanups WHERE task_id = ?1",
        [claim.task_id().to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(status, "PROCESSING", "the finalization did not run");
    Ok(())
}
