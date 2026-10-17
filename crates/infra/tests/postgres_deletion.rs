//! Atomic profile and job hard delete over a disposable PostgreSQL 18.
//!
//! Each test works on its own migrated database and skips with a notice when no server is
//! configured. On CI a missing server fails the test. All data is synthetic.

mod german_support;

use std::error::Error;
use std::sync::Arc;

use german_support::{
    Fixture, TestResult, activate, cleanup_repository, fail_at, fixture_failure, gate_inserts,
    insert_asset, insert_owners, later, now, options, pdf, release_gate, resume_write, save_resume,
    start, wait_for_blocker, wait_for_lock,
};
use jobniac_core::cleanup::{CleanupExecutor, OwnedAssetRemover, RemovalFailure, RemovalOutcome};
use jobniac_core::clock::{Clock, FixedClock};
use jobniac_core::deletion::{EntityDeletion, JobDeletionRepository, ProfileDeletionRepository};
use jobniac_core::errors::{ErrorCode, error_envelope};
use jobniac_core::generated_assets::AssetIdentity;
use jobniac_core::generated_assets::{
    GeneratedReservation, GenerationDescriptor, GenerationPurpose, ProfileResumeDocument,
};
use jobniac_infra::postgres::deletion::{APPLICATION_NAME, PostgresEntityDeletionRepository};
use jobniac_infra::postgres::generated::IntentError;
use jobniac_infra::postgres::generated::assets::retire_asset;
use jobniac_infra::postgres::generated::bindings::{ASSET_COLUMNS, AssetRow};
use jobniac_infra::postgres::generated::intents::insert_generation_intent;
use jobniac_infra::postgres::generated::profile_links::PgProfileResumeLinks;
use jobniac_infra::postgres::german::RESUME_APPLICATION_NAME;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use tokio_postgres::Client;
use uuid::Uuid;

/// The application name of the deletion sessions (the pool of the repository).
const APP: &str = "jobniac-entity-deletion";

/// The tables a refused or rolled-back deletion must leave untouched.
const HISTORY: [&str; 4] = [
    "profile.profiles",
    "resume.resumes",
    "resume.resume_variants",
    "match.reports",
];

/// The repository of one fixture, stamping retirements with a fixed instant.
fn repository(fixture: &Fixture) -> PostgresEntityDeletionRepository {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(later()));
    // The deletion sessions carry their own application name, as the composition root builds them.
    let uow = PgUnitOfWork::new(PgPool::with_application_name(
        Some(fixture.config.clone()),
        APPLICATION_NAME,
    ));
    PostgresEntityDeletionRepository::new(uow, clock)
}

/// The number of rows of `table`.
async fn count(admin: &Client, table: &str) -> Result<i64, Box<dyn Error>> {
    let row = admin
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await?;
    Ok(row.try_get(0)?)
}

