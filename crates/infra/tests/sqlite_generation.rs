//! Classified generated PDFs against a migrated SQLite file: reservation with its birth intent, the
//! find and proof of the reservation, activation of a standalone output with its completed intent,
//! the abandonment of a pending output with its retirement, the recovery of an expired pending
//! output and the renewal of a live fence. The file port is an in-memory fake; all data is
//! synthetic.

mod sqlite_generated_support;
mod sqlite_support;

use std::sync::Mutex;

use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::service::{GeneratedAssetRepository, PreparationOutcome};
use jobniac_core::generated_assets::{
    GenerationState, IntentReservation, MaterializationOutcome, PublicationFailureCategory,
};
use jobniac_infra::sqlite::generated::repository::SqliteGeneratedAssetRepository;
use sqlite_generated_support::{
    PDF, RecordingWriter, identity, instant, lifecycle, lock, register_root, repository,
    standalone_descriptor,
};
use sqlite_support::{SqliteFixture, TestResult};
use time::OffsetDateTime;
use uuid::Uuid;

/// Reserves one classified standalone output at `at`.
async fn reserve(
    repository: &SqliteGeneratedAssetRepository,
    root: Uuid,
    at: OffsetDateTime,
) -> Result<IntentReservation, AppError> {
    repository
        .reserve_classified_pdf(
            &identity(root, PDF),
            Uuid::new_v4(),
            "synthetic.pdf",
            PDF,
            &standalone_descriptor(),
            Uuid::new_v4(),
            at,
        )
        .await
}

#[tokio::test]
async fn a_standalone_reservation_is_found_and_activated_with_its_completed_intent() -> TestResult {
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, instant(0)).await?;
    let repo = repository(&fixture, 0);
    let claim = reserve(&repo, root, instant(0))
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
    assert_eq!(lifecycle(&fixture, claim.asset_id())?, "ACTIVE");

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
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, instant(0)).await?;
    let repo = repository(&fixture, 0);
    let claim = reserve(&repo, root, instant(0))
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
    assert_ne!(lifecycle(&fixture, claim.asset_id())?, "ACTIVE");
    Ok(())
}

#[tokio::test]
async fn abandoning_a_pending_output_prepares_its_retirement_once() -> TestResult {
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, instant(0)).await?;
    let repo = repository(&fixture, 0);
    let claim = reserve(&repo, root, instant(0))
        .await
        .map_err(|error| error.to_string())?;
    let first = repo
        .abandon_generation(&claim)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(first, PreparationOutcome::Prepared);
    assert_eq!(lifecycle(&fixture, claim.asset_id())?, "RETIRING");
    let second = repo
        .abandon_generation(&claim)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(second, PreparationOutcome::AlreadyRetired);
    Ok(())
}

#[tokio::test]
async fn an_expired_pending_output_is_recovered_once_and_a_live_fence_renews() -> TestResult {
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, instant(0)).await?;
    let repo = repository(&fixture, 0);
    let live = reserve(&repo, root, instant(0))
        .await
        .map_err(|error| error.to_string())?;
    let renewed = repo
        .renew_generation(&live, Uuid::new_v4())
        .await
        .map_err(|error| error.to_string())?
        .ok_or("a live pending claim must renew")?;
    assert_eq!(renewed.intent.state, GenerationState::Pending);

    let later = repository(&fixture, 3_600);
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
