//! Standalone generated PDFs against a migrated SQLite file: reservation, reconciliation,
//! activation, retirement with cleanup rows, abandoned-reservation recovery and the serialization
//! of concurrent materializations. The file port is an in-memory fake; all data is synthetic.

mod sqlite_generated_support;
mod sqlite_support;

use std::sync::Arc;

use jobniac_core::clock::FixedClock;
use jobniac_core::generated_assets::standalone::{
    StandaloneAssetRepository, StandaloneAssetService,
};
use jobniac_core::generated_assets::{
    GeneratedAssetWriter, MaterializationOutcome, PublicationFailureCategory,
};
use jobniac_infra::sqlite::generated::repository::SqliteGeneratedAssetRepository;
use sqlite_generated_support::{PDF, RecordingWriter, instant, lock, register_root, text};
use sqlite_support::{SqliteFixture, TestResult};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const LIFECYCLE_SQL: &str = "SELECT lifecycle FROM jobniac_generated_assets WHERE asset_id = ?1";
const MATERIALIZED_SQL: &str = "SELECT CASE WHEN materialized_at IS NOT NULL THEN 'true' \
    ELSE 'false' END FROM jobniac_generated_assets WHERE asset_id = ?1";
const CLEANUP_SQL: &str = "SELECT status || ':' || COALESCE(failure_category, '-') \
    FROM jobniac_generated_asset_cleanups WHERE asset_id = ?1";

fn now() -> OffsetDateTime {
    instant(0)
}

/// The service over the SQLite repository, the recording file fake and a fixed clock.
fn service(
    fixture: &SqliteFixture,
    writer: &Arc<RecordingWriter>,
    root: Uuid,
    at: OffsetDateTime,
) -> StandaloneAssetService {
    let clock = Arc::new(FixedClock::new(at));
    let repository = SqliteGeneratedAssetRepository::new(fixture.unit_of_work(), clock.clone());
    StandaloneAssetService::new(
        Arc::new(repository) as Arc<dyn StandaloneAssetRepository>,
        Arc::clone(writer) as Arc<dyn GeneratedAssetWriter>,
        root,
        clock,
        true,
    )
}

#[tokio::test]
async fn reserve_round_trips_through_the_repository() -> TestResult {
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, now()).await?;
    let writer = Arc::new(RecordingWriter::default());
    let svc = service(&fixture, &writer, root, now());
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let repository = SqliteGeneratedAssetRepository::new(
        fixture.unit_of_work(),
        Arc::new(FixedClock::new(now())),
    );
    assert_eq!(
        repository
            .find_reservation(reservation.asset.asset_id())
            .await?,
        Some(reservation.clone())
    );
    assert_eq!(
        text(
            &fixture.connection(),
            LIFECYCLE_SQL,
            reservation.asset.asset_id()
        )?,
        "RESERVED"
    );
    assert!(lock(&writer.calls).is_empty(), "reserving never publishes");
    Ok(())
}

#[tokio::test]
async fn materialize_activates_the_asset_once() -> TestResult {
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, now()).await?;
    let writer = Arc::new(RecordingWriter::default());
    let svc = service(&fixture, &writer, root, now());
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let asset = reservation.asset.asset_id();
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::Active
    );
    let conn = fixture.connection();
    assert_eq!(text(&conn, LIFECYCLE_SQL, asset)?, "ACTIVE");
    assert_eq!(text(&conn, MATERIALIZED_SQL, asset)?, "true");
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::Active
    );
    assert_eq!(lock(&writer.calls).len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_file_write_failure_retires_the_asset_with_a_pending_cleanup() -> TestResult {
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, now()).await?;
    let writer = Arc::new(RecordingWriter {
        failure: Some(PublicationFailureCategory::FileWriteFailed),
        ..RecordingWriter::default()
    });
    let svc = service(&fixture, &writer, root, now());
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let asset = reservation.asset.asset_id();
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::CleanupPending
    );
    let conn = fixture.connection();
    assert_eq!(text(&conn, LIFECYCLE_SQL, asset)?, "RETIRING");
    assert_eq!(text(&conn, CLEANUP_SQL, asset)?, "PENDING:-");
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::Ineligible
    );
    Ok(())
}

