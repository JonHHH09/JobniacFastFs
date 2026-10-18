//! Bounded job search over the posting index against a migrated SQLite database: the persisted
//! ranking with full skills, the candidate boundary and its nullable total, hot tokens (more than
//! 500 matching jobs), the hot-posting bound, the index plan, and the failure envelopes.
//!
//! The statement-count tests need the `test-hooks` feature. All data is synthetic.

mod sqlite_job_support;

use std::error::Error;
use std::sync::Arc;

use jobniac_core::clock::SystemClock;
use jobniac_core::errors::{ErrorCode, error_envelope};
#[cfg(feature = "test-hooks")]
use jobniac_core::jobs::JobSearchRepository;
use jobniac_core::jobs::JobSearchService;
#[cfg(feature = "test-hooks")]
use jobniac_core::profiles::repository::READ_ONLY;
use jobniac_infra::sqlite::codec::{instant_text, uuid_text};
use jobniac_infra::sqlite::jobs::SqliteJobRepository;
#[cfg(feature = "test-hooks")]
use jobniac_infra::sqlite::jobs::search::statements_on_this_thread;
use jobniac_infra::sqlite::jobs::search::{
    MAX_POSTINGS_PER_TOKEN, MAX_SEARCH_CANDIDATES, POSTING_FETCH_LIMIT, SEARCH_CANDIDATES_SQL,
    candidate_binds,
};
use jobniac_text::search::{prefixes, tokens};
use rusqlite::Connection;
use serde_json::json;
use sqlite_job_support::{
    TestResult, closed_uow, insert_text_job_row, search, service, start, unconfigured_uow,
};
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

const NOW: OffsetDateTime = datetime!(2026-09-29 00:00 UTC);

/// A job row with a `Other` description, its provenance and one title posting per token of
/// `title`.
fn insert_job(connection: &Connection, id: Uuid, title: &str) -> Result<(), Box<dyn Error>> {
    insert_text_job_row(connection, id, title, NOW)?;
    for term in tokens(Some(title)) {
        connection.execute(
            "INSERT INTO job_search_terms (job_id, field_key, term, weight) \
             VALUES (?1, 'job.title', ?2, 8)",
            rusqlite::params![uuid_text(id), term],
        )?;
    }
    Ok(())
}

/// A `Java` skill of `job_id` with its skill posting (weight 7).
fn insert_java_skill(
    connection: &Connection,
    job_id: Uuid,
    skill_id: Uuid,
) -> Result<(), Box<dyn Error>> {
    connection.execute(
        "INSERT INTO job_job_skills \
         (id, job_id, skill, normalized_skill, required, display_order, created_at) \
         VALUES (?1, ?2, 'Java', 'java', 1, 0, ?3)",
        rusqlite::params![uuid_text(skill_id), uuid_text(job_id), instant_text(NOW)],
    )?;
    connection.execute(
        "INSERT INTO job_search_terms (job_id, field_key, term, weight) \
         VALUES (?1, ?2, 'java', 7)",
        rusqlite::params![uuid_text(job_id), format!("skills:{skill_id}")],
    )?;
    Ok(())
}

/// Inserts `count` jobs titled `Java` with identifiers 1 to `count`, in one transaction.
fn insert_java_jobs(connection: &Connection, count: u128) -> Result<Vec<Uuid>, Box<dyn Error>> {
    connection.execute_batch("BEGIN")?;
    let mut ids = Vec::new();
    for value in 1..=count {
        let id = Uuid::from_u128(value);
        insert_job(connection, id, "Java")?;
        ids.push(id);
    }
    connection.execute_batch("COMMIT")?;
    Ok(ids)
}