/// The lifecycle of an asset and the status of its cleanup task, if any.
async fn state(admin: &Client, asset: Uuid) -> Result<(String, Option<String>), Box<dyn Error>> {
    let row = admin
        .query_one(
            "SELECT asset.lifecycle, task.status FROM jobniac.generated_assets asset \
             LEFT JOIN jobniac.generated_asset_cleanups task USING (asset_id) \
             WHERE asset.asset_id = $1",
            &[&asset],
        )
        .await?;
    Ok((row.try_get(0)?, row.try_get(1)?))
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
async fn owned_asset(
    fixture: &Fixture,
    label: &str,
) -> Result<GeneratedReservation, Box<dyn Error>> {
    let content = pdf(label);
    let binding = fixture
        .uow
        .transaction(options(), async |tx| {
            let binding = insert_asset(tx, fixture.root, now(), &content, false).await?;
            activate(tx, binding.asset.asset_id()).await?;
            Ok::<_, jobniac_infra::postgres::generated::IntentError>(binding)
        })
        .await?;
    Ok(binding)
}

/// Inserts a profile resume link of `profile` to `binding` (the master resume).
async fn link(
    admin: &Client,
    profile: Uuid,
    binding: &GeneratedReservation,
) -> Result<(), Box<dyn Error>> {
    admin
        .execute(
            "INSERT INTO profile.profile_resume_documents (id, profile_id, document_id, file_path, \
             resume_type, created_at, updated_at) VALUES ($1, $2, $3, $4, 'master_resume', $5, $5)",
            &[
                &Uuid::new_v4(),
                &profile,
                &binding.document_id,
                &binding.asset.relative_path(),
                &now(),
            ],
        )
        .await?;
    Ok(())
}

/// Attaches `binding` as the photo of `profile`.
async fn photo(
    admin: &Client,
    profile: Uuid,
    binding: &GeneratedReservation,
) -> Result<(), Box<dyn Error>> {
    admin
        .execute(
            "INSERT INTO profile.profile_personal_details (profile_id, photo_document_id, \
             created_at, updated_at) VALUES ($1, $2, $3, $3)",
            &[&profile, &binding.document_id, &now()],
        )
        .await?;
    Ok(())
}

/// Attaches `binding` as a PDF source of `profile` through an extraction row.
async fn pdf_source(
    admin: &Client,
    profile: Uuid,
    binding: &GeneratedReservation,
) -> Result<(), Box<dyn Error>> {
    pdf_source_of(admin, profile, binding.document_id).await
}

/// Attaches the document `file` as a PDF source of `profile` through an extraction row.
async fn pdf_source_of(admin: &Client, profile: Uuid, file: Uuid) -> Result<(), Box<dyn Error>> {
    let extraction = Uuid::new_v4();
    admin
        .execute(
            "INSERT INTO document.pdf_extractions (id, file_id, extractor, character_count, \
             page_count, truncated, extracted_text, created_at) \
             VALUES ($1, $2, 'synthetic', 1, 1, false, 'x', $3)",
            &[&extraction, &file, &now()],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO profile.profile_pdf_sources (id, profile_id, pdf_extraction_id, \
             source_type, created_at) VALUES ($1, $2, $3, 'resume_pdf', $4)",
            &[&Uuid::new_v4(), &profile, &extraction, &now()],
        )
        .await?;
    Ok(())
}

/// Inserts a Germany resume of `profile` for `job` with one variant per binding.
async fn resume(
    admin: &Client,
    profile: Uuid,
    job: Uuid,
    bindings: &[&GeneratedReservation],
) -> Result<Uuid, Box<dyn Error>> {
    let resume = Uuid::new_v4();
    admin
        .execute(
            "INSERT INTO resume.resumes (id, profile_id, job_id, format, profile_revision, \
             job_revision, created_at, updated_at) VALUES ($1, $2, $3, 'germany', $4, $4, $4, $4)",
            &[&resume, &profile, &job, &now()],
        )
        .await?;
    for (binding, language) in bindings.iter().zip(["en", "de"]) {
        admin
            .execute(
                "INSERT INTO resume.resume_variants (id, resume_id, language, document_id, \
                 file_path, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $6)",
                &[
                    &Uuid::new_v4(),
                    &resume,
                    &language,
                    &binding.document_id,
                    &binding.asset.relative_path(),
                    &now(),
                ],
            )
            .await?;
    }
    Ok(resume)
}

/// Inserts one matching report of `profile` against `job`.
async fn report(admin: &Client, profile: Uuid, job: Uuid) -> Result<(), Box<dyn Error>> {
    admin
        .execute(
            "INSERT INTO match.reports (id, profile_id, job_id, profile_revision, job_revision, \
             algorithm_version, overall_score, confidence, outcome, blocker_mismatch, components, \
             evidence, created_at) VALUES ($1, $2, $3, $4, $4, 'synthetic', 50, 50, \
             'PARTIAL_MATCH', false, '[]'::jsonb, '[]'::jsonb, $4)",
            &[&Uuid::new_v4(), &profile, &job, &now()],
        )
        .await?;
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

async fn world(fixture: &Fixture) -> Result<World, Box<dyn Error>> {
    let admin = fixture.admin().await;
    let master = owned_asset(fixture, "master").await?;
    let photo_asset = owned_asset(fixture, "photo").await?;
    let pdf_asset = owned_asset(fixture, "pdf").await?;
    let en = owned_asset(fixture, "en").await?;
    let de = owned_asset(fixture, "de").await?;
    link(&admin, fixture.profile, &master).await?;
    photo(&admin, fixture.profile, &photo_asset).await?;
    pdf_source(&admin, fixture.profile, &pdf_asset).await?;
    let resume_id = resume(&admin, fixture.profile, fixture.job, &[&en, &de]).await?;
    report(&admin, fixture.profile, fixture.job).await?;
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
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let world = world(&fixture).await?;
    let admin = fixture.admin().await;
    let documents_before = count(&admin, "document.documents").await?;
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
        assert_eq!(count(&admin, table).await?, 0, "{table}");
    }
    for binding in [
        &world.master,
        &world.photo,
        &world.pdf,
        &world.en,
        &world.de,
    ] {
        assert_eq!(state(&admin, binding.asset.asset_id()).await?, retired());
    }
    assert_eq!(count(&admin, "document.documents").await?, documents_before);
    Ok(())
}

#[tokio::test]
async fn job_deletion_removes_job_history_and_preserves_the_profile_link() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let world = world(&fixture).await?;
    let admin = fixture.admin().await;
    let outcome = repository(&fixture).delete_job(fixture.job).await?;
    let mut expected = [world.en.asset.asset_id(), world.de.asset.asset_id()];
    expected.sort_unstable();
    assert!(outcome.deleted);
    assert_eq!(outcome.retired_asset_ids, expected.to_vec());
    assert_eq!(count(&admin, "job_schema.jobs").await?, 0);
    assert_eq!(count(&admin, "resume.resumes").await?, 0);
    assert_eq!(count(&admin, "match.reports").await?, 0);
    assert_eq!(count(&admin, "profile.profiles").await?, 1);
    assert_eq!(count(&admin, "profile.profile_resume_documents").await?, 1);
    assert_eq!(state(&admin, world.en.asset.asset_id()).await?, retired());
    assert_eq!(
        state(&admin, world.master.asset.asset_id()).await?,
        active()
    );
    Ok(())
}

