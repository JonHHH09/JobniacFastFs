//! Atomic profile and job hard delete over a migrated temporary SQLite file.
//!
//! Each test works on its own database. All data is synthetic.

mod sqlite_german_support;
mod sqlite_support;

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use jobniac_core::cleanup::{CleanupExecutor, OwnedAssetRemover, RemovalFailure, RemovalOutcome};
use jobniac_core::clock::{Clock, FixedClock};
use jobniac_core::deletion::{EntityDeletion, JobDeletionRepository, ProfileDeletionRepository};
use jobniac_core::errors::{ErrorCode, error_envelope};
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedReservation, GenerationDescriptor, GenerationPurpose,
    ProfileResumeDocument, sha256_hex,
};
use jobniac_infra::sqlite::deletion::SqliteEntityDeletionRepository;
use jobniac_infra::sqlite::generated::intents::insert_generation_intent;
use jobniac_infra::sqlite::generated::profile_links::SqliteProfileResumeLinks;
use jobniac_infra::sqlite::pool::{PoolOptions, SqlitePool};
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use rusqlite::{Connection, params};
use sqlite_german_support::{
    Fixture, TestResult, activate, cleanup_repository, fail_at, insert_asset, insert_owners, later,
    now, options, pdf, resume_write, save_resume, start, stored,
};
use uuid::Uuid;

type BoxError = Box<dyn Error + Send + Sync>;

/// The tables a refused or rolled-back deletion must leave untouched.
const HISTORY: [&str; 4] = [
    "profile_profiles",
    "resume_resumes",
    "resume_resume_variants",
    "match_reports",
];

/// The repository of one fixture, stamping retirements with a fixed instant.
fn repository(fixture: &Fixture) -> SqliteEntityDeletionRepository {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(later()));
    SqliteEntityDeletionRepository::new(fixture.uow.clone(), clock)
}

/// The number of rows of `table`.
fn count(admin: &Connection, table: &str) -> Result<i64, BoxError> {
    Ok(
        admin.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })?,
    )
}

