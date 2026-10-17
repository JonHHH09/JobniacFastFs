//! Paste-first capture, get, text and fingerprint reuse, and the keyset listing of jobs against a
//! disposable PostgreSQL 18 server.
//!
//! The server-backed tests
//! skip with a notice when `JOBNIAC_TEST_PG_PORT` is unset; the unconfigured and unreachable cases
//! need no server. All data is synthetic.

mod job_support;

use job_support::{
    TestResult, closed_port_uow, count, fixed_service, has_posting, postings, service, start,
    text_graph, unconfigured_uow,
};
use jobniac_core::clock::SystemClock;
use jobniac_core::errors::AppError;
use jobniac_core::errors::{ErrorCode, PERSISTENCE_CONSTRAINT_MESSAGE, error_envelope};
use jobniac_core::jobs::{CaptureStatus, JobAggregate, JobRepository};
use jobniac_core::profiles::repository::{READ_ONLY, READ_WRITE};
use jobniac_infra::postgres::jobs::PgJobRepository;
use jobniac_infra::postgres::jobs::terms::job_search_terms;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use serde_json::json;
use std::sync::Arc;
use time::OffsetDateTime;
use time::macros::datetime;
use tokio_postgres::Client;
use uuid::Uuid;

const CREATED_AT: OffsetDateTime = datetime!(2026-09-29 10:00 UTC);
const UNAVAILABLE: &str = "Local database is unavailable";
const UNCONFIGURED: &str = "Local database is not configured";

/// One create in its own read-write scope, as the service runs it.
async fn create(
    uow: &PgUnitOfWork,
    graph: &JobAggregate,
) -> Result<(JobAggregate, bool), AppError> {
    uow.transaction(READ_WRITE, async |tx| {
        PgJobRepository.create_text_job(tx, graph).await
    })
    .await
}

/// Creates the graphs concurrently, each in its own scope and session.
async fn create_concurrently(
    uow: &PgUnitOfWork,
    graphs: Vec<JobAggregate>,
) -> Result<Vec<(JobAggregate, bool)>, Box<dyn std::error::Error>> {
    let mut tasks = tokio::task::JoinSet::new();
    for graph in graphs {
        let uow = uow.clone();
        tasks.spawn(async move {
            uow.transaction(READ_WRITE, async |tx| {
                PgJobRepository.create_text_job(tx, &graph).await
            })
            .await
        });
    }
    let mut results = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        results.push(joined??);
    }
    Ok(results)
}

/// The one winner of a concurrent create: exactly one `created` and every result is that job.
fn single_winner(results: &[(JobAggregate, bool)]) -> Result<JobAggregate, &'static str> {
    let winners: Vec<&JobAggregate> = results
        .iter()
        .filter(|(_, created)| *created)
        .map(|(job, _)| job)
        .collect();
    let [winner] = winners.as_slice() else {
        return Err("exactly one create must report created");
    };
    if results.iter().any(|(job, _)| job != *winner) {
        return Err("every concurrent create must return the winner");
    }
    Ok((*winner).clone())
}

/// Replaces the input text hash of a graph's provenance row.
fn set_text_hash(graph: &mut JobAggregate, hash: &str) {
    if let Some(text) = graph.text_ingestion.as_mut() {
        text.input_text_hash = hash.to_owned();
    }
}

/// Rows of one child table for `job_id` (`jobs` is keyed by `id`, the others by `job_id`).
async fn rows_for(
    client: &Client,
    table: &str,
    job_id: Uuid,
) -> Result<i64, Box<dyn std::error::Error>> {
    let key = if table == "jobs" { "id" } else { "job_id" };
    let row = client
        .query_one(
            &format!("SELECT count(*) FROM job_schema.{table} WHERE {key} = $1"),
            &[&job_id],
        )
        .await?;
    Ok(row.get(0))
}

/// Asserts that no row of the graph of `job_id` was written.
async fn assert_no_graph(client: &Client, job_id: Uuid) -> TestResult {
    for table in ["jobs", "job_skills", "job_text_ingestions", "search_terms"] {
        assert_eq!(rows_for(client, table, job_id).await?, 0, "table {table}");
    }
    Ok(())
}

#[tokio::test]
async fn an_unconfigured_store_refuses_before_any_connection() -> TestResult {
    let uow = unconfigured_uow();
    let graph = text_graph(10, CREATED_AT);
    let error = create(&uow, &graph)
        .await
        .err()
        .ok_or("a create without a database must fail")?;
    assert_eq!(error_envelope(&error).message, UNCONFIGURED);
    let capture = service(&uow, Arc::new(SystemClock))?
        .add_job_from_text(&json!({"text": "Java backend"}))
        .await
        .err()
        .ok_or("a capture without a database must fail")?;
    assert_eq!(error_envelope(&capture).message, UNCONFIGURED);
    Ok(())
}