#[tokio::test]
async fn the_persisted_ranking_uses_full_skills_and_the_limit_count() -> TestResult {
    let fixture = start();
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
    let fixture = start();
    let connection = fixture.connection();
    let count = u128::try_from(MAX_SEARCH_CANDIDATES + 1)?;
    let ids = insert_java_jobs(&connection, count)?;

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
    let fixture = start();
    let connection = fixture.connection();
    // 600 jobs match `java` with score 8. Three jobs with the same title also carry a `Java`
    // skill, so their score is 15. The SQL sample keeps the three best-scored jobs and the 498
    // smallest identifiers of the others, and the core orders the hydrated sample by score and then
    // identifier.
    connection.execute_batch("BEGIN")?;
    for value in 1..=600_u128 {
        insert_job(&connection, Uuid::from_u128(value), "Java")?;
    }
    let skilled: Vec<Uuid> = (1000..=1002_u128).map(Uuid::from_u128).collect();
    for (index, id) in skilled.iter().enumerate() {
        insert_job(&connection, *id, "Java")?;
        insert_java_skill(
            &connection,
            *id,
            Uuid::from_u128(5000 + u128::try_from(index)?),
        )?;
    }
    connection.execute_batch("COMMIT")?;

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

/// Adds `count` description postings `java00000`, `java00001`, ... to `job_id`.
fn insert_hot_postings(
    connection: &Connection,
    job_id: Uuid,
    prefix: &str,
    count: i64,
) -> Result<(), Box<dyn Error>> {
    connection.execute(
        "WITH RECURSIVE g(n) AS (SELECT 0 UNION ALL SELECT n + 1 FROM g WHERE n < ?3 - 1) \
         INSERT INTO job_search_terms (job_id, field_key, term, weight) \
         SELECT ?1, 'job.description', ?2 || printf('%05d', n), 3 FROM g",
        rusqlite::params![uuid_text(job_id), prefix, count],
    )?;
    Ok(())
}

#[tokio::test]
async fn a_hot_posting_list_is_bounded_and_the_score_is_recomputed_from_the_hydrated_job()
-> TestResult {
    let fixture = start();
    let connection = fixture.connection();
    let id = Uuid::from_u128(1);
    insert_job(&connection, id, "Java")?;
    insert_hot_postings(
        &connection,
        id,
        "java",
        i64::try_from(POSTING_FETCH_LIMIT + 10)?,
    )?;

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
    let fixture = start();
    let connection = fixture.connection();
    let ids = insert_java_jobs(&connection, 520)?;
    insert_hot_postings(&connection, ids[0], "unrelated", 20_000)?;
    connection.execute_batch("ANALYZE")?;

    let query_tokens = vec!["java".to_owned()];
    let query_prefixes = prefixes(&query_tokens).ok_or("the prefixes of a valid token")?;
    let owners: Vec<&str> = query_prefixes
        .owners()
        .iter()
        .map(|owner| &**owner)
        .collect();
    let (maximum, fetch, candidates) = candidate_binds()?;
    let explain = format!("EXPLAIN QUERY PLAN {SEARCH_CANDIDATES_SQL}");
    let mut statement = connection.prepare(&explain)?;
    let rows = statement
        .query_map(
            rusqlite::params![
                serde_json::to_string(&query_tokens)?,
                serde_json::to_string(query_prefixes.values())?,
                serde_json::to_string(&owners)?,
                maximum,
                fetch,
                maximum,
                fetch,
                candidates,
            ],
            |row| row.get::<_, String>(3),
        )?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    let plan = rows.join("\n");

    assert!(plan.contains("job_search_terms_term_prefix_idx"), "{plan}");
    assert!(
        !plan
            .lines()
            .any(|line| line == "SCAN posting" || line == "SCAN job_search_terms"),
        "{plan}"
    );
    assert!(!SEARCH_CANDIDATES_SQL.contains("description"));
    assert_eq!(MAX_POSTINGS_PER_TOKEN, 4008);
    Ok(())
}

#[tokio::test]
async fn an_unconfigured_store_and_an_unavailable_database_keep_the_unavailable_envelopes()
-> TestResult {
    let error = JobSearchService::new(unconfigured_uow(), SqliteJobRepository)
        .search_jobs(&json!({"query": "java"}))
        .await
        .err()
        .ok_or("a search without a database must fail")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.message, "Local database is not configured");

    let error = JobSearchService::new(closed_uow(), SqliteJobRepository)
        .search_jobs(&json!({"query": "java"}))
        .await
        .err()
        .ok_or("a search of an unavailable database must fail")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::InternalError);
    assert_eq!(envelope.message, "Local database is unavailable");
    assert!(envelope.details.is_empty());
    Ok(())
}

/// A search of several jobs issues exactly three statements: the candidate sample and the batch
/// reads of the jobs and of their skills. Hydration is never per job.
#[cfg(feature = "test-hooks")]
#[tokio::test]
async fn a_search_issues_three_statements_regardless_of_the_number_of_jobs() -> TestResult {
    let fixture = start();
    let connection = fixture.connection();
    insert_java_jobs(&connection, 5)?;
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
#[cfg(feature = "test-hooks")]
#[tokio::test]
async fn an_empty_token_list_issues_no_statement() -> TestResult {
    let fixture = start();
    let before = statements_on_this_thread();
    let candidates = fixture
        .uow
        .transaction(READ_ONLY, async |tx| {
            SqliteJobRepository.search_job_candidates(tx, &[], 10).await
        })
        .await?;
    assert_eq!(candidates.matched_count(), 0);
    assert!(!candidates.has_more());
    assert!(candidates.items().is_empty());
    assert_eq!(statements_on_this_thread(), before);
    Ok(())
}