#[tokio::test]
async fn shared_asset_that_stays_referenced_is_not_retired() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let world = world(&fixture).await?;
    let admin = fixture.admin().await;
    let other = Uuid::new_v4();
    admin
        .execute(
            "INSERT INTO profile.profiles (id, full_name, email, created_at, updated_at) \
             VALUES ($1, 'Other', 'other@example.test', $2, $2)",
            &[&other, &now()],
        )
        .await?;
    photo(&admin, other, &world.en).await?;
    let outcome = repository(&fixture).delete_profile(fixture.profile).await?;
    assert!(
        !outcome
            .retired_asset_ids
            .contains(&world.en.asset.asset_id())
    );
    assert_eq!(state(&admin, world.en.asset.asset_id()).await?, active());
    assert_eq!(
        state(&admin, world.master.asset.asset_id()).await?,
        retired()
    );
    Ok(())
}

#[tokio::test]
async fn unowned_legacy_document_is_untouched() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let admin = fixture.admin().await;
    let other = Uuid::new_v4();
    let blob = Uuid::new_v4();
    let document = Uuid::new_v4();
    let content = pdf("legacy");
    admin
        .execute(
            "INSERT INTO profile.profiles (id, full_name, email, created_at, updated_at) \
             VALUES ($1, 'Other', 'other@example.test', $2, $2)",
            &[&other, &now()],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO document.blobs (id, sha256, byte_size, content, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
            &[
                &blob,
                &jobniac_core::generated_assets::sha256_hex(&content),
                &i64::try_from(content.len())?,
                &content,
                &now(),
            ],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO document.documents (id, blob_id, original_file_name, media_type, \
             created_at, updated_at) VALUES ($1, $2, 'legacy.pdf', 'application/pdf', $3, $3)",
            &[&document, &blob, &now()],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO profile.profile_resume_documents (id, profile_id, document_id, file_path, \
             resume_type, created_at, updated_at) VALUES ($1, $2, $3, 'legacy/not-owned.pdf', \
             'master_resume', $4, $4)",
            &[&Uuid::new_v4(), &other, &document, &now()],
        )
        .await?;
    let documents = count(&admin, "document.documents").await?;
    let generated = count(&admin, "jobniac.generated_assets").await?;
    let outcome = repository(&fixture).delete_profile(other).await?;
    assert_eq!(
        outcome,
        EntityDeletion {
            deleted: true,
            retired_asset_ids: Vec::new(),
        }
    );
    assert_eq!(count(&admin, "document.documents").await?, documents);
    assert_eq!(count(&admin, "jobniac.generated_assets").await?, generated);
    Ok(())
}

/// Inserts a second synthetic profile and returns its id.
async fn other_profile(admin: &Client) -> Result<Uuid, Box<dyn Error>> {
    let other = Uuid::new_v4();
    admin
        .execute(
            "INSERT INTO profile.profiles (id, full_name, email, created_at, updated_at) \
             VALUES ($1, 'Other', 'other@example.test', $2, $2)",
            &[&other, &now()],
        )
        .await?;
    Ok(other)
}

