//! PostgreSQL tests of the matching adapter.
//!
//! Reuse and immutability of reports, staleness after a profile or job revision change, missing
//! entities, concurrent single pairs, batch and history paging, and the unconfigured and unreachable
//! database envelopes. Server-backed tests skip without `JOBNIAC_TEST_PG_PORT`.

mod match_support;

use std::collections::BTreeSet;
use std::error::Error;
use std::sync::Arc;

use jobniac_core::errors::error_envelope;
use jobniac_core::matching::{MatchView, MatchingRepository};
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::matching::{MATCHING_APPLICATION_NAME, PgMatchingRepository};
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use match_support::{
    TestResult, analysis, closed_port_environment, count, fixed_clock, profile_support, profiles,
    seed_job, seed_profile, seed_profile_with, start,
};
use serde_json::{Value, json};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};
use tokio_postgres::Client;
use uuid::Uuid;

const T0: OffsetDateTime = datetime!(2025-01-02 03:04:05 UTC);

/// The number of attempts a trigger has seen. A sequence keeps its count when the attempt rolls back.
const ATTEMPTS_SQL: &str = "SELECT last_value FROM public.match_attempts";

/// Makes the first `failures` writes to `match.reports` fail with SQLSTATE 40001, counting attempts
/// with a sequence. A `deferred` failure is raised at `COMMIT` (a deferred constraint trigger), an
/// immediate one at the insert.
async fn fail_report_attempts(
    client: &Client,
    failures: u32,
    deferred: bool,
) -> Result<(), Box<dyn Error>> {
    client
        .batch_execute("CREATE SEQUENCE public.match_attempts")
        .await?;
    client
        .batch_execute(&format!(
            "CREATE FUNCTION public.match_fail_attempt() RETURNS trigger LANGUAGE plpgsql AS $fn$ \
             BEGIN \
               IF nextval('public.match_attempts') <= {failures} THEN \
                 RAISE EXCEPTION 'synthetic serialization conflict' \
                   USING ERRCODE = 'serialization_failure'; \
               END IF; \
               RETURN NEW; \
             END $fn$"
        ))
        .await?;
    let trigger = if deferred {
        "CREATE CONSTRAINT TRIGGER match_fail_attempt AFTER INSERT ON match.reports \
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.match_fail_attempt()"
    } else {
        "CREATE TRIGGER match_fail_attempt BEFORE INSERT ON match.reports FOR EACH ROW \
         EXECUTE FUNCTION public.match_fail_attempt()"
    };
    client.batch_execute(trigger).await?;
    Ok(())
}

fn pair(profile: Uuid, job: Uuid) -> Value {
    json!({"profileId": profile.to_string(), "jobId": job.to_string()})
}

/// Walks every page of the report history for `base` (the filters and the limit), in order.
async fn all_reports(
    service: &jobniac_core::matching::MatchAnalysisService<
        jobniac_infra::postgres::matching::PgMatchingRepository,
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
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "reuse@example.test").await?;
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
    let client = fixture.client().await;
    assert_eq!(
        count(&client, "SELECT count(*) FROM match.reports").await?,
        1
    );
    Ok(())
}

/// The immutability trigger refuses updates only. Deletes are not refused, because profile and job
/// deletion cascade to their reports (see `deleting_a_profile_cascades_to_its_reports`).
#[tokio::test]
async fn stored_reports_cannot_be_updated() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "immutable@example.test").await?;
    let job = seed_job(&fixture.uow, 0x102, T0).await?;
    analysis(&fixture.uow, fixed_clock(T0))?
        .analyze_job_match(&pair(profile, job))
        .await?;
    let client = fixture.client().await;
    let error = client
        .execute("UPDATE match.reports SET overall_score = 0", &[])
        .await
        .err()
        .ok_or("the update must be refused")?;
    let message = error
        .as_db_error()
        .map_or_else(String::new, |db| db.message().to_owned());
    assert_eq!(message, "match history is immutable");
    Ok(())
}

#[tokio::test]
async fn a_profile_revision_change_marks_the_report_stale() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let email = "stale-profile@example.test";
    let project = Uuid::new_v4().to_string();
    let technology = Uuid::new_v4().to_string();
    let profile = seed_profile_with(&fixture.uow, email, &project, &technology).await?;
    let job = seed_job(&fixture.uow, 0x103, T0).await?;
    let service = analysis(&fixture.uow, fixed_clock(T0))?;
    let report = service.analyze_job_match(&pair(profile, job)).await?;
    profiles(&fixture.uow)
        .update_profile(
            &profile.to_string(),
            &json!(0),
            &profile_support::request(email, None, &project, &technology),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
    let current = service
        .get_match_report(&json!({"id": report.report.id.to_string()}))
        .await?;
    assert!(current.stale);
    assert_eq!(current.report, report.report);
    Ok(())
}

