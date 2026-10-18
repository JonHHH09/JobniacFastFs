//! SQLite tests of the advisory review adapter.
//!
//! Submission in one transaction, reuse of the stored review and disagreement by fingerprint,
//! concurrent submissions, status changes that keep the linked issue, the insert-only tables,
//! missing entities, keyset history with ties, and the unconfigured database envelope. Each test
//! works on its own database file.

mod sqlite_match_support;

use std::collections::BTreeSet;
use std::error::Error;
use std::sync::Arc;

use jobniac_core::errors::error_envelope;
use jobniac_core::matching::{MatchReport, Outcome};
use jobniac_core::reviews::{MatchDisagreement, MatchReview};
use jobniac_infra::sqlite::jobs::codec::{instant_text, uuid_text};
use jobniac_infra::sqlite::match_reviews::SqliteMatchReviewRepository;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use serde_json::{Value, json};
use sqlite_match_support::{
    TestResult, analysis, count, fixed_clock, reviews, seed_job, seed_profile, start,
    unconfigured_uow,
};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const T0: OffsetDateTime = datetime!(2025-01-02 03:04:05 UTC);

/// The wire name of an outcome.
fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::StrongMatch => "STRONG_MATCH",
        Outcome::PartialMatch => "PARTIAL_MATCH",
        Outcome::WeakMatch => "WEAK_MATCH",
        Outcome::InsufficientEvidence => "INSUFFICIENT_EVIDENCE",
    }
}

/// A review request of `report` that scores `overall` and names `reviewer`.
fn review_request(report: &MatchReport, overall: i64, reviewer: &str) -> Value {
    json!({
        "reportId": report.id.to_string(),
        "reviewer": reviewer,
        "model": "model-x",
        "reviewVersion": "v1",
        "overallScore": overall,
        "outcome": outcome_name(report.outcome),
        "blockerMismatch": false,
        "components": [],
        "evidence": [],
        "summary": "score_adjustment",
    })
}

/// A score thirty points away from the report, so the review diverges.
fn diverging_score(report: &MatchReport) -> i64 {
    if report.overall_score >= 50 {
        report.overall_score - 30
    } else {
        report.overall_score + 30
    }
}

/// Creates a profile, a job and one report, and returns the report.
async fn seeded_report(
    uow: &SqliteUnitOfWork,
    connection: &rusqlite::Connection,
    email: &str,
    job_key: u128,
) -> Result<MatchReport, Box<dyn Error>> {
    let profile = seed_profile(connection, email)?;
    let job = seed_job(uow, job_key, T0).await?;
    let view = analysis(uow, fixed_clock(T0))?
        .analyze_job_match(&json!({"profileId": profile.to_string(), "jobId": job.to_string()}))
        .await?;
    Ok(view.report)
}

#[tokio::test]
async fn a_resubmission_returns_the_stored_review_and_disagreement() -> TestResult {
    let fixture = start();
    let report = seeded_report(
        &fixture.uow,
        &fixture.connection(),
        "review-once@example.test",
        0x401,
    )
    .await?;
    let first = reviews(&fixture.uow, fixed_clock(T0 + Duration::minutes(1)))?
        .submit_match_review(&review_request(
            &report,
            diverging_score(&report),
            "reviewer-a",
        ))
        .await?;
    let disagreement = first
        .disagreement
        .clone()
        .ok_or("a diverging review must create a disagreement")?;
    let again = reviews(&fixture.uow, fixed_clock(T0 + Duration::minutes(9)))?
        .submit_match_review(&review_request(
            &report,
            diverging_score(&report),
            "reviewer-a",
        ))
        .await?;
    assert_eq!(again.review.id, first.review.id);
    assert_eq!(again.review.created_at, first.review.created_at);
    let again_disagreement = again
        .disagreement
        .ok_or("the resubmission must return the disagreement")?;
    assert_eq!(again_disagreement.id, disagreement.id);
    let client = fixture.connection();
    assert_eq!(count(&client, "SELECT count(*) FROM match_reviews")?, 1);
    assert_eq!(
        count(&client, "SELECT count(*) FROM match_disagreements")?,
        1
    );
    let service = reviews(&fixture.uow, fixed_clock(T0))?;
    let fetched: MatchReview = service
        .get_match_review(&json!({"id": first.review.id.to_string()}))
        .await?;
    assert_eq!(fetched, first.review);
    let listed = service
        .list_match_reviews(&json!({"reportId": report.id.to_string()}))
        .await?;
    assert_eq!(listed.reviews, vec![first.review]);
    let disagreements = service
        .list_match_disagreements(&json!({"reportId": report.id.to_string()}))
        .await?;
    assert_eq!(disagreements.disagreements.len(), 1);
    Ok(())
}

