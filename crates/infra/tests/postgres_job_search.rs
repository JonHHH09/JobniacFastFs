//! Bounded job search over the posting index against a disposable PostgreSQL 18 server: the
//! persisted ranking with full skills, the candidate boundary and its nullable total, hot tokens
//! (more than 500 matching jobs), the hot-posting bound, the index plan, and the failure envelopes.
//!
//! The server-backed tests skip with a notice when
//! `JOBNIAC_TEST_PG_PORT` is unset; the unconfigured and unreachable cases need no server. All data
//! is synthetic.

mod job_support;

use std::error::Error;
use std::sync::Arc;

use job_support::{TestResult, closed_port_uow, search, service, start, unconfigured_uow};
use jobniac_core::clock::SystemClock;
use jobniac_core::errors::{ErrorCode, error_envelope};
use jobniac_core::jobs::{JobSearchRepository, JobSearchService};
use jobniac_core::profiles::repository::READ_ONLY;
use jobniac_infra::postgres::jobs::PgJobRepository;
use jobniac_infra::postgres::jobs::search::{
    MAX_POSTINGS_PER_TOKEN, MAX_SEARCH_CANDIDATES, POSTING_FETCH_LIMIT, SEARCH_CANDIDATES_SQL,
    statements_on_this_thread,
};
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use jobniac_text::search::{prefixes, tokens};
use serde_json::json;
use time::OffsetDateTime;
use time::macros::datetime;
use tokio_postgres::{Client, Transaction};
use uuid::Uuid;

const NOW: OffsetDateTime = datetime!(2026-09-29 00:00 UTC);

/// A job row with a `Other` description, its provenance and one title posting per token of
/// `title`, in the transaction of the caller (the deferred provenance check runs at its commit).
async fn insert_job(
    transaction: &Transaction<'_>,
    id: Uuid,
    title: &str,
) -> Result<(), Box<dyn Error>> {
    let fingerprint = format!("{:064x}", id.as_u128());
    transaction
        .execute(
            "INSERT INTO job_schema.jobs \
             (id, source_method, title, description, canonical_fingerprint, created_at, updated_at) \
             VALUES ($1, 'text', $2, 'Other', $3, $4, $4)",
            &[&id, &title, &fingerprint, &NOW],
        )
        .await?;
    transaction
        .execute(
            "INSERT INTO job_schema.job_text_ingestions (id, job_id, input_text_hash, created_at) \
             VALUES ($1, $2, $3, $4)",
            &[
                &Uuid::from_u128(id.as_u128() + 100_000),
                &id,
                &fingerprint,
                &NOW,
            ],
        )
        .await?;
    for term in tokens(Some(title)) {
        transaction
            .execute(
                "INSERT INTO job_schema.search_terms (job_id, field_key, term, weight) \
                 VALUES ($1, 'job.title', $2, 8)",
                &[&id, &term],
            )
            .await?;
    }
    Ok(())
}

/// A `Java` skill of `job_id` with its skill posting (weight 7), in the caller's transaction.
async fn insert_java_skill(
    transaction: &Transaction<'_>,
    job_id: Uuid,
    skill_id: Uuid,
) -> Result<(), Box<dyn Error>> {
    transaction
        .execute(
            "INSERT INTO job_schema.job_skills \
             (id, job_id, skill, normalized_skill, required, display_order, created_at) \
             VALUES ($1, $2, 'Java', 'java', true, 0, $3)",
            &[&skill_id, &job_id, &NOW],
        )
        .await?;
    transaction
        .execute(
            "INSERT INTO job_schema.search_terms (job_id, field_key, term, weight) \
             VALUES ($1, $2, 'java', 7)",
            &[&job_id, &format!("skills:{skill_id}")],
        )
        .await?;
    Ok(())
}

/// Inserts `count` jobs titled `Java` with identifiers 1 to `count`, in one transaction.
async fn insert_java_jobs(client: &mut Client, count: u128) -> Result<Vec<Uuid>, Box<dyn Error>> {
    let transaction = client.transaction().await?;
    let mut ids = Vec::new();
    for value in 1..=count {
        let id = Uuid::from_u128(value);
        insert_job(&transaction, id, "Java").await?;
        ids.push(id);
    }
    transaction.commit().await?;
    Ok(ids)
}