#[tokio::test]
async fn owned_asset_held_by_a_second_profile_is_retired_when_that_profile_goes() -> TestResult {
    for holder in ["photo", "pdf_source"] {
        let Some(fixture) = start().await? else {
            return Ok(());
        };
        let master = owned_asset(&fixture, "held").await?;
        let admin = fixture.admin().await;
        link(&admin, fixture.profile, &master).await?;
        let other = other_profile(&admin).await?;
        if holder == "photo" {
            photo(&admin, other, &master).await?;
        } else {
            pdf_source(&admin, other, &master).await?;
        }
        assert!(
            repository(&fixture)
                .delete_profile(fixture.profile)
                .await?
                .deleted
        );
        assert_eq!(
            state(&admin, master.asset.asset_id()).await?,
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
            state(&admin, master.asset.asset_id()).await?,
            retired(),
            "{holder}"
        );
    }
    Ok(())
}

/// Inserts a plain imported document (no generated asset) and returns its id.
async fn plain_document(admin: &Client) -> Result<Uuid, Box<dyn Error>> {
    let content = pdf("plain");
    let blob = Uuid::new_v4();
    let document = Uuid::new_v4();
    admin
        .execute(
            "INSERT INTO document.blobs (id, sha256, byte_size, content, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
            &[
                &blob,
                &jobniac_core::generated_assets::sha256_hex(&content),
                &i64::try_from(content.len())?,
                &content,
                &now(),
            ],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO document.documents (id, blob_id, original_file_name, media_type, \
             created_at, updated_at) VALUES ($1, $2, 'plain.pdf', 'application/pdf', $3, $3)",
            &[&document, &blob, &now()],
        )
        .await?;
    Ok(document)
}

/// Deletes a second profile that holds a plain imported document, and checks that no document or
/// generated asset row changes. `attach` links the document to that profile.
async fn assert_plain_document_untouched<F, Fut>(attach: F) -> TestResult
where
    F: FnOnce(Client, Uuid, Uuid) -> Fut,
    Fut: std::future::Future<Output = TestResult>,
{
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let admin = fixture.admin().await;
    let other = other_profile(&admin).await?;
    let document = plain_document(&admin).await?;
    attach(fixture.admin().await, other, document).await?;
    let documents = count(&admin, "document.documents").await?;
    let generated = count(&admin, "jobniac.generated_assets").await?;
    let outcome = repository(&fixture).delete_profile(other).await?;
    assert!(outcome.deleted);
    assert!(outcome.retired_asset_ids.is_empty());
    assert_eq!(count(&admin, "document.documents").await?, documents);
    assert_eq!(count(&admin, "jobniac.generated_assets").await?, generated);
    Ok(())
}

#[tokio::test]
async fn plain_imported_document_as_photo_is_untouched() -> TestResult {
    assert_plain_document_untouched(|admin, other, document| async move {
        admin
            .execute(
                "INSERT INTO profile.profile_personal_details (profile_id, photo_document_id, \
                 created_at, updated_at) VALUES ($1, $2, $3, $3)",
                &[&other, &document, &now()],
            )
            .await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn plain_imported_document_as_pdf_source_is_untouched() -> TestResult {
    assert_plain_document_untouched(|admin, other, document| async move {
        pdf_source_of(&admin, other, document).await
    })
    .await
}

#[tokio::test]
async fn pending_generation_refuses_deletion_and_preserves_everything() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    world(&fixture).await?;
    let admin = fixture.admin().await;
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
        let before = count(&admin, "match.reports").await?;
        let content = pdf(kind);
        fixture
            .uow
            .transaction(options(), async |tx| {
                let pending = insert_asset(tx, fixture.root, now(), &content, true).await?;
                insert_generation_intent(tx, &pending, &descriptor, Uuid::new_v4(), now()).await?;
                Ok::<_, jobniac_infra::postgres::generated::IntentError>(())
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
        assert_eq!(count(&admin, "match.reports").await?, before);
        assert_eq!(count(&admin, "profile.profiles").await?, 1);
        assert_eq!(count(&admin, "job_schema.jobs").await?, 1);
    }
    Ok(())
}

#[tokio::test]
async fn missing_and_repeated_deletion() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let world = world(&fixture).await?;
    let admin = fixture.admin().await;
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
        assert_eq!(state(&admin, binding.asset.asset_id()).await?, retired());
    }
    Ok(())
}

#[tokio::test]
async fn retirement_failure_rolls_back_the_whole_delete() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let world = world(&fixture).await?;
    let admin = fixture.admin().await;
    let before = [
        count(&admin, "profile.profiles").await?,
        count(&admin, "job_schema.jobs").await?,
        count(&admin, "resume.resumes").await?,
        count(&admin, "match.reports").await?,
    ];
    fail_at(&admin, "jobniac.generated_asset_cleanups", "INSERT", "true").await?;
    let error = repository(&fixture)
        .delete_profile(fixture.profile)
        .await
        .err()
        .ok_or("the retirement failure must refuse the deletion")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::InternalError);
    assert_eq!(envelope.message, "Local database is unavailable");
    assert!(envelope.details.is_empty());
    assert!(!format!("{error:?}").contains("synthetic"));
    assert_eq!(
        [
            count(&admin, "profile.profiles").await?,
            count(&admin, "job_schema.jobs").await?,
            count(&admin, "resume.resumes").await?,
            count(&admin, "match.reports").await?,
        ],
        before
    );
    assert_eq!(
        state(&admin, world.master.asset.asset_id()).await?,
        active()
    );
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
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let world = world(&fixture).await?;
    let admin = fixture.admin().await;
    let outcome = repository(&fixture).delete_profile(fixture.profile).await?;
    assert_eq!(outcome.retired_asset_ids.len(), 5);
    let executor = CleanupExecutor::new(
        Some(Arc::new(cleanup_repository(&fixture.config, later()))),
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
            state(&admin, binding.asset.asset_id()).await?,
            ("DELETED".to_owned(), Some("COMPLETED".to_owned()))
        );
    }
    Ok(())
}

