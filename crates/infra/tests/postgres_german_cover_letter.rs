//! German cover-letter store against a disposable PostgreSQL 18 server: the round trip, the owner and
//! resume compare-and-set, the incoming binding, rollback of every stage, retirement of the
//! replaced asset, the pair lock shared with the resume writer, and classified claims. The
//! server-backed tests skip with a notice when `JOBNIAC_TEST_PG_PORT` is unset. All data is
//! synthetic.

mod german_support;

use std::sync::Arc;

use german_support::{
    Fixture, LETTER_INVALID, NOT_CONFIGURED, SequenceClock, TestResult, UNAVAILABLE, asset_state,
    attach_photo, cleanup_repository, closed_port_config, fail_at, gate_inserts, gate_retirements,
    insert_letter, insert_owners, later, letter_write, now, options, refusal, release_gate,
    resume_write, revision, save_letter, save_resume, snapshot, start, table_rows,
    wait_for_blocker, wait_for_lock,
};
use jobniac_core::cleanup::CleanupRepository;
use jobniac_core::clock::FixedClock;
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedReservation, GenerationDescriptor, GenerationPurpose,
    IntentReservation, PreparationOutcome, sha256_hex,
};
use jobniac_core::german::content::{CoverLetterInput, GermanCoverLetterContent};
use jobniac_core::german::cover_letter_store::{
    GermanCoverLetterRepository, map_cover_letter_content,
};
use jobniac_core::german::resume_store::{Resume, ResumeFields};
use jobniac_infra::postgres::german::{PgGermanCoverLetterRepository, cover_letter};
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use time::Duration;
use uuid::Uuid;

const APP: &str = "jobniac-german-cover-letter";
const RESUME_APP: &str = "jobniac-german-resume";

/// A fixture with the pair's resume stored (no variants), and that resume.
async fn prepared() -> Result<Option<(Fixture, Resume)>, Box<dyn std::error::Error>> {
    let Some(fixture) = start().await? else {
        return Ok(None);
    };
    let write = resume_write(fixture.profile, fixture.job, &[])?;
    save_resume(&fixture.resumes(later()), &write, &[]).await?;
    let resume = write.resume().clone();
    Ok(Some((fixture, resume)))
}

