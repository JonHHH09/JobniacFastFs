//! German resume store over a migrated temporary SQLite file: row round trips, replacement with
//! cascade and retirement, the owner compare-and-set, rollback of every stage, binding identity,
//! the writer serialization and insert order, and classified generation claims. All data is
//! synthetic.

mod sqlite_german_support;
mod sqlite_support;

use std::sync::Arc;
use std::time::Duration as StdDuration;

use jobniac_core::cleanup::CleanupRepository;
use jobniac_core::clock::FixedClock;
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedReservation, GenerationDescriptor, GenerationPurpose,
    IntentReservation, PreparationOutcome, sha256_hex,
};
use jobniac_core::german::resume_store::{
    DisplayOrder, EntryWrite, GermanResumeRepository, ResumeAggregateWrite, ResumeVariant,
    Revision, SectionWrite, VariantFields, VariantWrite,
};
use jobniac_infra::sqlite::generated::guards::{
    generated_asset_referenced, guard_document_path_reference,
};
use jobniac_infra::sqlite::german::SqliteGermanResumeRepository;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use rusqlite::{Connection, params};
use sqlite_german_support::{
    NOT_CONFIGURED, RESUME_INVALID, SequenceClock, TestResult, UNAVAILABLE, asset_state,
    attach_photo, bullet_row, cleanup_repository, entry_row, fail_at, insert_letter, insert_owners,
    later, now, pdf, refusal, resume_row, resume_write, revision, save_resume, section_row,
    snapshot, start, stored, table_rows, variant_row, variant_write,
};
use uuid::Uuid;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

fn variants_of(write: &ResumeAggregateWrite) -> Vec<ResumeVariant> {
    write
        .variants()
        .iter()
        .map(|variant| variant.variant().clone())
        .collect()
}

fn refs(bindings: &[GeneratedReservation]) -> Vec<&GeneratedReservation> {
    bindings.iter().collect()
}

fn count(admin: &Connection, sql: &str, key: Uuid) -> Result<i64, BoxError> {
    Ok(admin.query_row(sql, [key.to_string()], |row| row.get(0))?)
}