/// The lifecycle of an asset and the status of its cleanup task, if any.
fn state(admin: &Connection, asset: Uuid) -> Result<(String, Option<String>), BoxError> {
    Ok(admin.query_row(
        "SELECT asset.lifecycle, task.status FROM jobniac_generated_assets asset \
         LEFT JOIN jobniac_generated_asset_cleanups task USING (asset_id) \
         WHERE asset.asset_id = ?1",
        [asset.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

/// The state a retired asset with one pending cleanup task has.
fn retired() -> (String, Option<String>) {
    ("RETIRING".to_owned(), Some("PENDING".to_owned()))
}

/// The state an asset in use has.
fn active() -> (String, Option<String>) {
    ("ACTIVE".to_owned(), None)
}

/// Creates one ACTIVE owned asset in the fixture root.
async fn owned_asset(fixture: &Fixture, label: &str) -> Result<GeneratedReservation, BoxError> {
    let content = pdf(label);
    let binding = fixture
        .uow
        .transaction(options(), async |tx| {
            let binding = insert_asset(tx, fixture.root, now(), &content, false)?;
            activate(tx, binding.asset.asset_id())?;
            Ok::<_, jobniac_infra::sqlite::generated::IntentError>(binding)
        })
        .await?;
    Ok(binding)
}

/// Inserts a profile resume link of `profile` to `binding` (the master resume).
fn link(admin: &Connection, profile: Uuid, binding: &GeneratedReservation) -> Result<(), BoxError> {
    admin.execute(
        "INSERT INTO profile_profile_resume_documents (id, profile_id, document_id, file_path, \
         resume_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'master_resume', ?5, ?5)",
        params![
            Uuid::new_v4().to_string(),
            profile.to_string(),
            binding.document_id.to_string(),
            binding.asset.relative_path(),
            stored(now())
        ],
    )?;
    Ok(())
}

/// Attaches `binding` as the photo of `profile`.
fn photo(
    admin: &Connection,
    profile: Uuid,
    binding: &GeneratedReservation,
) -> Result<(), BoxError> {
    photo_of(admin, profile, binding.document_id)
}

/// Attaches the document `file` as the photo of `profile`.
fn photo_of(admin: &Connection, profile: Uuid, file: Uuid) -> Result<(), BoxError> {
    admin.execute(
        "INSERT INTO profile_profile_personal_details (profile_id, photo_document_id, \
         created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![profile.to_string(), file.to_string(), stored(now())],
    )?;
    Ok(())
}

/// Attaches `binding` as a PDF source of `profile` through an extraction row.
fn pdf_source(
    admin: &Connection,
    profile: Uuid,
    binding: &GeneratedReservation,
) -> Result<(), BoxError> {
    pdf_source_of(admin, profile, binding.document_id)
}

/// Attaches the document `file` as a PDF source of `profile` through an extraction row.
fn pdf_source_of(admin: &Connection, profile: Uuid, file: Uuid) -> Result<(), BoxError> {
    let extraction = Uuid::new_v4();
    admin.execute(
        "INSERT INTO document_pdf_extractions (id, file_id, extractor, character_count, \
         page_count, truncated, extracted_text, created_at) \
         VALUES (?1, ?2, 'synthetic', 1, 1, 0, 'x', ?3)",
        params![extraction.to_string(), file.to_string(), stored(now())],
    )?;
    admin.execute(
        "INSERT INTO profile_profile_pdf_sources (id, profile_id, pdf_extraction_id, \
         source_type, created_at) VALUES (?1, ?2, ?3, 'resume_pdf', ?4)",
        params![
            Uuid::new_v4().to_string(),
            profile.to_string(),
            extraction.to_string(),
            stored(now())
        ],
    )?;
    Ok(())
}

/// Inserts a Germany resume of `profile` for `job` with one variant per binding.
fn resume(
    admin: &Connection,
    profile: Uuid,
    job: Uuid,
    bindings: &[&GeneratedReservation],
) -> Result<Uuid, BoxError> {
    let resume = Uuid::new_v4();
    admin.execute(
        "INSERT INTO resume_resumes (id, profile_id, job_id, format, profile_revision, \
         job_revision, created_at, updated_at) VALUES (?1, ?2, ?3, 'germany', ?4, ?4, ?4, ?4)",
        params![
            resume.to_string(),
            profile.to_string(),
            job.to_string(),
            stored(now())
        ],
    )?;
    for (binding, language) in bindings.iter().zip(["en", "de"]) {
        admin.execute(
            "INSERT INTO resume_resume_variants (id, resume_id, language, document_id, \
             file_path, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                Uuid::new_v4().to_string(),
                resume.to_string(),
                language,
                binding.document_id.to_string(),
                binding.asset.relative_path(),
                stored(now())
            ],
        )?;
    }
    Ok(resume)
}

/// Inserts one matching report of `profile` against `job`.
fn report(admin: &Connection, profile: Uuid, job: Uuid) -> Result<(), BoxError> {
    admin.execute(
        "INSERT INTO match_reports (id, profile_id, job_id, profile_revision, job_revision, \
         algorithm_version, overall_score, confidence, outcome, blocker_mismatch, components, \
         evidence, created_at) VALUES (?1, ?2, ?3, ?4, ?4, 'synthetic', 50, 50, \
         'PARTIAL_MATCH', 0, '[]', '[]', ?4)",
        params![
            Uuid::new_v4().to_string(),
            profile.to_string(),
            job.to_string(),
            stored(now())
        ],
    )?;
    Ok(())
}

/// The bindings of one profile with history: the master link, the photo, a PDF source, the English
/// and German resume variants of the fixture job, and a matching report.
struct World {
    master: GeneratedReservation,
    photo: GeneratedReservation,
    pdf: GeneratedReservation,
    en: GeneratedReservation,
    de: GeneratedReservation,
    /// The Germany resume of the fixture profile and job.
    resume: Uuid,
}

async fn world(fixture: &Fixture) -> Result<World, BoxError> {
    let admin = fixture.admin();
    let master = owned_asset(fixture, "master").await?;
    let photo_asset = owned_asset(fixture, "photo").await?;
    let pdf_asset = owned_asset(fixture, "pdf").await?;
    let en = owned_asset(fixture, "en").await?;
    let de = owned_asset(fixture, "de").await?;
    link(&admin, fixture.profile, &master)?;
    photo(&admin, fixture.profile, &photo_asset)?;
    pdf_source(&admin, fixture.profile, &pdf_asset)?;
    let resume_id = resume(&admin, fixture.profile, fixture.job, &[&en, &de])?;
    report(&admin, fixture.profile, fixture.job)?;
    Ok(World {
        master,
        photo: photo_asset,
        pdf: pdf_asset,
        en,
        de,
        resume: resume_id,
    })
}

#[tokio::test]
async fn profile_deletion_removes_history_and_retires_every_owned_asset_atomically() -> TestResult {
    let fixture = start()?;
    let world = world(&fixture).await?;
    let admin = fixture.admin();
    let documents_before = count(&admin, "document_documents")?;
    let outcome = repository(&fixture).delete_profile(fixture.profile).await?;
    assert!(outcome.deleted);
    let mut expected = [
        world.master.asset.asset_id(),
        world.photo.asset.asset_id(),
        world.pdf.asset.asset_id(),
        world.en.asset.asset_id(),
        world.de.asset.asset_id(),
    ];
    expected.sort_unstable();
    assert_eq!(outcome.retired_asset_ids, expected.to_vec());
    for table in HISTORY {
        assert_eq!(count(&admin, table)?, 0, "{table}");
    }
    for binding in [
        &world.master,
        &world.photo,
        &world.pdf,
        &world.en,
        &world.de,
    ] {
        assert_eq!(state(&admin, binding.asset.asset_id())?, retired());
    }
    assert_eq!(count(&admin, "document_documents")?, documents_before);
    Ok(())
}

#[tokio::test]
async fn job_deletion_removes_job_history_and_preserves_the_profile_link() -> TestResult {
    let fixture = start()?;
    let world = world(&fixture).await?;
    let admin = fixture.admin();
    let outcome = repository(&fixture).delete_job(fixture.job).await?;
    let mut expected = [world.en.asset.asset_id(), world.de.asset.asset_id()];
    expected.sort_unstable();
    assert!(outcome.deleted);
    assert_eq!(outcome.retired_asset_ids, expected.to_vec());
    assert_eq!(count(&admin, "job_jobs")?, 0);
    assert_eq!(count(&admin, "resume_resumes")?, 0);
    assert_eq!(count(&admin, "match_reports")?, 0);
    assert_eq!(count(&admin, "profile_profiles")?, 1);
    assert_eq!(count(&admin, "profile_profile_resume_documents")?, 1);
    assert_eq!(state(&admin, world.en.asset.asset_id())?, retired());
    assert_eq!(state(&admin, world.master.asset.asset_id())?, active());
    Ok(())
}

#[tokio::test]
async fn shared_asset_that_stays_referenced_is_not_retired() -> TestResult {
    let fixture = start()?;
    let world = world(&fixture).await?;
    let admin = fixture.admin();
    let other = other_profile(&admin)?;
    photo(&admin, other, &world.en)?;
    let outcome = repository(&fixture).delete_profile(fixture.profile).await?;
    assert!(
        !outcome
            .retired_asset_ids
            .contains(&world.en.asset.asset_id())
    );
    assert_eq!(state(&admin, world.en.asset.asset_id())?, active());
    assert_eq!(state(&admin, world.master.asset.asset_id())?, retired());
    Ok(())
}

#[tokio::test]
async fn unowned_legacy_document_is_untouched() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let other = other_profile(&admin)?;
    let blob = Uuid::new_v4();
    let document = Uuid::new_v4();
    let content = pdf("legacy");
    admin.execute(
        "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            blob.to_string(),
            sha256_hex(&content),
            i64::try_from(content.len())?,
            content,
            stored(now())
        ],
    )?;
    admin.execute(
        "INSERT INTO document_documents (id, blob_id, original_file_name, media_type, \
         created_at, updated_at) VALUES (?1, ?2, 'legacy.pdf', 'application/pdf', ?3, ?3)",
        params![document.to_string(), blob.to_string(), stored(now())],
    )?;
    admin.execute(
        "INSERT INTO profile_profile_resume_documents (id, profile_id, document_id, file_path, \
         resume_type, created_at, updated_at) VALUES (?1, ?2, ?3, 'legacy/not-owned.pdf', \
         'master_resume', ?4, ?4)",
        params![
            Uuid::new_v4().to_string(),
            other.to_string(),
            document.to_string(),
            stored(now())
        ],
    )?;
    let documents = count(&admin, "document_documents")?;
    let generated = count(&admin, "jobniac_generated_assets")?;
    let outcome = repository(&fixture).delete_profile(other).await?;
    assert_eq!(
        outcome,
        EntityDeletion {
            deleted: true,
            retired_asset_ids: Vec::new(),
        }
    );
    assert_eq!(count(&admin, "document_documents")?, documents);
    assert_eq!(count(&admin, "jobniac_generated_assets")?, generated);
    Ok(())
}

/// Inserts a second synthetic profile and returns its id.
fn other_profile(admin: &Connection) -> Result<Uuid, BoxError> {
    let other = Uuid::new_v4();
    admin.execute(
        "INSERT INTO profile_profiles (id, full_name, email, created_at, updated_at) \
         VALUES (?1, 'Other', 'other@example.test', ?2, ?2)",
        params![other.to_string(), stored(now())],
    )?;
    Ok(other)
}

#[tokio::test]
async fn owned_asset_held_by_a_second_profile_is_retired_when_that_profile_goes() -> TestResult {
    for holder in ["photo", "pdf_source"] {
        let fixture = start()?;
        let master = owned_asset(&fixture, "held").await?;
        let admin = fixture.admin();
        link(&admin, fixture.profile, &master)?;
        let other = other_profile(&admin)?;
        if holder == "photo" {
            photo(&admin, other, &master)?;
        } else {
            pdf_source(&admin, other, &master)?;
        }
        assert!(
            repository(&fixture)
                .delete_profile(fixture.profile)
                .await?
                .deleted
        );
        assert_eq!(
            state(&admin, master.asset.asset_id())?,
            active(),
            "{holder}"
        );
        let outcome = repository(&fixture).delete_profile(other).await?;
        assert_eq!(
            outcome.retired_asset_ids,
            vec![master.asset.asset_id()],
            "{holder}"
        );
        assert_eq!(
            state(&admin, master.asset.asset_id())?,
            retired(),
            "{holder}"
        );
    }
    Ok(())
}

/// Inserts a plain imported document (no generated asset) and returns its id.
fn plain_document(admin: &Connection) -> Result<Uuid, BoxError> {
    let content = pdf("plain");
    let blob = Uuid::new_v4();
    let document = Uuid::new_v4();
    admin.execute(
        "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            blob.to_string(),
            sha256_hex(&content),
            i64::try_from(content.len())?,
            content,
            stored(now())
        ],
    )?;
    admin.execute(
        "INSERT INTO document_documents (id, blob_id, original_file_name, media_type, \
         created_at, updated_at) VALUES (?1, ?2, 'plain.pdf', 'application/pdf', ?3, ?3)",
        params![document.to_string(), blob.to_string(), stored(now())],
    )?;
    Ok(document)
}

