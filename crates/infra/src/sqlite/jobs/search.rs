//! Bounded job search.
//!
//! The candidate sample is the single query below, with `?n` binds: one more than
//! [`MAX_SEARCH_CANDIDATES`] jobs are ranked by the adapter (score, title, then identifier). The
//! first [`MAX_SEARCH_CANDIDATES`] of them are batch-hydrated with their skills in the same
//! snapshot of the read-only scope, and the core re-scores the hydrated data. `has_more` is set
//! when more than the sample exists or when a posting list was cut off at
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
use rusqlite::{Connection, params};
use uuid::Uuid;

use super::graph::{
    JOB_SELECT_BATCH_SQL, RawJob, RawSkill, SKILLS_BATCH_SQL, read_job, read_skill,
};
use super::{db, run, unavailable};
use crate::sqlite::codec::{parse_uuid, uuid_array_text};
use crate::sqlite::unit_of_work::SqliteTransaction;

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

/// The candidate sample query, with `?n` binds. The eight binds are, in order: the query tokens
/// (a JSON array); the prefix values (a JSON array); their owners (a JSON array of the same
/// length); the posting bound (token branch); the posting fetch limit (token branch); the posting
/// bound (prefix branch); the posting fetch limit (prefix branch); and the candidate limit,
/// `MAX_SEARCH_CANDIDATES + 1`.
///
/// Each token reads its postings with one bounded range scan of the prefix index, in index order,
/// as a correlated sub-query that returns the postings as a JSON array (SQLite has no `LATERAL`).
/// The array position is the posting's ordinal, so a posting past the bound is flagged `truncated`
/// and left out of the hits, as in the PostgreSQL query.
pub const SEARCH_CANDIDATES_SQL: &str = r#"
WITH query_tokens AS (
    SELECT value AS token FROM json_each(?1)
), query_prefixes AS (
    SELECT prefix.value AS prefix, owner.value AS query_token
    FROM json_each(?2) AS prefix
    JOIN json_each(?3) AS owner ON owner.key = prefix.key
), query_prefix_sets AS (
    SELECT query_token, json_group_array(prefix) AS prefixes
    FROM query_prefixes GROUP BY query_token
), token_samples AS (
    SELECT query.token AS query_token,
           (SELECT json_group_array(json_array(sample.job_id, sample.field_key, sample.weight))
            FROM (
                SELECT posting.job_id, posting.field_key, posting.weight
                FROM job_search_terms AS posting
                WHERE posting.term >= query.token
                  AND posting.term < query.token || '~'
                ORDER BY posting.term, posting.job_id, posting.field_key
                LIMIT ?5
            ) AS sample) AS postings
    FROM query_tokens AS query
), prefix_samples AS (
    SELECT prefix_set.query_token AS query_token,
           (SELECT json_group_array(json_array(sample.job_id, sample.field_key, sample.weight))
            FROM (
                SELECT posting.job_id, posting.field_key, posting.weight
                FROM job_search_terms AS posting
                WHERE posting.term IN (SELECT value FROM json_each(prefix_set.prefixes))
                ORDER BY posting.term, posting.job_id, posting.field_key
                LIMIT ?7
            ) AS sample) AS postings
    FROM query_prefix_sets AS prefix_set
), raw_hits AS (
    SELECT json_extract(hit.value, '$[0]') AS job_id, json_extract(hit.value, '$[1]') AS field_key,
           sample.query_token AS query_token, json_extract(hit.value, '$[2]') AS weight,
           hit.key + 1 > ?4 AS truncated
    FROM token_samples AS sample, json_each(sample.postings) AS hit
    UNION ALL
    SELECT json_extract(hit.value, '$[0]'), json_extract(hit.value, '$[1]'),
           sample.query_token, json_extract(hit.value, '$[2]'),
           hit.key + 1 > ?6
    FROM prefix_samples AS sample, json_each(sample.postings) AS hit
), posting_bounds AS (
    SELECT COALESCE(MAX(truncated), 0) AS posting_truncated FROM raw_hits
), posting_hits AS (
    SELECT DISTINCT job_id, field_key, query_token, weight
    FROM raw_hits WHERE NOT truncated
), scored AS (
    SELECT job_id, SUM(weight) AS score FROM posting_hits GROUP BY job_id
), matching AS (
    SELECT job.id AS id, job.title AS title, scored.score AS score,
           posting_bounds.posting_truncated AS posting_truncated
    FROM scored
    JOIN job_jobs AS job ON job.id = scored.job_id
    CROSS JOIN posting_bounds
    ORDER BY scored.score DESC, job.title, job.id
    LIMIT ?8
)
SELECT id, title, score, posting_truncated FROM matching ORDER BY score DESC, title, id
"#;

