//! German cover-letter store over a migrated temporary SQLite file: the round trip, the owner and
//! resume compare-and-set, the incoming binding, rollback of every stage, retirement of the
//! replaced asset, the writer serialization shared with the resume writer, and classified claims.
//! All data is synthetic.

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
use jobniac_core::german::content::{CoverLetterInput, GermanCoverLetterContent};
use jobniac_core::german::cover_letter_store::{
    GermanCoverLetterRepository, map_cover_letter_content,
};
use jobniac_core::german::resume_store::{GermanResumeRepository, Resume, ResumeFields};
use jobniac_infra::sqlite::error::SqliteError;
use jobniac_infra::sqlite::generated::guards::{
    generated_asset_referenced, guard_document_path_reference,
};
use jobniac_infra::sqlite::german::SqliteGermanCoverLetterRepository;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use rusqlite::{Connection, params};
use sqlite_german_support::{
    Fixture, LETTER_INVALID, NOT_CONFIGURED, SequenceClock, TestResult, UNAVAILABLE, asset_state,
    attach_photo, cleanup_repository, fail_at, insert_letter, insert_owners, later, letter_write,
    now, refusal, resume_write, revision, save_letter, save_resume, snapshot, start, stored,
    table_rows,
};
use time::Duration;
use uuid::Uuid;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A fixture with the pair's resume stored (no variants), and that resume.
async fn prepared() -> Result<(Fixture, Resume), BoxError> {
    let fixture = start()?;
    let write = resume_write(fixture.profile, fixture.job, &[])?;
    save_resume(&fixture.resumes(later()), &write, &[]).await?;
    let resume = write.resume().clone();
    Ok((fixture, resume))
}

/// A resume row with the identity of `resume` but other owners or timestamps.
fn resume_like(resume: &Resume, profile: Uuid, job: Uuid) -> Result<Resume, BoxError> {
    Ok(Resume::new(ResumeFields {
        id: resume.id(),
        profile_id: profile,
        job_id: job,
        format: "germany",
        profile_revision: resume.profile_revision(),
        job_revision: resume.job_revision(),
        created_at: resume.created_at(),
        updated_at: resume.updated_at(),
    })?)
}

fn conflict_message() -> (&'static str, &'static str) {
    ("conflict", "German cover-letter source revision conflict")
}

fn count(admin: &Connection, sql: &str, key: Uuid) -> Result<i64, BoxError> {
    Ok(admin.query_row(sql, [key.to_string()], |row| row.get(0))?)
}

