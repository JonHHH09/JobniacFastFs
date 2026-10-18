//! The current profile resume links against a migrated SQLite file: creation, the owner
//! compare-and-set, retirement of the replaced asset in the same commit, refused bindings, fenced
//! classified replacement, reference classes that keep the previous asset, and rollbacks on an
//! injected write failure. Profiles are seeded with raw SQL; all data is synthetic.

mod sqlite_generated_support;
mod sqlite_support;

use std::sync::Arc;

use jobniac_core::clock::FixedClock;
use jobniac_core::errors::{AppError, ErrorCode};
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedReservation, GenerationDescriptor, GenerationPurpose, GenerationState,
    IntentReservation, ProfileResumeDocument, sha256_hex,
};
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::sqlite::error::SqliteError;
use jobniac_infra::sqlite::generated::IntentError;
use jobniac_infra::sqlite::generated::intents::{
    find_generation_intent, insert_generation_intent, renew_generation_intent,
};
use jobniac_infra::sqlite::generated::profile_links::SqliteProfileResumeLinks;
use jobniac_infra::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};
use rusqlite::params;
use sqlite_generated_support::{count, fail_at, lifecycle, pdf, seed_profile, stored};
use sqlite_support::{SqliteFixture, TestResult};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const MASTER: &str = "master_resume";
const CANADIAN: &str = "canadian_resume";

/// A migrated database with one synthetic profile and one asset root.
struct Fixture {
    sqlite: SqliteFixture,
    uow: SqliteUnitOfWork,
    profile: Uuid,
    root: Uuid,
    /// The profile's `created_at` and `updated_at`: the owner revision the CAS compares against.
    base: OffsetDateTime,
}

fn start() -> Fixture {
    let sqlite = SqliteFixture::start();
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid instant");
    let profile = Uuid::new_v4();
    let root = Uuid::new_v4();
    let conn = sqlite.connection();
    seed_profile(&conn, profile, 0, base).expect("profile seeds");
    conn.execute(
        "INSERT INTO jobniac_generated_asset_roots (id, created_at) VALUES (?1, ?2)",
        params![root.to_string(), stored(base)],
    )
    .expect("root seeds");
    let uow = sqlite.unit_of_work();
    Fixture {
        sqlite,
        uow,
        profile,
        root,
        base,
    }
}

fn options() -> TransactionOptions {
    TransactionOptions {
        read_only: false,
        requires_new: false,
    }
}

fn fixture_failure(_: SqliteError) -> IntentError {
    IntentError::Database(AppError::Internal)
}

fn links(fixture: &Fixture, at: OffsetDateTime) -> SqliteProfileResumeLinks {
    SqliteProfileResumeLinks::new(fixture.uow.clone(), Arc::new(FixedClock::new(at)))
}

/// Inserts a document and a RESERVED asset in the current transaction.
fn insert_asset(
    tx: &SqliteTransaction,
    root: Uuid,
    now: OffsetDateTime,
    content: &[u8],
    classified: bool,
) -> Result<GeneratedReservation, IntentError> {
    let blob = Uuid::new_v4();
    let document = Uuid::new_v4();
    let asset_id = Uuid::new_v4();
    let digest = sha256_hex(content);
    let size = i64::try_from(content.len()).map_err(|_| IntentError::Invalid)?;
    let at = stored(now);
    tx.run(|conn| {
        conn.execute(
            "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![blob.to_string(), digest, size, content, at],
        )?;
        conn.execute(
            "INSERT INTO document_documents (id, blob_id, original_file_name, media_type, \
             created_at, updated_at) VALUES (?1, ?2, 'synthetic-resume.pdf', 'application/pdf', ?3, ?3)",
            params![document.to_string(), blob.to_string(), at],
        )?;
        conn.execute(
            "INSERT INTO jobniac_generated_assets (asset_id, root_id, relative_path, sha256, \
             size_bytes, document_id, lifecycle, created_at, generation_intent_required) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'RESERVED', ?7, ?8)",
            params![
                asset_id.to_string(),
                root.to_string(),
                format!("{asset_id}.pdf"),
                digest,
                size,
                document.to_string(),
                at,
                classified
            ],
        )?;
        Ok(())
    })
    .map_err(fixture_failure)?;
    Ok(GeneratedReservation {
        asset: AssetIdentity::new(asset_id, root, &digest, content.len() as u64)?,
        document_id: document,
    })
}

fn activate(
    tx: &SqliteTransaction,
    asset_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), IntentError> {
    tx.run(|conn| {
        conn.execute(
            "UPDATE jobniac_generated_assets SET lifecycle = 'ACTIVE', materialized_at = ?2 \
             WHERE asset_id = ?1",
            params![asset_id.to_string(), stored(now)],
        )?;
        Ok(())
    })
    .map_err(fixture_failure)
}

