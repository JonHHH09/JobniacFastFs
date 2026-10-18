//! SQLite tests of the matching adapter.
//!
//! Reuse and immutability of reports, staleness after a profile or job revision change, missing
//! entities, concurrent single pairs, batch and history paging, and the unconfigured and
//! unavailable database envelopes. Each test works on its own database file.

mod sqlite_match_support;

use std::collections::BTreeSet;
use std::error::Error;
use std::sync::Arc;

use jobniac_core::errors::error_envelope;
use jobniac_core::matching::{MatchView, MatchingRepository};
use jobniac_infra::sqlite::codec::{instant_text, uuid_text};
use jobniac_infra::sqlite::matching::SqliteMatchingRepository;
use serde_json::{Value, json};
use sqlite_match_support::sqlite_job_support::closed_uow;
use sqlite_match_support::{
    PROFILE_AT, TestResult, analysis, count, fixed_clock, seed_job, seed_profile, start,
    unconfigured_uow,
};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const T0: OffsetDateTime = datetime!(2025-01-02 03:04:05 UTC);

fn pair(profile: Uuid, job: Uuid) -> Value {
    json!({"profileId": profile.to_string(), "jobId": job.to_string()})
}

/// Walks every page of the report history for `base` (the filters and the limit), in order.
async fn all_reports(
    service: &jobniac_core::matching::MatchAnalysisService<
        jobniac_infra::sqlite::matching::SqliteMatchingRepository,
    >,
    base: &Value,
) -> Result<Vec<MatchView>, Box<dyn Error>> {
    let mut views = Vec::new();
    let mut request = base.clone();
    loop {
        let page = service.list_match_reports(&request).await?;
        views.extend(page.reports);
        match page.next_cursor {
            Some(cursor) => request["cursor"] = json!(cursor),
            None => return Ok(views),
        }
    }
}

#[tokio::test]
async fn a_pair_is_stored_once_and_a_second_analysis_reuses_the_report() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "reuse@example.test")?;
    let job = seed_job(&fixture.uow, 0x101, T0).await?;
    let first = analysis(&fixture.uow, fixed_clock(T0))?
        .analyze_job_match(&pair(profile, job))
        .await?;
    assert!(!first.stale);
    let later = analysis(&fixture.uow, fixed_clock(T0 + Duration::hours(1)))?
        .analyze_job_match(&pair(profile, job))
        .await?;
    assert_eq!(later.report.id, first.report.id);
    assert_eq!(later.report.created_at, first.report.created_at);
    assert!(!later.stale);
    let client = fixture.connection();
    assert_eq!(count(&client, "SELECT count(*) FROM match_reports")?, 1);
    Ok(())
}

/// The immutability trigger refuses updates only. Deletes are not refused, because profile and job
/// deletion cascade to their reports (see `deleting_a_profile_cascades_to_its_reports`).
#[tokio::test]
async fn stored_reports_cannot_be_updated() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "immutable@example.test")?;
    let job = seed_job(&fixture.uow, 0x102, T0).await?;
    analysis(&fixture.uow, fixed_clock(T0))?
        .analyze_job_match(&pair(profile, job))
        .await?;
    let client = fixture.connection();
    let error = client
        .execute("UPDATE match_reports SET overall_score = 0", [])
        .err()
        .ok_or("the update must be refused")?;
    let message = match &error {
        rusqlite::Error::SqliteFailure(_, Some(message)) => message.clone(),
        _ => String::new(),
    };
    assert_eq!(message, "match history is immutable");
    Ok(())
}

#[tokio::test]
async fn a_profile_revision_change_marks_the_report_stale() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "stale-profile@example.test")?;
    let job = seed_job(&fixture.uow, 0x103, T0).await?;
    let service = analysis(&fixture.uow, fixed_clock(T0))?;
    let report = service.analyze_job_match(&pair(profile, job)).await?;
    assert_eq!(report.report.profile_revision, PROFILE_AT);
    fixture.connection().execute(
        "UPDATE profile_profiles SET updated_at = ?1, revision = revision + 1 WHERE id = ?2",
        rusqlite::params![
            instant_text(PROFILE_AT + Duration::days(1)),
            uuid_text(profile)
        ],
    )?;
    let current = service
        .get_match_report(&json!({"id": report.report.id.to_string()}))
        .await?;
    assert!(current.stale);
    assert_eq!(current.report, report.report);
    Ok(())
}