/// Deletes a second profile that holds a plain imported document, and checks that no document or
/// generated asset row changes. `attach` links the document to that profile.
async fn assert_plain_document_untouched(
    attach: impl FnOnce(&Connection, Uuid, Uuid) -> TestResult,
) -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let other = other_profile(&admin)?;
    let document = plain_document(&admin)?;
    attach(&admin, other, document)?;
    let documents = count(&admin, "document_documents")?;
    let generated = count(&admin, "jobniac_generated_assets")?;
    let outcome = repository(&fixture).delete_profile(other).await?;
    assert!(outcome.deleted);
    assert!(outcome.retired_asset_ids.is_empty());
    assert_eq!(count(&admin, "document_documents")?, documents);
    assert_eq!(count(&admin, "jobniac_generated_assets")?, generated);
    Ok(())
}

#[tokio::test]
async fn plain_imported_document_as_photo_is_untouched() -> TestResult {
    assert_plain_document_untouched(photo_of).await
}

#[tokio::test]
async fn plain_imported_document_as_pdf_source_is_untouched() -> TestResult {
    assert_plain_document_untouched(pdf_source_of).await
}

#[tokio::test]
async fn pending_generation_refuses_deletion_and_preserves_everything() -> TestResult {
    let fixture = start()?;
    world(&fixture).await?;
    let admin = fixture.admin();
    for (kind, descriptor) in [
        (
            "profile",
            GenerationDescriptor::new(
                Uuid::new_v4(),
                GenerationPurpose::ProfileResume,
                Some(fixture.profile),
                None,
                None,
                Some("master_resume".to_owned()),
            )?,
        ),
        (
            "job",
            GenerationDescriptor::new(
                Uuid::new_v4(),
                GenerationPurpose::GermanResume,
                Some(fixture.profile),
                Some(fixture.job),
                Some(Uuid::new_v4()),
                Some("en".to_owned()),
            )?,
        ),
    ] {
        let before = count(&admin, "match_reports")?;
        let content = pdf(kind);
        fixture
            .uow
            .transaction(options(), async |tx| {
                let pending = insert_asset(tx, fixture.root, now(), &content, true)?;
                insert_generation_intent(tx, &pending, &descriptor, Uuid::new_v4(), now()).await?;
                Ok::<_, jobniac_infra::sqlite::generated::IntentError>(())
            })
            .await?;
        let error = if kind == "profile" {
            repository(&fixture)
                .delete_profile(fixture.profile)
                .await
                .err()
        } else {
            repository(&fixture).delete_job(fixture.job).await.err()
        }
        .ok_or("a pending generation must refuse the deletion")?;
        let envelope = error_envelope(&error);
        assert_eq!(envelope.code, ErrorCode::Conflict, "{kind}");
        assert_eq!(
            envelope.details.get("reason").map(String::as_str),
            Some("generation_pending")
        );
        assert_eq!(count(&admin, "match_reports")?, before);
        assert_eq!(count(&admin, "profile_profiles")?, 1);
        assert_eq!(count(&admin, "job_jobs")?, 1);
    }
    Ok(())
}