#[tokio::test]
async fn the_persisted_ranking_uses_full_skills_and_the_limit_count() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let jobs = service(&fixture.uow, Arc::new(SystemClock))?;
    let first = jobs
        .add_job_from_text(&json!({
            "text": "Java Backend Developer\nSkills: Java, Spring Boot",
            "company": "Example Corp",
            "location": "Remote",
        }))
        .await?
        .job;
    jobs.add_job_from_text(&json!({"text": "Data Analyst\nSkills: SQL, Tableau"}))
        .await?;

    let result = search(&fixture.uow)
        .search_jobs(&json!({"query": "java spring", "limit": 1}))
        .await?;
    assert_eq!(result.query_tokens, ["java", "spring"]);
    assert_eq!(result.total_matches, Some(1));
    assert_eq!(result.matched_count, 1);
    assert_eq!(result.returned_count, 1);
    assert_eq!(result.jobs.len(), 1);
    let top = &result.jobs[0];
    assert_eq!(top.job, first.job);
    assert_eq!(top.skills, first.skills);
    assert_eq!(top.score, 28);
    assert_eq!(
        top.matched_fields,
        ["job.title", "job.description", "job.skills"]
    );

    let reverse = search(&fixture.uow)
        .search_jobs(&json!({"query": "javascript"}))
        .await?;
    let found = reverse
        .jobs
        .first()
        .ok_or("javascript must match the Java job")?;
    assert_eq!(found.job.id, first.job.id);
    assert_eq!(found.score, 18);
    Ok(())
}

#[tokio::test]
async fn the_candidate_boundary_gives_a_null_total_and_the_final_uuid_order() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let mut client = fixture.client().await;
    let count = u128::try_from(MAX_SEARCH_CANDIDATES + 1)?;
    let ids = insert_java_jobs(&mut client, count).await?;

    let result = search(&fixture.uow)
        .search_jobs(&json!({"query": "java", "limit": 100}))
        .await?;

    assert_eq!(result.total_matches, None);
    assert!(result.has_more);
    assert_eq!(result.matched_count, i64::try_from(MAX_SEARCH_CANDIDATES)?);
    assert_eq!(result.returned_count, 100);
    let listed: Vec<Uuid> = result.jobs.iter().map(|found| found.job.id).collect();
    assert_eq!(listed, &ids[..100]);
    Ok(())
}

#[tokio::test]
async fn a_hot_token_keeps_the_best_sample_and_orders_the_result_exactly() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let mut client = fixture.client().await;
    // 600 jobs match `java` with score 8. Three jobs with the same title also carry a `Java`
    // skill, so their score is 15. The SQL sample keeps the three best-scored jobs and the 498
    // smallest identifiers of the others, and the core orders the hydrated sample by score and then
    // identifier.
    let transaction = client.transaction().await?;
    for value in 1..=600_u128 {
        insert_job(&transaction, Uuid::from_u128(value), "Java").await?;
    }
    let skilled: Vec<Uuid> = (1000..=1002_u128).map(Uuid::from_u128).collect();
    for (index, id) in skilled.iter().enumerate() {
        insert_job(&transaction, *id, "Java").await?;
        insert_java_skill(
            &transaction,
            *id,
            Uuid::from_u128(5000 + u128::try_from(index)?),
        )
        .await?;
    }
    transaction.commit().await?;

    let result = search(&fixture.uow)
        .search_jobs(&json!({"query": "java", "limit": 5}))
        .await?;

    assert_eq!(result.total_matches, None);
    assert!(result.has_more);
    assert_eq!(result.matched_count, i64::try_from(MAX_SEARCH_CANDIDATES)?);
    let listed: Vec<(Uuid, i64)> = result
        .jobs
        .iter()
        .map(|found| (found.job.id, found.score))
        .collect();
    assert_eq!(
        listed,
        [
            (skilled[0], 15),
            (skilled[1], 15),
            (skilled[2], 15),
            (Uuid::from_u128(1), 8),
            (Uuid::from_u128(2), 8),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_hot_posting_list_is_bounded_and_the_score_is_recomputed_from_the_hydrated_job()
-> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let mut client = fixture.client().await;
    let id = Uuid::from_u128(1);
    let transaction = client.transaction().await?;
    insert_job(&transaction, id, "Java").await?;
    transaction.commit().await?;
    let postings = i64::try_from(POSTING_FETCH_LIMIT + 10)?;
    client
        .execute(
            "INSERT INTO job_schema.search_terms (job_id, field_key, term, weight) \
             SELECT $1, 'job.description', 'java' || lpad(g::text, 5, '0'), 3 \
             FROM generate_series(0, $2::integer - 1) AS g",
            &[&id, &i32::try_from(postings)?],
        )
        .await?;

    let result = search(&fixture.uow)
        .search_jobs(&json!({"query": "java"}))
        .await?;

    assert!(result.has_more);
    assert_eq!(result.total_matches, None);
    assert_eq!(result.matched_count, 1);
    let found = result.jobs.first().ok_or("the job must match")?;
    assert_eq!(found.score, 8);
    assert_eq!(found.matched_fields, ["job.title"]);
    Ok(())
}

#[tokio::test]
async fn the_index_plan_reads_the_prefix_index_and_never_scans_all_postings() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let mut client = fixture.client().await;
    let ids = insert_java_jobs(&mut client, 520).await?;
    client
        .execute(
            "INSERT INTO job_schema.search_terms (job_id, field_key, term, weight) \
             SELECT $1, 'job.description', 'unrelated' || lpad(g::text, 5, '0'), 3 \
             FROM generate_series(0, 19999) AS g",
            &[&ids[0]],
        )
        .await?;
    client
        .batch_execute("ANALYZE job_schema.jobs; ANALYZE job_schema.search_terms;")
        .await?;

    let query_tokens = vec!["java".to_owned()];
    let query_prefixes = prefixes(&query_tokens).ok_or("the prefixes of a valid token")?;
    let owners: Vec<&str> = query_prefixes
        .owners()
        .iter()
        .map(|owner| &**owner)
        .collect();
    let maximum = i64::try_from(MAX_POSTINGS_PER_TOKEN)?;
    let fetch = i64::try_from(POSTING_FETCH_LIMIT)?;
    let candidates = i64::try_from(MAX_SEARCH_CANDIDATES + 1)?;
    let explain = format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT) {SEARCH_CANDIDATES_SQL}");
    let rows = client
        .query(
            explain.as_str(),
            &[
                &query_tokens,
                &query_prefixes.values(),
                &owners,
                &maximum,
                &fetch,
                &maximum,
                &fetch,
                &candidates,
            ],
        )
        .await?;
    let plan: Vec<String> = rows.iter().map(|row| row.get::<_, String>(0)).collect();
    let plan = plan.join("\n");

    assert!(plan.contains("job_search_terms_term_prefix_idx"), "{plan}");
    assert!(!plan.contains("Seq Scan on search_terms"), "{plan}");
    assert!(!SEARCH_CANDIDATES_SQL.contains("description"));
    assert_eq!(MAX_POSTINGS_PER_TOKEN, 4008);
    Ok(())
}