#[tokio::test]
async fn concurrent_double_delete_has_exactly_one_winner() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let world = world(&fixture).await?;
    let admin = fixture.admin().await;
    let holder = fixture.admin().await;
    holder.batch_execute("BEGIN").await?;
    holder
        .execute(
            "SELECT id FROM profile.profiles WHERE id = $1 FOR UPDATE",
            &[&fixture.profile],
        )
        .await?;
    let repo = Arc::new(repository(&fixture));
    let first = {
        let repo = Arc::clone(&repo);
        let profile = fixture.profile;
        tokio::spawn(async move { repo.delete_profile(profile).await })
    };
    wait_for_lock(&fixture.database, APP).await?;
    let second = {
        let repo = Arc::clone(&repo);
        let profile = fixture.profile;
        tokio::spawn(async move { repo.delete_profile(profile).await })
    };
    holder.batch_execute("ROLLBACK").await?;
    let results = [first.await??, second.await??];
    let mut deleted: Vec<bool> = results.iter().map(|outcome| outcome.deleted).collect();
    deleted.sort_unstable();
    assert_eq!(deleted, vec![false, true]);
    assert_eq!(
        state(&admin, world.master.asset.asset_id()).await?,
        retired()
    );
    assert_eq!(count(&admin, "jobniac.generated_asset_cleanups").await?, 5);
    Ok(())
}

/// `application_name` of the profile-link sessions.
const LINK_APP: &str = "jobniac-profile-resume-links";

/// The advisory key the link gate holds (`913719`).
const LINK_GATE: i64 = 913_719;

/// The profile-link repository of the fixture, on sessions named like the production sessions.
fn links(fixture: &Fixture) -> PgProfileResumeLinks {
    let pool = PgPool::with_application_name(Some(fixture.config.clone()), LINK_APP);
    PgProfileResumeLinks::new(PgUnitOfWork::new(pool), Arc::new(FixedClock::new(later())))
}

/// A synthetic Canadian resume link of `profile` to `binding` (created `now`, updated `later`).
fn canadian_link(
    profile: Uuid,
    binding: &GeneratedReservation,
) -> Result<ProfileResumeDocument, Box<dyn Error>> {
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
async fn cover_letter(
    admin: &Client,
    profile: Uuid,
    job: Uuid,
    resume: Uuid,
    binding: &GeneratedReservation,
) -> Result<(), Box<dyn Error>> {
    let letter = Uuid::new_v4();
    let variant = Uuid::new_v4();
    admin
        .execute(
            "INSERT INTO cover_letter.cover_letters (id, profile_id, job_id, resume_id, \
             profile_revision, job_revision, resume_revision, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            &[
                &letter,
                &profile,
                &job,
                &resume,
                &now(),
                &now(),
                &later(),
                &now(),
                &later(),
            ],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO cover_letter.cover_letter_variants (id, cover_letter_id, format, \
             language, document_id, file_path, subject, salutation, closing, signature, \
             created_at, updated_at) \
             VALUES ($1,$2,'germany','de',$3,$4,'Subject','Hello','Regards','Synthetic',$5,$6)",
            &[
                &variant,
                &letter,
                &binding.document_id,
                &binding.asset.relative_path(),
                &now(),
                &later(),
            ],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO cover_letter.cover_letter_paragraphs (id, variant_id, display_order, \
             text) VALUES ($1, $2, 0, 'Synthetic paragraph')",
            &[&Uuid::new_v4(), &variant],
        )
        .await?;
    Ok(())
}