/// The posting-bound binds of [`SEARCH_CANDIDATES_SQL`], in bind order.
pub fn candidate_binds() -> Result<(i64, i64, i64), AppError> {
    Ok((
        count_bind(MAX_POSTINGS_PER_TOKEN)?,
        count_bind(POSTING_FETCH_LIMIT)?,
        count_bind(MAX_SEARCH_CANDIDATES + 1)?,
    ))
}

/// Returns at most [`MAX_SEARCH_CANDIDATES`] candidate jobs for normalized query tokens, hydrated
/// with their skills. Empty tokens match nothing and read nothing.
pub(crate) fn search_job_candidates(
    tx: &SqliteTransaction,
    query_tokens: &[String],
) -> Result<SearchCandidates<JobAggregate>, AppError> {
    if query_tokens.is_empty() {
        return SearchCandidates::new(0, false, Vec::new());
    }
    let query_prefixes = prefixes(query_tokens).ok_or(AppError::Internal)?;
    let tokens = serde_json::to_string(query_tokens).map_err(|_| AppError::Internal)?;
    let values = serde_json::to_string(&query_prefixes.values()).map_err(|_| AppError::Internal)?;
    let owners: Vec<&str> = query_prefixes
        .owners()
        .iter()
        .map(|owner| &**owner)
        .collect();
    let owners = serde_json::to_string(&owners).map_err(|_| AppError::Internal)?;
    let (maximum, posting_fetch, candidate_limit) = candidate_binds()?;
    run(tx, |connection| {
        #[cfg(feature = "test-hooks")]
        count_statement();
        let rows =
            db(connection
                .prepare_cached(SEARCH_CANDIDATES_SQL)
                .and_then(|mut statement| {
                    statement
                        .query_map(
                            params![
                                tokens,
                                values,
                                owners,
                                maximum,
                                posting_fetch,
                                maximum,
                                posting_fetch,
                                candidate_limit
                            ],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(3)?)),
                        )?
                        .collect::<rusqlite::Result<Vec<_>>>()
                }))?;
        let Some((_, posting_truncated)) = rows.first() else {
            return SearchCandidates::new(0, false, Vec::new());
        };
        let has_more = rows.len() > MAX_SEARCH_CANDIDATES || *posting_truncated;
        let mut ids = Vec::with_capacity(rows.len().min(MAX_SEARCH_CANDIDATES));
        for (id, _) in rows.iter().take(MAX_SEARCH_CANDIDATES) {
            ids.push(parse_uuid(id).ok_or(AppError::Internal)?);
        }
        let items = hydrate(connection, &ids)?;
        let matched = count_bind(items.len())?;
        SearchCandidates::new(matched, has_more, items)
    })
}

/// Batch-hydrates the jobs and their skills of `ids`, in the order of `ids`.
/// The two reads share the snapshot of the caller. A job that vanished since the sample fails closed
/// as unavailable.
fn hydrate(connection: &Connection, ids: &[Uuid]) -> Result<Vec<JobAggregate>, AppError> {
    let bind = uuid_array_text(ids);
    #[cfg(feature = "test-hooks")]
    count_statement();
    let job_rows: Vec<RawJob> =
        db(connection
            .prepare_cached(JOB_SELECT_BATCH_SQL)
            .and_then(|mut statement| {
                statement
                    .query_map([&bind], read_job)?
                    .collect::<rusqlite::Result<Vec<_>>>()
            }))?;
    let mut jobs: HashMap<Uuid, JobRecord> = HashMap::with_capacity(job_rows.len());
    for row in job_rows {
        let job = row.into_record().map_err(|_| AppError::Internal)?;
        jobs.insert(job.id, job);
    }
    #[cfg(feature = "test-hooks")]
    count_statement();
    let skill_rows: Vec<RawSkill> =
        db(connection
            .prepare_cached(SKILLS_BATCH_SQL)
            .and_then(|mut statement| {
                statement
                    .query_map([&bind], read_skill)?
                    .collect::<rusqlite::Result<Vec<_>>>()
            }))?;
    let mut skills: HashMap<Uuid, Vec<JobSkillRecord>> =
        ids.iter().map(|id| (*id, Vec::new())).collect();
    for row in skill_rows {
        let skill = row.into_record().map_err(|_| AppError::Internal)?;
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

/// A count as an integer bind (or a bounded length as the `matchedCount`).
fn count_bind(value: usize) -> Result<i64, AppError> {
    i64::try_from(value).map_err(|_| AppError::Internal)
}