#[tokio::test]
async fn nested_rows_cardinality_and_source_read_order() -> TestResult {
    for count in 0..=2usize {
        let fixture = start()?;
        let admin = fixture.admin();
        let mut bindings = Vec::new();
        for _ in 0..count {
            bindings.push(fixture.active_binding().await?);
        }
        let write = resume_write(fixture.profile, fixture.job, &refs(&bindings))?;
        let repository = fixture.resumes(later());
        let result = save_resume(&repository, &write, &refs(&bindings)).await?;
        assert_eq!(&result.resume, write.resume());
        assert_eq!(result.variants, variants_of(&write));
        assert!(result.previous_variants.is_empty());
        let id = write.resume().id();
        assert_eq!(
            repository.find_by_id(id).await?.as_ref(),
            Some(write.resume())
        );
        // The format of a read is normalized like the stored one.
        assert_eq!(
            repository
                .find_by_profile_job_format(fixture.profile, fixture.job, " GERMANY ")
                .await?
                .as_ref(),
            Some(write.resume())
        );
        assert_eq!(repository.find_by_id(Uuid::new_v4()).await?, None);
        assert!(repository.find_variants(Uuid::new_v4()).await?.is_empty());
        let mut sorted = variants_of(&write);
        sorted.sort_by(|a, b| a.language().cmp(b.language()));
        assert_eq!(repository.find_variants(id).await?, sorted);
        for variant_write in write.variants() {
            let section_write = &variant_write.sections()[0];
            let entry_write = &section_write.entries()[0];
            let section: (String, String, String, i64) = admin.query_row(
                "SELECT variant_id, section_type, title, display_order \
                 FROM resume_resume_sections WHERE id = ?1",
                [section_write.section().id().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            assert_eq!(section.0, variant_write.variant().id().to_string());
            assert_eq!(section.1, "experience");
            assert_eq!(section.2, "Work");
            assert_eq!(section.3, 0);
            type Entry = (
                String,
                i64,
                String,
                String,
                Option<String>,
                Option<String>,
                Option<String>,
                String,
            );
            let entry: Entry = admin.query_row(
                "SELECT entry_type, display_order, title, organization, location, \
                 start_date, end_date, metadata FROM resume_resume_entries WHERE id = ?1",
                [entry_write.entry().id().to_string()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )?;
            assert_eq!(entry.0, "experience");
            assert_eq!(entry.1, 7);
            assert_eq!(entry.2, "Developer");
            assert_eq!(entry.3, "Company");
            assert_eq!(entry.4, None);
            assert_eq!(entry.5.as_deref(), Some("2020-02-29"));
            assert_eq!(entry.6, None);
            assert_eq!(entry.7, "{\"opaque\": true}");
            let mut statement = admin.prepare(
                "SELECT id, display_order, text FROM resume_resume_entry_bullets \
                 WHERE entry_id = ?1",
            )?;
            let bullets: Vec<(String, i64, String)> = statement
                .query_map([entry_write.entry().id().to_string()], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect::<Result<_, _>>()?;
            assert_eq!(bullets.len(), 1);
            assert_eq!(bullets[0].0, entry_write.bullets()[0].id().to_string());
            assert_eq!(bullets[0].1, 4);
            assert_eq!(bullets[0].2, "Built something");
        }
        for binding in &bindings {
            assert_eq!(asset_state(&admin, binding)?.lifecycle, "ACTIVE");
        }
    }
    Ok(())
}

#[tokio::test]
async fn replacement_cascades_old_tree_and_letters_with_atomic_cleanup_intents() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let old = [
        fixture.active_binding().await?,
        fixture.active_binding().await?,
    ];
    let letter_binding = fixture.active_binding().await?;
    let new = [fixture.active_binding().await?];
    let repository = fixture.resumes(later());
    let before = resume_write(fixture.profile, fixture.job, &refs(&old))?;
    save_resume(&repository, &before, &refs(&old)).await?;
    insert_letter(&admin, before.resume(), &letter_binding)?;
    let after = resume_write(fixture.profile, fixture.job, &refs(&new))?;
    let result = save_resume(&repository, &after, &refs(&new)).await?;
    let mut expected = variants_of(&before);
    expected.sort_by(|a, b| a.language().cmp(b.language()));
    assert_eq!(result.previous_variants, expected);
    assert_eq!(repository.find_by_id(before.resume().id()).await?, None);
    assert_eq!(
        repository.find_by_id(after.resume().id()).await?.as_ref(),
        Some(after.resume())
    );
    let rows = snapshot(&admin)?;
    assert_eq!(table_rows(&rows, "resume_resumes").len(), 1);
    assert_eq!(table_rows(&rows, "resume_resume_entries").len(), 1);
    for table in [
        "cover_letter_cover_letters",
        "cover_letter_cover_letter_variants",
        "cover_letter_cover_letter_paragraphs",
    ] {
        assert!(table_rows(&rows, table).is_empty());
    }
    for binding in old.iter().chain([&letter_binding]) {
        let state = asset_state(&admin, binding)?;
        assert_eq!(state.lifecycle, "RETIRING");
        assert_eq!(state.status.as_deref(), Some("PENDING"));
        assert!(state.retirement_id.is_some());
        assert_eq!(state.retirement_id, state.task_retirement_id);
        assert!(state.task_id.is_some());
        assert_eq!(state.retired_at, Some(later()));
        assert_eq!(state.next_attempt_at, Some(later()));
    }
    assert_eq!(asset_state(&admin, &new[0])?.lifecycle, "ACTIVE");
    assert_eq!(table_rows(&rows, "document_documents").len(), 4);
    assert_eq!(table_rows(&rows, "document_blobs").len(), 4);
    Ok(())
}

fn referenced(admin: &Connection, binding: &GeneratedReservation) -> Result<bool, BoxError> {
    Ok(generated_asset_referenced(
        admin,
        Some(binding.document_id),
        binding.asset.relative_path(),
    )?)
}

#[tokio::test]
async fn shared_or_reused_active_binding_is_not_retired() -> TestResult {
    for preserve in ["photo", "reuse"] {
        let fixture = start()?;
        let admin = fixture.admin();
        let binding = fixture.active_binding().await?;
        let repository = fixture.resumes(later());
        let before = resume_write(fixture.profile, fixture.job, &[&binding])?;
        save_resume(&repository, &before, &[&binding]).await?;
        let after = if preserve == "photo" {
            attach_photo(&admin, fixture.profile, &binding)?;
            resume_write(fixture.profile, fixture.job, &[])?
        } else {
            resume_write(fixture.profile, fixture.job, &[&binding])?
        };
        let bindings: Vec<&GeneratedReservation> = if preserve == "photo" {
            Vec::new()
        } else {
            vec![&binding]
        };
        save_resume(&repository, &after, &bindings).await?;
        let state = asset_state(&admin, &binding)?;
        assert_eq!(state.lifecycle, "ACTIVE");
        assert!(state.task_id.is_none());
        assert!(referenced(&admin, &binding)?);
    }
    Ok(())
}

#[tokio::test]
async fn stale_numeric_or_timestamp_snapshot_preserves_every_row() -> TestResult {
    for (owner, change) in [
        ("profile", "numeric"),
        ("profile", "timestamp"),
        ("job", "numeric"),
        ("job", "timestamp"),
    ] {
        let fixture = start()?;
        let admin = fixture.admin();
        let old = fixture.active_binding().await?;
        let new = fixture.active_binding().await?;
        let repository = fixture.resumes(later());
        let before = resume_write(fixture.profile, fixture.job, &[&old])?;
        save_resume(&repository, &before, &[&old]).await?;
        let candidate = resume_write(fixture.profile, fixture.job, &[&new])?;
        let (table, identifier) = if owner == "profile" {
            ("profile_profiles", fixture.profile)
        } else {
            ("job_jobs", fixture.job)
        };
        if change == "numeric" {
            admin.execute(
                &format!("UPDATE {table} SET revision = revision + 1 WHERE id = ?1"),
                [identifier.to_string()],
            )?;
        } else {
            admin.execute(
                &format!("UPDATE {table} SET updated_at = ?1 WHERE id = ?2"),
                params![stored(later()), identifier.to_string()],
            )?;
        }
        let rows = snapshot(&admin)?;
        let (code, message) = refusal(save_resume(&repository, &candidate, &[&new]).await)?;
        assert_eq!(code.code(), "conflict");
        assert_eq!(message, "German resume source revision conflict");
        assert_eq!(snapshot(&admin)?, rows);
    }
    Ok(())
}

#[tokio::test]
async fn missing_source_owner_does_not_change_existing_tree() -> TestResult {
    for owner in ["profile", "job"] {
        let fixture = start()?;
        let admin = fixture.admin();
        let binding = fixture.active_binding().await?;
        let repository = fixture.resumes(later());
        let before = resume_write(fixture.profile, fixture.job, &[&binding])?;
        save_resume(&repository, &before, &[&binding]).await?;
        let (profile, job) = if owner == "profile" {
            (Uuid::new_v4(), fixture.job)
        } else {
            (fixture.profile, Uuid::new_v4())
        };
        let candidate = resume_write(profile, job, &[])?;
        let rows = snapshot(&admin)?;
        let (code, message) = refusal(save_resume(&repository, &candidate, &[]).await)?;
        assert_eq!(code.code(), "not_found");
        assert_eq!(message, "German resume source owner was not found");
        assert_eq!(snapshot(&admin)?, rows);
    }
    Ok(())
}

#[tokio::test]
async fn matching_numeric_preconditions_preserve_distinct_timestamp_provenance() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    admin.execute(
        "UPDATE profile_profiles SET revision = 3 WHERE id = ?1",
        [fixture.profile.to_string()],
    )?;
    admin.execute(
        "UPDATE job_jobs SET revision = 5 WHERE id = ?1",
        [fixture.job.to_string()],
    )?;
    let write = resume_write(fixture.profile, fixture.job, &[])?;
    let repository = fixture.resumes(later());
    let result = repository
        .replace_germany_resume(&write, &[], revision(3), revision(5), None)
        .await?;
    assert_eq!(result.resume.profile_revision(), now());
    assert_eq!(result.resume.job_revision(), now());
    assert_eq!(
        repository.find_by_id(write.resume().id()).await?.as_ref(),
        Some(write.resume())
    );
    Ok(())
}

#[tokio::test]
async fn entry_and_bullet_orders_and_cross_table_ids_are_not_artificially_unique() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let binding = fixture.active_binding().await?;
    let parent = resume_row(fixture.profile, fixture.job)?;
    let variant = variant_row(parent.id(), "en", &binding)?;
    let section = section_row(variant.id(), 0)?;
    let first = entry_row(Uuid::new_v4(), section.id(), 7, "First")?;
    let first_bullet = bullet_row(Uuid::new_v4(), first.id(), 4, "First A")?;
    // The second entry reuses the section id, and its bullets share one position.
    let second = entry_row(section.id(), section.id(), 7, "Second")?;
    let bullets = vec![
        bullet_row(section.id(), second.id(), 4, "Second A")?,
        bullet_row(Uuid::new_v4(), second.id(), 4, "Second B")?,
    ];
    let entries = vec![
        EntryWrite::new(first, vec![first_bullet])?,
        EntryWrite::new(second.clone(), bullets)?,
    ];
    let variant_write =
        VariantWrite::new(variant, vec![SectionWrite::new(section.clone(), entries)?])?;
    let write = ResumeAggregateWrite::new(parent, vec![variant_write])?;
    save_resume(&fixture.resumes(later()), &write, &[&binding]).await?;
    assert_eq!(
        count(
            &admin,
            "SELECT count(*) FROM resume_resume_entries \
             WHERE section_id = ?1 AND display_order = 7",
            section.id()
        )?,
        2
    );
    assert_eq!(
        count(
            &admin,
            "SELECT count(*) FROM resume_resume_entry_bullets \
             WHERE entry_id = ?1 AND display_order = 4",
            second.id()
        )?,
        2
    );
    Ok(())
}

#[tokio::test]
async fn failure_rolls_back_old_tree_letters_and_all_retirement() -> TestResult {
    for (table, event, condition) in [
        ("resume_resumes", "INSERT", "TRUE"),
        ("resume_resume_variants", "INSERT", "TRUE"),
        ("resume_resume_sections", "INSERT", "TRUE"),
        ("resume_resume_entries", "INSERT", "TRUE"),
        ("resume_resume_entry_bullets", "INSERT", "TRUE"),
        (
            "jobniac_generated_assets",
            "UPDATE",
            "NEW.lifecycle = 'RETIRING'",
        ),
        ("jobniac_generated_asset_cleanups", "INSERT", "TRUE"),
    ] {
        let fixture = start()?;
        let admin = fixture.admin();
        let old = fixture.active_binding().await?;
        let letter = fixture.active_binding().await?;
        let new = fixture.active_binding().await?;
        let repository = fixture.resumes(later());
        let before = resume_write(fixture.profile, fixture.job, &[&old])?;
        save_resume(&repository, &before, &[&old]).await?;
        insert_letter(&admin, before.resume(), &letter)?;
        let after = resume_write(fixture.profile, fixture.job, &[&new])?;
        let rows = snapshot(&admin)?;
        fail_at(&admin, table, event, condition)?;
        let result = save_resume(&repository, &after, &[&new]).await;
        // A trigger failure is an integrity error of the write and keeps its mapping.
        assert!(result.is_err(), "{table}");
        assert_eq!(snapshot(&admin)?, rows, "{table}");
        assert_eq!(
            repository.find_by_id(before.resume().id()).await?.as_ref(),
            Some(before.resume())
        );
    }
    Ok(())
}

#[tokio::test]
async fn unavailable_new_binding_leaves_previous_resume_intact() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let old = fixture.active_binding().await?;
    let new = fixture.reserved_binding().await?;
    let repository = fixture.resumes(later());
    let before = resume_write(fixture.profile, fixture.job, &[&old])?;
    save_resume(&repository, &before, &[&old]).await?;
    assert_eq!(asset_state(&admin, &new)?.lifecycle, "RESERVED");
    let rows = snapshot(&admin)?;
    let candidate = resume_write(fixture.profile, fixture.job, &[&new])?;
    let (code, message) = refusal(save_resume(&repository, &candidate, &[&new]).await)?;
    assert_eq!((code, message.as_str()), RESUME_INVALID);
    assert_eq!(snapshot(&admin)?, rows);
    Ok(())
}