/// Creates one ACTIVE, unclassified asset in its own committed transaction.
async fn active_asset(
    fixture: &Fixture,
    now: OffsetDateTime,
    label: &str,
) -> Result<GeneratedReservation, IntentError> {
    let content = pdf(label);
    fixture
        .uow
        .transaction(options(), async |tx| {
            let reservation = insert_asset(tx, fixture.root, now, &content, false)?;
            activate(tx, reservation.asset.asset_id(), now)?;
            Ok(reservation)
        })
        .await
}

fn link(
    profile: Uuid,
    binding: &GeneratedReservation,
    resume_type: &str,
    created: OffsetDateTime,
    updated: OffsetDateTime,
) -> Result<ProfileResumeDocument, Box<dyn std::error::Error + Send + Sync>> {
    Ok(ProfileResumeDocument::new(
        Uuid::new_v4(),
        profile,
        binding.document_id,
        binding.asset.relative_path(),
        resume_type,
        created,
        updated,
    )?)
}

fn task_count(fixture: &Fixture, asset: Uuid) -> i64 {
    fixture
        .sqlite
        .connection()
        .query_row(
            "SELECT count(*) FROM jobniac_generated_asset_cleanups WHERE asset_id = ?1",
            [asset.to_string()],
            |row| row.get(0),
        )
        .expect("count reads")
}

fn refusal_code<T: std::fmt::Debug>(
    result: Result<T, AppError>,
) -> Result<ErrorCode, Box<dyn std::error::Error + Send + Sync>> {
    match result {
        Err(AppError::Application(error)) => Ok(error.code()),
        other => Err(format!("unexpected outcome: {other:?}").into()),
    }
}

async fn current(
    links: &SqliteProfileResumeLinks,
    profile: Uuid,
    resume_type: &str,
) -> Result<Option<ProfileResumeDocument>, AppError> {
    links
        .find_by_profile_id_and_resume_type(profile, resume_type)
        .await
}

#[tokio::test]
async fn create_without_previous_link_inserts_and_retires_nothing() -> TestResult {
    let fixture = start();
    let (profile, base) = (fixture.profile, fixture.base);
    let first = active_asset(&fixture, base, "first").await?;
    let links = links(&fixture, base);
    let candidate = link(
        profile,
        &first,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(2),
    )?;
    let result = links.replace(&candidate, &first, 0, base, None).await?;
    assert!(result.previous.is_none());
    assert_eq!(result.saved, candidate);
    assert_eq!(current(&links, profile, MASTER).await?, Some(candidate));
    assert_eq!(
        lifecycle(&fixture.sqlite, first.asset.asset_id())?,
        "ACTIVE"
    );
    assert_eq!(task_count(&fixture, first.asset.asset_id()), 0);
    Ok(())
}

#[tokio::test]
async fn owner_cas_refuses_stale_revision_and_timestamp_then_replace_retires_previous() -> TestResult
{
    let fixture = start();
    let (profile, base) = (fixture.profile, fixture.base);
    let now = base + Duration::seconds(10);
    let old = active_asset(&fixture, now, "old").await?;
    let new = active_asset(&fixture, now, "new").await?;
    let links = links(&fixture, now);
    let previous = link(
        profile,
        &old,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(2),
    )?;
    links.replace(&previous, &old, 0, base, None).await?;
    let candidate = link(
        profile,
        &new,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(3),
    )?;

    let stale_revision = links.replace(&candidate, &new, 1, base, None).await;
    assert_eq!(refusal_code(stale_revision)?, ErrorCode::Conflict);
    let stale_timestamp = links
        .replace(&candidate, &new, 0, base + Duration::seconds(1), None)
        .await;
    assert_eq!(refusal_code(stale_timestamp)?, ErrorCode::Conflict);
    assert_eq!(
        current(&links, profile, MASTER).await?,
        Some(previous.clone())
    );
    assert_eq!(lifecycle(&fixture.sqlite, old.asset.asset_id())?, "ACTIVE");

    let result = links.replace(&candidate, &new, 0, base, None).await?;
    assert_eq!(result.previous, Some(previous.clone()));
    assert_eq!(result.saved.id, previous.id);
    assert_eq!(result.saved.created_at, previous.created_at);
    assert_eq!(result.saved.document_id, new.document_id);
    assert_eq!(result.saved.file_path, new.asset.relative_path());
    assert_eq!(current(&links, profile, MASTER).await?, Some(result.saved));
    assert_eq!(
        lifecycle(&fixture.sqlite, old.asset.asset_id())?,
        "RETIRING"
    );
    assert_eq!(task_count(&fixture, old.asset.asset_id()), 1);
    assert_eq!(lifecycle(&fixture.sqlite, new.asset.asset_id())?, "ACTIVE");
    Ok(())
}

