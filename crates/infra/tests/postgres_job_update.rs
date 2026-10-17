//! Revision-checked job edits, compare-and-set writes, provenance and search postings against a
//! disposable PostgreSQL 18 server.
//!
//! The server-backed tests skip with a notice when
//! `JOBNIAC_TEST_PG_PORT` is unset; the unreachable case needs no server. All data is synthetic.

mod job_support;

use std::error::Error;
use std::time::{Duration, Instant};

use job_support::{
    TestResult, closed_port_uow, fixed_service, has_posting, postings, search, start, text_graph,
};
use jobniac_core::errors::{AppError, ErrorCode, PERSISTENCE_CONSTRAINT_MESSAGE, error_envelope};
use jobniac_core::jobs::{
    CaptureStatus, JobAggregate, JobRepository, JobSearchResponse, JobSkillRecord,
};
use jobniac_core::profiles::repository::READ_WRITE;
use jobniac_infra::postgres::jobs::PgJobRepository;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

const CREATED_AT: OffsetDateTime = datetime!(2026-10-01 10:00 UTC);
const EDITED_AT: OffsetDateTime = datetime!(2026-10-02 09:00 UTC);
const CONFLICT: &str = "Job revision conflict";
const UNAVAILABLE: &str = "Local database is unavailable";

/// Merges the keys of `extra` into the object `base`.
fn merge(mut base: Value, extra: Value) -> Value {
    if let (Value::Object(target), Value::Object(source)) = (&mut base, extra) {
        target.extend(source);
    }
    base
}

/// Creates a text job at `CREATED_AT` from `text`, with the test defaults (source label
/// `original paste` and the explicit skill `Java`) and the overrides in `extra`.
async fn create_job(
    uow: &PgUnitOfWork,
    text: &str,
    extra: Value,
) -> Result<JobAggregate, Box<dyn Error>> {
    let request = merge(
        json!({"text": text, "sourceLabel": "original paste", "skills": ["Java"]}),
        extra,
    );
    Ok(fixed_service(uow, CREATED_AT)?
        .add_job_from_text(&request)
        .await?
        .job)
}

/// A replacement aggregate of `original`: a new title and fingerprint, fresh content, revision 1, the
/// edit instant, and one new skill.
fn replacement(original: &JobAggregate, title: &str, skill: &str) -> JobAggregate {
    let mut graph = original.clone();
    graph.job.title = title.to_owned();
    graph.job.description = "Fresh replacement content".to_owned();
    graph.job.revision = 1;
    graph.job.updated_at = EDITED_AT;
    graph.job.canonical_fingerprint = hex::encode(Sha256::digest(title.as_bytes()));
    graph.skills = vec![JobSkillRecord {
        id: Uuid::new_v4(),
        job_id: original.job.id,
        skill: skill.to_owned(),
        normalized_skill: skill.to_lowercase(),
        required: true,
        display_order: 0,
        created_at: EDITED_AT,
    }];
    graph
}

/// The stored aggregate of `job_id`, read through the repository in a read-only scope.
async fn stored(uow: &PgUnitOfWork, job_id: Uuid) -> Result<Option<JobAggregate>, AppError> {
    uow.transaction(jobniac_core::profiles::repository::READ_ONLY, async |tx| {
        PgJobRepository.get_job(tx, job_id).await
    })
    .await
}

/// One repository update in its own read-write scope, as the service runs it.
async fn update(
    uow: &PgUnitOfWork,
    graph: &JobAggregate,
    expected_revision: i64,
) -> Result<Option<JobAggregate>, AppError> {
    uow.transaction(READ_WRITE, async |tx| {
        PgJobRepository
            .update_job_aggregate(tx, graph, expected_revision)
            .await
    })
    .await
}