#[tokio::test]
async fn exact_binding_identity_is_required_without_mutation() -> TestResult {
    for mismatch in ["root", "hash", "size", "document", "path"] {
        let fixture = start()?;
        let admin = fixture.admin();
        let binding = fixture.active_binding().await?;
        let mut write = resume_write(fixture.profile, fixture.job, &[&binding])?;
        let asset = &binding.asset;
        let altered = match mismatch {
            "root" => GeneratedReservation {
                asset: AssetIdentity::new(
                    asset.asset_id(),
                    Uuid::new_v4(),
                    asset.sha256(),
                    asset.size_bytes(),
                )?,
                document_id: binding.document_id,
            },
            "hash" => GeneratedReservation {
                asset: AssetIdentity::new(
                    asset.asset_id(),
                    asset.root_id(),
                    &"0".repeat(64),
                    asset.size_bytes(),
                )?,
                document_id: binding.document_id,
            },
            "size" => GeneratedReservation {
                asset: AssetIdentity::new(
                    asset.asset_id(),
                    asset.root_id(),
                    asset.sha256(),
                    asset.size_bytes() + 1,
                )?,
                document_id: binding.document_id,
            },
            "document" => GeneratedReservation {
                asset: asset.clone(),
                document_id: Uuid::new_v4(),
            },
            _ => {
                let parent = resume_row(fixture.profile, fixture.job)?;
                let variant = ResumeVariant::new(VariantFields {
                    id: Uuid::new_v4(),
                    resume_id: parent.id(),
                    language: "en",
                    document_id: binding.document_id,
                    file_path: "unowned-legacy.pdf",
                    created_at: now(),
                    updated_at: later(),
                })?;
                write = ResumeAggregateWrite::new(parent, vec![variant_write(variant)?])?;
                binding.clone()
            }
        };
        let rows = snapshot(&admin)?;
        let (code, message) =
            refusal(save_resume(&fixture.resumes(later()), &write, &[&altered]).await)?;
        assert_eq!((code, message.as_str()), RESUME_INVALID, "{mismatch}");
        assert_eq!(snapshot(&admin)?, rows, "{mismatch}");
    }
    Ok(())
}