#[tokio::test]
async fn an_agreeing_review_creates_no_disagreement() -> TestResult {
    let fixture = start();
    let report = seeded_report(
        &fixture.uow,
        &fixture.connection(),
        "agree@example.test",
        0x402,
    )
    .await?;
    let submission = reviews(&fixture.uow, fixed_clock(T0))?
        .submit_match_review(&review_request(&report, report.overall_score, "reviewer-b"))
        .await?;
    assert!(submission.disagreement.is_none());
    let client = fixture.connection();
    assert_eq!(
        count(&client, "SELECT count(*) FROM match_disagreements")?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn acknowledgement_and_link_keep_the_stored_identity_and_issue() -> TestResult {
    let fixture = start();
    let report = seeded_report(
        &fixture.uow,
        &fixture.connection(),
        "status@example.test",
        0x403,
    )
    .await?;
    let service = reviews(&fixture.uow, fixed_clock(T0 + Duration::minutes(5)))?;
    let submission = service
        .submit_match_review(&review_request(
            &report,
            diverging_score(&report),
            "reviewer-c",
        ))
        .await?;
    let review_id = submission.review.id;
    let disagreement: MatchDisagreement = submission
        .disagreement
        .ok_or("a diverging review must create a disagreement")?;
    let acknowledged = service
        .acknowledge_match_disagreement(&json!({"disagreementId": disagreement.id.to_string()}))
        .await?;
    assert_eq!(acknowledged.status.as_str(), "ACKNOWLEDGED");
    assert_eq!(acknowledged.linear_issue_id, None);
    let linked = service
        .link_match_disagreement(&json!({
            "disagreementId": disagreement.id.to_string(),
            "linearIssueId": "  LIN-42 ",
        }))
        .await?;
    assert_eq!(linked.status.as_str(), "LINKED");
    assert_eq!(linked.linear_issue_id.as_deref(), Some("LIN-42"));
    let kept = service
        .acknowledge_match_disagreement(&json!({"disagreementId": disagreement.id.to_string()}))
        .await?;
    assert_eq!(kept.status.as_str(), "ACKNOWLEDGED");
    assert_eq!(kept.linear_issue_id.as_deref(), Some("LIN-42"));
    assert_eq!(kept.id, disagreement.id);
    assert_eq!(kept.created_at, disagreement.created_at);
    // A resubmission after the link returns the stored review and disagreement, with the status and
    // issue they have now, under the first submission's identifiers.
    let resubmitted = service
        .submit_match_review(&review_request(
            &report,
            diverging_score(&report),
            "reviewer-c",
        ))
        .await?;
    assert_eq!(resubmitted.review.id, review_id);
    let stored = resubmitted
        .disagreement
        .ok_or("the resubmission must return the disagreement")?;
    assert_eq!(stored.id, disagreement.id);
    assert_eq!(stored.review_id, review_id);
    // The status is the one the last change set (the acknowledgement above), not a reset to OPEN.
    assert_eq!(stored.status.as_str(), "ACKNOWLEDGED");
    assert_eq!(stored.linear_issue_id.as_deref(), Some("LIN-42"));
    let missing = Uuid::from_u128(0x9abc);
    let error = service
        .acknowledge_match_disagreement(&json!({"disagreementId": missing.to_string()}))
        .await
        .err()
        .ok_or("an unknown disagreement must fail")?;
    assert_eq!(
        error_envelope(&error).message,
        format!("match disagreement not found: {missing}")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_submissions_reuse_one_review_and_disagreement() -> TestResult {
    let fixture = start();
    let report = seeded_report(
        &fixture.uow,
        &fixture.connection(),
        "concurrent-review@example.test",
        0x404,
    )
    .await?;
    let service = Arc::new(reviews(&fixture.uow, fixed_clock(T0))?);
    let request = review_request(&report, diverging_score(&report), "reviewer-d");
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let service = Arc::clone(&service);
        let request = request.clone();
        tasks.push(tokio::spawn(async move {
            service.submit_match_review(&request).await
        }));
    }
    let mut review_ids = BTreeSet::new();
    let mut disagreement_ids = BTreeSet::new();
    for task in tasks {
        let submission = task.await??;
        review_ids.insert(submission.review.id);
        if let Some(disagreement) = submission.disagreement {
            disagreement_ids.insert(disagreement.id);
        }
    }
    assert_eq!(review_ids.len(), 1);
    assert_eq!(disagreement_ids.len(), 1);
    let client = fixture.connection();
    assert_eq!(count(&client, "SELECT count(*) FROM match_reviews")?, 1);
    assert_eq!(
        count(&client, "SELECT count(*) FROM match_disagreements")?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_report_aborts_without_any_write() -> TestResult {
    let fixture = start();
    let missing = Uuid::from_u128(0x5555);
    let error = reviews(&fixture.uow, fixed_clock(T0))?
        .submit_match_review(&json!({
            "reportId": missing.to_string(),
            "reviewer": "reviewer-e",
            "model": "model-x",
            "reviewVersion": "v1",
            "overallScore": 10,
            "outcome": "WEAK_MATCH",
            "blockerMismatch": false,
            "components": [],
            "evidence": [],
            "summary": "review_consistent",
        }))
        .await
        .err()
        .ok_or("an unknown report must fail")?;
    assert_eq!(
        error_envelope(&error).message,
        format!("match report not found: {missing}")
    );
    let client = fixture.connection();
    assert_eq!(count(&client, "SELECT count(*) FROM match_reviews")?, 0);
    Ok(())
}

#[tokio::test]
async fn stored_reviews_cannot_be_updated() -> TestResult {
    let fixture = start();
    let report = seeded_report(
        &fixture.uow,
        &fixture.connection(),
        "review-immutable@example.test",
        0x406,
    )
    .await?;
    reviews(&fixture.uow, fixed_clock(T0))?
        .submit_match_review(&review_request(&report, report.overall_score, "reviewer-f"))
        .await?;
    let client = fixture.connection();
    let error = client
        .execute("UPDATE match_reviews SET summary = 'review_consistent'", [])
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
async fn review_history_keeps_ties_and_pages_by_report() -> TestResult {
    let fixture = start();
    let report = seeded_report(
        &fixture.uow,
        &fixture.connection(),
        "history-review@example.test",
        0x407,
    )
    .await?;
    let service = reviews(&fixture.uow, fixed_clock(T0))?;
    for reviewer in ["reviewer-g", "reviewer-h", "reviewer-i"] {
        service
            .submit_match_review(&review_request(&report, report.overall_score, reviewer))
            .await?;
    }
    for base in [
        json!({"reportId": report.id.to_string(), "limit": 2}),
        json!({"limit": 1}),
    ] {
        let mut seen = Vec::new();
        let mut request = base.clone();
        loop {
            let page = service.list_match_reviews(&request).await?;
            seen.extend(page.reviews.iter().map(|review| review.id));
            match page.next_cursor {
                Some(cursor) => request["cursor"] = json!(cursor),
                None => break,
            }
        }
        assert_eq!(seen.len(), 3, "filter {base}");
        let mut sorted = seen.clone();
        sorted.sort_by(|left, right| right.cmp(left));
        assert_eq!(seen, sorted, "ties are ordered by identifier, descending");
    }
    Ok(())
}

#[tokio::test]
async fn an_unconfigured_database_reports_not_configured() -> TestResult {
    let service = jobniac_core::reviews::MatchReviewService::new(
        SqliteMatchReviewRepository::new(unconfigured_uow()),
        sqlite_match_support::codec()?,
        fixed_clock(T0),
    );
    let error = service
        .submit_match_review(&json!({
            "reportId": Uuid::from_u128(3).to_string(),
            "overallScore": 10,
            "blockerMismatch": false,
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
async fn a_failing_disagreement_write_rolls_back_the_review() -> TestResult {
    let fixture = start();
    let report = seeded_report(
        &fixture.uow,
        &fixture.connection(),
        "atomic-review@example.test",
        0x408,
    )
    .await?;
    let client = fixture.connection();
    client.execute_batch(
        "CREATE TRIGGER reject_disagreement BEFORE INSERT ON match_disagreements \
         BEGIN SELECT RAISE(ABORT, 'synthetic constraint'); END",
    )?;
    let error = reviews(&fixture.uow, fixed_clock(T0))?
        .submit_match_review(&review_request(
            &report,
            diverging_score(&report),
            "reviewer-j",
        ))
        .await
        .err()
        .ok_or("a rejected disagreement must fail the submission")?;
    assert_eq!(
        error_envelope(&error).message,
        "Request data violates a persistence constraint"
    );
    assert_eq!(count(&client, "SELECT count(*) FROM match_reviews")?, 0);
    assert_eq!(
        count(&client, "SELECT count(*) FROM match_disagreements")?,
        0
    );
    Ok(())
}

/// Stored rows that break the persisted-record rules are never returned, not even in part: the single
/// read and the listing report "Local database is unavailable". The rows are inserted directly. The
/// immutability trigger refuses updates only, so inserts are allowed.
#[tokio::test]
async fn invalid_stored_reviews_fail_closed() -> TestResult {
    let fixture = start();
    let report = seeded_report(
        &fixture.uow,
        &fixture.connection(),
        "fail-closed-review@example.test",
        0x409,
    )
    .await?;
    let service = reviews(&fixture.uow, fixed_clock(T0))?;
    service
        .submit_match_review(&review_request(
            &report,
            diverging_score(&report),
            "reviewer-k",
        ))
        .await?;
    let client = fixture.connection();
    let invalid_evidence = [
        json!([{"component": "TECHNICAL", "status": "MATCH", "sourceType": "notes",
                "fact": "reach reviewer@example.test", "outcomeChangingDefect": false}]),
        json!([{"component": "TECHNICAL", "status": "MATCH", "sourceType": "notes",
                "fact": "plain text", "outcomeChangingDefect": false, "rawText": "private note"}]),
    ];
    for (index, evidence) in invalid_evidence.iter().enumerate() {
        let offset = u128::try_from(index)?;
        let id = Uuid::from_u128(0x9100 + offset);
        let fingerprint = Uuid::from_u128(0x9200 + offset);
        client.execute(
            "INSERT INTO match_reviews \
             (id, fingerprint, report_id, reviewer, model, review_version, overall_score, outcome, \
              blocker_mismatch, components, evidence, summary, created_at) \
             VALUES (?1, ?2, ?3, 'reviewer-x', 'model-x', 'v1', 10, 'WEAK_MATCH', 0, \
                     '[]', ?4, 'review_consistent', ?5)",
            rusqlite::params![
                uuid_text(id),
                uuid_text(fingerprint),
                uuid_text(report.id),
                evidence.to_string(),
                instant_text(T0)
            ],
        )?;
        let error = service
            .get_match_review(&json!({"id": id.to_string()}))
            .await
            .err()
            .ok_or("an invalid stored review must not be returned")?;
        assert_eq!(
            error_envelope(&error).message,
            "Local database is unavailable"
        );
    }
    let error = service
        .list_match_reviews(&json!({"reportId": report.id.to_string()}))
        .await
        .err()
        .ok_or("a listing with an invalid row must fail as a whole")?;
    assert_eq!(
        error_envelope(&error).message,
        "Local database is unavailable"
    );
    Ok(())
}

#[tokio::test]
async fn invalid_stored_disagreements_fail_closed() -> TestResult {
    let fixture = start();
    let report = seeded_report(
        &fixture.uow,
        &fixture.connection(),
        "fail-closed-disagreement@example.test",
        0x40a,
    )
    .await?;
    let service = reviews(&fixture.uow, fixed_clock(T0))?;
    service
        .submit_match_review(&review_request(
            &report,
            diverging_score(&report),
            "reviewer-l",
        ))
        .await?;
    // An agreeing review has no disagreement, so the invalid row can take its review identifier.
    let agreeing = service
        .submit_match_review(&review_request(&report, report.overall_score, "reviewer-m"))
        .await?;
    assert!(agreeing.disagreement.is_none());
    let client = fixture.connection();
    client.execute(
        "INSERT INTO match_disagreements \
         (id, fingerprint, report_id, review_id, policy_version, reasons, evidence_defect_codes, \
          status, linear_issue_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, 'divergence-v1', '[\"BOGUS\"]', '[]', \
                 'OPEN', NULL, ?5, ?5)",
        rusqlite::params![
            uuid_text(Uuid::from_u128(0x9300)),
            uuid_text(Uuid::from_u128(0x9301)),
            uuid_text(report.id),
            uuid_text(agreeing.review.id),
            instant_text(T0)
        ],
    )?;
    let error = service
        .list_match_disagreements(&json!({"reportId": report.id.to_string()}))
        .await
        .err()
        .ok_or("a listing with an invalid disagreement must fail as a whole")?;
    assert_eq!(
        error_envelope(&error).message,
        "Local database is unavailable"
    );
    Ok(())
}