/// The number of rows of the cover-letter tables: letters, then variants.
async fn letter_counts(admin: &Client) -> Result<(i64, i64), Box<dyn Error>> {
    Ok((
        count(admin, "cover_letter.cover_letters").await?,
        count(admin, "cover_letter.cover_letter_variants").await?,
    ))
}

#[tokio::test]
async fn cover_letter_and_its_variant_asset_are_removed_with_the_profile() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let world = world(&fixture).await?;
    let admin = fixture.admin().await;
    let letter = owned_asset(&fixture, "letter").await?;
    cover_letter(&admin, fixture.profile, fixture.job, world.resume, &letter).await?;
    assert_eq!(letter_counts(&admin).await?, (1, 1));
    let outcome = repository(&fixture).delete_profile(fixture.profile).await?;
    assert!(outcome.deleted);
    assert!(outcome.retired_asset_ids.contains(&letter.asset.asset_id()));
    assert_eq!(letter_counts(&admin).await?, (0, 0));
    assert_eq!(state(&admin, letter.asset.asset_id()).await?, retired());
    Ok(())
}

#[tokio::test]
async fn cover_letter_variant_asset_is_retired_with_its_job() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let world = world(&fixture).await?;
    let admin = fixture.admin().await;
    let letter = owned_asset(&fixture, "letter").await?;
    cover_letter(&admin, fixture.profile, fixture.job, world.resume, &letter).await?;
    let outcome = repository(&fixture).delete_job(fixture.job).await?;
    assert!(outcome.deleted);
    assert!(outcome.retired_asset_ids.contains(&letter.asset.asset_id()));
    assert!(
        outcome
            .retired_asset_ids
            .contains(&world.en.asset.asset_id())
    );
    assert_eq!(letter_counts(&admin).await?, (0, 0));
    assert_eq!(state(&admin, letter.asset.asset_id()).await?, retired());
    assert_eq!(count(&admin, "profile.profiles").await?, 1);
    Ok(())
}