#[tokio::test]
async fn an_unreachable_database_is_reported_without_its_details() -> TestResult {
    let uow = closed_port_uow()?;
    let jobs = fixed_service(&uow, CREATED_AT)?;
    let error = jobs
        .get_job("00000000-0000-0000-0000-000000000010")
        .await
        .err()
        .ok_or("a read of an unreachable database must fail")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.message, UNAVAILABLE);
    assert!(envelope.details.is_empty());
    let capture = jobs
        .add_job_from_text(&json!({"text": "Java backend"}))
        .await
        .err()
        .ok_or("a capture of an unreachable database must fail")?;
    assert_eq!(error_envelope(&capture).message, UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn a_missing_job_reads_as_not_found_and_a_text_hash_as_absent() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let missing = Uuid::from_u128(7);
    let found = fixture
        .uow
        .transaction(READ_ONLY, async |tx| {
            PgJobRepository.get_job(tx, missing).await
        })
        .await?;
    assert!(found.is_none());
    let hash = "c".repeat(64);
    let by_hash = fixture
        .uow
        .transaction(READ_ONLY, async |tx| {
            PgJobRepository.find_by_text_hash(tx, &hash).await
        })
        .await?;
    assert!(by_hash.is_none());
    Ok(())
}

#[tokio::test]
async fn create_round_trips_the_graph_and_writes_the_search_postings() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let graph = text_graph(10, CREATED_AT);
    let (saved, created) = create(&fixture.uow, &graph).await?;
    assert!(created);
    assert_eq!(saved, graph);
    let text = graph.text_ingestion.as_ref().ok_or("text provenance")?;
    let by_hash = fixture
        .uow
        .transaction(READ_ONLY, async |tx| {
            PgJobRepository
                .find_by_text_hash(tx, &text.input_text_hash)
                .await
        })
        .await?;
    assert_eq!(by_hash, Some(graph.clone()));
    let read = service(&fixture.uow, Arc::new(SystemClock))?
        .get_job(&graph.job.id.to_string())
        .await?;
    assert_eq!(read, graph);

    let client = fixture.client().await;
    assert_eq!(rows_for(&client, "jobs", graph.job.id).await?, 1);
    assert_eq!(
        rows_for(&client, "job_text_ingestions", graph.job.id).await?,
        1
    );
    assert_eq!(
        rows_for(&client, "job_link_ingestions", graph.job.id).await?,
        0
    );
    let terms = postings(&client, graph.job.id).await?;
    let skill_key = format!("skills:{}", Uuid::from_u128(1010));
    for (field, term, weight) in [
        ("job.title", "platform", 8),
        ("job.company", "example", 5),
        ("job.location", "remote", 3),
        ("job.description", "build", 3),
        ("job.experienceRequirement", "5+", 4),
        ("job.employmentType", "full", 2),
        ("job.seniority", "senior", 2),
        (skill_key.as_str(), "python", 7),
    ] {
        assert!(
            has_posting(&terms, field, term, weight),
            "missing posting {field} {term} {weight}"
        );
    }
    let mut expected: Vec<(String, String, i16)> = job_search_terms(&graph.job, &graph.skills)
        .into_iter()
        .map(|term| Ok((term.field_key, term.term, i16::try_from(term.weight)?)))
        .collect::<Result<_, std::num::TryFromIntError>>()?;
    let mut stored = terms;
    expected.sort();
    stored.sort();
    assert_eq!(
        stored, expected,
        "the stored postings must be exactly the expected set"
    );
    Ok(())
}

#[tokio::test]
async fn a_check_violation_maps_to_the_safe_error_and_rolls_back_the_graph() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let mut graph = text_graph(20, CREATED_AT);
    graph.skills[0].display_order = -1;
    let error = create(&fixture.uow, &graph)
        .await
        .err()
        .ok_or("a negative display order must be refused")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::ValidationError);
    assert_eq!(envelope.message, PERSISTENCE_CONSTRAINT_MESSAGE);
    assert!(envelope.details.is_empty());
    assert_no_graph(&fixture.client().await, graph.job.id).await
}

#[tokio::test]
async fn a_deferred_provenance_trigger_failure_rolls_back_the_entire_graph() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let client = fixture.client().await;
    client
        .batch_execute(
            "CREATE FUNCTION job_schema.jobniac_test_reject_ingestion() RETURNS trigger \
             LANGUAGE plpgsql AS $body$ \
             BEGIN \
                 RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'injected trigger rejection'; \
             END; \
             $body$; \
             CREATE CONSTRAINT TRIGGER jobniac_test_reject_ingestion \
             AFTER INSERT ON job_schema.job_text_ingestions \
             DEFERRABLE INITIALLY DEFERRED \
             FOR EACH ROW EXECUTE FUNCTION job_schema.jobniac_test_reject_ingestion();",
        )
        .await?;
    let graph = text_graph(21, CREATED_AT);
    let error = create(&fixture.uow, &graph)
        .await
        .err()
        .ok_or("the injected trigger must reject the commit")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::ValidationError);
    assert_eq!(envelope.message, PERSISTENCE_CONSTRAINT_MESSAGE);
    assert_no_graph(&client, graph.job.id).await
}

