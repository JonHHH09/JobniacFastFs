//! Scenarios of the current profile resume links that the single SQLite writer must keep: the
//! writer that commits first and the writer that re-reads state and is refused (cleanup, renewal,
//! expiry recovery, completion), refusals and rollbacks that leave every table untouched, and real
//! contention on one file (two pools, a held `BEGIN IMMEDIATE`, a writer paused inside its
//! transaction) with the reads a WAL snapshot allows meanwhile.
//!
//! The contention tests never sleep to synchronize: a clock that blocks inside the replacement
//! pauses it while it holds the write lock, and channels tell the test when it is there. They run
//! on a multi-thread runtime because a paused or waiting writer parks its worker. All data is
//! synthetic.

mod sqlite_generated_support;
mod sqlite_scenarios_support;
mod sqlite_support;

use std::error::Error;
use std::sync::Arc;

use jobniac_core::cleanup::CleanupRepository;
use jobniac_core::clock::{Clock, FixedClock};
use jobniac_core::errors::{AppError, ErrorCode, error_envelope};
use jobniac_core::generated_assets::service::GeneratedAssetRepository;
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedReservation, GenerationDescriptor, GenerationPurpose, GenerationState,
    IntentReservation, PreparationOutcome, ProfileResumeDocument, RETRY_BATCH_SIZE, sha256_hex,
};
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::sqlite::cleanup::SqliteCleanupRepository;
use jobniac_infra::sqlite::error::SqliteError;
use jobniac_infra::sqlite::generated::IntentError;
use jobniac_infra::sqlite::generated::assets::retire_asset;
use jobniac_infra::sqlite::generated::bindings::{ASSET_COLUMNS, AssetRow};
use jobniac_infra::sqlite::generated::intents::{find_generation_intent, insert_generation_intent};
use jobniac_infra::sqlite::generated::profile_links::SqliteProfileResumeLinks;
use jobniac_infra::sqlite::generated::repository::SqliteGeneratedAssetRepository;
use jobniac_infra::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};
use rusqlite::params;
use sqlite_generated_support::{count, fail_at, lifecycle, pdf, seed_profile, stored};
use sqlite_scenarios_support::{
    PauseClock, TableState, TickClock, WriteHold, assert_still_waiting, raw_connection,
    second_pool, spawn_raw_write, table_state,
};
use sqlite_support::{SqliteFixture, TestResult};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

type BoxError = Box<dyn Error + Send + Sync>;

const MASTER: &str = "master_resume";
const CANADIAN: &str = "canadian_resume";

/// The tables whose rows a refused or failed replacement must leave untouched.
const STATE_TABLES: [&str; 8] = [
    "profile_profiles",
    "profile_profile_resume_documents",
    "document_documents",
    "document_blobs",
    "jobniac_generated_assets",
    "jobniac_generated_asset_cleanups",
    "jobniac_generation_intents",
    "profile_profile_pdf_sources",
];

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

fn fixed(at: OffsetDateTime) -> Arc<dyn Clock> {
    Arc::new(FixedClock::new(at))
}

/// The profile-link repository over the fixture's pool.
fn links(f: &Fixture, clock: Arc<dyn Clock>) -> SqliteProfileResumeLinks {
    SqliteProfileResumeLinks::new(f.uow.clone(), clock)
}

/// The profile-link repository over a second pool of the same file.
fn links_on_second_pool(f: &Fixture, clock: Arc<dyn Clock>) -> SqliteProfileResumeLinks {
    let uow = SqliteUnitOfWork::new(second_pool(f.sqlite.path()));
    SqliteProfileResumeLinks::new(uow, clock)
}

/// The retirement repository of the fixture.
fn cleanup(f: &Fixture, clock: Arc<dyn Clock>) -> SqliteCleanupRepository {
    SqliteCleanupRepository::new(f.uow.clone(), clock)
}

/// The classified generation repository of the fixture.
fn generated(f: &Fixture, clock: Arc<dyn Clock>) -> SqliteGeneratedAssetRepository {
    SqliteGeneratedAssetRepository::new(f.uow.clone(), clock)
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
    f: &Fixture,
    now: OffsetDateTime,
    label: &str,
) -> Result<GeneratedReservation, IntentError> {
    let content = pdf(label);
    f.uow
        .transaction(options(), async |tx| {
            let reservation = insert_asset(tx, f.root, now, &content, false)?;
            activate(tx, reservation.asset.asset_id(), now)?;
            Ok(reservation)
        })
        .await
}

/// Creates one ACTIVE classified asset with its pending intent, committed in one transaction.
async fn classified_claim(
    f: &Fixture,
    descriptor: &GenerationDescriptor,
    now: OffsetDateTime,
) -> Result<IntentReservation, IntentError> {
    let content = pdf("classified");
    f.uow
        .transaction(options(), async |tx| {
            let reservation = insert_asset(tx, f.root, now, &content, true)?;
            let claim =
                insert_generation_intent(tx, &reservation, descriptor, Uuid::new_v4(), now).await?;
            activate(tx, reservation.asset.asset_id(), now + Duration::seconds(1))?;
            Ok(claim)
        })
        .await
}