#[tokio::test]
async fn an_edit_replaces_fields_skills_and_search_without_rewriting_the_text_identity()
-> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = &fixture.uow;
    let original = create_job(
        uow,
        "Java backend",
        json!({"company": "Example Corp", "location": "Remote"}),
    )
    .await?;
    let original_skill_ids: Vec<Uuid> = original.skills.iter().map(|skill| skill.id).collect();
    let service = fixed_service(uow, EDITED_AT)?;
    let job_id = original.job.id.to_string();
    let updated = service
        .update_job(&json!({
            "jobId": job_id,
            "expectedRevision": original.job.revision,
            "sourceLabel": "Edited display",
            "title": "Kubernetes engineer",
            "company": " ",
            "location": "Toronto",
            "description": "Build cloud platforms",
            "skills": ["Kubernetes; Docker", "kubernetes"],
        }))
        .await?;

    assert_eq!(service.get_job(&job_id).await?, updated);
    assert_eq!(updated.job.revision, 1);
    assert_eq!(updated.job.source_method, original.job.source_method);
    assert_eq!(updated.job.created_at, original.job.created_at);
    assert_eq!(updated.job.updated_at, EDITED_AT);
    assert_eq!(updated.job.company, None);
    assert_eq!(updated.job.source_label.as_deref(), Some("Edited display"));
    assert_eq!(updated.text_ingestion, original.text_ingestion);
    assert!(updated.link_ingestion.is_none());
    let normalized: Vec<&str> = updated
        .skills
        .iter()
        .map(|skill| skill.normalized_skill.as_str())
        .collect();
    assert_eq!(normalized, ["kubernetes", "docker"]);
    assert!(
        updated
            .skills
            .iter()
            .all(|skill| !original_skill_ids.contains(&skill.id))
    );

    assert!(
        search(uow)
            .search_jobs(&json!({"query": "java"}))
            .await?
            .jobs
            .is_empty()
    );
    let result = search(uow)
        .search_jobs(&json!({"query": "kubernetes"}))
        .await?;
    let top = result
        .jobs
        .first()
        .ok_or("kubernetes must match the edited job")?;
    assert_eq!(top.job.id, original.job.id);
    assert_eq!(top.score, 15);

    let client = fixture.client().await;
    let terms = postings(&client, original.job.id).await?;
    assert!(has_posting(&terms, "job.title", "kubernetes", 8));
    assert!(!terms.iter().any(|(_, term, _)| term == "java"));

    let reused = service
        .add_job_from_text(&json!({"text": "Java backend"}))
        .await?;
    assert_eq!(reused.status, CaptureStatus::ReusedExistingJob);
    assert_eq!(reused.job, updated);
    Ok(())
}

#[tokio::test]
async fn null_skills_keep_their_identifiers_and_empty_skills_clear_the_index() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = &fixture.uow;
    let original = create_job(
        uow,
        "Backend engineer",
        json!({"description": "Build systems"}),
    )
    .await?;
    let service = fixed_service(uow, EDITED_AT)?;
    let job_id = original.job.id.to_string();
    let preserved = service
        .update_job(&json!({
            "jobId": job_id,
            "expectedRevision": original.job.revision,
            "skills": null,
        }))
        .await?;
    assert_eq!(preserved.skills, original.skills);
    assert_eq!(
        preserved.job.canonical_fingerprint,
        original.job.canonical_fingerprint
    );

    let cleared = service
        .update_job(&json!({
            "jobId": job_id,
            "expectedRevision": preserved.job.revision,
            "skills": [],
        }))
        .await?;
    assert!(cleared.skills.is_empty());
    assert_eq!(cleared.job.revision, 2);
    assert_eq!(cleared.text_ingestion, original.text_ingestion);
    assert!(
        search(uow)
            .search_jobs(&json!({"query": "java"}))
            .await?
            .jobs
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn a_link_edit_keeps_the_original_ingestion_and_the_link_prefix_in_the_fingerprint()
-> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = &fixture.uow;
    let original = create_job(
        uow,
        "Backend engineer",
        json!({"description": "Build systems"}),
    )
    .await?;
    let job_id = original.job.id;
    let url = "https://example.invalid/jobs/25";
    let normalized_url = format!("{url}?jk=25");
    // Synthetic fixture: the provenance changes only inside one owned transaction, which the
    // deferred provenance triggers check at commit.
    let mut client = fixture.client().await;
    let transaction = client.transaction().await?;
    transaction
        .execute(
            "DELETE FROM job_schema.job_text_ingestions WHERE job_id = $1",
            &[&job_id],
        )
        .await?;
    transaction
        .execute(
            "UPDATE job_schema.jobs SET source_method = 'link' WHERE id = $1",
            &[&job_id],
        )
        .await?;
    let link_id = Uuid::new_v4();
    let status: i32 = 200;
    transaction
        .execute(
            "INSERT INTO job_schema.job_link_ingestions \
             (id, job_id, url, normalized_url, fetched_at, http_status, source_title, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            &[
                &link_id,
                &job_id,
                &url,
                &normalized_url,
                &CREATED_AT,
                &status,
                &"Original title",
                &CREATED_AT,
            ],
        )
        .await?;
    transaction.commit().await?;

    let service = fixed_service(uow, EDITED_AT)?;
    let before = service.get_job(&job_id.to_string()).await?;
    let updated = service
        .update_job(&json!({
            "jobId": job_id.to_string(),
            "expectedRevision": before.job.revision,
            "sourceLabel": "Edited",
            "title": "Platform engineer",
        }))
        .await?;
    assert_eq!(updated.link_ingestion, before.link_ingestion);
    assert!(updated.text_ingestion.is_none());
    assert_eq!(updated.job.source_method, "link");
    let expected = format!("{normalized_url}\nplatform engineer\n\n\nbuild systems");
    assert_eq!(
        updated.job.canonical_fingerprint,
        hex::encode(Sha256::digest(expected.as_bytes()))
    );
    assert_eq!(updated.job.revision, 1);
    Ok(())
}