#[tokio::test]
async fn a_duplicate_text_hash_reuses_the_original_job() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let mut original = text_graph(30, CREATED_AT);
    set_text_hash(&mut original, &"e".repeat(64));
    original.job.canonical_fingerprint = "f".repeat(64);
    let mut duplicate = text_graph(31, CREATED_AT);
    set_text_hash(&mut duplicate, &"e".repeat(64));
    duplicate.job.canonical_fingerprint = "1".repeat(64);
    duplicate.job.title = "Ignored override".to_owned();

    let (saved, created) = create(&fixture.uow, &original).await?;
    let (reused, duplicate_created) = create(&fixture.uow, &duplicate).await?;

    assert!(created);
    assert!(!duplicate_created);
    assert_eq!(reused, saved);
    assert_eq!(saved, original);
    let client = fixture.client().await;
    assert_eq!(rows_for(&client, "jobs", Uuid::from_u128(31)).await?, 0);
    assert_eq!(
        count(&client, "SELECT count(*) FROM job_schema.jobs").await?,
        1
    );
    assert_eq!(
        count(
            &client,
            "SELECT count(*) FROM job_schema.job_text_ingestions"
        )
        .await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn a_duplicate_fingerprint_reuses_the_original_job() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let mut original = text_graph(40, CREATED_AT);
    set_text_hash(&mut original, &"1".repeat(64));
    original.job.canonical_fingerprint = "2".repeat(64);
    let mut duplicate = text_graph(41, CREATED_AT);
    set_text_hash(&mut duplicate, &"3".repeat(64));
    duplicate.job.canonical_fingerprint = "2".repeat(64);
    duplicate.job.title = "Ignored alternate title".to_owned();

    let (saved, created) = create(&fixture.uow, &original).await?;
    let (reused, duplicate_created) = create(&fixture.uow, &duplicate).await?;

    assert!(created);
    assert!(!duplicate_created);
    assert_eq!(reused, saved);
    assert_eq!(saved, original);
    let by_hash = fixture
        .uow
        .transaction(READ_ONLY, async |tx| {
            PgJobRepository.find_by_text_hash(tx, &"3".repeat(64)).await
        })
        .await?;
    assert!(by_hash.is_none());
    let client = fixture.client().await;
    assert_eq!(
        count(&client, "SELECT count(*) FROM job_schema.jobs").await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn concurrent_submissions_with_one_text_hash_persist_one_winner() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let graphs: Vec<JobAggregate> = (0..8_u128)
        .map(|index| {
            let mut graph = text_graph(100 + index, CREATED_AT);
            set_text_hash(&mut graph, &"4".repeat(64));
            graph.job.canonical_fingerprint = format!("{:064x}", 100 + index);
            graph.job.title = format!("Candidate {index}");
            graph
        })
        .collect();

    let results = create_concurrently(&fixture.uow, graphs).await?;

    let winner = single_winner(&results)?;
    let client = fixture.client().await;
    assert_eq!(
        count(&client, "SELECT count(*) FROM job_schema.jobs").await?,
        1
    );
    assert_eq!(
        count(
            &client,
            "SELECT count(*) FROM job_schema.job_text_ingestions"
        )
        .await?,
        1
    );
    assert!(winner.job.title.starts_with("Candidate "));
    Ok(())
}

#[tokio::test]
async fn concurrent_submissions_with_one_fingerprint_persist_one_winner() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let graphs: Vec<JobAggregate> = (0..8_u128)
        .map(|index| {
            let mut graph = text_graph(200 + index, CREATED_AT);
            set_text_hash(&mut graph, &format!("{:064x}", 200 + index));
            graph.job.canonical_fingerprint = "5".repeat(64);
            graph
        })
        .collect();

    let results = create_concurrently(&fixture.uow, graphs).await?;

    single_winner(&results)?;
    let client = fixture.client().await;
    assert_eq!(
        count(&client, "SELECT count(*) FROM job_schema.jobs").await?,
        1
    );
    assert_eq!(
        count(
            &client,
            "SELECT count(*) FROM job_schema.job_text_ingestions"
        )
        .await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn the_service_reuses_the_job_for_the_same_text_and_reports_the_status() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let jobs = service(&fixture.uow, Arc::new(SystemClock))?;
    let request = json!({
        "text": "Java Backend Developer\nSkills: Java, Spring Boot",
        "company": "Example Corp",
        "location": "Remote",
    });
    let first = jobs.add_job_from_text(&request).await?;
    let second = jobs.add_job_from_text(&request).await?;
    assert_eq!(first.status, CaptureStatus::CreatedJob);
    assert_eq!(second.status, CaptureStatus::ReusedExistingJob);
    assert_eq!(second.job, first.job);
    let read = jobs.get_job(&first.job.job.id.to_string()).await?;
    assert_eq!(read, first.job);
    Ok(())
}

/// Inserts a text job row directly, in the transaction of `client`, without its search postings
/// (the listing does not read them).
async fn insert_listing_row(
    transaction: &tokio_postgres::Transaction<'_>,
    graph: &JobAggregate,
) -> TestResult {
    let job = &graph.job;
    transaction
        .execute(
            "INSERT INTO job_schema.jobs \
             (id, source_method, title, description, canonical_fingerprint, created_at, updated_at) \
             VALUES ($1, 'text', $2, $3, $4, $5, $5)",
            &[
                &job.id,
                &job.title,
                &job.description,
                &job.canonical_fingerprint,
                &job.created_at,
            ],
        )
        .await?;
    let text = graph.text_ingestion.as_ref().ok_or("text provenance")?;
    transaction
        .execute(
            "INSERT INTO job_schema.job_text_ingestions (id, job_id, input_text_hash, created_at) \
             VALUES ($1, $2, $3, $4)",
            &[
                &text.id,
                &text.job_id,
                &text.input_text_hash,
                &text.created_at,
            ],
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn the_listing_pages_jobs_in_created_time_and_uuid_order() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let jobs = service(&fixture.uow, Arc::new(SystemClock))?;
    let base = datetime!(2026-09-29 10:00 UTC);
    let ids: Vec<Uuid> = (0..5_u128)
        .map(|index| Uuid::from_u128(300 + index))
        .collect();
    for (index, id) in ids.iter().enumerate() {
        let index = i64::try_from(index)?;
        let graph = text_graph(id.as_u128(), base + time::Duration::seconds(index));
        create(&fixture.uow, &graph).await?;
    }

    let first = jobs.list_jobs(Some(2), None).await?;
    let first_cursor = first
        .next_cursor
        .clone()
        .ok_or("the first page needs a cursor")?;
    let second = jobs.list_jobs(Some(2), Some(&first_cursor)).await?;
    let second_cursor = second
        .next_cursor
        .clone()
        .ok_or("the second page needs a cursor")?;
    let third = jobs.list_jobs(Some(2), Some(&second_cursor)).await?;

    let listed: Vec<Uuid> = first
        .jobs
        .iter()
        .chain(second.jobs.iter())
        .chain(third.jobs.iter())
        .map(|job| job.id)
        .collect();
    let expected: Vec<Uuid> = ids.iter().rev().copied().collect();
    assert_eq!(listed, expected);
    assert_ne!(first_cursor, second_cursor);
    assert!(third.next_cursor.is_none());
    assert_eq!(
        first
            .jobs
            .first()
            .ok_or("the first page is empty")?
            .description,
        "Build platform services with Python"
    );
    Ok(())
}

#[tokio::test]
async fn the_listing_cursor_is_a_created_at_watermark_not_a_commit_snapshot() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let jobs = service(&fixture.uow, Arc::new(SystemClock))?;
    let base = datetime!(2026-09-29 10:00 UTC);
    let older = text_graph(401, base);
    let newest = text_graph(403, base + time::Duration::seconds(2));
    let late = text_graph(402, base + time::Duration::seconds(1));
    create(&fixture.uow, &older).await?;
    create(&fixture.uow, &newest).await?;

    let mut pending = fixture.client().await;
    let transaction = pending.transaction().await?;
    insert_listing_row(&transaction, &late).await?;

    let first = jobs.list_jobs(Some(1), None).await?;
    let first_ids: Vec<Uuid> = first.jobs.iter().map(|job| job.id).collect();
    assert_eq!(first_ids, [newest.job.id]);
    let first_cursor = first
        .next_cursor
        .clone()
        .ok_or("the first page needs a cursor")?;
    transaction.commit().await?;

    let second = jobs.list_jobs(Some(1), Some(&first_cursor)).await?;
    let second_ids: Vec<Uuid> = second.jobs.iter().map(|job| job.id).collect();
    assert_eq!(second_ids, [late.job.id]);
    let second_cursor = second
        .next_cursor
        .clone()
        .ok_or("the second page needs a cursor")?;
    let third = jobs.list_jobs(Some(1), Some(&second_cursor)).await?;
    let third_ids: Vec<Uuid> = third.jobs.iter().map(|job| job.id).collect();
    assert_eq!(third_ids, [older.job.id]);
    Ok(())
}