#[tokio::test]
async fn roundtrip_and_paragraph_read_order() -> TestResult {
    for orders in [Vec::<i64>::new(), vec![8, 2]] {
        let (fixture, resume) = prepared().await?;
        let admin = fixture.admin();
        let binding = fixture.active_binding().await?;
        let write = letter_write(&resume, &binding, &orders)?;
        let repository = fixture.letters(later());
        let result = save_letter(&repository, &write, &binding).await?;
        assert_eq!(&result.cover_letter, write.cover_letter());
        assert_eq!(&result.variant, write.variant().variant());
        assert!(result.previous_variants.is_empty());
        let id = write.cover_letter().id();
        assert_eq!(
            repository.find_by_id(id).await?.as_ref(),
            Some(write.cover_letter())
        );
        assert_eq!(
            repository
                .find_by_profile_job_resume(resume.profile_id(), resume.job_id(), resume.id())
                .await?
                .as_ref(),
            Some(write.cover_letter())
        );
        assert_eq!(
            repository.find_variants(id).await?,
            vec![write.variant().variant().clone()]
        );
        let mut sorted = write.variant().paragraphs().to_vec();
        sorted.sort_by_key(|paragraph| paragraph.display_order());
        assert_eq!(
            repository
                .find_paragraphs(write.variant().variant().id())
                .await?,
            sorted
        );
        assert_eq!(repository.find_by_id(Uuid::new_v4()).await?, None);
        assert_eq!(
            repository
                .find_by_profile_job_resume(resume.profile_id(), resume.job_id(), Uuid::new_v4())
                .await?,
            None
        );
        assert!(repository.find_variants(Uuid::new_v4()).await?.is_empty());
        assert!(repository.find_paragraphs(Uuid::new_v4()).await?.is_empty());
        assert_eq!(asset_state(&admin, &binding)?.lifecycle, "ACTIVE");
        for paragraph in write.variant().paragraphs() {
            let row: (String, i64, String) = admin.query_row(
                "SELECT variant_id, display_order, text \
                 FROM cover_letter_cover_letter_paragraphs WHERE id = ?1",
                [paragraph.id().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            assert_eq!(row.0, paragraph.variant_id().to_string());
            assert_eq!(row.1, i64::from(paragraph.display_order().get()));
            assert_eq!(row.2, paragraph.text());
        }
    }
    Ok(())
}

#[tokio::test]
async fn paragraph_inserts_follow_encounter_order() -> TestResult {
    let (fixture, resume) = prepared().await?;
    let admin = fixture.admin();
    let binding = fixture.active_binding().await?;
    let write = letter_write(&resume, &binding, &[8, 2])?;
    admin.execute_batch(
        "CREATE TABLE synthetic_letter_order ( \
             ordinal INTEGER PRIMARY KEY AUTOINCREMENT, row_id TEXT); \
         CREATE TRIGGER synthetic_letter_order AFTER INSERT \
         ON cover_letter_cover_letter_paragraphs FOR EACH ROW BEGIN \
             INSERT INTO synthetic_letter_order(row_id) VALUES (NEW.id); END;",
    )?;
    save_letter(&fixture.letters(later()), &write, &binding).await?;
    let mut statement =
        admin.prepare("SELECT row_id FROM synthetic_letter_order ORDER BY ordinal")?;
    let observed = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let expected: Vec<String> = write
        .variant()
        .paragraphs()
        .iter()
        .map(|paragraph| paragraph.id().to_string())
        .collect();
    assert_eq!(observed, expected);
    Ok(())
}

#[tokio::test]
async fn stale_owner_or_resume_provenance_preserves_every_row() -> TestResult {
    for owner in ["profile", "job"] {
        for change in ["numeric", "timestamp", "stored_resume_timestamp"] {
            let (fixture, resume) = prepared().await?;
            let admin = fixture.admin();
            let binding = fixture.active_binding().await?;
            let write = letter_write(&resume, &binding, &[8, 2])?;
            let (table, identifier) = if owner == "profile" {
                ("profile_profiles", resume.profile_id())
            } else {
                ("job_jobs", resume.job_id())
            };
            match change {
                "stored_resume_timestamp" => {
                    admin.execute(
                        &format!("UPDATE resume_resumes SET {owner}_revision = ?1 WHERE id = ?2"),
                        params![
                            stored(now() - Duration::seconds(1)),
                            resume.id().to_string()
                        ],
                    )?;
                }
                "numeric" => {
                    admin.execute(
                        &format!("UPDATE {table} SET revision = revision + 1 WHERE id = ?1"),
                        [identifier.to_string()],
                    )?;
                }
                _ => {
                    admin.execute(
                        &format!("UPDATE {table} SET updated_at = ?1 WHERE id = ?2"),
                        params![stored(later()), identifier.to_string()],
                    )?;
                }
            }
            let rows = snapshot(&admin)?;
            let (code, message) =
                refusal(save_letter(&fixture.letters(later()), &write, &binding).await)?;
            assert_eq!((code.code(), message.as_str()), conflict_message());
            assert_eq!(snapshot(&admin)?, rows, "{owner} {change}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn numeric_only_historical_edit_with_fresh_cas_is_timestamp_only_limit() -> TestResult {
    for owner in ["profile", "job"] {
        let (fixture, resume) = prepared().await?;
        let admin = fixture.admin();
        let binding = fixture.active_binding().await?;
        let (table, identifier) = if owner == "profile" {
            ("profile_profiles", resume.profile_id())
        } else {
            ("job_jobs", resume.job_id())
        };
        admin.execute(
            &format!("UPDATE {table} SET revision = revision + 1 WHERE id = ?1"),
            [identifier.to_string()],
        )?;
        let write = letter_write(&resume, &binding, &[8, 2])?;
        let (profile_revision, job_revision) = if owner == "profile" { (1, 0) } else { (0, 1) };
        let result = fixture
            .letters(later())
            .replace_german_cover_letter(
                &write,
                &binding,
                revision(profile_revision),
                revision(job_revision),
                None,
            )
            .await?;
        // Historical resume numeric provenance is absent; this proves only timestamp freshness.
        assert_eq!(&result.cover_letter, write.cover_letter());
    }
    Ok(())
}

#[tokio::test]
async fn exact_resume_dependency_is_required_without_mutation() -> TestResult {
    for defect in ["missing", "profile", "job", "updated_at"] {
        let (fixture, resume) = prepared().await?;
        let admin = fixture.admin();
        let binding = fixture.active_binding().await?;
        let source = match defect {
            "missing" => {
                // A resume row that was never stored: the letter names an unknown resume.
                let other = resume_write(fixture.profile, fixture.job, &[])?;
                other.resume().clone()
            }
            "profile" => {
                let other = Uuid::new_v4();
                admin.execute(
                    "INSERT INTO profile_profiles (id, full_name, email, created_at, \
                     updated_at) VALUES (?1, 'Other', 'other@example.test', ?2, ?2)",
                    params![other.to_string(), stored(now())],
                )?;
                resume_like(&resume, other, resume.job_id())?
            }
            "job" => {
                let other = Uuid::new_v4();
                let owner_profile = Uuid::new_v4();
                insert_owners(&admin, owner_profile, other, "Other")?;
                resume_like(&resume, resume.profile_id(), other)?
            }
            _ => {
                admin.execute(
                    "UPDATE resume_resumes SET updated_at = ?1 WHERE id = ?2",
                    params![
                        stored(later() + Duration::seconds(1)),
                        resume.id().to_string()
                    ],
                )?;
                resume.clone()
            }
        };
        let write = letter_write(&source, &binding, &[8, 2])?;
        let rows = snapshot(&admin)?;
        let (code, message) =
            refusal(save_letter(&fixture.letters(later()), &write, &binding).await)?;
        assert_eq!(
            (code.code(), message.as_str()),
            conflict_message(),
            "{defect}"
        );
        assert_eq!(snapshot(&admin)?, rows, "{defect}");
    }
    Ok(())
}

#[tokio::test]
async fn missing_owner_is_content_free_and_preserves_rows() -> TestResult {
    for owner in ["profile", "job"] {
        let (fixture, resume) = prepared().await?;
        let admin = fixture.admin();
        let binding = fixture.active_binding().await?;
        let source = if owner == "profile" {
            resume_like(&resume, Uuid::new_v4(), resume.job_id())?
        } else {
            resume_like(&resume, resume.profile_id(), Uuid::new_v4())?
        };
        let write = letter_write(&source, &binding, &[8, 2])?;
        let rows = snapshot(&admin)?;
        let (code, message) =
            refusal(save_letter(&fixture.letters(later()), &write, &binding).await)?;
        assert_eq!(code.code(), "not_found");
        assert_eq!(message, "German cover-letter source owner was not found");
        assert_eq!(snapshot(&admin)?, rows);
    }
    Ok(())
}

#[tokio::test]
async fn incoming_asset_must_be_active() -> TestResult {
    let (fixture, resume) = prepared().await?;
    let admin = fixture.admin();
    let binding = fixture.reserved_binding().await?;
    assert_eq!(asset_state(&admin, &binding)?.lifecycle, "RESERVED");
    let rows = snapshot(&admin)?;
    let write = letter_write(&resume, &binding, &[8, 2])?;
    let (code, message) = refusal(save_letter(&fixture.letters(later()), &write, &binding).await)?;
    assert_eq!((code, message.as_str()), LETTER_INVALID);
    assert_eq!(snapshot(&admin)?, rows);
    Ok(())
}

#[tokio::test]
async fn incoming_identity_matches_immutable_stored_binding() -> TestResult {
    for field in [
        "root_id",
        "sha256",
        "size_bytes",
        "asset_id",
        "document_id",
        "relative_path",
    ] {
        let (fixture, resume) = prepared().await?;
        let admin = fixture.admin();
        let binding = fixture.active_binding().await?;
        let asset = &binding.asset;
        let altered = match field {
            "root_id" => GeneratedReservation {
                asset: AssetIdentity::new(
                    asset.asset_id(),
                    Uuid::new_v4(),
                    asset.sha256(),
                    asset.size_bytes(),
                )?,
                document_id: binding.document_id,
            },
            "sha256" => GeneratedReservation {
                asset: AssetIdentity::new(
                    asset.asset_id(),
                    asset.root_id(),
                    &"0".repeat(64),
                    asset.size_bytes(),
                )?,
                document_id: binding.document_id,
            },
            "size_bytes" => GeneratedReservation {
                asset: AssetIdentity::new(
                    asset.asset_id(),
                    asset.root_id(),
                    asset.sha256(),
                    asset.size_bytes() + 1,
                )?,
                document_id: binding.document_id,
            },
            "document_id" => GeneratedReservation {
                asset: asset.clone(),
                document_id: Uuid::new_v4(),
            },
            // A different asset id also changes the derived relative path.
            _ => GeneratedReservation {
                asset: AssetIdentity::new(
                    Uuid::new_v4(),
                    asset.root_id(),
                    asset.sha256(),
                    asset.size_bytes(),
                )?,
                document_id: binding.document_id,
            },
        };
        // The write and the binding stay consistent, so the database admission is what refuses.
        let write = letter_write(&resume, &altered, &[8, 2])?;
        let rows = snapshot(&admin)?;
        let (code, message) =
            refusal(save_letter(&fixture.letters(later()), &write, &altered).await)?;
        assert_eq!((code, message.as_str()), LETTER_INVALID, "{field}");
        assert_eq!(snapshot(&admin)?, rows, "{field}");
    }
    Ok(())
}

#[tokio::test]
async fn every_mutation_stage_rolls_back_tree_and_retirement() -> TestResult {
    for (table, event, condition) in [
        ("cover_letter_cover_letters", "DELETE", "TRUE"),
        ("cover_letter_cover_letters", "INSERT", "TRUE"),
        ("cover_letter_cover_letter_variants", "INSERT", "TRUE"),
        ("cover_letter_cover_letter_paragraphs", "INSERT", "TRUE"),
        (
            "jobniac_generated_assets",
            "UPDATE",
            "NEW.lifecycle = 'RETIRING'",
        ),
        ("jobniac_generated_asset_cleanups", "INSERT", "TRUE"),
    ] {
        let (fixture, resume) = prepared().await?;
        let admin = fixture.admin();
        let old = fixture.active_binding().await?;
        let new = fixture.active_binding().await?;
        let repository = fixture.letters(later());
        let before = letter_write(&resume, &old, &[8, 2])?;
        save_letter(&repository, &before, &old).await?;
        let rows = snapshot(&admin)?;
        fail_at(&admin, table, event, condition)?;
        let candidate = letter_write(&resume, &new, &[8, 2])?;
        // A trigger failure is an integrity error of the write and keeps its mapping.
        let result = save_letter(&repository, &candidate, &new).await;
        assert!(result.is_err(), "{table} {event}");
        assert_eq!(snapshot(&admin)?, rows, "{table} {event}");
        assert_eq!(
            repository
                .find_by_id(before.cover_letter().id())
                .await?
                .as_ref(),
            Some(before.cover_letter())
        );
    }
    Ok(())
}

#[tokio::test]
async fn shared_or_reused_old_binding_is_not_retired() -> TestResult {
    for preserve in ["photo", "reuse", "resume_document"] {
        let (fixture, resume) = prepared().await?;
        let admin = fixture.admin();
        let old = fixture.active_binding().await?;
        let new = fixture.active_binding().await?;
        let repository = fixture.letters(later());
        let before = letter_write(&resume, &old, &[8, 2])?;
        save_letter(&repository, &before, &old).await?;
        if preserve == "photo" {
            attach_photo(&admin, resume.profile_id(), &old)?;
        } else if preserve == "resume_document" {
            admin.execute(
                "INSERT INTO resume_resume_variants \
                 (id, resume_id, language, document_id, file_path, created_at, updated_at) \
                 VALUES (?1, ?2, 'de', ?3, ?4, ?5, ?6)",
                params![
                    Uuid::new_v4().to_string(),
                    resume.id().to_string(),
                    old.document_id.to_string(),
                    old.asset.relative_path(),
                    stored(now()),
                    stored(later())
                ],
            )?;
        }
        let incoming = if preserve == "reuse" { &old } else { &new };
        let after = letter_write(&resume, incoming, &[8, 2])?;
        let result = save_letter(&repository, &after, incoming).await?;
        assert_eq!(
            result.previous_variants,
            vec![before.variant().variant().clone()]
        );
        assert_eq!(
            repository.find_by_id(before.cover_letter().id()).await?,
            None
        );
        let state = asset_state(&admin, &old)?;
        assert_eq!(state.lifecycle, "ACTIVE", "{preserve}");
        assert!(state.task_id.is_none());
        assert!(generated_asset_referenced(
            &admin,
            Some(old.document_id),
            old.asset.relative_path()
        )?);
    }
    Ok(())
}

#[tokio::test]
async fn replacement_retires_old_owned_binding_and_preserves_document_bytes() -> TestResult {
    let (fixture, resume) = prepared().await?;
    let admin = fixture.admin();
    let old = fixture.active_binding().await?;
    let new = fixture.active_binding().await?;
    let old_id = insert_letter(&admin, &resume, &old)?;
    let repository = fixture.letters(later());
    let previous = repository.find_variants(old_id).await?;
    let rows = snapshot(&admin)?;
    let after = letter_write(&resume, &new, &[8, 2])?;
    let result = save_letter(&repository, &after, &new).await?;
    assert_eq!(result.previous_variants, previous);
    assert_eq!(repository.find_by_id(old_id).await?, None);
    assert_eq!(
        repository
            .find_by_id(after.cover_letter().id())
            .await?
            .as_ref(),
        Some(after.cover_letter())
    );
    assert!(
        repository
            .find_paragraphs(previous[0].id())
            .await?
            .is_empty()
    );
    let state = asset_state(&admin, &old)?;
    assert_eq!(state.lifecycle, "RETIRING");
    assert_eq!(state.status.as_deref(), Some("PENDING"));
    assert!(state.retirement_id.is_some());
    assert_eq!(state.retirement_id, state.task_retirement_id);
    assert!(state.task_id.is_some());
    assert_eq!(state.retired_at, Some(later()));
    assert_eq!(state.next_attempt_at, Some(later()));
    assert_eq!(asset_state(&admin, &new)?.lifecycle, "ACTIVE");
    // The letter store deletes nothing from the resume side and keeps the document bytes.
    let committed = snapshot(&admin)?;
    for table in ["resume_resumes", "document_documents", "document_blobs"] {
        assert_eq!(table_rows(&committed, table), table_rows(&rows, table));
    }
    Ok(())
}

#[tokio::test]
async fn replacement_preserves_other_resume_identities_and_owner_pairs() -> TestResult {
    let (fixture, resume) = prepared().await?;
    let admin = fixture.admin();
    let (other_profile, other_job) = (Uuid::new_v4(), Uuid::new_v4());
    insert_owners(&admin, other_profile, other_job, "Other")?;
    for (profile, job) in [
        (resume.profile_id(), other_job),
        (other_profile, resume.job_id()),
        (other_profile, other_job),
    ] {
        let protected_id = Uuid::new_v4();
        admin.execute(
            "INSERT INTO resume_resumes (id, profile_id, job_id, format, profile_revision, \
             job_revision, created_at, updated_at) \
             VALUES (?1, ?2, ?3, 'germany', ?4, ?4, ?4, ?5)",
            params![
                protected_id.to_string(),
                profile.to_string(),
                job.to_string(),
                stored(now()),
                stored(later())
            ],
        )?;
        let binding = fixture.active_binding().await?;
        let protected_resume = Resume::new(ResumeFields {
            id: protected_id,
            profile_id: profile,
            job_id: job,
            format: "germany",
            profile_revision: now(),
            job_revision: now(),
            created_at: now(),
            updated_at: later(),
        })?;
        insert_letter(&admin, &protected_resume, &binding)?;
    }
    let protected_rows = snapshot(&admin)?;
    let old = fixture.active_binding().await?;
    let new = fixture.active_binding().await?;
    let repository = fixture.letters(later());
    save_letter(&repository, &letter_write(&resume, &old, &[8, 2])?, &old).await?;
    save_letter(&repository, &letter_write(&resume, &new, &[8, 2])?, &new).await?;
    let committed = snapshot(&admin)?;
    for (table, rows) in sqlite_german_support::TABLES.iter().zip(&protected_rows) {
        for row in rows {
            assert!(
                table_rows(&committed, table).contains(row),
                "unrelated row changed in {table}"
            );
        }
    }
    assert_eq!(asset_state(&admin, &old)?.lifecycle, "RETIRING");
    assert_eq!(
        table_rows(&committed, "jobniac_generated_asset_cleanups").len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn unowned_old_document_and_legacy_path_survive_replacement() -> TestResult {
    let (fixture, resume) = prepared().await?;
    let admin = fixture.admin();
    let (blob, document, letter, variant) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let content = sqlite_german_support::pdf("legacy");
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
        "INSERT INTO cover_letter_cover_letters (id, profile_id, job_id, resume_id, \
         profile_revision, job_revision, resume_revision, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?5,?6,?5,?6)",
        params![
            letter.to_string(),
            resume.profile_id().to_string(),
            resume.job_id().to_string(),
            resume.id().to_string(),
            stored(now()),
            stored(later())
        ],
    )?;
    admin.execute(
        "INSERT INTO cover_letter_cover_letter_variants (id, cover_letter_id, format, \
         language, document_id, file_path, subject, salutation, closing, signature, \
         created_at, updated_at) VALUES (?1,?2,'germany','de',?3,'legacy/path.pdf', \
         'Subject','Hello','Regards','Synthetic',?4,?5)",
        params![
            variant.to_string(),
            letter.to_string(),
            document.to_string(),
            stored(now()),
            stored(later())
        ],
    )?;
    let new = fixture.active_binding().await?;
    let repository = fixture.letters(later());
    let previous = repository.find_variants(letter).await?;
    let result = save_letter(&repository, &letter_write(&resume, &new, &[8, 2])?, &new).await?;
    assert_eq!(result.previous_variants, previous);
    assert_eq!(repository.find_by_id(letter).await?, None);
    assert_eq!(
        count(
            &admin,
            "SELECT count(*) FROM document_documents WHERE id = ?1",
            document
        )?,
        1
    );
    let rows = snapshot(&admin)?;
    assert!(table_rows(&rows, "jobniac_generated_asset_cleanups").is_empty());
    Ok(())
}

#[tokio::test]
async fn stored_pdf_bytes_are_verified_without_disabling_immutable_guards() -> TestResult {
    for defect in ["invalid_pdf", "corrupt_hash"] {
        let (fixture, resume) = prepared().await?;
        let admin = fixture.admin();
        let claimed: Vec<u8> = if defect == "corrupt_hash" {
            sqlite_german_support::pdf("claimed")
        } else {
            b"not a PDF".to_vec()
        };
        let content: Vec<u8> = if defect == "corrupt_hash" {
            vec![b'x'; claimed.len()]
        } else {
            claimed.clone()
        };
        let (asset_id, document, blob) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let digest = sha256_hex(&claimed);
        let size = i64::try_from(claimed.len())?;
        // Corrupt preexisting content is created before ownership; the guards stay enabled.
        admin.execute(
            "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![blob.to_string(), digest, size, content, stored(now())],
        )?;
        admin.execute(
            "INSERT INTO document_documents (id, blob_id, original_file_name, media_type, \
             created_at, updated_at) \
             VALUES (?1, ?2, 'synthetic.pdf', 'application/pdf', ?3, ?3)",
            params![document.to_string(), blob.to_string(), stored(now())],
        )?;
        admin.execute(
            "INSERT INTO jobniac_generated_assets (asset_id, root_id, relative_path, sha256, \
             size_bytes, document_id, lifecycle, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'RESERVED', ?7)",
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
            asset: AssetIdentity::new(asset_id, fixture.root, &digest, u64::try_from(size)?)?,
            document_id: document,
        };
        let rows = snapshot(&admin)?;
        let write = letter_write(&resume, &binding, &[8, 2])?;
        let (code, message) =
            refusal(save_letter(&fixture.letters(later()), &write, &binding).await)?;
        if defect == "invalid_pdf" {
            assert_eq!((code, message.as_str()), LETTER_INVALID);
        } else {
            assert_eq!((code, message.as_str()), UNAVAILABLE);
        }
        assert_eq!(snapshot(&admin)?, rows, "{defect}");
    }
    Ok(())
}

#[tokio::test]
async fn resume_and_letter_replacements_serialize_on_the_pair() -> TestResult {
    for winner in ["resume", "letter"] {
        let (fixture, resume) = prepared().await?;
        let admin = fixture.admin();
        let letter_binding = fixture.active_binding().await?;
        let resume_binding = fixture.active_binding().await?;
        let letters = fixture.letters(later());
        let resumes = fixture.resumes(later());
        let letter = letter_write(&resume, &letter_binding, &[8, 2])?;
        let replacement = resume_write(fixture.profile, fixture.job, &[&resume_binding])?;
        let save_letter_now = || {
            letters.replace_german_cover_letter(
                &letter,
                &letter_binding,
                revision(0),
                revision(0),
                None,
            )
        };
        let save_resume_now = || {
            resumes.replace_germany_resume(
                &replacement,
                std::slice::from_ref(&resume_binding),
                revision(0),
                revision(0),
                None,
            )
        };
        if winner == "resume" {
            // The resume replacement commits first, so the letter built from the old resume is
            // stale.
            save_resume_now().await?;
            let (code, message) = refusal(save_letter_now().await)?;
            assert_eq!((code.code(), message.as_str()), conflict_message());
        } else {
            // The letter commits first, and the resume replacement then deletes it.
            save_letter_now().await?;
            save_resume_now().await?;
        }
        assert_eq!(resumes.find_by_id(resume.id()).await?, None);
        assert!(
            resumes
                .find_by_id(replacement.resume().id())
                .await?
                .is_some()
        );
        assert_eq!(letters.find_by_id(letter.cover_letter().id()).await?, None);
        let expected = if winner == "resume" {
            "ACTIVE"
        } else {
            "RETIRING"
        };
        assert_eq!(asset_state(&admin, &letter_binding)?.lifecycle, expected);
        assert_eq!(asset_state(&admin, &resume_binding)?.lifecycle, "ACTIVE");
    }
    Ok(())
}

/// A classified letter claim of the prepared resume.
async fn letter_claim(fixture: &Fixture, resume: &Resume) -> Result<IntentReservation, BoxError> {
    let descriptor = GenerationDescriptor::new(
        Uuid::new_v4(),
        GenerationPurpose::GermanCoverLetter,
        Some(resume.profile_id()),
        Some(resume.job_id()),
        Some(resume.id()),
        Some("de".to_owned()),
    )?;
    Ok(fixture.classified_claim(&descriptor).await?)
}

#[tokio::test]
async fn classified_cover_letter_requires_exact_live_resume_claim() -> TestResult {
    for defect in ["missing", "expired", "token", "scope"] {
        let (fixture, resume) = prepared().await?;
        let admin = fixture.admin();
        let claim = letter_claim(&fixture, &resume).await?;
        let rows = snapshot(&admin)?;
        let mut clock = later();
        let mut proof = Some(claim.clone());
        match defect {
            "missing" => proof = None,
            "expired" => {
                clock = claim
                    .intent
                    .lease_expires_at
                    .ok_or("a pending intent has an expiry")?;
            }
            "token" => {
                if let Some(proof) = proof.as_mut() {
                    proof.intent.lease_token = Some(Uuid::new_v4());
                }
            }
            _ => {
                if let Some(proof) = proof.as_mut() {
                    proof.intent.descriptor.resume_id = Some(Uuid::new_v4());
                }
            }
        }
        let write = letter_write(&resume, &claim.reservation, &[8, 2])?;
        let result = fixture
            .letters(clock)
            .replace_german_cover_letter(
                &write,
                &claim.reservation,
                revision(0),
                revision(0),
                proof.as_ref(),
            )
            .await;
        let (code, message) = refusal(result)?;
        assert_eq!((code, message.as_str()), LETTER_INVALID, "{defect}");
        assert_eq!(snapshot(&admin)?, rows, "{defect}");
    }
    Ok(())
}

#[tokio::test]
async fn classified_cover_letter_completion_rolls_back_link_and_retirement() -> TestResult {
    for failure in ["completion", "expiry_after_link"] {
        let (fixture, resume) = prepared().await?;
        let admin = fixture.admin();
        let old = fixture.active_binding().await?;
        save_letter(
            &fixture.letters(later()),
            &letter_write(&resume, &old, &[8, 2])?,
            &old,
        )
        .await?;
        let claim = letter_claim(&fixture, &resume).await?;
        let rows = snapshot(&admin)?;
        let (repository, integrity_error) = if failure == "completion" {
            fail_at(
                &admin,
                "jobniac_generation_intents",
                "UPDATE",
                "NEW.state = 'COMPLETED'",
            )?;
            (fixture.letters(later()), true)
        } else {
            let expiry = claim
                .intent
                .lease_expires_at
                .ok_or("a pending intent has an expiry")?;
            let clock = Arc::new(SequenceClock::new(vec![later(), later(), expiry]));
            (fixture.letters_with(clock), false)
        };
        let write = letter_write(&resume, &claim.reservation, &[8, 2])?;
        let result = repository
            .replace_german_cover_letter(
                &write,
                &claim.reservation,
                revision(0),
                revision(0),
                Some(&claim),
            )
            .await;
        if integrity_error {
            assert!(result.is_err(), "{failure}");
        } else {
            let (code, message) = refusal(result)?;
            assert_eq!((code, message.as_str()), LETTER_INVALID);
        }
        assert_eq!(snapshot(&admin)?, rows, "{failure}");
        assert_eq!(asset_state(&admin, &old)?.lifecycle, "ACTIVE");
    }
    Ok(())
}

#[tokio::test]
async fn classified_cover_letter_completion_and_retirement_preserve_terminal_history() -> TestResult
{
    let (fixture, resume) = prepared().await?;
    let admin = fixture.admin();
    let claim = letter_claim(&fixture, &resume).await?;
    let repository = fixture.letters(later());
    let write = letter_write(&resume, &claim.reservation, &[8, 2])?;
    repository
        .replace_german_cover_letter(
            &write,
            &claim.reservation,
            revision(0),
            revision(0),
            Some(&claim),
        )
        .await?;
    let state_of = |admin: &Connection| -> Result<String, BoxError> {
        Ok(admin.query_row(
            "SELECT state FROM jobniac_generation_intents WHERE asset_id = ?1",
            [claim.asset_id().to_string()],
            |row| row.get(0),
        )?)
    };
    assert_eq!(state_of(&admin)?, "COMPLETED");
    let replacement = fixture.active_binding().await?;
    save_letter(
        &repository,
        &letter_write(&resume, &replacement, &[8, 2])?,
        &replacement,
    )
    .await?;
    assert_eq!(
        asset_state(&admin, &claim.reservation)?.lifecycle,
        "RETIRING"
    );
    assert_eq!(state_of(&admin)?, "COMPLETED");
    Ok(())
}

/// The cover-letter repository without a database, which must refuse every call.
fn unconfigured_letters() -> SqliteGermanCoverLetterRepository {
    SqliteGermanCoverLetterRepository::new(
        SqliteUnitOfWork::new(SqlitePool::unconfigured()),
        Arc::new(FixedClock::new(later())),
    )
}

#[tokio::test]
async fn unconfigured_repository_is_safe() -> TestResult {
    let repository = unconfigured_letters();
    let (code, message) = refusal(repository.find_by_id(Uuid::new_v4()).await)?;
    assert_eq!((code, message.as_str()), NOT_CONFIGURED);
    let (code, message) = refusal(repository.find_variants(Uuid::new_v4()).await)?;
    assert_eq!((code, message.as_str()), NOT_CONFIGURED);
    let (code, message) = refusal(repository.find_paragraphs(Uuid::new_v4()).await)?;
    assert_eq!((code, message.as_str()), NOT_CONFIGURED);
    Ok(())
}

#[tokio::test]
async fn closed_pool_failure_has_no_private_driver_context() -> TestResult {
    let (fixture, resume) = prepared().await?;
    let pool = SqlitePool::open(fixture.sqlite.path())?;
    pool.close();
    let repository = SqliteGermanCoverLetterRepository::new(
        SqliteUnitOfWork::new(pool),
        Arc::new(FixedClock::new(later())),
    );
    let binding = fixture.active_binding().await?;
    let write = letter_write(&resume, &binding, &[8, 2])?;
    let (code, message) = refusal(save_letter(&repository, &write, &binding).await)?;
    assert_eq!((code, message.as_str()), UNAVAILABLE);
    assert!(!message.contains("sqlite") && !message.contains("connect"));
    let (code, message) = refusal(repository.find_by_id(write.cover_letter().id()).await)?;
    assert_eq!((code, message.as_str()), UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn invalid_aggregate_fails_before_connecting() -> TestResult {
    let (fixture, resume) = prepared().await?;
    let binding = fixture.active_binding().await?;
    // Two paragraphs with the same display order: validation refuses the aggregate before any
    // connection, so the unconfigured repository reports the validation error, not the
    // not-configured one.
    let write = letter_write(&resume, &binding, &[3, 3])?;
    let (code, message) = refusal(
        unconfigured_letters()
            .replace_german_cover_letter(&write, &binding, revision(0), revision(0), None)
            .await,
    )?;
    assert_eq!((code, message.as_str()), LETTER_INVALID);
    Ok(())
}

#[tokio::test]
async fn actual_admitted_content_mapper_roundtrip() -> TestResult {
    let (fixture, resume) = prepared().await?;
    let binding = fixture.active_binding().await?;
    let base = letter_write(&resume, &binding, &[])?;
    let content = GermanCoverLetterContent::new(CoverLetterInput {
        sender_name: Some("Person".to_owned()),
        personal_fields: Vec::new(),
        recipient_company: None,
        recipient_location: None,
        job_title: Some("Developer".to_owned()),
        subject: Some("Bewerbung \u{1F600}".to_owned()),
        salutation: Some("Hallo".to_owned()),
        paragraphs: Some(vec![
            Some("Erster Absatz \u{1F600}".to_owned()),
            Some("Zweiter Absatz".to_owned()),
        ]),
        closing: Some("Gr\u{FC}\u{DF}e".to_owned()),
        signature: Some("Person".to_owned()),
    })?;
    let mut next = 0u128;
    let write = map_cover_letter_content(
        base.cover_letter().clone(),
        base.variant().variant().clone(),
        &content,
        &mut || {
            next += 1;
            Uuid::from_u128(next)
        },
    )?;
    let repository = fixture.letters(later());
    save_letter(&repository, &write, &binding).await?;
    let variants = repository.find_variants(write.cover_letter().id()).await?;
    assert_eq!(variants.len(), 1);
    let variant = &variants[0];
    assert_eq!(variant.subject(), content.subject());
    assert_eq!(variant.salutation(), content.salutation());
    assert_eq!(variant.closing(), content.closing());
    assert_eq!(variant.signature(), content.signature());
    let paragraphs = repository.find_paragraphs(variant.id()).await?;
    let texts: Vec<&str> = paragraphs.iter().map(|row| row.text()).collect();
    assert_eq!(
        texts,
        content
            .paragraphs()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    let orders: Vec<_> = paragraphs.iter().map(|row| row.display_order()).collect();
    assert_eq!(
        orders,
        vec![
            sqlite_german_support::order(0),
            sqlite_german_support::order(1)
        ]
    );
    Ok(())
}

#[tokio::test]
async fn cleanup_winner_invalidates_binding_without_letter_mutation() -> TestResult {
    let (fixture, resume) = prepared().await?;
    let admin = fixture.admin();
    let old = fixture.active_binding().await?;
    let new = fixture.active_binding().await?;
    let repository = fixture.letters(later());
    save_letter(&repository, &letter_write(&resume, &old, &[8, 2])?, &old).await?;
    let candidate = letter_write(&resume, &new, &[8, 2])?;
    // The cleanup prepares the unreferenced new asset first and retires it.
    let outcome = cleanup_repository(&fixture, later())
        .prepare_cleanup(new.asset.asset_id())
        .await?;
    assert_eq!(outcome, PreparationOutcome::Prepared);
    let snapshot_before = snapshot(&admin)?;
    let (code, message) = refusal(save_letter(&repository, &candidate, &new).await)?;
    assert_eq!((code, message.as_str()), LETTER_INVALID);
    assert_eq!(snapshot(&admin)?, snapshot_before);
    assert_eq!(asset_state(&admin, &old)?.lifecycle, "ACTIVE");
    assert_eq!(asset_state(&admin, &new)?.lifecycle, "RETIRING");
    Ok(())
}

#[tokio::test]
async fn letter_winner_makes_cleanup_observe_reference() -> TestResult {
    let (fixture, resume) = prepared().await?;
    let admin = fixture.admin();
    let binding = fixture.active_binding().await?;
    let repository = fixture.letters(later());
    let write = letter_write(&resume, &binding, &[8, 2])?;
    assert_eq!(
        &save_letter(&repository, &write, &binding)
            .await?
            .cover_letter,
        write.cover_letter()
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
async fn attachment_winner_remains_visible_after_writer_wait() -> TestResult {
    let (fixture, resume) = prepared().await?;
    let admin = fixture.admin();
    let old = fixture.active_binding().await?;
    let new = fixture.active_binding().await?;
    let repository = fixture.letters(later());
    let before = letter_write(&resume, &old, &[8, 2])?;
    save_letter(&repository, &before, &old).await?;
    // The attachment holds its transaction open while the replacement queues behind it.
    admin.execute_batch("BEGIN IMMEDIATE")?;
    attach_photo(&admin, fixture.profile, &old)?;
    let candidate = letter_write(&resume, &new, &[8, 2])?;
    let replacement = tokio::spawn({
        let repository = repository.clone();
        let binding = new.clone();
        async move {
            repository
                .replace_german_cover_letter(&candidate, &binding, revision(0), revision(0), None)
                .await
        }
    });
    tokio::time::sleep(StdDuration::from_millis(200)).await;
    assert!(!replacement.is_finished());
    admin.execute_batch("COMMIT")?;
    let result = replacement.await??;
    assert_eq!(
        result.previous_variants,
        vec![before.variant().variant().clone()]
    );
    let state = asset_state(&admin, &old)?;
    assert_eq!(state.lifecycle, "ACTIVE");
    assert_eq!(state.task_id, None);
    let outcome = cleanup_repository(&fixture, later())
        .prepare_cleanup(old.asset.asset_id())
        .await?;
    assert_eq!(outcome, PreparationOutcome::Referenced);
    Ok(())
}

#[tokio::test]
async fn schema_and_guards_reject_unreachable_dependency_states_without_mutation() -> TestResult {
    for defect in [
        "non_germany",
        "path_only",
        "reserved_attachment",
        "resume_identity",
    ] {
        let (fixture, resume) = prepared().await?;
        let admin = fixture.admin();
        let binding = if defect == "reserved_attachment" {
            fixture.reserved_binding().await?
        } else {
            fixture.active_binding().await?
        };
        let snapshot_before = snapshot(&admin)?;
        // Every probe runs in one transaction that is rolled back, so a rejected parent row
        // cannot survive the rejected child.
        admin.execute_batch("BEGIN")?;
        let outcome: Result<(), BoxError> = match defect {
            "resume_identity" => {
                let other = Uuid::new_v4();
                insert_owners(&admin, other, Uuid::new_v4(), "Other")?;
                let altered = resume_like(&resume, other, resume.job_id())?;
                insert_letter(&admin, &altered, &binding).map(|_| ())
            }
            // The attachment guard the writers call before every variant insert (rule 14).
            "reserved_attachment" => guard_document_path_reference(
                &admin,
                binding.document_id,
                binding.asset.relative_path(),
            )
            .map_err(Into::into),
            "non_germany" => admin
                .execute(
                    "UPDATE resume_resumes SET format = 'france' WHERE id = ?1",
                    [resume.id().to_string()],
                )
                .map(|_| ())
                .map_err(Into::into),
            _ => admin
                .execute(
                    "INSERT INTO resume_resume_variants \
                     (id, resume_id, language, document_id, file_path, created_at, updated_at) \
                     VALUES (?1, ?2, 'de', NULL, ?3, ?4, ?5)",
                    params![
                        Uuid::new_v4().to_string(),
                        resume.id().to_string(),
                        binding.asset.relative_path(),
                        stored(now()),
                        stored(later())
                    ],
                )
                .map(|_| ())
                .map_err(Into::into),
        };
        admin.execute_batch("ROLLBACK")?;
        let error = match outcome {
            Err(error) => error,
            Ok(()) => return Err(format!("{defect} was accepted").into()),
        };
        if defect == "resume_identity" {
            let foreign_key = error
                .downcast_ref::<rusqlite::Error>()
                .is_some_and(|error| {
                    matches!(
                        error,
                        rusqlite::Error::SqliteFailure(failure, _)
                            if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY
                    )
                });
            assert!(foreign_key, "{error}");
        }
        if defect == "reserved_attachment" {
            assert!(matches!(
                error.downcast_ref::<SqliteError>(),
                Some(SqliteError::Constraint { sqlstate, .. }) if sqlstate == "23514"
            ));
        }
        assert_eq!(snapshot(&admin)?, snapshot_before, "{defect}");
        let expected = if defect == "reserved_attachment" {
            "RESERVED"
        } else {
            "ACTIVE"
        };
        assert_eq!(
            asset_state(&admin, &binding)?.lifecycle,
            expected,
            "{defect}"
        );
    }
    Ok(())
}