/// A raw connection that holds the database write lock until it commits, so a repository write
/// queues behind it exactly as it queues behind another writer.
fn hold_write_lock(admin: &Connection) -> Result<(), BoxError> {
    admin.execute_batch("BEGIN IMMEDIATE")?;
    Ok(())
}

#[tokio::test]
async fn readers_see_old_complete_state_until_rows_and_cleanup_commit() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let old = fixture.active_binding().await?;
    let new = fixture.active_binding().await?;
    let repository = fixture.resumes(later());
    let before = resume_write(fixture.profile, fixture.job, &[&old])?;
    save_resume(&repository, &before, &[&old]).await?;
    let after = resume_write(fixture.profile, fixture.job, &[&new])?;
    let rows = snapshot(&admin)?;
    hold_write_lock(&admin)?;
    let task = tokio::spawn({
        let repository = repository.clone();
        let after = after.clone();
        let bindings = vec![new.clone()];
        async move {
            repository
                .replace_germany_resume(&after, &bindings, revision(0), revision(0), None)
                .await
        }
    });
    tokio::time::sleep(StdDuration::from_millis(200)).await;
    assert!(!task.is_finished());
    assert_eq!(snapshot(&fixture.admin())?, rows);
    assert_eq!(
        repository.find_by_id(before.resume().id()).await?.as_ref(),
        Some(before.resume())
    );
    assert_eq!(repository.find_by_id(after.resume().id()).await?, None);
    admin.execute_batch("COMMIT")?;
    assert_eq!(&task.await??.resume, after.resume());
    assert_eq!(asset_state(&admin, &old)?.lifecycle, "RETIRING");
    assert_eq!(asset_state(&admin, &new)?.lifecycle, "ACTIVE");
    Ok(())
}

#[tokio::test]
async fn competing_pair_replacements_serialize_without_duplicate_retirement() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let first_binding = fixture.active_binding().await?;
    let second_binding = fixture.active_binding().await?;
    let repository = fixture.resumes(later());
    let first = resume_write(fixture.profile, fixture.job, &[&first_binding])?;
    let second = resume_write(fixture.profile, fixture.job, &[&second_binding])?;
    let spawn = |write: &ResumeAggregateWrite, binding: &GeneratedReservation| {
        let repository = repository.clone();
        let write = write.clone();
        let bindings = vec![binding.clone()];
        tokio::spawn(async move {
            repository
                .replace_germany_resume(&write, &bindings, revision(0), revision(0), None)
                .await
        })
    };
    let one = spawn(&first, &first_binding);
    let two = spawn(&second, &second_binding);
    let (one, two) = (one.await??, two.await??);
    // The single writer runs them one after the other: exactly one of them replaced the other.
    let winner_is_second = one.previous_variants.is_empty();
    let (loser_binding, winner, winner_binding) = if winner_is_second {
        (&first_binding, &second, &second_binding)
    } else {
        (&second_binding, &first, &first_binding)
    };
    let replaced = if winner_is_second { &two } else { &one };
    assert_eq!(replaced.previous_variants.len(), 1);
    assert_eq!(
        fixture
            .resumes(later())
            .find_by_profile_job_format(fixture.profile, fixture.job, "germany")
            .await?
            .as_ref(),
        Some(winner.resume())
    );
    assert_eq!(asset_state(&admin, loser_binding)?.lifecycle, "RETIRING");
    assert_eq!(asset_state(&admin, winner_binding)?.lifecycle, "ACTIVE");
    let rows = snapshot(&admin)?;
    assert_eq!(
        table_rows(&rows, "jobniac_generated_asset_cleanups").len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn unowned_legacy_document_survives_old_resume_removal() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let (blob, document) = (Uuid::new_v4(), Uuid::new_v4());
    let content = pdf("legacy");
    let size = i64::try_from(content.len())?;
    admin.execute(
        "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            blob.to_string(),
            sha256_hex(&content),
            size,
            content,
            stored(now())
        ],
    )?;
    admin.execute(
        "INSERT INTO document_documents (id, blob_id, original_file_name, media_type, \
         created_at, updated_at) VALUES (?1, ?2, 'legacy.pdf', 'application/pdf', ?3, ?3)",
        params![document.to_string(), blob.to_string(), stored(now())],
    )?;
    let repository = fixture.resumes(later());
    let old = resume_write(fixture.profile, fixture.job, &[])?;
    save_resume(&repository, &old, &[]).await?;
    admin.execute(
        "INSERT INTO resume_resume_variants \
         (id, resume_id, language, document_id, file_path, created_at, updated_at) \
         VALUES (?1, ?2, 'en', ?3, 'legacy/path.pdf', ?4, ?5)",
        params![
            Uuid::new_v4().to_string(),
            old.resume().id().to_string(),
            document.to_string(),
            stored(now()),
            stored(later())
        ],
    )?;
    let replacement = resume_write(fixture.profile, fixture.job, &[])?;
    let result = save_resume(&repository, &replacement, &[]).await?;
    assert_eq!(result.previous_variants[0].document_id(), document);
    assert_eq!(
        count(
            &admin,
            "SELECT count(*) FROM document_documents WHERE id = ?1",
            document
        )?,
        1
    );
    let rows = snapshot(&admin)?;
    assert!(table_rows(&rows, "jobniac_generated_assets").is_empty());
    assert!(table_rows(&rows, "jobniac_generated_asset_cleanups").is_empty());
    Ok(())
}