#[tokio::test]
async fn a_duplicate_fingerprint_on_update_leaves_every_row_and_posting_unchanged() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = &fixture.uow;
    let first = create_job(
        uow,
        "First",
        json!({"company": "Example", "description": "First content"}),
    )
    .await?;
    let second = create_job(
        uow,
        "Second",
        json!({"company": "Other", "description": "Second content"}),
    )
    .await?;
    let client = fixture.client().await;
    let before = postings(&client, second.job.id).await?;
    let service = fixed_service(uow, EDITED_AT)?;
    let error = service
        .update_job(&json!({
            "jobId": second.job.id.to_string(),
            "expectedRevision": second.job.revision,
            "title": first.job.title,
            "company": first.job.company,
            "description": first.job.description,
            "skills": ["Kubernetes"],
        }))
        .await
        .err()
        .ok_or("a duplicate fingerprint must be refused")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::ValidationError);
    assert_eq!(
        envelope.details.get("field").map(String::as_str),
        Some("canonicalFingerprint")
    );
    assert_eq!(
        envelope.details.get("reason"),
        Some(&format!("duplicates existing job {}", first.job.id))
    );
    assert_eq!(service.get_job(&second.job.id.to_string()).await?, second);
    assert_eq!(postings(&client, second.job.id).await?, before);
    Ok(())
}

#[tokio::test]
async fn a_repository_unique_race_is_sanitized_and_rolls_back() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = &fixture.uow;
    let first = create_job(uow, "First", json!({})).await?;
    let second = create_job(uow, "Second", json!({})).await?;
    let mut altered = replacement(&second, "New title", "Docker");
    altered.job.canonical_fingerprint = first.job.canonical_fingerprint.clone();
    let client = fixture.client().await;
    let before = postings(&client, second.job.id).await?;
    let error = update(uow, &altered, 0)
        .await
        .err()
        .ok_or("the unique fingerprint must refuse the update")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::ValidationError);
    assert_eq!(envelope.message, PERSISTENCE_CONSTRAINT_MESSAGE);
    assert!(envelope.details.is_empty());
    assert_eq!(stored(uow, second.job.id).await?, Some(second.clone()));
    assert_eq!(postings(&client, second.job.id).await?, before);
    Ok(())
}

#[tokio::test]
async fn a_child_constraint_failure_rolls_back_the_root_the_skills_and_the_postings() -> TestResult
{
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = &fixture.uow;
    let original = create_job(uow, "Java backend", json!({})).await?;
    let mut altered = replacement(&original, "New title", "Docker");
    altered.skills[0].display_order = -1;
    let client = fixture.client().await;
    let before = postings(&client, original.job.id).await?;
    let error = update(uow, &altered, 0)
        .await
        .err()
        .ok_or("a negative display order must be refused")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.message, PERSISTENCE_CONSTRAINT_MESSAGE);
    assert!(envelope.details.is_empty());
    assert_eq!(stored(uow, original.job.id).await?, Some(original.clone()));
    assert_eq!(postings(&client, original.job.id).await?, before);
    Ok(())
}