#[tokio::test]
async fn missing_and_repeated_deletion() -> TestResult {
    let fixture = start()?;
    let world = world(&fixture).await?;
    let admin = fixture.admin();
    let repo = repository(&fixture);
    let missing = Uuid::new_v4();
    assert_eq!(
        repo.delete_profile(missing).await?,
        EntityDeletion::default()
    );
    assert!(!repo.delete_job(missing).await?.deleted);
    assert!(repo.delete_profile(fixture.profile).await?.deleted);
    assert!(!repo.delete_profile(fixture.profile).await?.deleted);
    assert!(repo.delete_job(fixture.job).await?.deleted);
    assert!(!repo.delete_job(fixture.job).await?.deleted);
    for binding in [&world.master, &world.en, &world.de, &world.photo] {
        assert_eq!(state(&admin, binding.asset.asset_id())?, retired());
    }
    Ok(())
}

#[tokio::test]
async fn retirement_failure_rolls_back_the_whole_delete() -> TestResult {
    let fixture = start()?;
    let world = world(&fixture).await?;
    let admin = fixture.admin();
    let counts = |admin: &Connection| -> Result<[i64; 4], BoxError> {
        Ok([
            count(admin, "profile_profiles")?,
            count(admin, "job_jobs")?,
            count(admin, "resume_resumes")?,
            count(admin, "match_reports")?,
        ])
    };
    let before = counts(&admin)?;
    fail_at(&admin, "jobniac_generated_asset_cleanups", "INSERT", "true")?;
    let error = repository(&fixture)
        .delete_profile(fixture.profile)
        .await
        .err()
        .ok_or("the retirement failure must refuse the deletion")?;
    // A trigger failure is an integrity error and keeps its mapping, with no engine text.
    assert!(!format!("{error:?}").contains("synthetic"));
    assert!(error_envelope(&error).details.is_empty());
    assert_eq!(counts(&admin)?, before);
    assert_eq!(state(&admin, world.master.asset.asset_id())?, active());
    Ok(())
}