#[tokio::test]
async fn a_job_revision_change_marks_the_report_stale() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "stale-job@example.test")?;
    let job = seed_job(&fixture.uow, 0x104, T0).await?;
    let service = analysis(&fixture.uow, fixed_clock(T0))?;
    let report = service.analyze_job_match(&pair(profile, job)).await?;
    let client = fixture.connection();
    client.execute(
        "UPDATE job_jobs SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![instant_text(T0 + Duration::days(1)), uuid_text(job)],
    )?;
    let current = service
        .get_match_report(&json!({"id": report.report.id.to_string()}))
        .await?;
    assert!(current.stale);
    Ok(())
}

#[tokio::test]
async fn missing_entities_abort_before_any_report_is_written() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "missing@example.test")?;
    let job = seed_job(&fixture.uow, 0x105, T0).await?;
    let service = analysis(&fixture.uow, fixed_clock(T0))?;
    let unknown = Uuid::from_u128(0x9999);
    let no_profile = service
        .analyze_job_match(&pair(unknown, job))
        .await
        .err()
        .ok_or("an unknown profile must fail")?;
    assert_eq!(
        error_envelope(&no_profile).message,
        format!("profile not found: {unknown}")
    );
    let no_job = service
        .analyze_job_match(&pair(profile, unknown))
        .await
        .err()
        .ok_or("an unknown job must fail")?;
    assert_eq!(
        error_envelope(&no_job).message,
        format!("job not found: {unknown}")
    );
    let client = fixture.connection();
    assert_eq!(count(&client, "SELECT count(*) FROM match_reports")?, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_single_pairs_reuse_one_original_report() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "concurrent@example.test")?;
    let job = seed_job(&fixture.uow, 0x106, T0).await?;
    let service = Arc::new(analysis(&fixture.uow, fixed_clock(T0))?);
    let request = pair(profile, job);
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let service = Arc::clone(&service);
        let request = request.clone();
        tasks.push(tokio::spawn(async move {
            service.analyze_job_match(&request).await
        }));
    }
    let mut identifiers = BTreeSet::new();
    for task in tasks {
        identifiers.insert(task.await??.report.id);
    }
    assert_eq!(identifiers.len(), 1);
    let client = fixture.connection();
    assert_eq!(count(&client, "SELECT count(*) FROM match_reports")?, 1);
    Ok(())
}

#[tokio::test]
async fn batch_pages_visit_every_candidate_job_once() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "batch@example.test")?;
    let mut jobs = Vec::new();
    for index in 0..5_u128 {
        let created = T0 + Duration::days(i64::try_from(index)?);
        jobs.push(seed_job(&fixture.uow, 0x200 + index, created).await?);
    }
    let service = analysis(&fixture.uow, fixed_clock(T0))?;
    let mut seen = Vec::new();
    let mut request = json!({"profileId": profile.to_string(), "limit": 2});
    loop {
        let batch = service.analyze_all_job_matches(&request).await?;
        assert!(batch.failed.is_empty());
        seen.extend(batch.succeeded.iter().map(|view| view.report.job_id));
        match batch.next_cursor {
            Some(cursor) => request["cursor"] = json!(cursor),
            None => break,
        }
    }
    let unique: BTreeSet<Uuid> = seen.iter().copied().collect();
    assert_eq!(seen.len(), 5);
    assert_eq!(unique, jobs.into_iter().collect());
    Ok(())
}