#[tokio::test]
async fn an_identity_failure_blocks_the_asset_with_its_category() -> TestResult {
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, now()).await?;
    let writer = Arc::new(RecordingWriter {
        failure: Some(PublicationFailureCategory::IdentityMismatch),
        ..RecordingWriter::default()
    });
    let svc = service(&fixture, &writer, root, now());
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let asset = reservation.asset.asset_id();
    assert_eq!(
        svc.materialize(&reservation).await?.outcome,
        MaterializationOutcome::Blocked
    );
    assert_eq!(
        text(&fixture.connection(), CLEANUP_SQL, asset)?,
        "BLOCKED:IDENTITY_MISMATCH"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_materializations_publish_once() -> TestResult {
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, now()).await?;
    let writer = Arc::new(RecordingWriter::default());
    let svc = service(&fixture, &writer, root, now());
    let reservation = svc.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let (first, second) =
        tokio::join!(svc.materialize(&reservation), svc.materialize(&reservation));
    assert_eq!(first?.outcome, MaterializationOutcome::Active);
    assert_eq!(second?.outcome, MaterializationOutcome::Active);
    assert_eq!(
        lock(&writer.calls).len(),
        1,
        "the second caller waits for the single writer and sees the active row"
    );
    Ok(())
}

#[tokio::test]
async fn recovery_retires_only_stale_reservations() -> TestResult {
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, now()).await?;
    let writer = Arc::new(RecordingWriter::default());
    // The stale reservations are created ten minutes before the recovery pass.
    let past = service(&fixture, &writer, root, now() - Duration::minutes(10));
    let svc = service(&fixture, &writer, root, now());
    let stale = past
        .reserve_pdf(&sqlite_generated_support::pdf("a"), "a.pdf")
        .await?;
    let active = past
        .reserve_pdf(&sqlite_generated_support::pdf("b"), "b.pdf")
        .await?;
    let fresh = svc
        .reserve_pdf(&sqlite_generated_support::pdf("c"), "c.pdf")
        .await?;
    svc.materialize(&active).await?;

    let recovered = svc.recover_abandoned().await?;
    assert_eq!(recovered, vec![stale.asset.asset_id()]);
    let conn = fixture.connection();
    assert_eq!(
        text(&conn, LIFECYCLE_SQL, stale.asset.asset_id())?,
        "RETIRING"
    );
    assert_eq!(
        text(&conn, CLEANUP_SQL, stale.asset.asset_id())?,
        "PENDING:-"
    );
    assert_eq!(
        text(&conn, LIFECYCLE_SQL, fresh.asset.asset_id())?,
        "RESERVED"
    );
    assert_eq!(
        text(&conn, LIFECYCLE_SQL, active.asset.asset_id())?,
        "ACTIVE"
    );
    Ok(())
}

#[tokio::test]
async fn recovery_leaves_a_stale_reservation_that_a_link_references() -> TestResult {
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, now()).await?;
    let writer = Arc::new(RecordingWriter::default());
    let past = service(&fixture, &writer, root, now() - Duration::minutes(10));
    let svc = service(&fixture, &writer, root, now());
    let reservation = past.reserve_pdf(PDF, "synthetic-resume.pdf").await?;
    let asset = reservation.asset.asset_id();
    // A profile photo that points at the owned document is a reference.
    let conn = fixture.connection();
    let profile = Uuid::new_v4();
    sqlite_generated_support::seed_profile(&conn, profile, 0, now())?;
    conn.execute(
        "INSERT INTO profile_profile_personal_details (profile_id, photo_document_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?3)",
        rusqlite::params![
            profile.to_string(),
            reservation.document_id.to_string(),
            sqlite_generated_support::stored(now())
        ],
    )?;
    assert!(svc.recover_abandoned().await?.is_empty());
    assert_eq!(text(&conn, LIFECYCLE_SQL, asset)?, "RESERVED");
    Ok(())
}