#[tokio::test]
async fn asset_shared_by_profile_link_and_job_variant_retires_with_the_last_owner() -> TestResult {
    for job_first in [true, false] {
        let Some(fixture) = start().await? else {
            return Ok(());
        };
        let admin = fixture.admin().await;
        let master = owned_asset(&fixture, "shared").await?;
        link(&admin, fixture.profile, &master).await?;
        let other = Uuid::new_v4();
        let second_job = Uuid::new_v4();
        insert_owners(&admin, other, second_job, "Second").await?;
        resume(&admin, other, second_job, &[&master]).await?;
        let repo = repository(&fixture);
        let outcome = if job_first {
            assert!(
                repo.delete_job(second_job)
                    .await?
                    .retired_asset_ids
                    .is_empty()
            );
            assert_eq!(state(&admin, master.asset.asset_id()).await?, active());
            repo.delete_profile(fixture.profile).await?
        } else {
            assert!(repo.delete_profile(fixture.profile).await?.deleted);
            assert_eq!(state(&admin, master.asset.asset_id()).await?, active());
            repo.delete_job(second_job).await?
        };
        assert_eq!(
            outcome.retired_asset_ids,
            vec![master.asset.asset_id()],
            "job_first={job_first}"
        );
        assert_eq!(
            state(&admin, master.asset.asset_id()).await?,
            retired(),
            "job_first={job_first}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn asset_already_retiring_in_the_cascade_set_is_skipped() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let world = world(&fixture).await?;
    let admin = fixture.admin().await;
    let master = world.master.asset.asset_id();
    // The schema forbids retiring a referenced asset, so the guards are disabled while the state is
    // staged in this disposable database only.
    admin
        .batch_execute("ALTER TABLE jobniac.generated_assets DISABLE TRIGGER USER")
        .await?;
    fixture
        .uow
        .transaction(options(), async |tx| {
            let row = tx
                .client()
                .query_one(
                    &format!(
                        "SELECT {ASSET_COLUMNS} FROM jobniac.generated_assets \
                         WHERE asset_id = $1 FOR UPDATE"
                    ),
                    &[&master],
                )
                .await
                .map_err(fixture_failure)?;
            let row = AssetRow::from_row(&row)?;
            retire_asset(tx, &row, now(), false, None).await?;
            Ok::<_, IntentError>(())
        })
        .await?;
    admin
        .batch_execute("ALTER TABLE jobniac.generated_assets ENABLE TRIGGER USER")
        .await?;
    let outcome = repository(&fixture).delete_profile(fixture.profile).await?;
    assert!(outcome.deleted);
    assert!(!outcome.retired_asset_ids.contains(&master));
    assert_eq!(outcome.retired_asset_ids.len(), 4);
    assert_eq!(state(&admin, master).await?, retired());
    assert_eq!(count(&admin, "jobniac.generated_asset_cleanups").await?, 5);
    Ok(())
}

#[tokio::test]
async fn retirement_failure_rolls_back_the_job_delete() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let world = world(&fixture).await?;
    let admin = fixture.admin().await;
    let before = [
        count(&admin, "profile.profiles").await?,
        count(&admin, "job_schema.jobs").await?,
        count(&admin, "resume.resumes").await?,
        count(&admin, "match.reports").await?,
    ];
    fail_at(&admin, "jobniac.generated_asset_cleanups", "INSERT", "true").await?;
    let error = repository(&fixture)
        .delete_job(fixture.job)
        .await
        .err()
        .ok_or("the retirement failure must refuse the job deletion")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::InternalError);
    assert_eq!(envelope.message, "Local database is unavailable");
    assert!(envelope.details.is_empty());
    assert!(!format!("{error:?}").contains("synthetic"));
    assert_eq!(
        [
            count(&admin, "profile.profiles").await?,
            count(&admin, "job_schema.jobs").await?,
            count(&admin, "resume.resumes").await?,
            count(&admin, "match.reports").await?,
        ],
        before
    );
    assert_eq!(state(&admin, world.en.asset.asset_id()).await?, active());
    assert_eq!(state(&admin, world.de.asset.asset_id()).await?, active());
    Ok(())
}

#[tokio::test]
async fn statement_and_lock_timeouts_become_the_fixed_unavailable_message() -> TestResult {
    // 57014 (statement timeout, query canceled) and 55P03 (lock not available) are the SQLSTATEs the
    // server raises for a timeout; each must reach the caller as the fixed unavailable message.
    for sqlstate in ["57014", "55P03"] {
        let Some(fixture) = start().await? else {
            return Ok(());
        };
        world(&fixture).await?;
        let admin = fixture.admin().await;
        admin
            .batch_execute(&format!(
                "CREATE FUNCTION jobniac.synthetic_timeout() RETURNS trigger LANGUAGE plpgsql \
                 AS $$ BEGIN RAISE EXCEPTION 'synthetic timeout' USING ERRCODE = '{sqlstate}'; \
                 END $$; \
                 CREATE TRIGGER synthetic_timeout BEFORE INSERT ON \
                 jobniac.generated_asset_cleanups FOR EACH ROW \
                 EXECUTE FUNCTION jobniac.synthetic_timeout();"
            ))
            .await?;
        let error = repository(&fixture)
            .delete_profile(fixture.profile)
            .await
            .err()
            .ok_or("a timeout must refuse the deletion")?;
        let envelope = error_envelope(&error);
        assert_eq!(envelope.code, ErrorCode::InternalError, "{sqlstate}");
        assert_eq!(
            envelope.message, "Local database is unavailable",
            "{sqlstate}"
        );
        assert!(envelope.details.is_empty(), "{sqlstate}");
        assert!(!format!("{error:?}").contains("synthetic"), "{sqlstate}");
        assert_eq!(count(&admin, "profile.profiles").await?, 1, "{sqlstate}");
    }
    Ok(())
}

