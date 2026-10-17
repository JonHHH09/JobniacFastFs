//! Bounded job search.
//!
//! The candidate sample is the recursive CTE below, with `$n` binds: one more than
//! [`MAX_SEARCH_CANDIDATES`] jobs are ranked by the adapter (score, title in `COLLATE "C"`, then
//! identifier). The first [`MAX_SEARCH_CANDIDATES`] of them are batch-hydrated with their skills in
//! the same REPEATABLE READ snapshot of the read-only scope, and the core re-scores the hydrated
//! data. `has_more` is set when more than the sample exists or when a posting list was cut off at
//! [`MAX_POSTINGS_PER_TOKEN`].
//!
//! Decoding failures are not mapped, so a row that does not decode is an internal error here (see
//! the module policy in [`super`]).

#[cfg(feature = "test-hooks")]
use std::cell::Cell;
use std::collections::HashMap;

use jobniac_core::errors::AppError;
use jobniac_core::jobs::{JobAggregate, JobRecord, JobSkillRecord};
use jobniac_core::pagination::SearchCandidates;
use jobniac_text::search::prefixes;
use tokio_postgres::Client;
use uuid::Uuid;

use super::graph::{JOB_SELECT_BATCH_SQL, SKILLS_BATCH_SQL, job_from_row, skill_from_row};
use crate::postgres::profiles::{col, db, unavailable};

#[cfg(feature = "test-hooks")]
thread_local! {
    /// Statements this adapter has issued on the current thread.
    static STATEMENTS: Cell<usize> = const { Cell::new(0) };
}

/// The number of statements this adapter has issued on the calling thread.
///
/// It exists for the statement-count tests: a search of one or more jobs issues exactly three
/// statements (the candidate sample, then the batch job and skill reads), and an empty token list
/// issues none. The count is per thread, so a current-thread test runtime sees only its own
/// statements. It has no role in the search itself. Built only with the `test-hooks` feature.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn statements_on_this_thread() -> usize {
    STATEMENTS.with(Cell::get)
}

/// Records one statement about to be issued on this thread.
#[cfg(feature = "test-hooks")]
fn count_statement() {
    STATEMENTS.with(|count| count.set(count.get() + 1));
}

/// Largest number of candidate jobs hydrated by one search.
pub const MAX_SEARCH_CANDIDATES: usize = 500;
/// Searched fields per job, which sizes the posting bound: seven text fields and the skills
/// (the posting bound multiplies by 8).
pub const SEARCH_FIELD_COUNT: usize = 8;
/// Postings read per query token before the posting bound is reported (4008).
pub const MAX_POSTINGS_PER_TOKEN: usize = (MAX_SEARCH_CANDIDATES + 1) * SEARCH_FIELD_COUNT;
/// Postings fetched per token: one more than the bound, so truncation is visible (4009).
pub const POSTING_FETCH_LIMIT: usize = MAX_POSTINGS_PER_TOKEN + 1;

/// The candidate sample query, with `$n` binds. The eight binds are, in order: the
/// query tokens; the prefix values; their owners; the posting bound (recursive branch); the posting
/// fetch limit (recursive branch); the posting bound (prefix branch); the posting fetch limit
/// (prefix branch); and the candidate limit, `MAX_SEARCH_CANDIDATES + 1`.
pub const SEARCH_CANDIDATES_SQL: &str = r#"
WITH query_tokens AS (
    SELECT token FROM unnest($1::text[]) AS token
), query_prefixes AS (
    SELECT prefix, query_token
    FROM unnest($2::text[], $3::text[]) AS value(prefix, query_token)
), query_prefix_sets AS (
    SELECT query_token, array_agg(prefix ORDER BY prefix COLLATE "C") AS prefixes
    FROM query_prefixes GROUP BY query_token
), raw_hits AS (
    SELECT posting.job_id, posting.field_key, query.token AS query_token, posting.weight,
           posting.ordinal > $4 AS truncated
    FROM query_tokens AS query
    CROSS JOIN LATERAL (
        WITH RECURSIVE sample(term, job_id, field_key, weight, ordinal) AS (
            (
                SELECT posting.term, posting.job_id, posting.field_key, posting.weight, 1::bigint
                FROM job_schema.search_terms AS posting
                WHERE posting.term COLLATE "C" >= query.token COLLATE "C"
                  AND posting.term COLLATE "C" < (query.token || '~') COLLATE "C"
                ORDER BY posting.term COLLATE "C", posting.job_id, posting.field_key
                LIMIT 1
            )
            UNION ALL
            SELECT next.term, next.job_id, next.field_key, next.weight, sample.ordinal + 1
            FROM sample
            CROSS JOIN LATERAL (
                SELECT posting.term, posting.job_id, posting.field_key, posting.weight
                FROM job_schema.search_terms AS posting
                WHERE posting.term COLLATE "C" >= query.token COLLATE "C"
                  AND posting.term COLLATE "C" < (query.token || '~') COLLATE "C"
                  AND (posting.term COLLATE "C", posting.job_id, posting.field_key)
                      > (sample.term COLLATE "C", sample.job_id, sample.field_key)
                ORDER BY posting.term COLLATE "C", posting.job_id, posting.field_key
                LIMIT 1
            ) AS next
            WHERE sample.ordinal < $5
        )
        SELECT * FROM sample
    ) AS posting
    UNION ALL
    SELECT posting.job_id, posting.field_key, prefix_set.query_token, posting.weight,
           posting.ordinal > $6 AS truncated
    FROM query_prefix_sets AS prefix_set
    CROSS JOIN LATERAL (
        SELECT sample.*,
               row_number() OVER (
                   ORDER BY sample.term COLLATE "C", sample.job_id, sample.field_key
               ) AS ordinal
        FROM (
            SELECT posting.term, posting.job_id, posting.field_key, posting.weight
            FROM job_schema.search_terms AS posting
            WHERE posting.term COLLATE "C" = ANY(prefix_set.prefixes)
            ORDER BY posting.term COLLATE "C", posting.job_id, posting.field_key
            LIMIT $7
        ) AS sample
    ) AS posting
), posting_bounds AS (
    SELECT COALESCE(bool_or(truncated), false) AS posting_truncated FROM raw_hits
), posting_hits AS (
    SELECT DISTINCT job_id, field_key, query_token, weight
    FROM raw_hits WHERE NOT truncated
), scored AS (
    SELECT job_id, SUM(weight)::integer AS score FROM posting_hits GROUP BY job_id
), matching AS (
    SELECT job.id, job.title, scored.score, posting_bounds.posting_truncated
    FROM scored
    CROSS JOIN LATERAL (
        SELECT job.id, job.title FROM job_schema.jobs AS job
        WHERE job.id = scored.job_id LIMIT 1
    ) AS job
    CROSS JOIN posting_bounds
    ORDER BY scored.score DESC, job.title COLLATE "C", job.id
    LIMIT $8
)
SELECT * FROM matching ORDER BY score DESC, title COLLATE "C", id
"#;