#[tokio::test]
async fn history_pages_follow_the_profile_job_and_pair_filters() -> TestResult {
    let fixture = start();
    let first = seed_profile(&fixture.connection(), "history-one@example.test")?;
    let second = seed_profile(&fixture.connection(), "history-two@example.test")?;
    let job_one = seed_job(&fixture.uow, 0x301, T0).await?;
    let job_two = seed_job(&fixture.uow, 0x302, T0).await?;
    let pairs = [(first, job_one), (first, job_two), (second, job_one)];
    for (step, (profile, job)) in pairs.into_iter().enumerate() {
        let instant = T0 + Duration::seconds(i64::try_from(step)?);
        analysis(&fixture.uow, fixed_clock(instant))?
            .analyze_job_match(&pair(profile, job))
            .await?;
    }
    let service = analysis(&fixture.uow, fixed_clock(T0))?;
    let everything = all_reports(&service, &json!({"limit": 1})).await?;
    assert_eq!(everything.len(), 3);
    assert!(
        everything
            .windows(2)
            .all(|window| window[0].report.created_at >= window[1].report.created_at),
        "history is newest first"
    );
    let by_profile = all_reports(
        &service,
        &json!({"profileId": first.to_string(), "limit": 1}),
    )
    .await?;
    assert_eq!(by_profile.len(), 2);
    let by_job = all_reports(&service, &json!({"jobId": job_one.to_string(), "limit": 1})).await?;
    assert_eq!(by_job.len(), 2);
    let by_pair = all_reports(
        &service,
        &json!({"profileId": first.to_string(), "jobId": job_one.to_string(), "limit": 1}),
    )
    .await?;
    assert_eq!(by_pair.len(), 1);
    assert_eq!(by_pair[0].report.job_id, job_one);
    Ok(())
}

#[tokio::test]
async fn a_stored_report_that_breaks_the_record_rules_is_refused() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "fail-closed@example.test")?;
    let job = seed_job(&fixture.uow, 0x107, T0).await?;
    let client = fixture.connection();
    // A technical component that earns more than its 40 available points is not a valid record.
    let stored = Uuid::new_v4();
    client.execute(
        "INSERT INTO match_reports \
         (id, profile_id, job_id, profile_revision, job_revision, algorithm_version, \
          overall_score, confidence, outcome, blocker_mismatch, components, evidence, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?4, 'deterministic-v1', 0, 0, 'INSUFFICIENT_EVIDENCE', 0, \
                 '[{\"component\":\"TECHNICAL\",\"earnedPoints\":99,\"availablePoints\":40,\
                   \"status\":\"MATCH\"}]', '[]', ?4)",
        rusqlite::params![
            uuid_text(stored),
            uuid_text(profile),
            uuid_text(job),
            instant_text(T0)
        ],
    )?;
    let error = analysis(&fixture.uow, fixed_clock(T0))?
        .get_match_report(&json!({"id": stored.to_string()}))
        .await
        .err()
        .ok_or("an invalid stored record must not be returned")?;
    assert_eq!(
        error_envelope(&error).message,
        "Local database is unavailable"
    );
    Ok(())
}

#[tokio::test]
async fn an_unknown_report_is_a_domain_error() -> TestResult {
    let fixture = start();
    let unknown = Uuid::from_u128(0x7777);
    let error = analysis(&fixture.uow, fixed_clock(T0))?
        .get_match_report(&json!({"id": unknown.to_string()}))
        .await
        .err()
        .ok_or("an unknown report must fail")?;
    assert_eq!(
        error_envelope(&error).message,
        format!("match report not found: {unknown}")
    );
    Ok(())
}

#[tokio::test]
async fn an_unconfigured_database_reports_not_configured() -> TestResult {
    let uow = unconfigured_uow();
    let error = analysis(&uow, fixed_clock(T0))?
        .analyze_job_match(&json!({
            "profileId": Uuid::from_u128(1).to_string(),
            "jobId": Uuid::from_u128(2).to_string(),
        }))
        .await
        .err()
        .ok_or("the unconfigured database must fail")?;
    assert_eq!(
        error_envelope(&error).message,
        "Local database is not configured"
    );
    Ok(())
}