#[tokio::test]
async fn nested_inserts_preserve_encounter_order_in_each_table() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let bindings = [
        fixture.active_binding().await?,
        fixture.active_binding().await?,
    ];
    let parent = resume_row(fixture.profile, fixture.job)?;
    let mut variants = Vec::new();
    let mut expected: Vec<(&str, Vec<Uuid>)> = vec![
        ("resume_resume_variants", Vec::new()),
        ("resume_resume_sections", Vec::new()),
        ("resume_resume_entries", Vec::new()),
        ("resume_resume_entry_bullets", Vec::new()),
    ];
    for (binding, language) in bindings.iter().zip(["en", "de"]) {
        let variant = variant_row(parent.id(), language, binding)?;
        expected[0].1.push(variant.id());
        let mut sections = Vec::new();
        for section_order in [8, 2] {
            let section = section_row(variant.id(), section_order)?;
            expected[1].1.push(section.id());
            let mut entries = Vec::new();
            for entry_order in [9, 3] {
                let entry = entry_row(Uuid::new_v4(), section.id(), entry_order, "Entry")?;
                expected[2].1.push(entry.id());
                let mut bullets = Vec::new();
                for position in [7, 1] {
                    let bullet = bullet_row(Uuid::new_v4(), entry.id(), position, "Bullet")?;
                    expected[3].1.push(bullet.id());
                    bullets.push(bullet);
                }
                entries.push(EntryWrite::new(entry, bullets)?);
            }
            sections.push(SectionWrite::new(section, entries)?);
        }
        variants.push(VariantWrite::new(variant, sections)?);
    }
    let write = ResumeAggregateWrite::new(parent, variants)?;
    admin.execute_batch(
        "CREATE TABLE synthetic_insert_order \
         (ordinal INTEGER PRIMARY KEY AUTOINCREMENT, table_name TEXT NOT NULL, \
          row_id TEXT NOT NULL);",
    )?;
    for (table, _) in &expected {
        admin.execute_batch(&format!(
            "CREATE TRIGGER synthetic_insert_order_{table} AFTER INSERT ON {table} \
             FOR EACH ROW BEGIN \
                 INSERT INTO synthetic_insert_order (table_name, row_id) \
                 VALUES ('{table}', NEW.id); END"
        ))?;
    }
    let result = save_resume(
        &fixture.resumes(later()),
        &write,
        &[&bindings[0], &bindings[1]],
    )
    .await?;
    assert_eq!(result.variants, variants_of(&write));
    for (table, identifiers) in &expected {
        let mut statement = admin.prepare(
            "SELECT row_id FROM synthetic_insert_order WHERE table_name = ?1 ORDER BY ordinal",
        )?;
        let observed = statement
            .query_map([table], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let identifiers: Vec<String> = identifiers.iter().map(Uuid::to_string).collect();
        assert_eq!(observed, identifiers, "{table}");
    }
    Ok(())
}

#[tokio::test]
async fn replacement_preserves_resumes_letters_and_assets_for_other_pairs() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let (other_profile, other_job) = (Uuid::new_v4(), Uuid::new_v4());
    insert_owners(&admin, other_profile, other_job, "Other")?;
    let repository = fixture.resumes(later());
    for (profile, job) in [
        (fixture.profile, other_job),
        (other_profile, fixture.job),
        (other_profile, other_job),
    ] {
        let binding = fixture.active_binding().await?;
        let protected = resume_write(profile, job, &[&binding])?;
        save_resume(&repository, &protected, &[&binding]).await?;
        insert_letter(&admin, protected.resume(), &binding)?;
    }
    let protected_rows = snapshot(&admin)?;
    let target = fixture.active_binding().await?;
    let before = resume_write(fixture.profile, fixture.job, &[&target])?;
    save_resume(&repository, &before, &[&target]).await?;
    insert_letter(&admin, before.resume(), &target)?;
    let after = resume_write(fixture.profile, fixture.job, &[])?;
    save_resume(&repository, &after, &[]).await?;
    let observed = snapshot(&admin)?;
    for (table, rows) in sqlite_german_support::TABLES.iter().zip(&protected_rows) {
        for row in rows {
            assert!(
                table_rows(&observed, table).contains(row),
                "unrelated row changed in {table}"
            );
        }
    }
    assert_eq!(repository.find_by_id(before.resume().id()).await?, None);
    assert_eq!(
        repository.find_by_id(after.resume().id()).await?.as_ref(),
        Some(after.resume())
    );
    assert_eq!(asset_state(&admin, &target)?.lifecycle, "RETIRING");
    assert_eq!(
        table_rows(&observed, "jobniac_generated_asset_cleanups").len(),
        1
    );
    Ok(())
}