/// The descriptor of a profile resume claim, as the replacement computes it.
fn profile_descriptor(
    profile: Uuid,
    resume_type: &str,
) -> Result<GenerationDescriptor, IntentError> {
    GenerationDescriptor::new(
        Uuid::new_v4(),
        GenerationPurpose::ProfileResume,
        Some(profile),
        None,
        None,
        Some(resume_type.to_owned()),
    )
    .map_err(IntentError::from)
}

/// A synthetic current link to `binding`, with the given timestamps.
fn link(
    profile: Uuid,
    binding: &GeneratedReservation,
    resume_type: &str,
    created: OffsetDateTime,
    updated: OffsetDateTime,
) -> Result<ProfileResumeDocument, BoxError> {
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

/// Saves the first (master) link of the profile to `binding` at owner revision zero.
async fn save_first(
    f: &Fixture,
    writer: &SqliteProfileResumeLinks,
    binding: &GeneratedReservation,
) -> Result<ProfileResumeDocument, BoxError> {
    let candidate = link(
        f.profile,
        binding,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(2),
    )?;
    Ok(writer
        .replace(&candidate, binding, 0, f.base, None)
        .await?
        .saved)
}

type State = TableState;

fn state(f: &Fixture) -> Result<State, BoxError> {
    table_state(&f.sqlite.connection(), &STATE_TABLES)
}

/// The rows of one table in a [`State`].
fn rows<'a>(state: &'a State, table: &str) -> &'a [String] {
    state
        .iter()
        .find(|(name, _)| *name == table)
        .map_or(&[], |(_, rows)| rows.as_slice())
}