#[tokio::test]
async fn older_candidate_timestamp_conflicts_before_any_mutation() -> TestResult {
    let fixture = start();
    let (profile, base) = (fixture.profile, fixture.base);
    let old = active_asset(&fixture, base, "old").await?;
    let new = active_asset(&fixture, base, "new").await?;
    let links = links(&fixture, base);
    let previous = link(
        profile,
        &old,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(5),
    )?;
    links.replace(&previous, &old, 0, base, None).await?;
    let older = link(
        profile,
        &new,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(2),
    )?;
    let refused = links.replace(&older, &new, 0, base, None).await;
    assert_eq!(refusal_code(refused)?, ErrorCode::Conflict);
    assert_eq!(current(&links, profile, MASTER).await?, Some(previous));
    assert_eq!(task_count(&fixture, old.asset.asset_id()), 0);
    Ok(())
}

#[tokio::test]
async fn unactivated_or_mismatched_binding_is_refused_and_keeps_the_previous_link() -> TestResult {
    let fixture = start();
    let (profile, base) = (fixture.profile, fixture.base);
    let old = active_asset(&fixture, base, "old").await?;
    let links = links(&fixture, base);
    let previous = link(
        profile,
        &old,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(2),
    )?;
    links.replace(&previous, &old, 0, base, None).await?;

    // A binding whose asset is still RESERVED (its activation never committed) is refused.
    let reserved_content = pdf("reserved");
    let reserved = fixture
        .uow
        .transaction(options(), async |tx| {
            insert_asset(tx, fixture.root, base, &reserved_content, false)
        })
        .await?;
    let candidate = link(
        profile,
        &reserved,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(3),
    )?;
    let refused = links.replace(&candidate, &reserved, 0, base, None).await;
    assert_eq!(refusal_code(refused)?, ErrorCode::ValidationError);

    // A link that names a different document than its binding is refused up front.
    let other = active_asset(&fixture, base, "other").await?;
    let mismatched = link(
        profile,
        &other,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(3),
    )?;
    let refused = links.replace(&mismatched, &reserved, 0, base, None).await;
    assert_eq!(refusal_code(refused)?, ErrorCode::ValidationError);

    assert_eq!(current(&links, profile, MASTER).await?, Some(previous));
    assert_eq!(lifecycle(&fixture.sqlite, old.asset.asset_id())?, "ACTIVE");
    assert_eq!(
        lifecycle(&fixture.sqlite, reserved.asset.asset_id())?,
        "RESERVED"
    );
    assert_eq!(task_count(&fixture, old.asset.asset_id()), 0);
    Ok(())
}

#[tokio::test]
async fn classified_replacement_requires_the_live_fence_and_completes_the_intent() -> TestResult {
    let fixture = start();
    let (profile, root, base) = (fixture.profile, fixture.root, fixture.base);
    let operation = Uuid::new_v4();
    let content = pdf("classified");
    let claim = fixture
        .uow
        .transaction(options(), async |tx| {
            let reservation = insert_asset(tx, root, base, &content, true)?;
            let descriptor = GenerationDescriptor::new(
                operation,
                GenerationPurpose::ProfileResume,
                Some(profile),
                None,
                None,
                Some(CANADIAN.to_owned()),
            )?;
            let claim =
                insert_generation_intent(tx, &reservation, &descriptor, Uuid::new_v4(), base)
                    .await?;
            activate(
                tx,
                reservation.asset.asset_id(),
                base + Duration::seconds(1),
            )?;
            Ok::<IntentReservation, IntentError>(claim)
        })
        .await?;
    let renewed = fixture
        .uow
        .transaction(options(), async |tx| {
            renew_generation_intent(tx, &claim, Uuid::new_v4(), base + Duration::seconds(2)).await
        })
        .await?;
    let links = links(&fixture, base + Duration::seconds(3));
    let candidate = link(
        profile,
        &claim.reservation,
        CANADIAN,
        base + Duration::seconds(1),
        base + Duration::seconds(3),
    )?;

    // The superseded fence cannot admit the replacement, and nothing is written.
    let stale = links
        .replace(&candidate, &claim.reservation, 0, base, Some(&claim))
        .await;
    assert_eq!(refusal_code(stale)?, ErrorCode::ValidationError);
    assert_eq!(current(&links, profile, CANADIAN).await?, None);

    // The renewed claim, which holds the current fence, completes the replacement.
    let result = links
        .replace(&candidate, &claim.reservation, 0, base, Some(&renewed))
        .await?;
    assert!(result.previous.is_none());
    assert_eq!(current(&links, profile, CANADIAN).await?, Some(candidate));
    let completed = fixture
        .uow
        .transaction(options(), async |tx| {
            find_generation_intent(tx, claim.asset_id(), false).await
        })
        .await?
        .ok_or("the completed intent must remain as history")?;
    assert_eq!(completed.state, GenerationState::Completed);
    Ok(())
}