#[tokio::test]
async fn an_unconfigured_store_and_an_unreachable_database_keep_the_unavailable_envelopes()
-> TestResult {
    let unconfigured: PgUnitOfWork = unconfigured_uow();
    let error = JobSearchService::new(unconfigured, PgJobRepository)
        .search_jobs(&json!({"query": "java"}))
        .await
        .err()
        .ok_or("a search without a database must fail")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.message, "Local database is not configured");

    let closed = closed_port_uow()?;
    let error = JobSearchService::new(closed, PgJobRepository)
        .search_jobs(&json!({"query": "java"}))
        .await
        .err()
        .ok_or("a search of an unreachable database must fail")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::InternalError);
    assert_eq!(envelope.message, "Local database is unavailable");
    assert!(envelope.details.is_empty());
    assert!(!format!("{error:?}").contains("jobniac_unreachable"));
    Ok(())
}

/// A search of several jobs issues exactly three statements: the candidate sample and the batch
/// reads of the jobs and of their skills. Hydration is never per job.
#[tokio::test]
async fn a_search_issues_three_statements_regardless_of_the_number_of_jobs() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let mut client = fixture.client().await;
    insert_java_jobs(&mut client, 5).await?;
    let before = statements_on_this_thread();
    let result = search(&fixture.uow)
        .search_jobs(&json!({"query": "java"}))
        .await?;
    assert_eq!(result.matched_count, 5);
    assert_eq!(result.jobs.len(), 5);
    assert_eq!(
        statements_on_this_thread() - before,
        3,
        "candidate sample, job batch and skill batch"
    );
    Ok(())
}

/// An empty token list matches nothing and issues no statement: the repository returns before it
/// reads anything, so the caller's scope stays untouched.
#[tokio::test]
async fn an_empty_token_list_issues_no_statement() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let before = statements_on_this_thread();
    let candidates = fixture
        .uow
        .transaction(READ_ONLY, async |tx| {
            PgJobRepository.search_job_candidates(tx, &[], 10).await
        })
        .await?;
    assert_eq!(candidates.matched_count(), 0);
    assert!(!candidates.has_more());
    assert!(candidates.items().is_empty());
    assert_eq!(statements_on_this_thread(), before);
    Ok(())
}
