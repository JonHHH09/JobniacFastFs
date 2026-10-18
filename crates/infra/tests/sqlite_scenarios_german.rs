//! Scenarios of the German resume and cover-letter stores that the single SQLite writer must keep:
//! the writer that commits first (cleanup, replacement, retirement) and the writer or attachment
//! that re-reads state and is refused, and numeric owner edits that serialize with a replacement on
//! one file.
//!
//! The contention tests never sleep to synchronize: a clock that blocks inside the replacement
//! pauses it while it holds the write lock, a raw connection can hold `BEGIN IMMEDIATE`, and
//! channels tell the test when the writer is there. They run on a multi-thread runtime because a
//! paused or waiting writer parks its worker. All data is synthetic.

mod sqlite_german_support;
mod sqlite_scenarios_support;
mod sqlite_support;

use jobniac_core::cleanup::CleanupRepository;
use jobniac_core::generated_assets::{GeneratedReservation, PreparationOutcome};
use jobniac_core::german::cover_letter_store::GermanCoverLetterRepository;
use jobniac_core::german::resume_store::{GermanResumeRepository, Resume};
use jobniac_infra::sqlite::error::SqliteError;
use jobniac_infra::sqlite::generated::guards::guard_document_reference;
use sqlite_german_support::{
    Fixture, RESUME_INVALID, TABLES, TestResult, asset_state, attach_photo, cleanup_repository,
    insert_letter, later, letter_write, options, refusal, resume_write, revision, save_letter,
    save_resume, snapshot, start, table_rows,
};
use sqlite_scenarios_support::{PauseClock, WriteHold, assert_still_waiting, spawn_raw_write};
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

/// The number of personal-details rows of the profile, which a photo attachment would add.
fn photo_rows(fixture: &Fixture) -> Result<i64, BoxError> {
    Ok(fixture.admin().query_row(
        "SELECT count(*) FROM profile_profile_personal_details WHERE profile_id = ?1",
        [fixture.profile.to_string()],
        |row| row.get(0),
    )?)
}

/// Attaches the binding's document as the profile photo the way a guarded writer does: the
/// reference guard first, in one write transaction, then the reference row. (No application path
/// writes a photo reference; the guard is the rule the other document references share.)
async fn guarded_attach(
    fixture: &Fixture,
    binding: &GeneratedReservation,
) -> Result<(), SqliteError> {
    fixture
        .uow
        .transaction::<(), SqliteError>(options(), async |tx| {
            tx.run(|conn| {
                guard_document_reference(conn, Some(binding.document_id))?;
                attach_photo(conn, fixture.profile, binding).map_err(|_| SqliteError::Internal)
            })
        })
        .await
}

/// Whether the attachment was refused by the check guard.
fn is_check_violation(result: &Result<(), SqliteError>) -> bool {
    matches!(
        result,
        Err(SqliteError::Constraint { sqlstate, .. }) if sqlstate == "23514"
    )
}

/// The numeric revision of the profile or the job.
fn owner_revision(fixture: &Fixture, owner: &str) -> Result<i64, BoxError> {
    let (sql, id) = if owner == "profile" {
        (
            "SELECT revision FROM profile_profiles WHERE id = ?1",
            fixture.profile,
        )
    } else {
        ("SELECT revision FROM job_jobs WHERE id = ?1", fixture.job)
    };
    Ok(fixture
        .admin()
        .query_row(sql, [id.to_string()], |row| row.get(0))?)
}

/// The statement that bumps the revision of the profile or the job, and the id it takes.
fn edit_of(fixture: &Fixture, owner: &str) -> (&'static str, Uuid) {
    if owner == "profile" {
        (
            "UPDATE profile_profiles SET revision = revision + 1 WHERE id = ?1",
            fixture.profile,
        )
    } else {
        (
            "UPDATE job_jobs SET revision = revision + 1 WHERE id = ?1",
            fixture.job,
        )
    }
}