#[tokio::test]
async fn coexisting_types_keep_their_links_and_only_the_replaced_asset_retires() -> TestResult {
    let fixture = start();
    let (profile, base) = (fixture.profile, fixture.base);
    let master = active_asset(&fixture, base, "master").await?;
    let canadian = active_asset(&fixture, base, "canadian").await?;
    let newer = active_asset(&fixture, base, "newer").await?;
    let links = links(&fixture, base);
    let at = |seconds| base + Duration::seconds(seconds);
    links
        .replace(
            &link(profile, &master, MASTER, at(1), at(2))?,
            &master,
            0,
            base,
            None,
        )
        .await?;
    links
        .replace(
            &link(profile, &canadian, CANADIAN, at(1), at(2))?,
            &canadian,
            0,
            base,
            None,
        )
        .await?;
    links
        .replace(
            &link(profile, &newer, MASTER, at(1), at(3))?,
            &newer,
            0,
            base,
            None,
        )
        .await?;
    assert_eq!(
        lifecycle(&fixture.sqlite, master.asset.asset_id())?,
        "RETIRING"
    );
    assert_eq!(
        lifecycle(&fixture.sqlite, canadian.asset.asset_id())?,
        "ACTIVE"
    );
    let kept = current(&links, profile, CANADIAN)
        .await?
        .ok_or("the link")?;
    assert_eq!(kept.document_id, canadian.document_id);
    Ok(())
}

#[tokio::test]
async fn every_reference_class_keeps_the_previous_asset_active() -> TestResult {
    let fixture = start();
    let (profile, base) = (fixture.profile, fixture.base);
    let old = active_asset(&fixture, base, "old").await?;
    let new = active_asset(&fixture, base, "new").await?;
    let links = links(&fixture, base);
    let at = |seconds| base + Duration::seconds(seconds);
    links
        .replace(
            &link(profile, &old, MASTER, at(1), at(2))?,
            &old,
            0,
            base,
            None,
        )
        .await?;
    // A personal-details photo references the previous document as well.
    fixture.sqlite.connection().execute(
        "INSERT INTO profile_profile_personal_details \
         (profile_id, photo_document_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![
            profile.to_string(),
            old.document_id.to_string(),
            stored(base)
        ],
    )?;
    links
        .replace(
            &link(profile, &new, MASTER, at(1), at(3))?,
            &new,
            0,
            base,
            None,
        )
        .await?;
    assert_eq!(lifecycle(&fixture.sqlite, old.asset.asset_id())?, "ACTIVE");
    assert_eq!(task_count(&fixture, old.asset.asset_id()), 0);
    Ok(())
}

#[tokio::test]
async fn link_and_retirement_failure_roll_back_every_row() -> TestResult {
    let fixture = start();
    let (profile, base) = (fixture.profile, fixture.base);
    let old = active_asset(&fixture, base, "old").await?;
    let new = active_asset(&fixture, base, "new").await?;
    let links = links(&fixture, base);
    let at = |seconds| base + Duration::seconds(seconds);
    let previous = link(profile, &old, MASTER, at(1), at(2))?;
    links.replace(&previous, &old, 0, base, None).await?;
    fail_at(
        &fixture.sqlite.connection(),
        "jobniac_generated_asset_cleanups",
        "INSERT",
        "TRUE",
    )?;
    let refused = links
        .replace(
            &link(profile, &new, MASTER, at(1), at(3))?,
            &new,
            0,
            base,
            None,
        )
        .await;
    assert!(refused.is_err());
    assert_eq!(current(&links, profile, MASTER).await?, Some(previous));
    assert_eq!(lifecycle(&fixture.sqlite, old.asset.asset_id())?, "ACTIVE");
    assert_eq!(
        count(
            &fixture.sqlite.connection(),
            "SELECT count(*) FROM jobniac_generated_asset_cleanups"
        )?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_owner_is_not_found() -> TestResult {
    let fixture = start();
    let base = fixture.base;
    let asset = active_asset(&fixture, base, "orphan").await?;
    let links = links(&fixture, base);
    let candidate = link(Uuid::new_v4(), &asset, MASTER, base, base)?;
    let refused = links.replace(&candidate, &asset, 0, base, None).await;
    assert_eq!(refusal_code(refused)?, ErrorCode::NotFound);
    Ok(())
}
