//! Snapshot scenarios of the matching adapter on SQLite. A pair analysis is one write scope
//! (`BEGIN IMMEDIATE`), so a job edit that commits while it waits for the writer is read whole: the
//! report carries the new revision and the new skills together, never a mix. A read-only scope is
//! one WAL snapshot: a commit that lands in the middle of it stays invisible until the scope ends.
//!
//! The writer wait is a raw connection that holds `BEGIN IMMEDIATE`; the test never sleeps to
//! synchronize. All data is synthetic.

#[allow(
    unused_imports,
    reason = "this binary uses a subset of the shared re-exports"
)]
mod sqlite_match_support;
mod sqlite_scenarios_support;

use std::path::PathBuf;

use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::sqlite::codec::{instant_text, uuid_text};
use jobniac_infra::sqlite::error::SqliteError;
use serde_json::{Value, json};
use sqlite_match_support::{TestResult, analysis, fixed_clock, seed_job, seed_profile, start};
use sqlite_scenarios_support::{WriteHold, assert_still_waiting};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const T0: OffsetDateTime = datetime!(2025-01-02 03:04:05 UTC);

fn pair(profile: Uuid, job: Uuid) -> Value {
    json!({"profileId": profile.to_string(), "jobId": job.to_string()})
}

/// The database file of a raw connection.
fn path_of(connection: &rusqlite::Connection) -> PathBuf {
    PathBuf::from(connection.path().expect("a file database"))
}

/// A job edit committed while the analysis waits for the writer is part of what it reads: the
/// root and the skills come from the one committed state, so the report keeps the new revision and
/// scores the new skill, not the old one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pair_analysis_reads_one_snapshot_across_a_concurrent_skill_update() -> TestResult {
    let fixture = start();
    let connection = fixture.connection();
    let profile = seed_profile(&connection, "snapshot@example.test")?;
    let job = seed_job(&fixture.uow, 0x560, T0).await?;

    // The baseline: the job as first stored (a Python skill, which the profile has).
    let baseline = analysis(&fixture.uow, fixed_clock(T0))?
        .analyze_job_match(&pair(profile, job))
        .await?;
    assert_eq!(baseline.report.job_revision, T0);

    // Another writer edits the job: a new revision instant and another required skill, in one
    // transaction that it holds open.
    let edited_at = T0 + Duration::days(1);
    let hold = WriteHold::begin(&path_of(&connection));
    hold.connection().execute(
        "UPDATE job_jobs SET updated_at = ?2, revision = revision + 1 WHERE id = ?1",
        rusqlite::params![uuid_text(job), instant_text(edited_at)],
    )?;
    hold.connection().execute(
        "UPDATE job_job_skills SET skill = 'Haskell', normalized_skill = 'haskell' \
         WHERE job_id = ?1",
        [uuid_text(job)],
    )?;
    let service = analysis(&fixture.uow, fixed_clock(T0 + Duration::days(2)))?;
    let request = pair(profile, job);
    let task = tokio::spawn(async move { service.analyze_job_match(&request).await });
    assert_still_waiting(&task, "the analysis").await;
    hold.commit();

    let view = task.await??;
    assert_eq!(view.report.job_revision, edited_at);
    assert!(!view.stale);
    // A torn read (new revision, old skill) would score exactly like the baseline.
    assert_ne!(view.report.components, baseline.report.components);
    assert_ne!(view.report.id, baseline.report.id);
    Ok(())
}

/// A read-only scope keeps its snapshot while another writer commits inside it, and the next scope
/// sees the commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_only_scope_keeps_one_snapshot_across_a_concurrent_commit() -> TestResult {
    let fixture = start();
    let job = seed_job(&fixture.uow, 0x561, T0).await?;
    let writer = fixture.connection();
    let read_updated = |connection: &rusqlite::Connection| -> Result<String, SqliteError> {
        Ok(connection.query_row(
            "SELECT updated_at FROM job_jobs WHERE id = ?1",
            [uuid_text(job)],
            |row| row.get(0),
        )?)
    };
    let read_only = TransactionOptions {
        read_only: true,
        ..TransactionOptions::default()
    };
    let edited_at = instant_text(T0 + Duration::days(1));
    let (first, second) = fixture
        .uow
        .transaction::<_, SqliteError>(read_only, async |tx| {
            let first = tx.run(read_updated)?;
            // Another writer commits in the middle of the read scope (WAL admits it).
            writer.execute(
                "UPDATE job_jobs SET updated_at = ?2 WHERE id = ?1",
                rusqlite::params![uuid_text(job), edited_at],
            )?;
            let second = tx.run(read_updated)?;
            Ok((first, second))
        })
        .await?;
    assert_eq!(first, instant_text(T0));
    assert_eq!(second, first, "the snapshot must not move inside the scope");
    let after = fixture
        .uow
        .transaction::<_, SqliteError>(read_only, async |tx| tx.run(read_updated))
        .await?;
    assert_eq!(after, edited_at);
    Ok(())
}