#[tokio::test]
async fn a_job_revision_change_marks_the_report_stale() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "stale-job@example.test").await?;
    let job = seed_job(&fixture.uow, 0x104, T0).await?;
    let service = analysis(&fixture.uow, fixed_clock(T0))?;
    let report = service.analyze_job_match(&pair(profile, job)).await?;
    let client = fixture.client().await;
    client
        .execute(
            "UPDATE job_schema.jobs SET updated_at = updated_at + INTERVAL '1 day' WHERE id = $1",
            &[&job],
        )
        .await?;
    let current = service
        .get_match_report(&json!({"id": report.report.id.to_string()}))
        .await?;
    assert!(current.stale);
    Ok(())
}

#[tokio::test]
async fn missing_entities_abort_before_any_report_is_written() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "missing@example.test").await?;
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
    let client = fixture.client().await;
    assert_eq!(
        count(&client, "SELECT count(*) FROM match.reports").await?,
        0
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_single_pairs_reuse_one_original_report() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "concurrent@example.test").await?;
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
    let client = fixture.client().await;
    assert_eq!(
        count(&client, "SELECT count(*) FROM match.reports").await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn batch_pages_visit_every_candidate_job_once() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "batch@example.test").await?;
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
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let first = seed_profile(&fixture.uow, "history-one@example.test").await?;
    let second = seed_profile(&fixture.uow, "history-two@example.test").await?;
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
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "fail-closed@example.test").await?;
    let job = seed_job(&fixture.uow, 0x107, T0).await?;
    let client = fixture.client().await;
    // A technical component that earns more than its 40 available points is not a valid record.
    let stored: Uuid = client
        .query_one(
            "INSERT INTO match.reports \
             (id, profile_id, job_id, profile_revision, job_revision, algorithm_version, \
              overall_score, confidence, outcome, blocker_mismatch, components, evidence, created_at) \
             SELECT gen_random_uuid(), profile.id, job.id, profile.updated_at, job.updated_at, \
                    'deterministic-v1', 0, 0, 'INSUFFICIENT_EVIDENCE', false, \
                    '[{\"component\":\"TECHNICAL\",\"earnedPoints\":99,\"availablePoints\":40,\
                      \"status\":\"MATCH\"}]'::jsonb, '[]'::jsonb, now() \
             FROM profile.profiles AS profile, job_schema.jobs AS job \
             WHERE profile.id = $1 AND job.id = $2 \
             RETURNING id",
            &[&profile, &job],
        )
        .await?
        .get(0);
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
    let Some(fixture) = start().await? else {
        return Ok(());
    };
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
    let uow = PgUnitOfWork::new(PgPool::new(None));
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
async fn an_unreachable_database_is_reported_as_unavailable() -> TestResult {
    let config = load_database_config(&closed_port_environment())
        .ok_or("the closed-port configuration is invalid")?;
    let uow = PgUnitOfWork::new(PgPool::with_application_name(
        Some(config),
        MATCHING_APPLICATION_NAME,
    ));
    let error = analysis(&uow, fixed_clock(T0))?
        .analyze_job_match(&json!({
            "profileId": Uuid::from_u128(1).to_string(),
            "jobId": Uuid::from_u128(2).to_string(),
        }))
        .await
        .err()
        .ok_or("the unreachable database must fail")?;
    let message = error_envelope(&error).message;
    assert_eq!(message, "Local database is unavailable");
    assert!(!message.contains("connect"));
    Ok(())
}

#[tokio::test]
async fn a_serialization_failure_is_retried_until_the_snapshot_commits() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "retry@example.test").await?;
    let job = seed_job(&fixture.uow, 0x110, T0).await?;
    let client = fixture.client().await;
    fail_report_attempts(&client, 2, false).await?;
    let view = analysis(&fixture.uow, fixed_clock(T0))?
        .analyze_job_match(&pair(profile, job))
        .await?;
    assert_eq!(view.report.job_id, job);
    assert_eq!(count(&client, ATTEMPTS_SQL).await?, 3);
    assert_eq!(
        count(&client, "SELECT count(*) FROM match.reports").await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn a_persistent_serialization_failure_is_unavailable_after_three_attempts() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "retry-out@example.test").await?;
    let job = seed_job(&fixture.uow, 0x111, T0).await?;
    let client = fixture.client().await;
    fail_report_attempts(&client, 3, false).await?;
    let error = analysis(&fixture.uow, fixed_clock(T0))?
        .analyze_job_match(&pair(profile, job))
        .await
        .err()
        .ok_or("the third serialization failure must be final")?;
    assert_eq!(
        error_envelope(&error).message,
        "Local database is unavailable"
    );
    assert_eq!(count(&client, ATTEMPTS_SQL).await?, 3);
    assert_eq!(
        count(&client, "SELECT count(*) FROM match.reports").await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn a_commit_time_serialization_failure_is_retried() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "commit-retry@example.test").await?;
    let job = seed_job(&fixture.uow, 0x112, T0).await?;
    let client = fixture.client().await;
    fail_report_attempts(&client, 1, true).await?;
    analysis(&fixture.uow, fixed_clock(T0))?
        .analyze_job_match(&pair(profile, job))
        .await?;
    assert_eq!(count(&client, ATTEMPTS_SQL).await?, 2);
    assert_eq!(
        count(&client, "SELECT count(*) FROM match.reports").await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn a_plain_save_is_not_retried() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "plain-save@example.test").await?;
    let job = seed_job(&fixture.uow, 0x113, T0).await?;
    let stored = analysis(&fixture.uow, fixed_clock(T0))?
        .analyze_job_match(&pair(profile, job))
        .await?;
    let client = fixture.client().await;
    fail_report_attempts(&client, 5, false).await?;
    let error = PgMatchingRepository::new(fixture.uow.clone())
        .save_report(stored.report)
        .await
        .err()
        .ok_or("a failing plain save must fail")?;
    assert_eq!(
        error_envelope(&error).message,
        "Local database is unavailable"
    );
    assert_eq!(count(&client, ATTEMPTS_SQL).await?, 1);
    Ok(())
}