/// A bilingual write whose two variants carry classified claims of one operation.
async fn classified_write(
    fixture: &sqlite_german_support::Fixture,
) -> Result<
    (
        ResumeAggregateWrite,
        Vec<GeneratedReservation>,
        Vec<IntentReservation>,
    ),
    BoxError,
> {
    let parent = resume_row(fixture.profile, fixture.job)?;
    let operation = Uuid::new_v4();
    let mut claims = Vec::new();
    for language in ["en", "de"] {
        let descriptor = GenerationDescriptor::new(
            operation,
            GenerationPurpose::GermanResume,
            Some(fixture.profile),
            Some(fixture.job),
            Some(parent.id()),
            Some(language.to_owned()),
        )?;
        claims.push(fixture.classified_claim(&descriptor).await?);
    }
    let bindings: Vec<GeneratedReservation> = claims
        .iter()
        .map(|claim| claim.reservation.clone())
        .collect();
    let mut variants = Vec::new();
    for (binding, language) in bindings.iter().zip(["en", "de"]) {
        variants.push(variant_write(variant_row(parent.id(), language, binding)?)?);
    }
    Ok((
        ResumeAggregateWrite::new(parent, variants)?,
        bindings,
        claims,
    ))
}

#[tokio::test]
async fn classified_bilingual_write_requires_all_exact_live_claims() -> TestResult {
    for defect in ["missing", "expired", "token", "scope", "operation"] {
        let fixture = start()?;
        let admin = fixture.admin();
        let (write, bindings, claims) = classified_write(&fixture).await?;
        let rows = snapshot(&admin)?;
        let mut clock = later();
        let mut proof = Some(claims.clone());
        match defect {
            "missing" => proof = None,
            "expired" => {
                clock = claims[1]
                    .intent
                    .lease_expires_at
                    .ok_or("a pending intent has an expiry")?;
            }
            "token" => {
                if let Some(proof) = proof.as_mut() {
                    proof[1].intent.lease_token = Some(Uuid::new_v4());
                }
            }
            "scope" => {
                if let Some(proof) = proof.as_mut() {
                    proof[1].intent.descriptor.resume_id = Some(Uuid::new_v4());
                }
            }
            _ => {
                if let Some(proof) = proof.as_mut() {
                    proof[1].intent.descriptor.operation_id = Uuid::new_v4();
                }
            }
        }
        let repository = fixture.resumes(clock);
        let result = repository
            .replace_germany_resume(
                &write,
                &bindings,
                revision(0),
                revision(0),
                proof.as_deref(),
            )
            .await;
        let (code, message) = refusal(result)?;
        assert_eq!((code, message.as_str()), RESUME_INVALID, "{defect}");
        assert_eq!(snapshot(&admin)?, rows, "{defect}");
    }
    Ok(())
}

#[tokio::test]
async fn classified_bilingual_completion_rolls_back_entire_replacement() -> TestResult {
    for failure in ["last_completion", "expiry_after_links"] {
        let fixture = start()?;
        let admin = fixture.admin();
        let old = fixture.active_binding().await?;
        let before_write = resume_write(fixture.profile, fixture.job, &[&old])?;
        save_resume(&fixture.resumes(later()), &before_write, &[&old]).await?;
        let (write, bindings, claims) = classified_write(&fixture).await?;
        let rows = snapshot(&admin)?;
        let (repository, expected_failure) = if failure == "last_completion" {
            let last = claims
                .iter()
                .map(IntentReservation::asset_id)
                .max()
                .ok_or("two claims")?;
            fail_at(
                &admin,
                "jobniac_generation_intents",
                "UPDATE",
                &format!("NEW.state = 'COMPLETED' AND NEW.asset_id = '{last}'"),
            )?;
            (fixture.resumes(later()), true)
        } else {
            let expiry = claims[0]
                .intent
                .lease_expires_at
                .ok_or("a pending intent has an expiry")?;
            let clock = Arc::new(SequenceClock::new(vec![later(), later(), expiry]));
            (fixture.resumes_with(clock), false)
        };
        let result = repository
            .replace_germany_resume(&write, &bindings, revision(0), revision(0), Some(&claims))
            .await;
        if expected_failure {
            // The failing statement is an integrity error and keeps its mapping.
            assert!(result.is_err(), "{failure}");
        } else {
            let (code, message) = refusal(result)?;
            assert_eq!((code, message.as_str()), RESUME_INVALID);
        }
        assert_eq!(snapshot(&admin)?, rows, "{failure}");
        assert_eq!(asset_state(&admin, &old)?.lifecycle, "ACTIVE");
    }
    Ok(())
}

#[tokio::test]
async fn classified_bilingual_completion_is_atomic_and_replacement_retires_history() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let (write, bindings, claims) = classified_write(&fixture).await?;
    let repository = fixture.resumes(later());
    repository
        .replace_germany_resume(&write, &bindings, revision(0), revision(0), Some(&claims))
        .await?;
    for claim in &claims {
        let state: String = admin.query_row(
            "SELECT state FROM jobniac_generation_intents WHERE asset_id = ?1",
            [claim.asset_id().to_string()],
            |row| row.get(0),
        )?;
        assert_eq!(state, "COMPLETED");
    }
    let rows = snapshot(&admin)?;
    let replay = repository
        .replace_germany_resume(&write, &bindings, revision(0), revision(0), Some(&claims))
        .await;
    assert!(replay.is_err());
    assert_eq!(snapshot(&admin)?, rows);
    let empty = resume_write(fixture.profile, fixture.job, &[])?;
    save_resume(&repository, &empty, &[]).await?;
    for binding in &bindings {
        assert_eq!(asset_state(&admin, binding)?.lifecycle, "RETIRING");
    }
    Ok(())
}