#[tokio::test]
async fn an_unavailable_database_is_reported_as_unavailable() -> TestResult {
    let uow = closed_uow();
    let error = analysis(&uow, fixed_clock(T0))?
        .analyze_job_match(&json!({
            "profileId": Uuid::from_u128(1).to_string(),
            "jobId": Uuid::from_u128(2).to_string(),
        }))
        .await
        .err()
        .ok_or("the unavailable database must fail")?;
    let message = error_envelope(&error).message;
    assert_eq!(message, "Local database is unavailable");
    assert!(!message.contains("connect"));
    Ok(())
}

/// A report write that the database refuses fails the analysis as a persistence constraint and
/// stores nothing. There is no serialization failure to retry: writers are serialized.
#[tokio::test]
async fn a_refused_report_write_is_a_constraint_error_and_stores_nothing() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "refused@example.test")?;
    let job = seed_job(&fixture.uow, 0x110, T0).await?;
    let client = fixture.connection();
    client.execute_batch(
        "CREATE TRIGGER match_reject_report BEFORE INSERT ON match_reports \
         BEGIN SELECT RAISE(ABORT, 'synthetic rejection'); END;",
    )?;
    let error = analysis(&fixture.uow, fixed_clock(T0))?
        .analyze_job_match(&pair(profile, job))
        .await
        .err()
        .ok_or("a refused write must fail the analysis")?;
    assert_eq!(
        error_envelope(&error).message,
        "Request data violates a persistence constraint"
    );
    assert_eq!(count(&client, "SELECT count(*) FROM match_reports")?, 0);
    Ok(())
}

/// A plain save of a report whose identity is stored returns the stored report, with its original
/// creation instant, and writes no second row.
#[tokio::test]
async fn a_plain_save_returns_the_stored_report() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "plain-save@example.test")?;
    let job = seed_job(&fixture.uow, 0x113, T0).await?;
    let stored = analysis(&fixture.uow, fixed_clock(T0))?
        .analyze_job_match(&pair(profile, job))
        .await?;
    let mut again = stored.report.clone();
    again.created_at = T0 + Duration::hours(3);
    let saved = SqliteMatchingRepository::new(fixture.uow.clone())
        .save_report(again)
        .await?;
    assert_eq!(saved, stored.report);
    let client = fixture.connection();
    assert_eq!(count(&client, "SELECT count(*) FROM match_reports")?, 1);
    Ok(())
}

#[tokio::test]
async fn a_failing_pair_in_a_batch_is_reported_and_the_batch_continues() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "batch-failure@example.test")?;
    let mut jobs = Vec::new();
    for index in 0..3_u128 {
        let created = T0 + Duration::days(i64::try_from(index)?);
        jobs.push(seed_job(&fixture.uow, 0x520 + index, created).await?);
    }
    let failing = jobs[1];
    let client = fixture.connection();
    client.execute_batch(&format!(
        "CREATE TRIGGER match_fail_job BEFORE INSERT ON match_reports \
         WHEN NEW.job_id = '{failing}' \
         BEGIN SELECT RAISE(ABORT, 'synthetic constraint'); END"
    ))?;
    let service = analysis(&fixture.uow, fixed_clock(T0))?;
    let mut succeeded = Vec::new();
    let mut failed = Vec::new();
    let mut request = json!({"profileId": profile.to_string(), "limit": 2});
    loop {
        let batch = service.analyze_all_job_matches(&request).await?;
        succeeded.extend(batch.succeeded.iter().map(|view| view.report.job_id));
        failed.extend(batch.failed);
        match batch.next_cursor {
            Some(cursor) => request["cursor"] = json!(cursor),
            None => break,
        }
    }
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].profile_id, profile.to_string());
    assert_eq!(failed[0].job_id, failing.to_string());
    assert_eq!(failed[0].error, "analysis failed");
    let expected: BTreeSet<Uuid> = jobs.iter().copied().filter(|job| *job != failing).collect();
    assert_eq!(succeeded.len(), 2);
    assert_eq!(succeeded.into_iter().collect::<BTreeSet<_>>(), expected);
    assert_eq!(
        count(
            &client,
            &format!("SELECT count(*) FROM match_reports WHERE job_id = '{failing}'"),
        )?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn jobs_created_at_one_instant_page_in_ascending_identifier_order() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "job-ties@example.test")?;
    for index in 0..5_u128 {
        seed_job(&fixture.uow, 0x530 + index, T0).await?;
    }
    let service = analysis(&fixture.uow, fixed_clock(T0))?;
    let mut seen = Vec::new();
    let mut request = json!({"profileId": profile.to_string(), "limit": 2});
    loop {
        let batch = service.analyze_all_job_matches(&request).await?;
        seen.extend(batch.succeeded.iter().map(|view| view.report.job_id));
        match batch.next_cursor {
            Some(cursor) => request["cursor"] = json!(cursor),
            None => break,
        }
    }
    let unique: BTreeSet<Uuid> = seen.iter().copied().collect();
    assert_eq!(seen.len(), 5, "every job once");
    assert_eq!(unique.len(), 5, "no duplicate across pages");
    assert!(
        seen.windows(2).all(|pair| pair[0] < pair[1]),
        "ties are ordered by identifier, ascending"
    );
    Ok(())
}