/// Returns at most [`MAX_SEARCH_CANDIDATES`] candidate jobs for normalized query tokens, hydrated
/// with their skills. Empty tokens match nothing and read nothing.
pub(crate) async fn search_job_candidates(
    client: &Client,
    query_tokens: &[String],
) -> Result<SearchCandidates<JobAggregate>, AppError> {
    if query_tokens.is_empty() {
        return SearchCandidates::new(0, false, Vec::new());
    }
    let query_prefixes = prefixes(query_tokens).ok_or(AppError::Internal)?;
    let owners: Vec<&str> = query_prefixes
        .owners()
        .iter()
        .map(|owner| &**owner)
        .collect();
    let maximum = count_bind(MAX_POSTINGS_PER_TOKEN)?;
    let posting_fetch = count_bind(POSTING_FETCH_LIMIT)?;
    let candidate_limit = count_bind(MAX_SEARCH_CANDIDATES + 1)?;
    #[cfg(feature = "test-hooks")]
    count_statement();
    let rows = db(client
        .query(
            SEARCH_CANDIDATES_SQL,
            &[
                &query_tokens,
                &query_prefixes.values(),
                &owners,
                &maximum,
                &posting_fetch,
                &maximum,
                &posting_fetch,
                &candidate_limit,
            ],
        )
        .await)?;
    let Some(first) = rows.first() else {
        return SearchCandidates::new(0, false, Vec::new());
    };
    let posting_truncated: bool = col(first, "posting_truncated")?;
    let has_more = rows.len() > MAX_SEARCH_CANDIDATES || posting_truncated;
    let mut ids = Vec::with_capacity(rows.len().min(MAX_SEARCH_CANDIDATES));
    for row in rows.iter().take(MAX_SEARCH_CANDIDATES) {
        ids.push(col::<Uuid>(row, "id")?);
    }
    let items = hydrate(client, &ids).await?;
    let matched = count_bind(items.len())?;
    SearchCandidates::new(matched, has_more, items)
}

/// Batch-hydrates the jobs and their skills of `ids`, in the order of `ids`.
/// The two reads share the snapshot of the caller. A job that vanished since the sample fails closed
/// as unavailable.
async fn hydrate(client: &Client, ids: &[Uuid]) -> Result<Vec<JobAggregate>, AppError> {
    #[cfg(feature = "test-hooks")]
    count_statement();
    let job_rows = db(client.query(JOB_SELECT_BATCH_SQL, &[&ids]).await)?;
    let mut jobs: HashMap<Uuid, JobRecord> = HashMap::with_capacity(job_rows.len());
    for row in &job_rows {
        let job = job_from_row(row).map_err(|_| AppError::Internal)?;
        jobs.insert(job.id, job);
    }
    #[cfg(feature = "test-hooks")]
    count_statement();
    let skill_rows = db(client.query(SKILLS_BATCH_SQL, &[&ids]).await)?;
    let mut skills: HashMap<Uuid, Vec<JobSkillRecord>> =
        ids.iter().map(|id| (*id, Vec::new())).collect();
    for row in &skill_rows {
        let skill = skill_from_row(row).map_err(|_| AppError::Internal)?;
        skills
            .get_mut(&skill.job_id)
            .ok_or(AppError::Internal)?
            .push(skill);
    }
    if jobs.len() != ids.len() {
        return Err(unavailable());
    }
    let mut aggregates = Vec::with_capacity(ids.len());
    for id in ids {
        let job = jobs.remove(id).ok_or_else(unavailable)?;
        let skills = skills.remove(id).unwrap_or_default();
        aggregates.push(JobAggregate {
            job,
            skills,
            link_ingestion: None,
            text_ingestion: None,
        });
    }
    Ok(aggregates)
}

/// A count as a `bigint` bind (or a bounded length as the `matchedCount`).
fn count_bind(value: usize) -> Result<i64, AppError> {
    i64::try_from(value).map_err(|_| AppError::Internal)
}