/// The resume repository without a database, which must refuse every call.
fn unconfigured_resumes() -> SqliteGermanResumeRepository {
    SqliteGermanResumeRepository::new(
        SqliteUnitOfWork::new(SqlitePool::unconfigured()),
        Arc::new(FixedClock::new(later())),
    )
}

#[test]
fn invalid_numeric_values_are_refused_before_any_repository_call() {
    assert!(Revision::new(-1).is_none());
    assert!(DisplayOrder::new(-1).is_none());
}

#[tokio::test]
async fn invalid_read_format_is_refused_before_connecting() -> TestResult {
    let repository = unconfigured_resumes();
    for format in ["", "   ", "FRANCE"] {
        let (code, message) = refusal(
            repository
                .find_by_profile_job_format(Uuid::new_v4(), Uuid::new_v4(), format)
                .await,
        )?;
        assert_eq!((code, message.as_str()), RESUME_INVALID, "{format:?}");
    }
    Ok(())
}

#[tokio::test]
async fn unconfigured_operations_use_the_fixed_error_without_connecting() -> TestResult {
    let repository = unconfigured_resumes();
    let (code, message) = refusal(repository.find_by_id(Uuid::new_v4()).await)?;
    assert_eq!((code, message.as_str()), NOT_CONFIGURED);
    let (code, message) = refusal(repository.find_variants(Uuid::new_v4()).await)?;
    assert_eq!((code, message.as_str()), NOT_CONFIGURED);
    let (code, message) = refusal(
        repository
            .find_by_profile_job_format(Uuid::new_v4(), Uuid::new_v4(), "germany")
            .await,
    )?;
    assert_eq!((code, message.as_str()), NOT_CONFIGURED);
    Ok(())
}