/// A resume row with the identity of `resume` but other owners or timestamps.
fn resume_like(
    resume: &Resume,
    profile: Uuid,
    job: Uuid,
) -> Result<Resume, Box<dyn std::error::Error>> {
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

#[tokio::test]
async fn roundtrip_and_paragraph_read_order() -> TestResult {
    for orders in [Vec::<i64>::new(), vec![8, 2]] {
        let Some((fixture, resume)) = prepared().await? else {
            return Ok(());
        };
        let admin = fixture.admin().await;
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
        assert_eq!(asset_state(&admin, &binding).await?.lifecycle, "ACTIVE");
        for paragraph in write.variant().paragraphs() {
            let row = admin
                .query_one(
                    "SELECT variant_id, display_order, text \
                     FROM cover_letter.cover_letter_paragraphs WHERE id = $1",
                    &[&paragraph.id()],
                )
                .await?;
            assert_eq!(
                row.try_get::<_, Uuid>("variant_id")?,
                paragraph.variant_id()
            );
            assert_eq!(
                row.try_get::<_, i32>("display_order")?,
                paragraph.display_order().get()
            );
            assert_eq!(row.try_get::<_, String>("text")?, paragraph.text());
        }
    }
    Ok(())
}

#[tokio::test]
async fn paragraph_inserts_follow_encounter_order() -> TestResult {
    let Some((fixture, resume)) = prepared().await? else {
        return Ok(());
    };
    let admin = fixture.admin().await;
    let binding = fixture.active_binding().await?;
    let write = letter_write(&resume, &binding, &[8, 2])?;
    admin
        .batch_execute(
            "CREATE TABLE jobniac.synthetic_letter_order ( \
                 ordinal bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, row_id uuid); \
             CREATE FUNCTION jobniac.synthetic_letter_order() RETURNS trigger LANGUAGE plpgsql \
             AS $$ BEGIN INSERT INTO jobniac.synthetic_letter_order(row_id) VALUES (NEW.id); \
                 RETURN NEW; END $$; \
             CREATE TRIGGER synthetic_letter_order AFTER INSERT \
             ON cover_letter.cover_letter_paragraphs FOR EACH ROW \
             EXECUTE FUNCTION jobniac.synthetic_letter_order();",
        )
        .await?;
    save_letter(&fixture.letters(later()), &write, &binding).await?;
    let rows = admin
        .query(
            "SELECT row_id FROM jobniac.synthetic_letter_order ORDER BY ordinal",
            &[],
        )
        .await?;
    let observed = rows
        .iter()
        .map(|row| row.try_get::<_, Uuid>("row_id"))
        .collect::<Result<Vec<_>, _>>()?;
    let expected: Vec<Uuid> = write
        .variant()
        .paragraphs()
        .iter()
        .map(|paragraph| paragraph.id())
        .collect();
    assert_eq!(observed, expected);
    Ok(())
}

#[tokio::test]
async fn stale_owner_or_resume_provenance_preserves_every_row() -> TestResult {
    for owner in ["profile", "job"] {
        for change in ["numeric", "timestamp", "stored_resume_timestamp"] {
            let Some((fixture, resume)) = prepared().await? else {
                return Ok(());
            };
            let admin = fixture.admin().await;
            let binding = fixture.active_binding().await?;
            let write = letter_write(&resume, &binding, &[8, 2])?;
            let (table, identifier) = if owner == "profile" {
                ("profile.profiles", resume.profile_id())
            } else {
                ("job_schema.jobs", resume.job_id())
            };
            match change {
                "stored_resume_timestamp" => {
                    admin
                        .execute(
                            &format!(
                                "UPDATE resume.resumes SET {owner}_revision = $1 WHERE id = $2"
                            ),
                            &[&(now() - Duration::seconds(1)), &resume.id()],
                        )
                        .await?;
                }
                "numeric" => {
                    admin
                        .execute(
                            &format!("UPDATE {table} SET revision = revision + 1 WHERE id = $1"),
                            &[&identifier],
                        )
                        .await?;
                }
                _ => {
                    admin
                        .execute(
                            &format!("UPDATE {table} SET updated_at = $1 WHERE id = $2"),
                            &[&later(), &identifier],
                        )
                        .await?;
                }
            }
            let rows = snapshot(&admin).await?;
            let (code, message) =
                refusal(save_letter(&fixture.letters(later()), &write, &binding).await)?;
            assert_eq!((code.code(), message.as_str()), conflict_message());
            assert_eq!(snapshot(&admin).await?, rows, "{owner} {change}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn numeric_only_historical_edit_with_fresh_cas_is_timestamp_only_limit() -> TestResult {
    for owner in ["profile", "job"] {
        let Some((fixture, resume)) = prepared().await? else {
            return Ok(());
        };
        let admin = fixture.admin().await;
        let binding = fixture.active_binding().await?;
        let (table, identifier) = if owner == "profile" {
            ("profile.profiles", resume.profile_id())
        } else {
            ("job_schema.jobs", resume.job_id())
        };
        admin
            .execute(
                &format!("UPDATE {table} SET revision = revision + 1 WHERE id = $1"),
                &[&identifier],
            )
            .await?;
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
        let Some((fixture, resume)) = prepared().await? else {
            return Ok(());
        };
        let admin = fixture.admin().await;
        let binding = fixture.active_binding().await?;
        let source = match defect {
            "missing" => {
                // A resume row that was never stored: the letter names an unknown resume.
                let other = resume_write(fixture.profile, fixture.job, &[])?;
                other.resume().clone()
            }
            "profile" => {
                let other = Uuid::new_v4();
                admin
                    .execute(
                        "INSERT INTO profile.profiles (id, full_name, email, created_at, \
                         updated_at) VALUES ($1, 'Other', 'other@example.test', $2, $2)",
                        &[&other, &now()],
                    )
                    .await?;
                resume_like(&resume, other, resume.job_id())?
            }
            "job" => {
                let other = Uuid::new_v4();
                let owner_profile = Uuid::new_v4();
                insert_owners(&admin, owner_profile, other, "Other").await?;
                resume_like(&resume, resume.profile_id(), other)?
            }
            _ => {
                admin
                    .execute(
                        "UPDATE resume.resumes SET updated_at = $1 WHERE id = $2",
                        &[&(later() + Duration::seconds(1)), &resume.id()],
                    )
                    .await?;
                resume.clone()
            }
        };
        let write = letter_write(&source, &binding, &[8, 2])?;
        let rows = snapshot(&admin).await?;
        let (code, message) =
            refusal(save_letter(&fixture.letters(later()), &write, &binding).await)?;
        assert_eq!(
            (code.code(), message.as_str()),
            conflict_message(),
            "{defect}"
        );
        assert_eq!(snapshot(&admin).await?, rows, "{defect}");
    }
    Ok(())
}

#[tokio::test]
async fn missing_owner_is_content_free_and_preserves_rows() -> TestResult {
    for owner in ["profile", "job"] {
        let Some((fixture, resume)) = prepared().await? else {
            return Ok(());
        };
        let admin = fixture.admin().await;
        let binding = fixture.active_binding().await?;
        let source = if owner == "profile" {
            resume_like(&resume, Uuid::new_v4(), resume.job_id())?
        } else {
            resume_like(&resume, resume.profile_id(), Uuid::new_v4())?
        };
        let write = letter_write(&source, &binding, &[8, 2])?;
        let rows = snapshot(&admin).await?;
        let (code, message) =
            refusal(save_letter(&fixture.letters(later()), &write, &binding).await)?;
        assert_eq!(code.code(), "not_found");
        assert_eq!(message, "German cover-letter source owner was not found");
        assert_eq!(snapshot(&admin).await?, rows);
    }
    Ok(())
}

#[tokio::test]
async fn incoming_asset_must_be_active() -> TestResult {
    let Some((fixture, resume)) = prepared().await? else {
        return Ok(());
    };
    let admin = fixture.admin().await;
    let binding = fixture.reserved_binding().await?;
    assert_eq!(asset_state(&admin, &binding).await?.lifecycle, "RESERVED");
    let rows = snapshot(&admin).await?;
    let write = letter_write(&resume, &binding, &[8, 2])?;
    let (code, message) = refusal(save_letter(&fixture.letters(later()), &write, &binding).await)?;
    assert_eq!((code, message.as_str()), LETTER_INVALID);
    assert_eq!(snapshot(&admin).await?, rows);
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
        let Some((fixture, resume)) = prepared().await? else {
            return Ok(());
        };
        let admin = fixture.admin().await;
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
        let rows = snapshot(&admin).await?;
        let (code, message) =
            refusal(save_letter(&fixture.letters(later()), &write, &altered).await)?;
        assert_eq!((code, message.as_str()), LETTER_INVALID, "{field}");
        assert_eq!(snapshot(&admin).await?, rows, "{field}");
    }
    Ok(())
}

#[tokio::test]
async fn every_mutation_stage_rolls_back_tree_and_retirement() -> TestResult {
    for (table, event, condition) in [
        ("cover_letter.cover_letters", "DELETE", "TRUE"),
        ("cover_letter.cover_letters", "INSERT", "TRUE"),
        ("cover_letter.cover_letter_variants", "INSERT", "TRUE"),
        ("cover_letter.cover_letter_paragraphs", "INSERT", "TRUE"),
        (
            "jobniac.generated_assets",
            "UPDATE",
            "NEW.lifecycle = 'RETIRING'",
        ),
        ("jobniac.generated_asset_cleanups", "INSERT", "TRUE"),
    ] {
        let Some((fixture, resume)) = prepared().await? else {
            return Ok(());
        };
        let admin = fixture.admin().await;
        let old = fixture.active_binding().await?;
        let new = fixture.active_binding().await?;
        let repository = fixture.letters(later());
        let before = letter_write(&resume, &old, &[8, 2])?;
        save_letter(&repository, &before, &old).await?;
        let rows = snapshot(&admin).await?;
        fail_at(&admin, table, event, condition).await?;
        let candidate = letter_write(&resume, &new, &[8, 2])?;
        let (code, message) = refusal(save_letter(&repository, &candidate, &new).await)?;
        assert_eq!((code, message.as_str()), UNAVAILABLE, "{table} {event}");
        assert_eq!(snapshot(&admin).await?, rows, "{table} {event}");
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
        let Some((fixture, resume)) = prepared().await? else {
            return Ok(());
        };
        let admin = fixture.admin().await;
        let old = fixture.active_binding().await?;
        let new = fixture.active_binding().await?;
        let repository = fixture.letters(later());
        let before = letter_write(&resume, &old, &[8, 2])?;
        save_letter(&repository, &before, &old).await?;
        if preserve == "photo" {
            attach_photo(&admin, resume.profile_id(), &old).await?;
        } else if preserve == "resume_document" {
            admin
                .execute(
                    "INSERT INTO resume.resume_variants \
                     (id, resume_id, language, document_id, file_path, created_at, updated_at) \
                     VALUES ($1, $2, 'de', $3, $4, $5, $6)",
                    &[
                        &Uuid::new_v4(),
                        &resume.id(),
                        &old.document_id,
                        &old.asset.relative_path(),
                        &now(),
                        &later(),
                    ],
                )
                .await?;
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
        let state = asset_state(&admin, &old).await?;
        assert_eq!(state.lifecycle, "ACTIVE", "{preserve}");
        assert!(state.task_id.is_none());
        let referenced = admin
            .query_one(
                "SELECT jobniac.generated_asset_referenced($1, $2) AS referenced",
                &[&old.document_id, &old.asset.relative_path()],
            )
            .await?;
        assert!(referenced.try_get::<_, bool>("referenced")?);
    }
    Ok(())
}

#[tokio::test]
async fn replacement_retires_old_owned_binding_and_preserves_document_bytes() -> TestResult {
    let Some((fixture, resume)) = prepared().await? else {
        return Ok(());
    };
    let admin = fixture.admin().await;
    let old = fixture.active_binding().await?;
    let new = fixture.active_binding().await?;
    let old_id = insert_letter(&admin, &resume, &old).await?;
    let repository = fixture.letters(later());
    let previous = repository.find_variants(old_id).await?;
    let rows = snapshot(&admin).await?;
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
    let state = asset_state(&admin, &old).await?;
    assert_eq!(state.lifecycle, "RETIRING");
    assert_eq!(state.status.as_deref(), Some("PENDING"));
    assert!(state.retirement_id.is_some());
    assert_eq!(state.retirement_id, state.task_retirement_id);
    assert!(state.task_id.is_some());
    assert_eq!(state.retired_at, Some(later()));
    assert_eq!(state.next_attempt_at, Some(later()));
    assert_eq!(asset_state(&admin, &new).await?.lifecycle, "ACTIVE");
    // The letter store deletes nothing from the resume side and keeps the document bytes.
    let committed = snapshot(&admin).await?;
    for table in ["resume.resumes", "document.documents", "document.blobs"] {
        assert_eq!(table_rows(&committed, table), table_rows(&rows, table));
    }
    Ok(())
}

#[tokio::test]
async fn replacement_preserves_other_resume_identities_and_owner_pairs() -> TestResult {
    let Some((fixture, resume)) = prepared().await? else {
        return Ok(());
    };
    let admin = fixture.admin().await;
    let (other_profile, other_job) = (Uuid::new_v4(), Uuid::new_v4());
    insert_owners(&admin, other_profile, other_job, "Other").await?;
    for (profile, job) in [
        (resume.profile_id(), other_job),
        (other_profile, resume.job_id()),
        (other_profile, other_job),
    ] {
        let protected_id = Uuid::new_v4();
        admin
            .execute(
                "INSERT INTO resume.resumes (id, profile_id, job_id, format, profile_revision, \
                 job_revision, created_at, updated_at) \
                 VALUES ($1, $2, $3, 'germany', $4, $4, $4, $5)",
                &[&protected_id, &profile, &job, &now(), &later()],
            )
            .await?;
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
        insert_letter(&admin, &protected_resume, &binding).await?;
    }
    let protected_rows = snapshot(&admin).await?;
    let old = fixture.active_binding().await?;
    let new = fixture.active_binding().await?;
    let repository = fixture.letters(later());
    save_letter(&repository, &letter_write(&resume, &old, &[8, 2])?, &old).await?;
    save_letter(&repository, &letter_write(&resume, &new, &[8, 2])?, &new).await?;
    let committed = snapshot(&admin).await?;
    for (table, rows) in german_support::TABLES.iter().zip(&protected_rows) {
        for row in rows {
            assert!(
                table_rows(&committed, table).contains(row),
                "unrelated row changed in {table}"
            );
        }
    }
    assert_eq!(asset_state(&admin, &old).await?.lifecycle, "RETIRING");
    assert_eq!(
        table_rows(&committed, "jobniac.generated_asset_cleanups").len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn unowned_old_document_and_legacy_path_survive_replacement() -> TestResult {
    let Some((fixture, resume)) = prepared().await? else {
        return Ok(());
    };
    let admin = fixture.admin().await;
    let (blob, document, letter, variant) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let content = german_support::pdf("legacy");
    admin
        .execute(
            "INSERT INTO document.blobs (id, sha256, byte_size, content, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
            &[
                &blob,
                &sha256_hex(&content),
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
            "INSERT INTO cover_letter.cover_letters (id, profile_id, job_id, resume_id, \
             profile_revision, job_revision, resume_revision, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$5,$6,$5,$6)",
            &[
                &letter,
                &resume.profile_id(),
                &resume.job_id(),
                &resume.id(),
                &now(),
                &later(),
            ],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO cover_letter.cover_letter_variants (id, cover_letter_id, format, \
             language, document_id, file_path, subject, salutation, closing, signature, \
             created_at, updated_at) VALUES ($1,$2,'germany','de',$3,'legacy/path.pdf', \
             'Subject','Hello','Regards','Synthetic',$4,$5)",
            &[&variant, &letter, &document, &now(), &later()],
        )
        .await?;
    let new = fixture.active_binding().await?;
    let repository = fixture.letters(later());
    let previous = repository.find_variants(letter).await?;
    let result = save_letter(&repository, &letter_write(&resume, &new, &[8, 2])?, &new).await?;
    assert_eq!(result.previous_variants, previous);
    assert_eq!(repository.find_by_id(letter).await?, None);
    let kept = admin
        .query_one(
            "SELECT count(*) AS total FROM document.documents WHERE id = $1",
            &[&document],
        )
        .await?;
    assert_eq!(kept.try_get::<_, i64>("total")?, 1);
    let rows = snapshot(&admin).await?;
    assert!(table_rows(&rows, "jobniac.generated_asset_cleanups").is_empty());
    Ok(())
}

#[tokio::test]
async fn stored_pdf_bytes_are_verified_without_disabling_immutable_guards() -> TestResult {
    for defect in ["invalid_pdf", "corrupt_hash"] {
        let Some((fixture, resume)) = prepared().await? else {
            return Ok(());
        };
        let admin = fixture.admin().await;
        let claimed: Vec<u8> = if defect == "corrupt_hash" {
            german_support::pdf("claimed")
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
        admin
            .execute(
                "INSERT INTO document.blobs (id, sha256, byte_size, content, created_at) \
                 VALUES ($1, $2, $3, $4, $5)",
                &[&blob, &digest, &size, &content, &now()],
            )
            .await?;
        admin
            .execute(
                "INSERT INTO document.documents (id, blob_id, original_file_name, media_type, \
                 created_at, updated_at) \
                 VALUES ($1, $2, 'synthetic.pdf', 'application/pdf', $3, $3)",
                &[&document, &blob, &now()],
            )
            .await?;
        admin
            .execute(
                "INSERT INTO jobniac.generated_assets (asset_id, root_id, relative_path, sha256, \
                 size_bytes, document_id, lifecycle, created_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, 'RESERVED', $7)",
                &[
                    &asset_id,
                    &fixture.root,
                    &format!("{asset_id}.pdf"),
                    &digest,
                    &size,
                    &document,
                    &now(),
                ],
            )
            .await?;
        admin
            .execute(
                "UPDATE jobniac.generated_assets SET lifecycle = 'ACTIVE', materialized_at = $2 \
                 WHERE asset_id = $1",
                &[&asset_id, &now()],
            )
            .await?;
        let binding = GeneratedReservation {
            asset: AssetIdentity::new(asset_id, fixture.root, &digest, u64::try_from(size)?)?,
            document_id: document,
        };
        let rows = snapshot(&admin).await?;
        let write = letter_write(&resume, &binding, &[8, 2])?;
        let (code, message) =
            refusal(save_letter(&fixture.letters(later()), &write, &binding).await)?;
        if defect == "invalid_pdf" {
            assert_eq!((code, message.as_str()), LETTER_INVALID);
        } else {
            assert_eq!((code, message.as_str()), UNAVAILABLE);
        }
        assert_eq!(snapshot(&admin).await?, rows, "{defect}");
    }
    Ok(())
}

#[tokio::test]
async fn resume_and_letter_replacements_share_pair_lock() -> TestResult {
    for winner in ["resume", "letter"] {
        let Some((fixture, resume)) = prepared().await? else {
            return Ok(());
        };
        let admin = fixture.admin().await;
        let letter_binding = fixture.active_binding().await?;
        let resume_binding = fixture.active_binding().await?;
        let letters = fixture.letters(later());
        let resumes = fixture.resumes(later());
        let letter = letter_write(&resume, &letter_binding, &[8, 2])?;
        let replacement = resume_write(fixture.profile, fixture.job, &[&resume_binding])?;
        let (table, key) = if winner == "resume" {
            ("resume.resumes", 913_717)
        } else {
            ("cover_letter.cover_letters", 913_719)
        };
        let gate = gate_inserts(&fixture.database, table, key).await?;
        let spawn_letter = || {
            let (repository, write, binding) =
                (letters.clone(), letter.clone(), letter_binding.clone());
            tokio::spawn(async move {
                repository
                    .replace_german_cover_letter(&write, &binding, revision(0), revision(0), None)
                    .await
                    .map(|_| ())
            })
        };
        let spawn_resume = || {
            use jobniac_core::german::resume_store::GermanResumeRepository;
            let (repository, write, binding) =
                (resumes.clone(), replacement.clone(), resume_binding.clone());
            tokio::spawn(async move {
                repository
                    .replace_germany_resume(&write, &[binding], revision(0), revision(0), None)
                    .await
                    .map(|_| ())
            })
        };
        let (first, first_app, second_app) = if winner == "resume" {
            (spawn_resume(), RESUME_APP, APP)
        } else {
            (spawn_letter(), APP, RESUME_APP)
        };
        wait_for_lock(&fixture.database, first_app).await?;
        let second = if winner == "resume" {
            spawn_letter()
        } else {
            spawn_resume()
        };
        wait_for_blocker(&fixture.database, second_app, first_app).await?;
        assert!(!first.is_finished() && !second.is_finished());
        release_gate(&gate, key).await?;
        first.await??;
        if winner == "resume" {
            let (code, message) = refusal(second.await?)?;
            assert_eq!((code.code(), message.as_str()), conflict_message());
        } else {
            second.await??;
        }
        assert_eq!(resumes_find(&fixture, resume.id()).await?, None);
        assert!(
            resumes_find(&fixture, replacement.resume().id())
                .await?
                .is_some()
        );
        assert_eq!(letters.find_by_id(letter.cover_letter().id()).await?, None);
        let expected = if winner == "resume" {
            "ACTIVE"
        } else {
            "RETIRING"
        };
        assert_eq!(
            asset_state(&admin, &letter_binding).await?.lifecycle,
            expected
        );
        assert_eq!(
            asset_state(&admin, &resume_binding).await?.lifecycle,
            "ACTIVE"
        );
    }
    Ok(())
}

async fn resumes_find(
    fixture: &Fixture,
    id: Uuid,
) -> Result<Option<Resume>, Box<dyn std::error::Error>> {
    use jobniac_core::german::resume_store::GermanResumeRepository;
    Ok(fixture.resumes(later()).find_by_id(id).await?)
}

#[tokio::test]
async fn owner_locks_serialize_numeric_edits() -> TestResult {
    for owner in ["profile", "job"] {
        for winner in ["edit", "letter"] {
            let Some((fixture, resume)) = prepared().await? else {
                return Ok(());
            };
            let admin = fixture.admin().await;
            let binding = fixture.active_binding().await?;
            let write = letter_write(&resume, &binding, &[8, 2])?;
            let repository = fixture.letters(later());
            let (table, identifier) = if owner == "profile" {
                ("profile.profiles", resume.profile_id())
            } else {
                ("job_schema.jobs", resume.job_id())
            };
            let update = format!("UPDATE {table} SET revision = revision + 1 WHERE id = $1");
            let spawn_save = || {
                let (repository, write, binding) =
                    (repository.clone(), write.clone(), binding.clone());
                tokio::spawn(async move {
                    repository
                        .replace_german_cover_letter(
                            &write,
                            &binding,
                            revision(0),
                            revision(0),
                            None,
                        )
                        .await
                })
            };
            if winner == "edit" {
                let editor = fixture.admin().await;
                editor.batch_execute("BEGIN").await?;
                editor.execute(&update, &[&identifier]).await?;
                let task = spawn_save();
                wait_for_lock(&fixture.database, APP).await?;
                assert!(!task.is_finished());
                editor.batch_execute("COMMIT").await?;
                let (code, message) = refusal(task.await?)?;
                assert_eq!((code.code(), message.as_str()), conflict_message());
                assert_eq!(
                    repository.find_by_id(write.cover_letter().id()).await?,
                    None
                );
                assert_eq!(asset_state(&admin, &binding).await?.lifecycle, "ACTIVE");
            } else {
                let editor_app = format!("synthetic-letter-{owner}-edit");
                let gate =
                    gate_inserts(&fixture.database, "cover_letter.cover_letters", 913_719).await?;
                let task = spawn_save();
                wait_for_lock(&fixture.database, APP).await?;
                let editor = fixture.admin().await;
                editor
                    .batch_execute(&format!("SET application_name = '{editor_app}'"))
                    .await?;
                let edit = tokio::spawn(async move {
                    editor.execute(&update, &[&identifier]).await.map(|_| ())
                });
                wait_for_blocker(&fixture.database, &editor_app, APP).await?;
                assert!(!edit.is_finished());
                assert_eq!(
                    repository.find_by_id(write.cover_letter().id()).await?,
                    None
                );
                release_gate(&gate, 913_719).await?;
                assert_eq!(&task.await??.cover_letter, write.cover_letter());
                edit.await??;
                assert_eq!(
                    repository
                        .find_by_id(write.cover_letter().id())
                        .await?
                        .as_ref(),
                    Some(write.cover_letter())
                );
            }
            let row = admin
                .query_one(
                    &format!("SELECT revision, updated_at FROM {table} WHERE id = $1"),
                    &[&identifier],
                )
                .await?;
            assert_eq!(row.try_get::<_, i64>("revision")?, 1);
            assert_eq!(row.try_get::<_, time::OffsetDateTime>("updated_at")?, now());
        }
    }
    Ok(())
}

/// A classified letter claim of the prepared resume.
async fn letter_claim(
    fixture: &Fixture,
    resume: &Resume,
) -> Result<IntentReservation, Box<dyn std::error::Error>> {
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
        let Some((fixture, resume)) = prepared().await? else {
            return Ok(());
        };
        let admin = fixture.admin().await;
        let claim = letter_claim(&fixture, &resume).await?;
        let rows = snapshot(&admin).await?;
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
        assert_eq!(snapshot(&admin).await?, rows, "{defect}");
    }
    Ok(())
}

#[tokio::test]
async fn classified_cover_letter_completion_rolls_back_link_and_retirement() -> TestResult {
    for failure in ["completion", "expiry_after_link"] {
        let Some((fixture, resume)) = prepared().await? else {
            return Ok(());
        };
        let admin = fixture.admin().await;
        let old = fixture.active_binding().await?;
        save_letter(
            &fixture.letters(later()),
            &letter_write(&resume, &old, &[8, 2])?,
            &old,
        )
        .await?;
        let claim = letter_claim(&fixture, &resume).await?;
        let rows = snapshot(&admin).await?;
        let (repository, unavailable) = if failure == "completion" {
            fail_at(
                &admin,
                "jobniac.generation_intents",
                "UPDATE",
                "NEW.state = 'COMPLETED'",
            )
            .await?;
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
        let (code, message) = refusal(result)?;
        if unavailable {
            assert_eq!((code, message.as_str()), UNAVAILABLE);
        } else {
            assert_eq!((code, message.as_str()), LETTER_INVALID);
        }
        assert_eq!(snapshot(&admin).await?, rows, "{failure}");
        assert_eq!(asset_state(&admin, &old).await?.lifecycle, "ACTIVE");
    }
    Ok(())
}

#[tokio::test]
async fn classified_cover_letter_completion_and_retirement_preserve_terminal_history() -> TestResult
{
    let Some((fixture, resume)) = prepared().await? else {
        return Ok(());
    };
    let admin = fixture.admin().await;
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
    let completed = admin
        .query_one(
            "SELECT state FROM jobniac.generation_intents WHERE asset_id = $1",
            &[&claim.asset_id()],
        )
        .await?;
    assert_eq!(completed.try_get::<_, String>("state")?, "COMPLETED");
    let replacement = fixture.active_binding().await?;
    save_letter(
        &repository,
        &letter_write(&resume, &replacement, &[8, 2])?,
        &replacement,
    )
    .await?;
    assert_eq!(
        asset_state(&admin, &claim.reservation).await?.lifecycle,
        "RETIRING"
    );
    let history = admin
        .query_one(
            "SELECT state FROM jobniac.generation_intents WHERE asset_id = $1",
            &[&claim.asset_id()],
        )
        .await?;
    assert_eq!(history.try_get::<_, String>("state")?, "COMPLETED");
    Ok(())
}

/// The cover-letter repository without configuration, which must refuse every call without
/// connecting.
fn unconfigured_letters() -> PgGermanCoverLetterRepository {
    PgGermanCoverLetterRepository::new(
        PgUnitOfWork::new(cover_letter::pool(None)),
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
async fn connection_failure_has_no_private_driver_context() -> TestResult {
    let Some((fixture, resume)) = prepared().await? else {
        return Ok(());
    };
    let repository = PgGermanCoverLetterRepository::new(
        PgUnitOfWork::new(cover_letter::pool(Some(closed_port_config(&fixture)?))),
        Arc::new(FixedClock::new(later())),
    );
    let binding = fixture.active_binding().await?;
    let write = letter_write(&resume, &binding, &[8, 2])?;
    let (code, message) = refusal(save_letter(&repository, &write, &binding).await)?;
    assert_eq!((code, message.as_str()), UNAVAILABLE);
    assert!(!message.contains("127.0.0.1") && !message.contains("connect"));
    let (code, message) = refusal(repository.find_by_id(write.cover_letter().id()).await)?;
    assert_eq!((code, message.as_str()), UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn invalid_aggregate_fails_before_connecting() -> TestResult {
    let Some((fixture, resume)) = prepared().await? else {
        return Ok(());
    };
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
    let Some((fixture, resume)) = prepared().await? else {
        return Ok(());
    };
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
        vec![german_support::order(0), german_support::order(1)]
    );
    Ok(())
}

#[tokio::test]
async fn cleanup_winner_invalidates_waiting_binding_without_letter_mutation() -> TestResult {
    let Some((fixture, resume)) = prepared().await? else {
        return Ok(());
    };
    let admin = fixture.admin().await;
    let old = fixture.active_binding().await?;
    let new = fixture.active_binding().await?;
    let repository = fixture.letters(later());
    save_letter(&repository, &letter_write(&resume, &old, &[8, 2])?, &old).await?;
    let candidate = letter_write(&resume, &new, &[8, 2])?;
    let snapshot_before = snapshot(&admin).await?;
    let gate = gate_retirements(&fixture.database, 913_718).await?;
    let cleanup = cleanup_repository(&fixture.config, later());
    let asset = new.asset.asset_id();
    let preparation = tokio::spawn(async move { cleanup.prepare_cleanup(asset).await });
    wait_for_lock(&fixture.database, "jobniac-cleanup").await?;
    let replacement = tokio::spawn({
        let repository = repository.clone();
        let binding = new.clone();
        async move {
            repository
                .replace_german_cover_letter(&candidate, &binding, revision(0), revision(0), None)
                .await
        }
    });
    wait_for_blocker(&fixture.database, APP, "jobniac-cleanup").await?;
    assert!(!replacement.is_finished());
    assert_eq!(snapshot(&admin).await?, snapshot_before);
    release_gate(&gate, 913_718).await?;
    assert_eq!(preparation.await??, PreparationOutcome::Prepared);
    let (code, message) = refusal(replacement.await?)?;
    assert_eq!((code, message.as_str()), LETTER_INVALID);
    let committed = snapshot(&admin).await?;
    for table in german_support::TABLES {
        if table != "jobniac.generated_assets" && table != "jobniac.generated_asset_cleanups" {
            assert_eq!(
                table_rows(&committed, table),
                table_rows(&snapshot_before, table),
                "{table}"
            );
        }
    }
    assert_eq!(asset_state(&admin, &old).await?.lifecycle, "ACTIVE");
    assert_eq!(asset_state(&admin, &new).await?.lifecycle, "RETIRING");
    Ok(())
}

#[tokio::test]
async fn letter_winner_makes_waiting_cleanup_observe_reference() -> TestResult {
    let Some((fixture, resume)) = prepared().await? else {
        return Ok(());
    };
    let admin = fixture.admin().await;
    let binding = fixture.active_binding().await?;
    let repository = fixture.letters(later());
    let write = letter_write(&resume, &binding, &[8, 2])?;
    let gate = gate_inserts(&fixture.database, "cover_letter.cover_letters", 913_719).await?;
    let replacement = tokio::spawn({
        let repository = repository.clone();
        let write = write.clone();
        let binding = binding.clone();
        async move {
            repository
                .replace_german_cover_letter(&write, &binding, revision(0), revision(0), None)
                .await
        }
    });
    wait_for_lock(&fixture.database, APP).await?;
    let cleanup = cleanup_repository(&fixture.config, later());
    let asset = binding.asset.asset_id();
    let preparation = tokio::spawn(async move { cleanup.prepare_cleanup(asset).await });
    wait_for_blocker(&fixture.database, "jobniac-cleanup", APP).await?;
    assert!(!preparation.is_finished());
    release_gate(&gate, 913_719).await?;
    assert_eq!(&replacement.await??.cover_letter, write.cover_letter());
    assert_eq!(preparation.await??, PreparationOutcome::Referenced);
    let state = asset_state(&admin, &binding).await?;
    assert_eq!(state.lifecycle, "ACTIVE");
    assert_eq!(state.task_id, None);
    Ok(())
}

#[tokio::test]
async fn letter_retirement_winner_rejects_waiting_attachment() -> TestResult {
    let Some((fixture, resume)) = prepared().await? else {
        return Ok(());
    };
    let admin = fixture.admin().await;
    let old = fixture.active_binding().await?;
    let new = fixture.active_binding().await?;
    let repository = fixture.letters(later());
    save_letter(&repository, &letter_write(&resume, &old, &[8, 2])?, &old).await?;
    let gate = gate_inserts(&fixture.database, "cover_letter.cover_letters", 913_719).await?;
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
    wait_for_lock(&fixture.database, APP).await?;
    let uow = PgUnitOfWork::new(PgPool::with_application_name(
        Some(fixture.config.clone()),
        "synthetic-letter-attachment",
    ));
    let (profile, photo) = (fixture.profile, old.clone());
    let attachment = tokio::spawn(async move {
        uow.transaction(options(), async |tx| {
            attach_photo(tx.client(), profile, &photo)
                .await
                .map_err(|_| AppError::Internal)
        })
        .await
    });
    wait_for_blocker(&fixture.database, "synthetic-letter-attachment", APP).await?;
    assert!(!attachment.is_finished());
    release_gate(&gate, 913_719).await?;
    replacement.await??;
    assert!(attachment.await?.is_err());
    assert_eq!(asset_state(&admin, &old).await?.lifecycle, "RETIRING");
    let row = admin
        .query_one(
            "SELECT count(*) AS total FROM profile.profile_personal_details WHERE profile_id = $1",
            &[&fixture.profile],
        )
        .await?;
    assert_eq!(row.try_get::<_, i64>("total")?, 0);
    Ok(())
}

#[tokio::test]
async fn attachment_winner_remains_visible_after_document_lock_wait() -> TestResult {
    let Some((fixture, resume)) = prepared().await? else {
        return Ok(());
    };
    let admin = fixture.admin().await;
    let old = fixture.active_binding().await?;
    let new = fixture.active_binding().await?;
    let repository = fixture.letters(later());
    let before = letter_write(&resume, &old, &[8, 2])?;
    save_letter(&repository, &before, &old).await?;
    admin.batch_execute("BEGIN").await?;
    attach_photo(&admin, fixture.profile, &old).await?;
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
    wait_for_lock(&fixture.database, APP).await?;
    assert!(!replacement.is_finished());
    admin.batch_execute("COMMIT").await?;
    let result = replacement.await??;
    assert_eq!(
        result.previous_variants,
        vec![before.variant().variant().clone()]
    );
    let state = asset_state(&admin, &old).await?;
    assert_eq!(state.lifecycle, "ACTIVE");
    assert_eq!(state.task_id, None);
    let outcome = cleanup_repository(&fixture.config, later())
        .prepare_cleanup(old.asset.asset_id())
        .await?;
    assert_eq!(outcome, PreparationOutcome::Referenced);
    Ok(())
}

#[tokio::test]
async fn schema_guards_reject_unreachable_dependency_states_without_mutation() -> TestResult {
    for defect in [
        "non_germany",
        "path_only",
        "reserved_attachment",
        "resume_identity",
    ] {
        let Some((fixture, resume)) = prepared().await? else {
            return Ok(());
        };
        let admin = fixture.admin().await;
        let binding = if defect == "reserved_attachment" {
            fixture.reserved_binding().await?
        } else {
            fixture.active_binding().await?
        };
        let snapshot_before = snapshot(&admin).await?;
        // Every probe runs in one transaction that is rolled back, so a rejected parent row
        // cannot survive the rejected child.
        admin.batch_execute("BEGIN").await?;
        let outcome: Result<(), Box<dyn std::error::Error>> = match defect {
            "resume_identity" => {
                let other = Uuid::new_v4();
                insert_owners(&admin, other, Uuid::new_v4(), "Other").await?;
                let altered = resume_like(&resume, other, resume.job_id())?;
                insert_letter(&admin, &altered, &binding).await.map(|_| ())
            }
            "reserved_attachment" => insert_letter(&admin, &resume, &binding).await.map(|_| ()),
            "non_germany" => admin
                .execute(
                    "UPDATE resume.resumes SET format = 'france' WHERE id = $1",
                    &[&resume.id()],
                )
                .await
                .map(|_| ())
                .map_err(Into::into),
            _ => admin
                .execute(
                    "INSERT INTO resume.resume_variants \
                     (id, resume_id, language, document_id, file_path, created_at, updated_at) \
                     VALUES ($1, $2, 'de', NULL, $3, $4, $5)",
                    &[
                        &Uuid::new_v4(),
                        &resume.id(),
                        &binding.asset.relative_path(),
                        &now(),
                        &later(),
                    ],
                )
                .await
                .map(|_| ())
                .map_err(Into::into),
        };
        admin.batch_execute("ROLLBACK").await?;
        let error = match outcome {
            Err(error) => error,
            Ok(()) => return Err(format!("{defect} was accepted").into()),
        };
        if defect == "resume_identity" {
            let constraint = error
                .downcast_ref::<tokio_postgres::Error>()
                .and_then(tokio_postgres::Error::as_db_error)
                .and_then(tokio_postgres::error::DbError::constraint);
            assert_eq!(constraint, Some("cover_letters_resume_identity_fk"));
        }
        assert_eq!(snapshot(&admin).await?, snapshot_before, "{defect}");
        let expected = if defect == "reserved_attachment" {
            "RESERVED"
        } else {
            "ACTIVE"
        };
        assert_eq!(
            asset_state(&admin, &binding).await?.lifecycle,
            expected,
            "{defect}"
        );
    }
    Ok(())
}