/// The lifecycle and the cleanup task status of one generated asset.
fn asset_state(f: &Fixture, asset: Uuid) -> Result<(String, Option<String>), BoxError> {
    Ok(f.sqlite.connection().query_row(
        "SELECT asset.lifecycle, task.status FROM jobniac_generated_assets asset \
         LEFT JOIN jobniac_generated_asset_cleanups task ON task.asset_id = asset.asset_id \
         WHERE asset.asset_id = ?1",
        [asset.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

fn task_count(f: &Fixture, asset: Uuid) -> Result<i64, BoxError> {
    count(
        &f.sqlite.connection(),
        &format!(
            "SELECT count(*) FROM jobniac_generated_asset_cleanups WHERE asset_id = '{asset}'"
        ),
    )
}

/// The state of the intent of one asset.
async fn intent_state(f: &Fixture, asset: Uuid) -> Result<GenerationState, BoxError> {
    let intent = f
        .uow
        .transaction(options(), async |tx| {
            find_generation_intent(tx, asset, false).await
        })
        .await?
        .ok_or("the intent must exist")?;
    Ok(intent.state)
}

/// The stored resume link of one (profile, type) pair.
async fn current(
    links: &SqliteProfileResumeLinks,
    profile: Uuid,
    resume_type: &str,
) -> Result<Option<ProfileResumeDocument>, BoxError> {
    Ok(links
        .find_by_profile_id_and_resume_type(profile, resume_type)
        .await?)
}

/// The code of any refused call, through its envelope, or a test failure when it was accepted.
fn code_of(result: Result<impl std::fmt::Debug, AppError>) -> Result<ErrorCode, BoxError> {
    match result {
        Err(error) => Ok(error_envelope(&error).code),
        Ok(value) => Err(format!("the call was accepted: {value:?}").into()),
    }
}

/// Asserts that a failed write carried nothing of the driver error.
fn assert_sanitized<T: std::fmt::Debug>(result: Result<T, AppError>) -> TestResult {
    match result {
        Err(error) => {
            assert!(
                !format!("{error:?}").contains("private"),
                "driver text leaked: {error:?}"
            );
            Ok(())
        }
        Ok(value) => Err(format!("the call was accepted: {value:?}").into()),
    }
}

// ---------------------------------------------------------------------------------------------
// The writer that commits first, and the writer that re-reads state
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn cleanup_before_link_wins_and_preserves_the_previous_link() -> TestResult {
    let f = start();
    let writer = links(&f, fixed(f.base));
    let old = active_asset(&f, f.base, "old").await?;
    let new = active_asset(&f, f.base, "new").await?;
    let previous = save_first(&f, &writer, &old).await?;
    let candidate = link(
        f.profile,
        &new,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(2),
    )?;
    // The retirement commits first and takes the unreferenced new asset out of service.
    let prepared = cleanup(&f, fixed(f.base))
        .prepare_cleanup(new.asset.asset_id())
        .await?;
    assert_eq!(prepared, PreparationOutcome::Prepared);
    let committed = state(&f)?;
    // The link writer comes second, re-reads the asset and is refused by its state.
    let refused = writer.replace(&candidate, &new, 0, f.base, None).await;
    assert_eq!(code_of(refused)?, ErrorCode::ValidationError);
    assert_eq!(state(&f)?, committed);
    assert_eq!(current(&writer, f.profile, MASTER).await?, Some(previous));
    assert_eq!(lifecycle(&f.sqlite, old.asset.asset_id())?, "ACTIVE");
    assert_eq!(lifecycle(&f.sqlite, new.asset.asset_id())?, "RETIRING");
    Ok(())
}

#[tokio::test]
async fn link_before_cleanup_wins() -> TestResult {
    let f = start();
    let writer = links(&f, fixed(f.base));
    let binding = active_asset(&f, f.base, "bound").await?;
    let candidate = link(
        f.profile,
        &binding,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(2),
    )?;
    writer
        .replace(&candidate, &binding, 0, f.base, None)
        .await?;
    // The cleanup comes second, re-reads the references and finds the link.
    let prepared = cleanup(&f, fixed(f.base))
        .prepare_cleanup(binding.asset.asset_id())
        .await?;
    assert_eq!(prepared, PreparationOutcome::Referenced);
    assert_eq!(lifecycle(&f.sqlite, binding.asset.asset_id())?, "ACTIVE");
    assert_eq!(task_count(&f, binding.asset.asset_id())?, 0);
    Ok(())
}

#[tokio::test]
async fn renewal_commit_fences_a_link_waiting_for_the_classified_binding() -> TestResult {
    let f = start();
    let old = active_asset(&f, f.base, "old").await?;
    let writer = links(&f, fixed(f.base));
    let previous = save_first(&f, &writer, &old).await?;
    let claim = classified_claim(&f, &profile_descriptor(f.profile, MASTER)?, f.base).await?;
    let candidate = link(
        f.profile,
        &claim.reservation,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(3),
    )?;
    // The renewal commits first and rotates the fence.
    let renewed = generated(&f, fixed(f.base + Duration::seconds(1)))
        .renew_generation(&claim, Uuid::new_v4())
        .await?
        .ok_or("the live claim must renew")?;
    assert_ne!(renewed.intent.lease_token, claim.intent.lease_token);
    let committed = state(&f)?;
    // The link writer holding the superseded fence is refused on its re-read, and writes nothing.
    let refused = links(&f, fixed(f.base + Duration::seconds(1)))
        .replace(&candidate, &claim.reservation, 0, f.base, Some(&claim))
        .await;
    assert_eq!(code_of(refused)?, ErrorCode::ValidationError);
    assert_eq!(state(&f)?, committed);
    assert_eq!(current(&writer, f.profile, MASTER).await?, Some(previous));
    assert_eq!(lifecycle(&f.sqlite, old.asset.asset_id())?, "ACTIVE");
    assert_eq!(lifecycle(&f.sqlite, claim.asset_id())?, "ACTIVE");
    assert_eq!(task_count(&f, claim.asset_id())?, 0);
    let stored = f
        .uow
        .transaction(options(), async |tx| {
            find_generation_intent(tx, claim.asset_id(), false).await
        })
        .await?
        .ok_or("the renewed intent must exist")?;
    assert_eq!(stored, renewed.intent);
    // The renewed claim holds the current fence and still completes the replacement.
    links(&f, fixed(f.base + Duration::seconds(1)))
        .replace(&candidate, &claim.reservation, 0, f.base, Some(&renewed))
        .await?;
    assert_eq!(
        intent_state(&f, claim.asset_id()).await?,
        GenerationState::Completed
    );
    Ok(())
}

#[tokio::test]
async fn classified_link_completion_wins_over_an_expiry_recovery_that_runs_meanwhile() -> TestResult
{
    let f = start();
    let claim = classified_claim(&f, &profile_descriptor(f.profile, MASTER)?, f.base).await?;
    let expires = claim
        .intent
        .lease_expires_at
        .ok_or("a pending intent has a lease")?;
    let writer = links(&f, fixed(f.base + Duration::seconds(1)));
    let candidate = link(
        f.profile,
        &claim.reservation,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(3),
    )?;
    // The link commits and completes the intent first.
    let saved = writer
        .replace(&candidate, &claim.reservation, 0, f.base, Some(&claim))
        .await?
        .saved;
    // The recovery runs after the lease has expired, re-reads the completed intent and must not
    // resurrect or abandon the claim.
    let swept = generated(&f, fixed(expires))
        .recover_expired_generations(f.root, RETRY_BATCH_SIZE)
        .await?;
    assert_eq!(swept, Vec::<Uuid>::new());
    assert_eq!(current(&writer, f.profile, MASTER).await?, Some(saved));
    assert_eq!(
        cleanup(&f, fixed(f.base))
            .prepare_cleanup(claim.asset_id())
            .await?,
        PreparationOutcome::Referenced
    );
    assert_eq!(lifecycle(&f.sqlite, claim.asset_id())?, "ACTIVE");
    assert_eq!(task_count(&f, claim.asset_id())?, 0);
    assert_eq!(
        intent_state(&f, claim.asset_id()).await?,
        GenerationState::Completed
    );
    Ok(())
}

#[tokio::test]
async fn expiry_recovery_commit_fences_a_waiting_link_without_resurrection() -> TestResult {
    let f = start();
    let old = active_asset(&f, f.base, "old").await?;
    let writer = links(&f, fixed(f.base));
    let previous = save_first(&f, &writer, &old).await?;
    let claim = classified_claim(&f, &profile_descriptor(f.profile, MASTER)?, f.base).await?;
    let expires = claim
        .intent
        .lease_expires_at
        .ok_or("a pending intent has a lease")?;
    let candidate = link(
        f.profile,
        &claim.reservation,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(3),
    )?;
    // The recovery commits first: it abandons the claim and retires its asset.
    let swept = generated(&f, fixed(expires))
        .recover_expired_generations(f.root, RETRY_BATCH_SIZE)
        .await?;
    assert_eq!(swept, vec![claim.asset_id()]);
    let committed = state(&f)?;
    // The link writer comes second, with a clock that still reads the lease as live, and is
    // refused by the committed state: nothing is resurrected.
    let refused = links(&f, fixed(f.base + Duration::seconds(1)))
        .replace(&candidate, &claim.reservation, 0, f.base, Some(&claim))
        .await;
    assert_eq!(code_of(refused)?, ErrorCode::ValidationError);
    assert_eq!(state(&f)?, committed);
    assert_eq!(current(&writer, f.profile, MASTER).await?, Some(previous));
    assert_eq!(lifecycle(&f.sqlite, old.asset.asset_id())?, "ACTIVE");
    assert_eq!(
        asset_state(&f, claim.asset_id())?,
        ("RETIRING".to_owned(), Some("PENDING".to_owned()))
    );
    assert_eq!(
        intent_state(&f, claim.asset_id()).await?,
        GenerationState::Abandoned
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Refusals and rollbacks that leave every table untouched
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_document_and_path_cannot_be_shared_with_a_second_profile() -> TestResult {
    let f = start();
    let writer = links(&f, fixed(f.base));
    let binding = active_asset(&f, f.base, "shared").await?;
    let previous = save_first(&f, &writer, &binding).await?;
    let other = Uuid::new_v4();
    seed_profile(&f.sqlite.connection(), other, 0, f.base)?;
    let before = state(&f)?;

    // A raw insert of a second link to the same document and path hits the unique indexes.
    let raw = f.sqlite.connection().execute(
        "INSERT INTO profile_profile_resume_documents (id, profile_id, document_id, file_path, \
         resume_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'master_resume', ?5, ?5)",
        params![
            Uuid::new_v4().to_string(),
            other.to_string(),
            binding.document_id.to_string(),
            binding.asset.relative_path(),
            stored(f.base)
        ],
    );
    assert!(
        matches!(
            raw,
            Err(rusqlite::Error::SqliteFailure(ref error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation
        ),
        "{raw:?}"
    );

    // The repository refuses the same sharing and rolls back.
    let candidate = link(
        other,
        &binding,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(2),
    )?;
    let refused = writer.replace(&candidate, &binding, 0, f.base, None).await;
    assert!(refused.is_err(), "{refused:?}");

    assert_eq!(state(&f)?, before);
    assert_eq!(current(&writer, f.profile, MASTER).await?, Some(previous));
    assert_eq!(current(&writer, other, MASTER).await?, None);
    assert_eq!(lifecycle(&f.sqlite, binding.asset.asset_id())?, "ACTIVE");
    assert_eq!(task_count(&f, binding.asset.asset_id())?, 0);
    let outcome = cleanup(&f, fixed(f.base))
        .prepare_cleanup(binding.asset.asset_id())
        .await?;
    assert_eq!(outcome, PreparationOutcome::Referenced);
    Ok(())
}

#[tokio::test]
async fn a_previous_link_with_an_unowned_path_is_replaced_without_touching_its_storage()
-> TestResult {
    let f = start();
    let owned = active_asset(&f, f.base, "owned").await?;
    let conn = f.sqlite.connection();
    let (blob, document, legacy) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let content = b"%PDF-test".to_vec();
    conn.execute(
        "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
         VALUES (?1, ?2, 9, ?3, ?4)",
        params![blob.to_string(), "1".repeat(64), content, stored(f.base)],
    )?;
    conn.execute(
        "INSERT INTO document_documents (id, blob_id, original_file_name, media_type, \
         created_at, updated_at) VALUES (?1, ?2, 'legacy.pdf', 'application/pdf', ?3, ?3)",
        params![document.to_string(), blob.to_string(), stored(f.base)],
    )?;
    conn.execute(
        "INSERT INTO profile_profile_resume_documents (id, profile_id, document_id, file_path, \
         resume_type, created_at, updated_at) \
         VALUES (?1, ?2, ?3, 'legacy/person/resume.pdf', 'master_resume', ?4, ?4)",
        params![
            legacy.to_string(),
            f.profile.to_string(),
            document.to_string(),
            stored(f.base)
        ],
    )?;
    let before = state(&f)?;
    let writer = links(&f, fixed(f.base));
    let candidate = link(
        f.profile,
        &owned,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(2),
    )?;
    let result = writer.replace(&candidate, &owned, 0, f.base, None).await?;
    assert_eq!(result.saved.id, legacy);
    assert_eq!(result.saved.file_path, owned.asset.relative_path());
    let previous = result
        .previous
        .ok_or("the legacy link must be the previous one")?;
    assert_eq!(previous.file_path, "legacy/person/resume.pdf");
    let after = state(&f)?;
    for table in [
        "document_documents",
        "document_blobs",
        "jobniac_generated_assets",
        "jobniac_generated_asset_cleanups",
    ] {
        assert_eq!(rows(&after, table), rows(&before, table), "{table}");
    }
    Ok(())
}

#[tokio::test]
async fn reserved_leftover_and_dropped_replacement_leave_the_old_link_and_asset_untouched()
-> TestResult {
    let f = start();
    let old = active_asset(&f, f.base, "old").await?;
    let writer = links(&f, fixed(f.base));
    let previous = save_first(&f, &writer, &old).await?;

    // A replacement whose transaction fails after it rewrote the link and retired the old asset
    // rolls back completely: the old link and the ACTIVE asset are restored.
    let dropped_content = pdf("dropped");
    let at = f.base + Duration::seconds(3);
    let abandoned = f
        .uow
        .transaction(options(), async |tx| {
            let reservation = insert_asset(tx, f.root, f.base, &dropped_content, false)?;
            activate(tx, reservation.asset.asset_id(), f.base)?;
            tx.run(|conn| {
                conn.execute(
                    "UPDATE profile_profile_resume_documents SET document_id = ?1, \
                     file_path = ?2, updated_at = ?3 WHERE profile_id = ?4 AND resume_type = ?5",
                    params![
                        reservation.document_id.to_string(),
                        reservation.asset.relative_path(),
                        stored(at),
                        f.profile.to_string(),
                        MASTER
                    ],
                )?;
                Ok(())
            })
            .map_err(fixture_failure)?;
            let row = tx
                .run(|conn| {
                    Ok(conn.query_row(
                        &format!(
                            "SELECT {ASSET_COLUMNS} FROM jobniac_generated_assets WHERE asset_id = ?1"
                        ),
                        [old.asset.asset_id().to_string()],
                        |row| Ok(AssetRow::from_row(row)),
                    )?)
                })
                .map_err(fixture_failure)??;
            retire_asset(tx, &row, at, false, None).await?;
            Err::<(), IntentError>(IntentError::Unavailable)
        })
        .await;
    assert!(matches!(abandoned, Err(IntentError::Unavailable)));
    assert_eq!(
        current(&writer, f.profile, MASTER).await?,
        Some(previous.clone())
    );
    assert_eq!(lifecycle(&f.sqlite, old.asset.asset_id())?, "ACTIVE");
    assert_eq!(task_count(&f, old.asset.asset_id())?, 0);

    // A RESERVED asset left by a committed reserve whose activation never ran is not a binding.
    let leftover = f
        .uow
        .transaction(options(), async |tx| {
            insert_asset(tx, f.root, f.base, &pdf("leftover"), false)
        })
        .await?;
    let candidate = link(
        f.profile,
        &leftover,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(4),
    )?;
    let refused = writer.replace(&candidate, &leftover, 0, f.base, None).await;
    assert_eq!(code_of(refused)?, ErrorCode::ValidationError);
    assert_eq!(current(&writer, f.profile, MASTER).await?, Some(previous));
    assert_eq!(lifecycle(&f.sqlite, old.asset.asset_id())?, "ACTIVE");
    assert_eq!(lifecycle(&f.sqlite, leftover.asset.asset_id())?, "RESERVED");
    Ok(())
}

#[tokio::test]
async fn source_preconditions_preserve_links_and_assets() -> TestResult {
    // A changed owner revision, a changed owner timestamp and a missing owner.
    for change in ["numeric", "timestamp", "missing"] {
        let f = start();
        let writer = links(&f, fixed(f.base));
        let old = active_asset(&f, f.base, "old").await?;
        let new = active_asset(&f, f.base, "new").await?;
        save_first(&f, &writer, &old).await?;
        let conn = f.sqlite.connection();
        let candidate_profile = if change == "missing" {
            Uuid::new_v4()
        } else {
            f.profile
        };
        match change {
            "numeric" => {
                conn.execute(
                    "UPDATE profile_profiles SET revision = 1 WHERE id = ?1",
                    [f.profile.to_string()],
                )?;
            }
            "timestamp" => {
                conn.execute(
                    "UPDATE profile_profiles SET updated_at = ?2 WHERE id = ?1",
                    params![f.profile.to_string(), stored(f.base + Duration::seconds(1))],
                )?;
            }
            _ => {}
        }
        let candidate = link(
            candidate_profile,
            &new,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(2),
        )?;
        let before = state(&f)?;
        let expected = if change == "missing" {
            ErrorCode::NotFound
        } else {
            ErrorCode::Conflict
        };
        let result = writer.replace(&candidate, &new, 0, f.base, None).await;
        assert_eq!(code_of(result)?, expected, "{change}");
        assert_eq!(state(&f)?, before, "{change}");
        assert_eq!(lifecycle(&f.sqlite, old.asset.asset_id())?, "ACTIVE");
        assert_eq!(lifecycle(&f.sqlite, new.asset.asset_id())?, "ACTIVE");
    }
    Ok(())
}

#[tokio::test]
async fn classified_profile_link_requires_scoped_live_pending_proof() -> TestResult {
    // A missing proof, an expired lease, a proof for another scope and a superseded token are each
    // refused, and the pending intent is left as it was.
    for failure in ["missing_proof", "expired", "wrong_scope", "wrong_token"] {
        let f = start();
        let old = active_asset(&f, f.base, "old").await?;
        save_first(&f, &links(&f, fixed(f.base)), &old).await?;
        let scope = if failure == "wrong_scope" {
            CANADIAN
        } else {
            MASTER
        };
        let claim = classified_claim(&f, &profile_descriptor(f.profile, scope)?, f.base).await?;
        let expires = claim
            .intent
            .lease_expires_at
            .ok_or("a pending intent has a lease")?;
        let clock = if failure == "expired" {
            fixed(expires)
        } else {
            fixed(f.base + Duration::seconds(1))
        };
        let writer = links(&f, clock);
        let mut proof_claim = claim.clone();
        if failure == "wrong_token" {
            proof_claim.intent.lease_token = Some(Uuid::new_v4());
        }
        let proof = if failure == "missing_proof" {
            None
        } else {
            Some(&proof_claim)
        };
        let candidate = link(
            f.profile,
            &claim.reservation,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(3),
        )?;
        let before = state(&f)?;
        let result = writer
            .replace(&candidate, &claim.reservation, 0, f.base, proof)
            .await;
        assert_eq!(code_of(result)?, ErrorCode::ValidationError, "{failure}");
        assert_eq!(state(&f)?, before, "{failure}");
        assert_eq!(
            intent_state(&f, claim.asset_id()).await?,
            GenerationState::Pending,
            "{failure}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn classified_completion_failure_rolls_back_the_link_and_the_retirement() -> TestResult {
    // An expiry after the link is written, and a failure of the intent completion, are both
    // rolled back.
    for failure in ["expiry_after_link", "completion_failure"] {
        let f = start();
        let old = active_asset(&f, f.base, "old").await?;
        save_first(&f, &links(&f, fixed(f.base)), &old).await?;
        let claim = classified_claim(&f, &profile_descriptor(f.profile, MASTER)?, f.base).await?;
        let expires = claim
            .intent
            .lease_expires_at
            .ok_or("a pending intent has a lease")?;
        let clock: Arc<dyn Clock> = if failure == "expiry_after_link" {
            // The replacement reads the clock for admission, retirement and completion.
            TickClock::new(vec![
                f.base + Duration::seconds(1),
                f.base + Duration::seconds(1),
                expires,
            ])
        } else {
            fixed(f.base + Duration::seconds(1))
        };
        if failure == "completion_failure" {
            fail_at(
                &f.sqlite.connection(),
                "jobniac_generation_intents",
                "UPDATE",
                "NEW.state = 'COMPLETED'",
            )?;
        }
        let writer = links(&f, clock);
        let candidate = link(
            f.profile,
            &claim.reservation,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(3),
        )?;
        let before = state(&f)?;
        let result = writer
            .replace(&candidate, &claim.reservation, 0, f.base, Some(&claim))
            .await;
        if failure == "expiry_after_link" {
            assert_eq!(code_of(result)?, ErrorCode::ValidationError);
        } else {
            assert_sanitized(result)?;
        }
        assert_eq!(state(&f)?, before, "{failure}");
        assert_eq!(lifecycle(&f.sqlite, old.asset.asset_id())?, "ACTIVE");
        assert_eq!(lifecycle(&f.sqlite, claim.asset_id())?, "ACTIVE");
        assert_eq!(
            intent_state(&f, claim.asset_id()).await?,
            GenerationState::Pending,
            "{failure}"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Real contention on one file
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn competing_creation_serializes_and_preserves_the_first_identity() -> TestResult {
    let f = start();
    let first = active_asset(&f, f.base, "first").await?;
    let second = active_asset(&f, f.base, "second").await?;
    let at = |seconds| f.base + Duration::seconds(seconds);
    let one = link(f.profile, &first, MASTER, at(1), at(2))?;
    let two = link(f.profile, &second, MASTER, at(1), at(2))?;
    // The first writer stops after it wrote the link and before it commits, holding the write
    // lock. The second one runs on another pool of the same file, so it queues behind the file
    // lock.
    let (clock, mut pause) = PauseClock::new(f.base, 2);
    let first_writer = tokio::spawn({
        let writer = links(&f, clock);
        let (one, first, base) = (one.clone(), first.clone(), f.base);
        async move { writer.replace(&one, &first, 0, base, None).await }
    });
    pause.reached().await;
    let second_writer = tokio::spawn({
        let writer = links_on_second_pool(&f, fixed(f.base));
        let (two, second, base) = (two.clone(), second.clone(), f.base);
        async move { writer.replace(&two, &second, 0, base, None).await }
    });
    assert_still_waiting(&second_writer, "the second creation").await;
    // The link is not committed yet, so a reader sees no link at all.
    assert_eq!(
        current(&links(&f, fixed(f.base)), f.profile, MASTER).await?,
        None
    );
    pause.release();
    let created = first_writer.await??;
    let replaced = second_writer.await??;
    assert_eq!(created.saved, one);
    assert_eq!(replaced.previous, Some(one.clone()));
    assert_eq!(replaced.saved.id, one.id);
    assert_eq!(replaced.saved.document_id, second.document_id);
    assert_eq!(
        count(
            &f.sqlite.connection(),
            "SELECT count(*) FROM profile_profile_resume_documents"
        )?,
        1
    );
    assert_eq!(lifecycle(&f.sqlite, first.asset.asset_id())?, "RETIRING");
    assert_eq!(lifecycle(&f.sqlite, second.asset.asset_id())?, "ACTIVE");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_replaces_of_one_profile_serialize_to_one_current_link() -> TestResult {
    let f = start();
    let old = active_asset(&f, f.base, "old").await?;
    let first = active_asset(&f, f.base, "first").await?;
    let second = active_asset(&f, f.base, "second").await?;
    save_first(&f, &links(&f, fixed(f.base)), &old).await?;
    let at = |seconds| f.base + Duration::seconds(seconds);
    let one = link(f.profile, &first, MASTER, at(1), at(3))?;
    let two = link(f.profile, &second, MASTER, at(1), at(3))?;
    // Both writers hold the same owner revision. The first stops inside its transaction; the
    // second one, on the same pool, queues behind the single writer. Equal timestamps are admitted,
    // so the second reads the first's committed link as its previous link.
    let (clock, mut pause) = PauseClock::new(f.base, 2);
    let first_writer = tokio::spawn({
        let writer = links(&f, clock);
        let (one, first, base) = (one.clone(), first.clone(), f.base);
        async move { writer.replace(&one, &first, 0, base, None).await }
    });
    pause.reached().await;
    let second_writer = tokio::spawn({
        let writer = links(&f, fixed(f.base));
        let (two, second, base) = (two.clone(), second.clone(), f.base);
        async move { writer.replace(&two, &second, 0, base, None).await }
    });
    assert_still_waiting(&second_writer, "the second replacement").await;
    pause.release();
    let accepted_first = first_writer.await??;
    let accepted_second = second_writer.await??;
    assert_eq!(accepted_first.saved.document_id, first.document_id);
    assert_eq!(
        accepted_second
            .previous
            .as_ref()
            .map(|link| link.document_id),
        Some(first.document_id)
    );
    let stored = current(&links(&f, fixed(f.base)), f.profile, MASTER)
        .await?
        .ok_or("one current link must remain")?;
    assert_eq!(stored.document_id, second.document_id);
    assert_eq!(
        count(
            &f.sqlite.connection(),
            "SELECT count(*) FROM profile_profile_resume_documents"
        )?,
        1
    );
    for retired in [&old, &first] {
        assert_eq!(lifecycle(&f.sqlite, retired.asset.asset_id())?, "RETIRING");
        assert_eq!(task_count(&f, retired.asset.asset_id())?, 1);
    }
    assert_eq!(lifecycle(&f.sqlite, second.asset.asset_id())?, "ACTIVE");
    assert_eq!(task_count(&f, second.asset.asset_id())?, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profile_edit_and_deletion_wait_for_the_writer_root_lock() -> TestResult {
    // The link writer holds the file lock while it is paused inside its transaction; an edit or a
    // deletion of the profile on its own connection must wait for it.
    for action in ["edit", "delete"] {
        let f = start();
        let binding = active_asset(&f, f.base, "bound").await?;
        let candidate = link(
            f.profile,
            &binding,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(2),
        )?;
        let sql = if action == "edit" {
            "UPDATE profile_profiles SET revision = revision + 1 WHERE id = ?1"
        } else {
            "DELETE FROM profile_profiles WHERE id = ?1"
        };
        let (clock, mut pause) = PauseClock::new(f.base, 2);
        let writer = links(&f, clock);
        let task = tokio::spawn({
            let writer = writer.clone();
            let (candidate, binding, base) = (candidate.clone(), binding.clone(), f.base);
            async move { writer.replace(&candidate, &binding, 0, base, None).await }
        });
        pause.reached().await;
        let change = spawn_raw_write(f.sqlite.path(), sql, f.profile.to_string());
        assert_still_waiting(&change, action).await;
        // The reader still sees the committed profile, unchanged.
        let revision: i64 = raw_connection(f.sqlite.path()).query_row(
            "SELECT revision FROM profile_profiles WHERE id = ?1",
            [f.profile.to_string()],
            |row| row.get(0),
        )?;
        assert_eq!(revision, 0, "{action}");
        pause.release();
        task.await??;
        assert_eq!(change.await??, 1, "{action}");
        let remaining: Option<i64> = raw_connection(f.sqlite.path())
            .query_row(
                "SELECT revision FROM profile_profiles WHERE id = ?1",
                [f.profile.to_string()],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        if action == "edit" {
            assert_eq!(remaining, Some(1));
        } else {
            assert_eq!(remaining, None, "the deleted profile must be gone");
        }
        // The link writer committed against the revision it read; a later attempt with the same
        // revision is refused by what the other writer committed.
        let later = writer.replace(&candidate, &binding, 0, f.base, None).await;
        let expected = if action == "edit" {
            ErrorCode::Conflict
        } else {
            ErrorCode::NotFound
        };
        assert_eq!(code_of(later)?, expected, "{action}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waiting_writer_rechecks_the_committed_owner_after_the_wait() -> TestResult {
    // The writer waits behind a held write lock whose owner then edits or deletes the profile. It
    // must read the owner as committed once the lock is free, not as it was before the wait.
    for action in ["edit", "delete"] {
        let f = start();
        let binding = active_asset(&f, f.base, "bound").await?;
        let writer = links(&f, fixed(f.base));
        let hold = WriteHold::begin(f.sqlite.path());
        let candidate = link(
            f.profile,
            &binding,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(2),
        )?;
        let task = tokio::spawn({
            let writer = writer.clone();
            let (candidate, binding, base) = (candidate.clone(), binding.clone(), f.base);
            async move { writer.replace(&candidate, &binding, 0, base, None).await }
        });
        assert_still_waiting(&task, "the link writer").await;
        let change = if action == "edit" {
            "UPDATE profile_profiles SET revision = 1 WHERE id = ?1"
        } else {
            "DELETE FROM profile_profiles WHERE id = ?1"
        };
        hold.connection().execute(change, [f.profile.to_string()])?;
        hold.commit();
        let expected = if action == "edit" {
            ErrorCode::Conflict
        } else {
            ErrorCode::NotFound
        };
        assert_eq!(code_of(task.await?)?, expected, "{action}");
        assert_eq!(
            count(
                &f.sqlite.connection(),
                "SELECT count(*) FROM profile_profile_resume_documents"
            )?,
            0
        );
        assert_eq!(lifecycle(&f.sqlite, binding.asset.asset_id())?, "ACTIVE");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn readers_see_the_old_state_until_the_link_and_retirement_commit() -> TestResult {
    let f = start();
    let old = active_asset(&f, f.base, "old").await?;
    let new = active_asset(&f, f.base, "new").await?;
    let reader = links(&f, fixed(f.base));
    let previous = save_first(&f, &reader, &old).await?;
    let before = state(&f)?;
    let candidate = link(
        f.profile,
        &new,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(3),
    )?;
    // The writer stops after it wrote the new link and before it retires the old asset: its rows
    // are uncommitted, so a WAL reader still sees the old state.
    let (clock, mut pause) = PauseClock::new(f.base, 2);
    let task = tokio::spawn({
        let writer = links(&f, clock);
        let (candidate, new, base) = (candidate.clone(), new.clone(), f.base);
        async move { writer.replace(&candidate, &new, 0, base, None).await }
    });
    pause.reached().await;
    assert_eq!(state(&f)?, before);
    assert_eq!(
        current(&reader, f.profile, MASTER).await?,
        Some(previous.clone())
    );
    assert_eq!(lifecycle(&f.sqlite, old.asset.asset_id())?, "ACTIVE");
    pause.release();
    let result = task.await??;
    assert_eq!(result.previous, Some(previous));
    assert_eq!(
        current(&reader, f.profile, MASTER).await?,
        Some(result.saved)
    );
    assert_eq!(lifecycle(&f.sqlite, old.asset.asset_id())?, "RETIRING");
    assert_eq!(task_count(&f, old.asset.asset_id())?, 1);
    Ok(())
}