#[tokio::test]
async fn closed_pool_failures_are_sanitized_for_every_operation() -> TestResult {
    let fixture = start()?;
    let pool = SqlitePool::open(fixture.sqlite.path())?;
    pool.close();
    let repository = SqliteGermanResumeRepository::new(
        SqliteUnitOfWork::new(pool),
        Arc::new(FixedClock::new(later())),
    );
    let (code, message) = refusal(repository.find_by_id(Uuid::new_v4()).await)?;
    assert_eq!((code, message.as_str()), UNAVAILABLE);
    let (code, message) = refusal(repository.find_variants(Uuid::new_v4()).await)?;
    assert_eq!((code, message.as_str()), UNAVAILABLE);
    let binding = fixture.active_binding().await?;
    let write = resume_write(fixture.profile, fixture.job, &[&binding])?;
    let (code, message) = refusal(save_resume(&repository, &write, &[&binding]).await)?;
    assert_eq!((code, message.as_str()), UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn scalar_supplementary_characters_roundtrip_unchanged() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let binding = fixture.active_binding().await?;
    let parent = resume_row(fixture.profile, fixture.job)?;
    let variant = variant_row(parent.id(), "en", &binding)?;
    let section = section_row(variant.id(), 0)?;
    let entry = entry_row(Uuid::new_v4(), section.id(), 7, "Pair \u{1F600}")?;
    let entry_id = entry.id();
    let bullet = bullet_row(Uuid::new_v4(), entry_id, 4, "Bullet")?;
    let write = ResumeAggregateWrite::new(
        parent,
        vec![VariantWrite::new(
            variant,
            vec![SectionWrite::new(
                section,
                vec![EntryWrite::new(entry, vec![bullet])?],
            )?],
        )?],
    )?;
    save_resume(&fixture.resumes(later()), &write, &[&binding]).await?;
    let title: String = admin.query_row(
        "SELECT title FROM resume_resume_entries WHERE id = ?1",
        [entry_id.to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(title, "Pair \u{1F600}");
    Ok(())
}

#[tokio::test]
async fn duplicate_positions_and_identities_fail_before_connecting() -> TestResult {
    let parent = resume_row(Uuid::new_v4(), Uuid::new_v4())?;
    let first = section_row(Uuid::new_v4(), 0)?;
    let second = section_row(first.variant_id(), 0)?;
    let entry = entry_row(Uuid::new_v4(), first.id(), 7, "Entry")?;
    let bullet = bullet_row(Uuid::new_v4(), entry.id(), 4, "Bullet")?;
    let entries = vec![EntryWrite::new(entry, vec![bullet])?];
    // Two sections of one variant with the same display order are refused.
    let variant = ResumeVariant::new(VariantFields {
        id: first.variant_id(),
        resume_id: parent.id(),
        language: "en",
        document_id: Uuid::new_v4(),
        file_path: "synthetic.pdf",
        created_at: now(),
        updated_at: later(),
    })?;
    let duplicate_sections = VariantWrite::new(
        variant.clone(),
        vec![
            SectionWrite::new(first, entries.clone())?,
            SectionWrite::new(second, Vec::new())?,
        ],
    )?;
    // The unconfigured repository must refuse with the validation error, never the
    // not-configured one: validation runs before any connection is attempted.
    let repository = unconfigured_resumes();
    let duplicate = ResumeAggregateWrite::new(parent.clone(), vec![duplicate_sections])?;
    let (code, message) = refusal(
        repository
            .replace_germany_resume(&duplicate, &[], revision(0), revision(0), None)
            .await,
    )?;
    assert_eq!((code, message.as_str()), RESUME_INVALID);
    // Two variant writes with the same identity are refused the same way.
    let same_identity = VariantWrite::new(variant.clone(), Vec::new())?;
    let identity = ResumeAggregateWrite::new(parent, vec![same_identity.clone(), same_identity])?;
    let (code, message) = refusal(
        repository
            .replace_germany_resume(&identity, &[], revision(0), revision(0), None)
            .await,
    )?;
    assert_eq!((code, message.as_str()), RESUME_INVALID);
    Ok(())
}

#[tokio::test]
async fn inflight_attachment_wins_and_keeps_removed_old_asset_active() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let binding = fixture.active_binding().await?;
    let repository = fixture.resumes(later());
    let before = resume_write(fixture.profile, fixture.job, &[&binding])?;
    save_resume(&repository, &before, &[&binding]).await?;
    let after = resume_write(fixture.profile, fixture.job, &[])?;
    // The attachment holds its transaction open while the replacement queues behind it.
    hold_write_lock(&admin)?;
    attach_photo(&admin, fixture.profile, &binding)?;
    let replacement = tokio::spawn({
        let repository = repository.clone();
        let after = after.clone();
        async move {
            repository
                .replace_germany_resume(&after, &[], revision(0), revision(0), None)
                .await
        }
    });
    tokio::time::sleep(StdDuration::from_millis(200)).await;
    assert!(!replacement.is_finished());
    admin.execute_batch("COMMIT")?;
    assert_eq!(&replacement.await??.resume, after.resume());
    assert_eq!(asset_state(&admin, &binding)?.lifecycle, "ACTIVE");
    let outcome = cleanup_repository(&fixture, later())
        .prepare_cleanup(binding.asset.asset_id())
        .await?;
    assert_eq!(outcome, PreparationOutcome::Referenced);
    Ok(())
}

#[tokio::test]
async fn uncertain_acknowledgement_never_retires_committed_replacement() -> TestResult {
    // The Rust port cannot lose a reply on purpose. The invariant is checked on the committed
    // state after a successful replacement: the binding stays referenced and active.
    let fixture = start()?;
    let admin = fixture.admin();
    let binding = fixture.active_binding().await?;
    let repository = fixture.resumes(later());
    let write = resume_write(fixture.profile, fixture.job, &[&binding])?;
    save_resume(&repository, &write, &[&binding]).await?;
    assert_eq!(
        repository.find_by_id(write.resume().id()).await?.as_ref(),
        Some(write.resume())
    );
    let outcome = cleanup_repository(&fixture, later())
        .prepare_cleanup(binding.asset.asset_id())
        .await?;
    assert_eq!(outcome, PreparationOutcome::Referenced);
    let state = asset_state(&admin, &binding)?;
    assert_eq!(state.lifecycle, "ACTIVE");
    assert_eq!(state.task_id, None);
    Ok(())
}

#[tokio::test]
async fn corrupt_blob_content_is_rejected_without_replacement() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let declared = pdf("declared");
    let (asset_id, blob, document) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let digest = sha256_hex(&declared);
    let size = i64::try_from(declared.len())?;
    // A blob whose metadata matches the declared identity but whose bytes are corrupt.
    let corrupt = vec![b'x'; declared.len()];
    admin.execute(
        "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![blob.to_string(), digest, size, corrupt, stored(now())],
    )?;
    admin.execute(
        "INSERT INTO document_documents (id, blob_id, original_file_name, media_type, \
         created_at, updated_at) VALUES (?1, ?2, 'synthetic.pdf', 'application/pdf', ?3, ?3)",
        params![document.to_string(), blob.to_string(), stored(now())],
    )?;
    admin.execute(
        "INSERT INTO jobniac_generated_assets (asset_id, root_id, relative_path, sha256, \
         size_bytes, document_id, lifecycle, created_at, generation_intent_required) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'RESERVED', ?7, 0)",
        params![
            asset_id.to_string(),
            fixture.root.to_string(),
            format!("{asset_id}.pdf"),
            digest,
            size,
            document.to_string(),
            stored(now())
        ],
    )?;
    admin.execute(
        "UPDATE jobniac_generated_assets SET lifecycle = 'ACTIVE', materialized_at = ?2 \
         WHERE asset_id = ?1",
        params![asset_id.to_string(), stored(now())],
    )?;
    let binding = GeneratedReservation {
        asset: AssetIdentity::new(asset_id, fixture.root, &digest, declared.len() as u64)?,
        document_id: document,
    };
    let rows = snapshot(&admin)?;
    let write = resume_write(fixture.profile, fixture.job, &[&binding])?;
    let (code, message) =
        refusal(save_resume(&fixture.resumes(later()), &write, &[&binding]).await)?;
    // The stored bytes fail their digest check, so the call is refused as an unavailable
    // database.
    assert_eq!((code, message.as_str()), UNAVAILABLE);
    assert_eq!(snapshot(&admin)?, rows);
    Ok(())
}

#[tokio::test]
async fn mismatched_owned_path_and_document_cannot_be_attached() -> TestResult {
    // Rule 14: the writers call the owned-document path guard before every variant insert. A
    // verified binding always satisfies it, so the guard is exercised directly with a document
    // of one asset and the path of another.
    let fixture = start()?;
    let admin = fixture.admin();
    let first = fixture.active_binding().await?;
    let second = fixture.active_binding().await?;
    let repository = fixture.resumes(later());
    let write = resume_write(fixture.profile, fixture.job, &[])?;
    save_resume(&repository, &write, &[]).await?;
    let rows = snapshot(&admin)?;
    let refused =
        guard_document_path_reference(&admin, first.document_id, second.asset.relative_path());
    assert!(matches!(
        refused,
        Err(jobniac_infra::sqlite::error::SqliteError::Constraint { ref sqlstate, .. })
            if sqlstate == "23514"
    ));
    assert!(
        guard_document_path_reference(&admin, first.document_id, first.asset.relative_path())
            .is_ok()
    );
    assert_eq!(snapshot(&admin)?, rows);
    Ok(())
}