#[tokio::test]
async fn unconfigured_repository_refuses_as_unavailable() -> TestResult {
    // With no database configured the unit of work reports the database as unavailable.
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(later()));
    let uow = PgUnitOfWork::new(PgPool::with_application_name(None, APPLICATION_NAME));
    let repo = PostgresEntityDeletionRepository::new(uow, clock);
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
async fn delete_queued_before_link_writer_leaves_no_dangling_reference() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let world = world(&fixture).await?;
    let admin = fixture.admin().await;
    let late = owned_asset(&fixture, "late").await?;
    let candidate = canadian_link(fixture.profile, &late)?;
    let holder = fixture.admin().await;
    holder.batch_execute("BEGIN").await?;
    holder
        .execute(
            "SELECT id FROM profile.profiles WHERE id = $1 FOR UPDATE",
            &[&fixture.profile],
        )
        .await?;
    let repo = Arc::new(repository(&fixture));
    let writer_links = Arc::new(links(&fixture));
    let delete = {
        let repo = Arc::clone(&repo);
        let profile = fixture.profile;
        tokio::spawn(async move { repo.delete_profile(profile).await })
    };
    wait_for_lock(&fixture.database, APP).await?;
    let writer = {
        let late = late.clone();
        tokio::spawn(async move {
            writer_links
                .replace(&candidate, &late, 0, now(), None)
                .await
        })
    };
    wait_for_lock(&fixture.database, LINK_APP).await?;
    holder.batch_execute("ROLLBACK").await?;
    assert!(delete.await??.deleted);
    let error = writer
        .await?
        .err()
        .ok_or("a link written after the delete must be refused")?;
    assert_eq!(error_envelope(&error).code, ErrorCode::NotFound);
    assert_eq!(count(&admin, "profile.profile_resume_documents").await?, 0);
    assert_eq!(state(&admin, late.asset.asset_id()).await?, active());
    for binding in [
        &world.master,
        &world.photo,
        &world.pdf,
        &world.en,
        &world.de,
    ] {
        assert_eq!(state(&admin, binding.asset.asset_id()).await?, retired());
    }
    Ok(())
}

#[tokio::test]
async fn link_writer_committed_before_delete_is_retired_by_it() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    world(&fixture).await?;
    let admin = fixture.admin().await;
    let late = owned_asset(&fixture, "late").await?;
    let candidate = canadian_link(fixture.profile, &late)?;
    let gate = gate_inserts(
        &fixture.database,
        "profile.profile_resume_documents",
        LINK_GATE,
    )
    .await?;
    let writer = {
        let writer_links = Arc::new(links(&fixture));
        let late = late.clone();
        tokio::spawn(async move {
            writer_links
                .replace(&candidate, &late, 0, now(), None)
                .await
        })
    };
    wait_for_lock(&fixture.database, LINK_APP).await?;
    let repo = Arc::new(repository(&fixture));
    let delete = {
        let repo = Arc::clone(&repo);
        let profile = fixture.profile;
        tokio::spawn(async move { repo.delete_profile(profile).await })
    };
    wait_for_blocker(&fixture.database, APP, LINK_APP).await?;
    assert!(!delete.is_finished());
    release_gate(&gate, LINK_GATE).await?;
    writer.await?.map_err(|_| "the link writer must commit")?;
    let outcome = delete.await??;
    assert!(outcome.deleted);
    assert!(outcome.retired_asset_ids.contains(&late.asset.asset_id()));
    assert_eq!(count(&admin, "profile.profile_resume_documents").await?, 0);
    assert_eq!(state(&admin, late.asset.asset_id()).await?, retired());
    Ok(())
}

#[tokio::test]
async fn job_delete_queued_before_german_writer_blocks_new_references() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    world(&fixture).await?;
    let admin = fixture.admin().await;
    let fresh = owned_asset(&fixture, "fresh").await?;
    let holder = fixture.admin().await;
    holder.batch_execute("BEGIN").await?;
    holder
        .execute(
            "SELECT id FROM job_schema.jobs WHERE id = $1 FOR UPDATE",
            &[&fixture.job],
        )
        .await?;
    let repo = Arc::new(repository(&fixture));
    let delete = {
        let repo = Arc::clone(&repo);
        let job = fixture.job;
        tokio::spawn(async move { repo.delete_job(job).await })
    };
    wait_for_lock(&fixture.database, APP).await?;
    let writer = {
        let german = fixture.resumes(later());
        let write = resume_write(fixture.profile, fixture.job, &[&fresh])?;
        let fresh = fresh.clone();
        tokio::spawn(async move { save_resume(&german, &write, &[&fresh]).await })
    };
    wait_for_lock(&fixture.database, RESUME_APPLICATION_NAME).await?;
    holder.batch_execute("ROLLBACK").await?;
    assert!(delete.await??.deleted);
    assert!(writer.await?.is_err());
    assert_eq!(count(&admin, "resume.resumes").await?, 0);
    assert_eq!(count(&admin, "resume.resume_variants").await?, 0);
    assert_eq!(state(&admin, fresh.asset.asset_id()).await?, active());
    Ok(())
}