/// A removal port that reports every owned file as removed.
#[derive(Debug)]
struct Removed;

impl OwnedAssetRemover for Removed {
    fn remove_owned(&self, _: &AssetIdentity) -> Result<RemovalOutcome, RemovalFailure> {
        Ok(RemovalOutcome::Removed)
    }
}

#[tokio::test]
async fn retired_assets_are_removed_by_the_cleanup_executor() -> TestResult {
    let fixture = start()?;
    let world = world(&fixture).await?;
    let admin = fixture.admin();
    let outcome = repository(&fixture).delete_profile(fixture.profile).await?;
    assert_eq!(outcome.retired_asset_ids.len(), 5);
    let executor = CleanupExecutor::new(
        Some(Arc::new(cleanup_repository(&fixture, later()))),
        Some(Arc::new(Removed)),
        Some(fixture.root),
        true,
    );
    let summary = executor.run_due_tasks().await?;
    assert_eq!(summary.completed, 5);
    assert_eq!(summary.state_failures, 0);
    for binding in [
        &world.master,
        &world.photo,
        &world.pdf,
        &world.en,
        &world.de,
    ] {
        assert_eq!(
            state(&admin, binding.asset.asset_id())?,
            ("DELETED".to_owned(), Some("COMPLETED".to_owned()))
        );
    }
    Ok(())
}