#[tokio::test]
async fn a_missing_or_stale_compare_and_set_never_replaces_the_children() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = &fixture.uow;
    let original = create_job(uow, "Java backend", json!({})).await?;
    let client = fixture.client().await;
    let before = postings(&client, original.job.id).await?;
    let stale = replacement(&original, "Stale title", "Docker");
    assert!(update(uow, &stale, 1).await?.is_none());
    assert_eq!(stored(uow, original.job.id).await?, Some(original.clone()));
    assert_eq!(postings(&client, original.job.id).await?, before);

    let mut missing = stale.clone();
    let missing_id = Uuid::new_v4();
    missing.job.id = missing_id;
    for skill in &mut missing.skills {
        skill.job_id = missing_id;
    }
    assert!(update(uow, &missing, 0).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn a_concurrent_compare_and_set_blocks_and_then_one_complete_aggregate_wins() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = &fixture.uow;
    let original = create_job(uow, "Java backend", json!({})).await?;
    let first = replacement(&original, "First winner", "Kubernetes");
    let second = replacement(&original, "Second winner", "Docker");
    let (written_tx, written_rx) = tokio::sync::oneshot::channel::<()>();
    let (allow_tx, allow_rx) = tokio::sync::oneshot::channel::<()>();

    // The first scope writes the job row and then waits until the test releases it, so the second
    // write must reach the PostgreSQL row lock and wait behind it.
    let winner = uow.transaction(READ_WRITE, async |tx| {
        let saved = PgJobRepository.update_job_aggregate(tx, &first, 0).await;
        let _ = written_tx.send(());
        let _ = allow_rx.await;
        saved
    });
    let loser_and_release = async {
        written_rx.await?;
        let loser = uow.transaction(READ_WRITE, async |tx| {
            PgJobRepository.update_job_aggregate(tx, &second, 0).await
        });
        let observe = async {
            let observer = fixture.client().await;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let blocked: bool = observer
                    .query_one(
                        "SELECT EXISTS (SELECT 1 FROM pg_stat_activity \
                         WHERE datname = current_database() AND application_name = 'jobniac' \
                         AND wait_event_type = 'Lock' AND query LIKE '%UPDATE job_schema.jobs%')",
                        &[],
                    )
                    .await?
                    .get(0);
                if blocked {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err::<(), Box<dyn Error>>(
                        "the second compare-and-set did not reach the row lock".into(),
                    );
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let _ = allow_tx.send(());
            Ok(())
        };
        let (loser_result, observed) = tokio::join!(loser, observe);
        observed?;
        Ok::<Option<JobAggregate>, Box<dyn Error>>(loser_result?)
    };
    let (winner_result, loser_result) = tokio::join!(winner, loser_and_release);

    let winner_aggregate = winner_result?.ok_or("the first compare-and-set must succeed")?;
    assert_eq!(winner_aggregate.job.revision, 1);
    assert!(loser_result?.is_none());
    let persisted = stored(uow, original.job.id)
        .await?
        .ok_or("the job must still exist")?;
    assert_eq!(persisted, first);
    assert_eq!(persisted.text_ingestion, original.text_ingestion);
    let result: JobSearchResponse = search(uow)
        .search_jobs(&json!({"query": "kubernetes"}))
        .await?;
    let top = result
        .jobs
        .first()
        .ok_or("kubernetes must match the winner")?;
    assert_eq!(top.job.title, "First winner");
    assert_eq!(top.skills, first.skills);
    assert!(
        search(uow)
            .search_jobs(&json!({"query": "docker"}))
            .await?
            .jobs
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn an_exhausted_revision_stays_unchanged_and_conflicts_safely() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = &fixture.uow;
    let original = create_job(uow, "Java backend", json!({})).await?;
    let client = fixture.client().await;
    client
        .execute(
            "UPDATE job_schema.jobs SET revision = 9223372036854775807 WHERE id = $1",
            &[&original.job.id],
        )
        .await?;
    let mut expected = original.clone();
    expected.job.revision = i64::MAX;
    let service = fixed_service(uow, EDITED_AT)?;
    let error = service
        .update_job(&json!({
            "jobId": original.job.id.to_string(),
            "expectedRevision": i64::MAX,
            "skills": [],
        }))
        .await
        .err()
        .ok_or("an exhausted revision must conflict")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::Conflict);
    assert_eq!(envelope.message, CONFLICT);
    assert_eq!(
        envelope.details.get("expectedRevision").map(String::as_str),
        Some("9223372036854775807")
    );
    assert_eq!(stored(uow, original.job.id).await?, Some(expected));
    Ok(())
}

#[tokio::test]
async fn writes_and_lookups_sanitize_database_failures() -> TestResult {
    let uow = closed_port_uow()?;
    let graph = text_graph(50, CREATED_AT);
    let write = update(&uow, &graph, 0)
        .await
        .err()
        .ok_or("a write to an unreachable database must fail")?;
    assert_eq!(error_envelope(&write).message, UNAVAILABLE);
    let lookup = uow
        .transaction(jobniac_core::profiles::repository::READ_ONLY, async |tx| {
            PgJobRepository
                .find_by_canonical_fingerprint(tx, &"a".repeat(64))
                .await
        })
        .await
        .err()
        .ok_or("a lookup of an unreachable database must fail")?;
    assert_eq!(error_envelope(&lookup).message, UNAVAILABLE);
    Ok(())
}