#[tokio::test]
async fn a_failing_pair_in_a_batch_is_reported_and_the_batch_continues() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "batch-failure@example.test").await?;
    let mut jobs = Vec::new();
    for index in 0..3_u128 {
        let created = T0 + Duration::days(i64::try_from(index)?);
        jobs.push(seed_job(&fixture.uow, 0x520 + index, created).await?);
    }
    let failing = jobs[1];
    let client = fixture.client().await;
    client
        .batch_execute(&format!(
            "CREATE FUNCTION public.match_fail_job() RETURNS trigger LANGUAGE plpgsql AS $fn$ \
             BEGIN \
               IF NEW.job_id = '{failing}'::uuid THEN \
                 RAISE EXCEPTION 'synthetic constraint' USING ERRCODE = 'check_violation'; \
               END IF; \
               RETURN NEW; \
             END $fn$; \
             CREATE TRIGGER match_fail_job BEFORE INSERT ON match.reports \
               FOR EACH ROW EXECUTE FUNCTION public.match_fail_job()"
        ))
        .await?;
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
            &format!("SELECT count(*) FROM match.reports WHERE job_id = '{failing}'"),
        )
        .await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn jobs_created_at_one_instant_page_in_ascending_identifier_order() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "job-ties@example.test").await?;
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
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let first = seed_profile(&fixture.uow, "tie-one@example.test").await?;
    let second = seed_profile(&fixture.uow, "tie-two@example.test").await?;
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

/// A job revision change committed while the analysis waits on the skill table is not part of its
/// snapshot: the root was read before the commit, and the report keeps that revision.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pair_analysis_reads_one_snapshot_across_a_concurrent_skill_update() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "snapshot@example.test").await?;
    let job = seed_job(&fixture.uow, 0x560, T0).await?;
    let client = fixture.client().await;
    let before: OffsetDateTime = client
        .query_one(
            "SELECT updated_at FROM job_schema.jobs WHERE id = $1",
            &[&job],
        )
        .await?
        .get(0);
    client.batch_execute("BEGIN").await?;
    client
        .batch_execute("LOCK TABLE job_schema.job_skills IN ACCESS EXCLUSIVE MODE")
        .await?;
    client
        .execute(
            "UPDATE job_schema.jobs SET updated_at = updated_at + INTERVAL '1 day' WHERE id = $1",
            &[&job],
        )
        .await?;
    let service = Arc::new(analysis(&fixture.uow, fixed_clock(T0))?);
    let request = pair(profile, job);
    let task = {
        let service = Arc::clone(&service);
        tokio::spawn(async move { service.analyze_job_match(&request).await })
    };
    let mut waiting = 0_i64;
    for _ in 0..400 {
        waiting = count(
            &client,
            "SELECT count(*) FROM pg_stat_activity \
             WHERE application_name = 'jobniac-matching' AND wait_event_type = 'Lock'",
        )
        .await?;
        if waiting > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    client.batch_execute("COMMIT").await?;
    let view = task.await??;
    assert!(waiting > 0, "the analysis must reach the locked skill read");
    assert_eq!(view.report.job_revision, before);
    Ok(())
}

#[tokio::test]
async fn deleting_a_profile_cascades_to_its_reports() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile = seed_profile(&fixture.uow, "cascade@example.test").await?;
    let job = seed_job(&fixture.uow, 0x570, T0).await?;
    analysis(&fixture.uow, fixed_clock(T0))?
        .analyze_job_match(&pair(profile, job))
        .await?;
    let client = fixture.client().await;
    client
        .execute("DELETE FROM profile.profiles WHERE id = $1", &[&profile])
        .await?;
    assert_eq!(
        count(&client, "SELECT count(*) FROM match.reports").await?,
        0
    );
    Ok(())
}