#[tokio::test]
async fn concurrent_double_delete_has_exactly_one_winner() -> TestResult {
    let fixture = start()?;
    let world = world(&fixture).await?;
    let admin = fixture.admin();
    let repo = Arc::new(repository(&fixture));
    let spawn = || {
        let repo = Arc::clone(&repo);
        let profile = fixture.profile;
        tokio::spawn(async move { repo.delete_profile(profile).await })
    };
    let (first, second) = (spawn(), spawn());
    let results = [first.await??, second.await??];
    let mut deleted: Vec<bool> = results.iter().map(|outcome| outcome.deleted).collect();
    deleted.sort_unstable();
    assert_eq!(deleted, vec![false, true]);
    assert_eq!(state(&admin, world.master.asset.asset_id())?, retired());
    assert_eq!(count(&admin, "jobniac_generated_asset_cleanups")?, 5);
    Ok(())
}

/// The profile-link repository of the fixture.
fn links(fixture: &Fixture) -> SqliteProfileResumeLinks {
    SqliteProfileResumeLinks::new(fixture.uow.clone(), Arc::new(FixedClock::new(later())))
}

/// A synthetic Canadian resume link of `profile` to `binding` (created `now`, updated `later`).
fn canadian_link(
    profile: Uuid,
    binding: &GeneratedReservation,
) -> Result<ProfileResumeDocument, BoxError> {
    Ok(ProfileResumeDocument::new(
        Uuid::new_v4(),
        profile,
        binding.document_id,
        binding.asset.relative_path(),
        "canadian_resume",
        now(),
        later(),
    )?)
}

/// Seeds a German cover letter of `resume` with one variant that references `binding`.
fn cover_letter(
    admin: &Connection,
    profile: Uuid,
    job: Uuid,
    resume: Uuid,
    binding: &GeneratedReservation,
) -> Result<(), BoxError> {
    let letter = Uuid::new_v4();
    let variant = Uuid::new_v4();
    admin.execute(
        "INSERT INTO cover_letter_cover_letters (id, profile_id, job_id, resume_id, \
         profile_revision, job_revision, resume_revision, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            letter.to_string(),
            profile.to_string(),
            job.to_string(),
            resume.to_string(),
            stored(now()),
            stored(now()),
            stored(later()),
            stored(now()),
            stored(later())
        ],
    )?;
    admin.execute(
        "INSERT INTO cover_letter_cover_letter_variants (id, cover_letter_id, format, \
         language, document_id, file_path, subject, salutation, closing, signature, \
         created_at, updated_at) \
         VALUES (?1,?2,'germany','de',?3,?4,'Subject','Hello','Regards','Synthetic',?5,?6)",
        params![
            variant.to_string(),
            letter.to_string(),
            binding.document_id.to_string(),
            binding.asset.relative_path(),
            stored(now()),
            stored(later())
        ],
    )?;
    admin.execute(
        "INSERT INTO cover_letter_cover_letter_paragraphs (id, variant_id, display_order, \
         text) VALUES (?1, ?2, 0, 'Synthetic paragraph')",
        params![Uuid::new_v4().to_string(), variant.to_string()],
    )?;
    Ok(())
}

/// The number of rows of the cover-letter tables: letters, then variants.
fn letter_counts(admin: &Connection) -> Result<(i64, i64), BoxError> {
    Ok((
        count(admin, "cover_letter_cover_letters")?,
        count(admin, "cover_letter_cover_letter_variants")?,
    ))
}

#[tokio::test]
async fn cover_letter_and_its_variant_asset_are_removed_with_the_profile() -> TestResult {
    let fixture = start()?;
    let world = world(&fixture).await?;
    let admin = fixture.admin();
    let letter = owned_asset(&fixture, "letter").await?;
    cover_letter(&admin, fixture.profile, fixture.job, world.resume, &letter)?;
    assert_eq!(letter_counts(&admin)?, (1, 1));
    let outcome = repository(&fixture).delete_profile(fixture.profile).await?;
    assert!(outcome.deleted);
    assert!(outcome.retired_asset_ids.contains(&letter.asset.asset_id()));
    assert_eq!(letter_counts(&admin)?, (0, 0));
    assert_eq!(state(&admin, letter.asset.asset_id())?, retired());
    Ok(())
}