#[tokio::test]
async fn report_history_ties_page_in_descending_identifier_order() -> TestResult {
    let fixture = start();
    let first = seed_profile(&fixture.connection(), "tie-one@example.test")?;
    let second = seed_profile(&fixture.connection(), "tie-two@example.test")?;
    let job_one = seed_job(&fixture.uow, 0x540, T0).await?;
    let job_two = seed_job(&fixture.uow, 0x541, T0).await?;
    let job_three = seed_job(&fixture.uow, 0x542, T0).await?;
    let pairs = [
        (first, job_one),
        (first, job_two),
        (first, job_three),
        (second, job_one),
    ];
    for (profile, job) in pairs {
        analysis(&fixture.uow, fixed_clock(T0))?
            .analyze_job_match(&pair(profile, job))
            .await?;
    }
    let service = analysis(&fixture.uow, fixed_clock(T0))?;
    let cases = [
        (json!({"limit": 2}), 4),
        (json!({"profileId": first.to_string(), "limit": 2}), 3),
        (json!({"jobId": job_one.to_string(), "limit": 1}), 2),
        (
            json!({"profileId": first.to_string(), "jobId": job_one.to_string(), "limit": 1}),
            1,
        ),
    ];
    for (base, expected) in cases {
        let views = all_reports(&service, &base).await?;
        let ids: Vec<Uuid> = views.iter().map(|view| view.report.id).collect();
        let unique: BTreeSet<Uuid> = ids.iter().copied().collect();
        assert_eq!(ids.len(), expected, "filter {base}");
        assert_eq!(unique.len(), expected, "no duplicate across pages: {base}");
        assert!(
            ids.windows(2).all(|pair| pair[0] > pair[1]),
            "ties are ordered by identifier, descending: {base}"
        );
    }
    Ok(())
}

/// The report keeps the revision instants of the profile and the job it was computed from.
#[tokio::test]
async fn a_pair_analysis_stores_the_revisions_it_read() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "revisions@example.test")?;
    let job = seed_job(&fixture.uow, 0x560, T0).await?;
    let view = analysis(&fixture.uow, fixed_clock(T0))?
        .analyze_job_match(&pair(profile, job))
        .await?;
    assert_eq!(view.report.job_revision, T0);
    assert_eq!(view.report.profile_revision, PROFILE_AT);
    assert!(!view.stale);
    Ok(())
}

#[tokio::test]
async fn deleting_a_profile_cascades_to_its_reports() -> TestResult {
    let fixture = start();
    let profile = seed_profile(&fixture.connection(), "cascade@example.test")?;
    let job = seed_job(&fixture.uow, 0x570, T0).await?;
    analysis(&fixture.uow, fixed_clock(T0))?
        .analyze_job_match(&pair(profile, job))
        .await?;
    let client = fixture.connection();
    client.execute(
        "DELETE FROM profile_profiles WHERE id = ?1",
        [uuid_text(profile)],
    )?;
    assert_eq!(count(&client, "SELECT count(*) FROM match_reports")?, 0);
    Ok(())
}