// ---------------------------------------------------------------------------------------------
// German resume
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn committing_cleanup_wins_and_waiting_replacement_preserves_old_tree() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let old = fixture.active_binding().await?;
    let new = fixture.active_binding().await?;
    let repository = fixture.resumes(later());
    let before = resume_write(fixture.profile, fixture.job, &[&old])?;
    save_resume(&repository, &before, &[&old]).await?;
    insert_letter(&admin, before.resume(), &old)?;
    let after = resume_write(fixture.profile, fixture.job, &[&new])?;
    let snapshot_before = snapshot(&admin)?;

    // The cleanup commits first and retires the unreferenced new asset.
    let outcome = cleanup_repository(&fixture, later())
        .prepare_cleanup(new.asset.asset_id())
        .await?;
    assert_eq!(outcome, PreparationOutcome::Prepared);
    let committed = snapshot(&admin)?;

    // The replacement comes second, re-reads the binding and is refused by its state.
    let (code, message) = refusal(save_resume(&repository, &after, &[&new]).await)?;
    assert_eq!((code, message.as_str()), RESUME_INVALID);
    assert_eq!(snapshot(&admin)?, committed);
    assert_eq!(
        repository.find_by_id(before.resume().id()).await?.as_ref(),
        Some(before.resume())
    );
    assert_eq!(repository.find_by_id(after.resume().id()).await?, None);
    for table in TABLES {
        if table != "jobniac_generated_assets" && table != "jobniac_generated_asset_cleanups" {
            assert_eq!(
                table_rows(&committed, table),
                table_rows(&snapshot_before, table),
                "{table}"
            );
        }
    }
    let old_state = asset_state(&admin, &old)?;
    assert_eq!(old_state.lifecycle, "ACTIVE");
    assert_eq!(old_state.task_id, None);
    let new_state = asset_state(&admin, &new)?;
    assert_eq!(new_state.lifecycle, "RETIRING");
    assert_eq!(new_state.status.as_deref(), Some("PENDING"));
    assert!(new_state.task_id.is_some() && new_state.retirement_id.is_some());
    assert_eq!(new_state.retired_at, Some(later()));
    assert_eq!(new_state.next_attempt_at, Some(later()));
    assert_eq!(
        table_rows(&committed, "jobniac_generated_asset_cleanups").len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn committing_replacement_binding_wins_against_waiting_cleanup() -> TestResult {
    let fixture = start()?;
    let binding = fixture.active_binding().await?;
    let repository = fixture.resumes(later());
    let write = resume_write(fixture.profile, fixture.job, &[&binding])?;

    // The replacement commits first and references the binding.
    save_resume(&repository, &write, &[&binding]).await?;

    // The cleanup comes second, re-reads the references and finds the resume variant.
    let outcome = cleanup_repository(&fixture, later())
        .prepare_cleanup(binding.asset.asset_id())
        .await?;
    assert_eq!(outcome, PreparationOutcome::Referenced);
    let state = asset_state(&fixture.admin(), &binding)?;
    assert_eq!(state.lifecycle, "ACTIVE");
    assert_eq!(state.task_id, None);
    Ok(())
}

#[tokio::test]
async fn replacement_retirement_wins_and_waiting_attachment_is_rejected() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    let binding = fixture.active_binding().await?;
    let repository = fixture.resumes(later());
    let before = resume_write(fixture.profile, fixture.job, &[&binding])?;
    save_resume(&repository, &before, &[&binding]).await?;

    // The replacement drops the variant and retires its asset first.
    save_resume(
        &repository,
        &resume_write(fixture.profile, fixture.job, &[])?,
        &[],
    )
    .await?;
    let committed = snapshot(&admin)?;

    // The attachment comes second, finds the asset retired and is rejected without a trace.
    assert!(is_check_violation(
        &guarded_attach(&fixture, &binding).await
    ));
    assert_eq!(snapshot(&admin)?, committed);
    let state = asset_state(&admin, &binding)?;
    assert_eq!(state.lifecycle, "RETIRING");
    assert_eq!(state.status.as_deref(), Some("PENDING"));
    assert_eq!(photo_rows(&fixture)?, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn numeric_source_update_waits_until_replacement_commits() -> TestResult {
    for owner in ["profile", "job"] {
        let fixture = start()?;
        let admin = fixture.admin();
        let old = fixture.active_binding().await?;
        let new = fixture.active_binding().await?;
        let repository = fixture.resumes(later());
        let before = resume_write(fixture.profile, fixture.job, &[&old])?;
        save_resume(&repository, &before, &[&old]).await?;
        let after = resume_write(fixture.profile, fixture.job, &[&new])?;
        let (sql, id) = edit_of(&fixture, owner);

        // The replacement stops inside its transaction, holding the write lock. The numeric edit
        // of the owner, on its own connection, must wait for it.
        let (clock, mut pause) = PauseClock::new(later(), 2);
        let replacement = tokio::spawn({
            let paused = fixture.resumes_with(clock);
            let (after, new) = (after.clone(), new.clone());
            async move {
                paused
                    .replace_germany_resume(&after, &[new], revision(0), revision(0), None)
                    .await
            }
        });
        pause.reached().await;
        let update = spawn_raw_write(fixture.sqlite.path(), sql, id.to_string());
        assert_still_waiting(&update, owner).await;
        assert_eq!(owner_revision(&fixture, owner)?, 0, "{owner}");
        assert_eq!(
            repository.find_by_id(before.resume().id()).await?.as_ref(),
            Some(before.resume())
        );
        assert_eq!(repository.find_by_id(after.resume().id()).await?, None);

        pause.release();
        assert_eq!(&replacement.await??.resume, after.resume());
        assert_eq!(update.await??, 1, "{owner}");
        assert_eq!(owner_revision(&fixture, owner)?, 1, "{owner}");
        assert_eq!(repository.find_by_id(before.resume().id()).await?, None);
        assert_eq!(
            repository.find_by_id(after.resume().id()).await?.as_ref(),
            Some(after.resume())
        );

        // A candidate built from the stale numeric revision is refused and changes nothing.
        let stale = resume_write(fixture.profile, fixture.job, &[&new])?;
        let rows = snapshot(&admin)?;
        let (code, message) = refusal(save_resume(&repository, &stale, &[&new]).await)?;
        assert_eq!(code.code(), "conflict");
        assert_eq!(message, "German resume source revision conflict");
        assert_eq!(snapshot(&admin)?, rows);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// German cover letter
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn letter_retirement_winner_rejects_waiting_attachment() -> TestResult {
    let (fixture, resume) = prepared().await?;
    let admin = fixture.admin();
    let old = fixture.active_binding().await?;
    let new = fixture.active_binding().await?;
    let repository = fixture.letters(later());
    save_letter(&repository, &letter_write(&resume, &old, &[8, 2])?, &old).await?;

    // The replacement commits first and retires the old letter asset.
    save_letter(&repository, &letter_write(&resume, &new, &[8, 2])?, &new).await?;
    let committed = snapshot(&admin)?;

    // The attachment comes second, finds the asset retired and is rejected without a trace.
    assert!(is_check_violation(&guarded_attach(&fixture, &old).await));
    assert_eq!(snapshot(&admin)?, committed);
    assert_eq!(asset_state(&admin, &old)?.lifecycle, "RETIRING");
    assert_eq!(asset_state(&admin, &new)?.lifecycle, "ACTIVE");
    assert_eq!(photo_rows(&fixture)?, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_locks_serialize_numeric_edits() -> TestResult {
    for owner in ["profile", "job"] {
        for winner in ["edit", "letter"] {
            let (fixture, resume) = prepared().await?;
            let admin = fixture.admin();
            let binding = fixture.active_binding().await?;
            let write = letter_write(&resume, &binding, &[8, 2])?;
            let repository = fixture.letters(later());
            let (sql, id) = edit_of(&fixture, owner);
            if winner == "edit" {
                // The edit holds the write lock and commits first; the letter built from the old
                // revision waits, re-reads the owner and is refused.
                let hold = WriteHold::begin(fixture.sqlite.path());
                hold.connection().execute(sql, [id.to_string()])?;
                let task = tokio::spawn({
                    let repository = repository.clone();
                    let (write, binding) = (write.clone(), binding.clone());
                    async move {
                        repository
                            .replace_german_cover_letter(
                                &write,
                                &binding,
                                revision(0),
                                revision(0),
                                None,
                            )
                            .await
                    }
                });
                assert_still_waiting(&task, "the letter").await;
                hold.commit();
                let (code, message) = refusal(task.await?)?;
                assert_eq!(
                    (code.code(), message.as_str()),
                    ("conflict", "German cover-letter source revision conflict")
                );
                assert_eq!(
                    repository.find_by_id(write.cover_letter().id()).await?,
                    None
                );
                assert_eq!(asset_state(&admin, &binding)?.lifecycle, "ACTIVE");
            } else {
                // The letter stops inside its transaction; the edit waits for it and commits
                // second.
                let (clock, mut pause) = PauseClock::new(later(), 2);
                let task = tokio::spawn({
                    let paused = fixture.letters_with(clock);
                    let (write, binding) = (write.clone(), binding.clone());
                    async move {
                        paused
                            .replace_german_cover_letter(
                                &write,
                                &binding,
                                revision(0),
                                revision(0),
                                None,
                            )
                            .await
                    }
                });
                pause.reached().await;
                let edit = spawn_raw_write(fixture.sqlite.path(), sql, id.to_string());
                assert_still_waiting(&edit, "the owner edit").await;
                assert_eq!(owner_revision(&fixture, owner)?, 0);
                assert_eq!(
                    repository.find_by_id(write.cover_letter().id()).await?,
                    None
                );
                pause.release();
                assert_eq!(&task.await??.cover_letter, write.cover_letter());
                assert_eq!(edit.await??, 1);
                assert_eq!(
                    repository
                        .find_by_id(write.cover_letter().id())
                        .await?
                        .as_ref(),
                    Some(write.cover_letter())
                );
            }
            assert_eq!(owner_revision(&fixture, owner)?, 1, "{owner} {winner}");
        }
    }
    Ok(())
}