#[tokio::test]
async fn cover_letter_variant_asset_is_retired_with_its_job() -> TestResult {
    let fixture = start()?;
    let world = world(&fixture).await?;
    let admin = fixture.admin();
    let letter = owned_asset(&fixture, "letter").await?;
    cover_letter(&admin, fixture.profile, fixture.job, world.resume, &letter)?;
    let outcome = repository(&fixture).delete_job(fixture.job).await?;
    assert!(outcome.deleted);
    assert!(outcome.retired_asset_ids.contains(&letter.asset.asset_id()));
    assert!(
        outcome
            .retired_asset_ids
            .contains(&world.en.asset.asset_id())
    );
    assert_eq!(letter_counts(&admin)?, (0, 0));
    assert_eq!(state(&admin, letter.asset.asset_id())?, retired());
    assert_eq!(count(&admin, "profile_profiles")?, 1);
    Ok(())
}

#[tokio::test]
async fn asset_shared_by_profile_link_and_job_variant_retires_with_the_last_owner() -> TestResult {
    for job_first in [true, false] {
        let fixture = start()?;
        let admin = fixture.admin();
        let master = owned_asset(&fixture, "shared").await?;
        link(&admin, fixture.profile, &master)?;
        let other = Uuid::new_v4();
        let second_job = Uuid::new_v4();
        insert_owners(&admin, other, second_job, "Second")?;
        resume(&admin, other, second_job, &[&master])?;
        let repo = repository(&fixture);
        let outcome = if job_first {
            assert!(
                repo.delete_job(second_job)
                    .await?
                    .retired_asset_ids
                    .is_empty()
            );
            assert_eq!(state(&admin, master.asset.asset_id())?, active());
            repo.delete_profile(fixture.profile).await?
        } else {
            assert!(repo.delete_profile(fixture.profile).await?.deleted);
            assert_eq!(state(&admin, master.asset.asset_id())?, active());
            repo.delete_job(second_job).await?
        };
        assert_eq!(
            outcome.retired_asset_ids,
            vec![master.asset.asset_id()],
            "job_first={job_first}"
        );
        assert_eq!(
            state(&admin, master.asset.asset_id())?,
            retired(),
            "job_first={job_first}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn asset_already_retiring_in_the_cascade_set_is_skipped() -> TestResult {
    let fixture = start()?;
    let world = world(&fixture).await?;
    let admin = fixture.admin();
    let master = world.master.asset.asset_id();
    // The retirement guard forbids retiring a referenced asset, so the state is staged with raw
    // statements (only the row-local schema triggers apply) in this disposable database.
    let retirement = Uuid::new_v4();
    admin.execute(
        "UPDATE jobniac_generated_assets SET lifecycle = 'RETIRING', retirement_id = ?2, \
         retired_at = ?3 WHERE asset_id = ?1",
        params![master.to_string(), retirement.to_string(), stored(now())],
    )?;
    admin.execute(
        "INSERT INTO jobniac_generated_asset_cleanups (task_id, asset_id, retirement_id, status, \
         next_attempt_at, created_at, updated_at) VALUES (?1, ?2, ?3, 'PENDING', ?4, ?4, ?4)",
        params![
            Uuid::new_v4().to_string(),
            master.to_string(),
            retirement.to_string(),
            stored(now())
        ],
    )?;
    let outcome = repository(&fixture).delete_profile(fixture.profile).await?;
    assert!(outcome.deleted);
    assert!(!outcome.retired_asset_ids.contains(&master));
    assert_eq!(outcome.retired_asset_ids.len(), 4);
    assert_eq!(state(&admin, master)?, retired());
    assert_eq!(count(&admin, "jobniac_generated_asset_cleanups")?, 5);
    Ok(())
}

#[tokio::test]
async fn retirement_failure_rolls_back_the_job_delete() -> TestResult {
    let fixture = start()?;
    let world = world(&fixture).await?;
    let admin = fixture.admin();
    let counts = |admin: &Connection| -> Result<[i64; 4], BoxError> {
        Ok([
            count(admin, "profile_profiles")?,
            count(admin, "job_jobs")?,
            count(admin, "resume_resumes")?,
            count(admin, "match_reports")?,
        ])
    };
    let before = counts(&admin)?;
    fail_at(&admin, "jobniac_generated_asset_cleanups", "INSERT", "true")?;
    let error = repository(&fixture)
        .delete_job(fixture.job)
        .await
        .err()
        .ok_or("the retirement failure must refuse the job deletion")?;
    assert!(!format!("{error:?}").contains("synthetic"));
    assert!(error_envelope(&error).details.is_empty());
    assert_eq!(counts(&admin)?, before);
    assert_eq!(state(&admin, world.en.asset.asset_id())?, active());
    assert_eq!(state(&admin, world.de.asset.asset_id())?, active());
    Ok(())
}

#[tokio::test]
async fn a_busy_database_becomes_the_fixed_unavailable_message() -> TestResult {
    // The engine reports a held write lock as busy; it must reach the caller as the fixed
    // unavailable message, with no engine text, and nothing may change.
    let fixture = start()?;
    world(&fixture).await?;
    let admin = fixture.admin();
    let pool = SqlitePool::open_with(
        fixture.sqlite.path(),
        PoolOptions {
            busy_timeout: Duration::from_millis(50),
            busy_retries: 0,
            ..PoolOptions::default()
        },
    )?;
    let repo = SqliteEntityDeletionRepository::new(
        SqliteUnitOfWork::new(pool),
        Arc::new(FixedClock::new(later())),
    );
    admin.execute_batch("BEGIN IMMEDIATE")?;
    let error = repo
        .delete_profile(fixture.profile)
        .await
        .err()
        .ok_or("a busy database must refuse the deletion")?;
    admin.execute_batch("ROLLBACK")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::InternalError);
    assert_eq!(envelope.message, "Local database is unavailable");
    assert!(envelope.details.is_empty());
    assert_eq!(count(&admin, "profile_profiles")?, 1);
    Ok(())
}

#[tokio::test]
async fn unconfigured_repository_refuses_as_unavailable() -> TestResult {
    // With no database configured the unit of work reports the database as unavailable.
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(later()));
    let uow = SqliteUnitOfWork::new(SqlitePool::unconfigured());
    let repo = SqliteEntityDeletionRepository::new(uow, clock);
    let error = repo
        .delete_job(Uuid::new_v4())
        .await
        .err()
        .ok_or("an unconfigured repository must refuse")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.message, "Local database is unavailable");
    assert!(envelope.details.is_empty());
    Ok(())
}

