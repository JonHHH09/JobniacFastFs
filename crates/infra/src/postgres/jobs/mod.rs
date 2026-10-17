//! PostgreSQL adapters of the job ports.
//!
//! One [`PgJobRepository`] implements [`JobRepository`] and [`JobSearchRepository`] over the
//! explicit transaction handle of [`PgTransaction`], as the profile adapter does. The use cases run
//! their repository calls in the scopes of [`PgUnitOfWork`], so this module never connects, begins
//! or commits a transaction.
//!
//! * [`graph`]: the job aggregate (single-job read, text provenance lookup, fingerprint lookup,
//!   paste-first create with advisory locks, and the revision-checked update).
//! * [`list`]: the bounded keyset listing with the `created_at` watermark.
//! * [`search`]: the bounded candidate sample (recursive CTE) and the batch hydration.
//! * [`terms`]: the search postings of a job (field keys and weights of the search index).
//!
//! The SQL uses `$n` binds, and every relation is schema-qualified (the session pins `search_path`
//! to `pg_catalog`).
//!
//! # Error policies
//!
//! * Write and read paths of the job repository follow one policy: a SQLSTATE
//!   class 23 is a persistence constraint, every other database failure is "Local database is
//!   unavailable" (see [`crate::postgres::profiles::db`]), and a value or aggregate of the wrong
//!   shape is the persistence-constraint envelope too ([`invalid_shape`]).
//! * The search path does not map decoding failures: a row that does not decode is an internal
//!   error there.

pub mod graph;
pub mod list;
pub mod search;
pub mod terms;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_core::job_links::service::JobLinkRepository;
use jobniac_core::jobs::{JobAggregate, JobRecord, JobRepository, JobSearchRepository};
use jobniac_core::pagination::{Page, PageRequest, SearchCandidates};
use tokio_postgres::Row;
use tokio_postgres::types::FromSql;
use uuid::Uuid;

use crate::postgres::unit_of_work::PgTransaction;

/// The repository adapter of the job ports. It holds no state: each call runs on the transaction
/// handle that the use case passes in.
#[derive(Debug, Clone, Copy, Default)]
pub struct PgJobRepository;

#[async_trait]
impl JobRepository for PgJobRepository {
    type Tx = PgTransaction;

    async fn find_by_text_hash(
        &self,
        tx: &mut PgTransaction,
        input_text_hash: &str,
    ) -> Result<Option<JobAggregate>, AppError> {
        graph::find_by_text_hash(tx.client(), input_text_hash).await
    }

    async fn get_job(
        &self,
        tx: &mut PgTransaction,
        job_id: Uuid,
    ) -> Result<Option<JobAggregate>, AppError> {
        graph::get_job(tx.client(), job_id).await
    }

    async fn list_jobs(
        &self,
        tx: &mut PgTransaction,
        request: &PageRequest<'_>,
    ) -> Result<Page<JobRecord>, AppError> {
        list::list_jobs(tx.client(), request).await
    }

    async fn create_text_job(
        &self,
        tx: &mut PgTransaction,
        graph: &JobAggregate,
    ) -> Result<(JobAggregate, bool), AppError> {
        graph::create_text_job(tx.client(), graph).await
    }

    async fn find_by_canonical_fingerprint(
        &self,
        tx: &mut PgTransaction,
        fingerprint: &str,
    ) -> Result<Option<JobAggregate>, AppError> {
        graph::find_by_canonical_fingerprint(tx.client(), fingerprint).await
    }

    async fn update_job_aggregate(
        &self,
        tx: &mut PgTransaction,
        graph: &JobAggregate,
        expected_revision: i64,
    ) -> Result<Option<JobAggregate>, AppError> {
        graph::update_job_aggregate(tx.client(), graph, expected_revision).await
    }
}

#[async_trait]
impl JobLinkRepository for PgJobRepository {
    type Tx = PgTransaction;

    async fn find_by_normalized_url(
        &self,
        tx: &mut PgTransaction,
        normalized_url: &str,
    ) -> Result<Option<JobAggregate>, AppError> {
        graph::find_by_normalized_url(tx.client(), normalized_url).await
    }

    async fn create_link_job(
        &self,
        tx: &mut PgTransaction,
        graph: &JobAggregate,
    ) -> Result<(JobAggregate, bool), AppError> {
        graph::create_link_job(tx.client(), graph).await
    }
}

#[async_trait]
impl JobSearchRepository for PgJobRepository {
    type Tx = PgTransaction;

    /// The sample size is fixed by the adapter ([`search::MAX_SEARCH_CANDIDATES`]); the caller's
    /// limit does not bound the sample, because the final scores need the complete skills.
    async fn search_job_candidates(
        &self,
        tx: &mut PgTransaction,
        query_tokens: &[String],
        _limit: usize,
    ) -> Result<SearchCandidates<JobAggregate>, AppError> {
        search::search_job_candidates(tx.client(), query_tokens).await
    }
}

/// The error of a job value or aggregate that does not have the expected shape. The
/// envelope is the constant persistence-constraint message.
pub(crate) fn invalid_shape() -> AppError {
    AppError::PersistenceConstraint {
        sqlstate: None,
        constraint: None,
    }
}

/// Reads one named column of a job row under the write and read policy: a value that
/// does not decode is [`invalid_shape`].
pub(crate) fn job_col<'a, T>(row: &'a Row, name: &str) -> Result<T, AppError>
where
    T: FromSql<'a>,
{
    row.try_get::<_, T>(name).map_err(|_| invalid_shape())
}

/// A required text: a string with at least one non-whitespace character, returned unchanged.
/// Whitespace also covers the file, group, record and unit separators (U+001C to
/// U+001F), which Rust's `char::is_whitespace` does not.
pub(crate) fn non_blank(value: &str) -> Result<&str, AppError> {
    if value.chars().all(is_text_space) {
        Err(invalid_shape())
    } else {
        Ok(value)
    }
}

fn is_text_space(character: char) -> bool {
    character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
}