#[tokio::test]
async fn link_written_after_the_delete_is_refused_and_leaves_no_dangling_reference() -> TestResult {
    let fixture = start()?;
    let world = world(&fixture).await?;
    let admin = fixture.admin();
    let late = owned_asset(&fixture, "late").await?;
    let candidate = canadian_link(fixture.profile, &late)?;
    assert!(
        repository(&fixture)
            .delete_profile(fixture.profile)
            .await?
            .deleted
    );
    let error = links(&fixture)
        .replace(&candidate, &late, 0, now(), None)
        .await
        .err()
        .ok_or("a link written after the delete must be refused")?;
    assert_eq!(error_envelope(&error).code, ErrorCode::NotFound);
    assert_eq!(count(&admin, "profile_profile_resume_documents")?, 0);
    assert_eq!(state(&admin, late.asset.asset_id())?, active());
    for binding in [
        &world.master,
        &world.photo,
        &world.pdf,
        &world.en,
        &world.de,
    ] {
        assert_eq!(state(&admin, binding.asset.asset_id())?, retired());
    }
    Ok(())
}

#[tokio::test]
async fn link_writer_committed_before_delete_is_retired_by_it() -> TestResult {
    let fixture = start()?;
    world(&fixture).await?;
    let admin = fixture.admin();
    let late = owned_asset(&fixture, "late").await?;
    let candidate = canadian_link(fixture.profile, &late)?;
    links(&fixture)
        .replace(&candidate, &late, 0, now(), None)
        .await
        .map_err(|_| "the link writer must commit")?;
    let outcome = repository(&fixture).delete_profile(fixture.profile).await?;
    assert!(outcome.deleted);
    assert!(outcome.retired_asset_ids.contains(&late.asset.asset_id()));
    assert_eq!(count(&admin, "profile_profile_resume_documents")?, 0);
    assert_eq!(state(&admin, late.asset.asset_id())?, retired());
    Ok(())
}

#[tokio::test]
async fn job_deleted_before_a_german_writer_blocks_new_references() -> TestResult {
    let fixture = start()?;
    world(&fixture).await?;
    let admin = fixture.admin();
    let fresh = owned_asset(&fixture, "fresh").await?;
    let german = fixture.resumes(later());
    let write = resume_write(fixture.profile, fixture.job, &[&fresh])?;
    assert!(repository(&fixture).delete_job(fixture.job).await?.deleted);
    assert!(save_resume(&german, &write, &[&fresh]).await.is_err());
    assert_eq!(count(&admin, "resume_resumes")?, 0);
    assert_eq!(count(&admin, "resume_resume_variants")?, 0);
    assert_eq!(state(&admin, fresh.asset.asset_id())?, active());
    Ok(())
}
