//! SQLite adapters of the job ports.
//!
//! One [`SqliteJobRepository`] implements [`JobRepository`], [`JobLinkRepository`] and
//! [`JobSearchRepository`] over the explicit transaction handle of [`SqliteTransaction`], as the
//! other adapters do. The use cases run their repository calls in the scopes of
//! [`SqliteUnitOfWork`](crate::sqlite::unit_of_work::SqliteUnitOfWork), so this module never opens,
//! begins or commits a transaction.
//!
//! * [`graph`]: the job aggregate (single-job read, text provenance lookup, fingerprint lookup,
//!   paste-first create and the revision-checked update).
//! * [`list`]: the bounded keyset listing with the `created_at` watermark.
//! * [`search`]: the bounded candidate sample and the batch hydration.
//! * [`terms`]: the search postings of a job (field keys and weights of the search index).
//!
//! Every write scope is `BEGIN IMMEDIATE` on the single writer, so no advisory locks or
//! shared reads are needed in the create paths: a create reads, then writes,
//! with no other writer in between. The job source provenance rule (exactly one provenance row of
//! the job's source method) was a deferred constraint trigger; it is a pre-commit check here.
//!
//! # Error policies
//!
//! * Write and read paths of the job repository follow one policy: a constraint violation is a
//!   persistence constraint, every other database failure is "Local database is unavailable" (see
//!   [`details_app_error`]), and a value or aggregate of the wrong shape is the
//!   persistence-constraint envelope too ([`invalid_shape`]).
//! * The search path does not map decoding failures: a row that does not decode is an internal
//!   error there.

pub mod graph;
pub mod list;
pub mod search;
pub mod terms;

use async_trait::async_trait;
use jobniac_core::errors::{AppError, ApplicationError};
use jobniac_core::job_links::service::JobLinkRepository;
use jobniac_core::jobs::{JobAggregate, JobRecord, JobRepository, JobSearchRepository};
use jobniac_core::pagination::{Page, PageRequest, SearchCandidates};
use rusqlite::Connection;
use uuid::Uuid;

use crate::sqlite::error::{SqliteError, details_app_error};
use crate::sqlite::unit_of_work::SqliteTransaction;

/// The repository adapter of the job ports. It holds no state: each call runs on the transaction
/// handle that the use case passes in.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteJobRepository;

#[async_trait]
impl JobRepository for SqliteJobRepository {
    type Tx = SqliteTransaction;

    async fn find_by_text_hash(
        &self,
        tx: &mut SqliteTransaction,
        input_text_hash: &str,
    ) -> Result<Option<JobAggregate>, AppError> {
        graph::find_by_text_hash(tx, input_text_hash)
    }

    async fn get_job(
        &self,
        tx: &mut SqliteTransaction,
        job_id: Uuid,
    ) -> Result<Option<JobAggregate>, AppError> {
        graph::get_job(tx, job_id)
    }

    async fn list_jobs(
        &self,
        tx: &mut SqliteTransaction,
        request: &PageRequest<'_>,
    ) -> Result<Page<JobRecord>, AppError> {
        list::list_jobs(tx, request)
    }

    async fn create_text_job(
        &self,
        tx: &mut SqliteTransaction,
        graph: &JobAggregate,
    ) -> Result<(JobAggregate, bool), AppError> {
        graph::create_text_job(tx, graph)
    }

    async fn find_by_canonical_fingerprint(
        &self,
        tx: &mut SqliteTransaction,
        fingerprint: &str,
    ) -> Result<Option<JobAggregate>, AppError> {
        graph::find_by_canonical_fingerprint(tx, fingerprint)
    }

    async fn update_job_aggregate(
        &self,
        tx: &mut SqliteTransaction,
        graph: &JobAggregate,
        expected_revision: i64,
    ) -> Result<Option<JobAggregate>, AppError> {
        graph::update_job_aggregate(tx, graph, expected_revision)
    }
}

#[async_trait]
impl JobLinkRepository for SqliteJobRepository {
    type Tx = SqliteTransaction;

    async fn find_by_normalized_url(
        &self,
        tx: &mut SqliteTransaction,
        normalized_url: &str,
    ) -> Result<Option<JobAggregate>, AppError> {
        graph::find_by_normalized_url(tx, normalized_url)
    }

    async fn create_link_job(
        &self,
        tx: &mut SqliteTransaction,
        graph: &JobAggregate,
    ) -> Result<(JobAggregate, bool), AppError> {
        graph::create_link_job(tx, graph)
    }
}

#[async_trait]
impl JobSearchRepository for SqliteJobRepository {
    type Tx = SqliteTransaction;

    /// The sample size is fixed by the adapter ([`search::MAX_SEARCH_CANDIDATES`]); the caller's
    /// limit does not bound the sample, because the final scores need the complete skills.
    async fn search_job_candidates(
        &self,
        tx: &mut SqliteTransaction,
        query_tokens: &[String],
        _limit: usize,
    ) -> Result<SearchCandidates<JobAggregate>, AppError> {
        search::search_job_candidates(tx, query_tokens)
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

/// The "Local database is unavailable" envelope, for a read that must find a row the same
/// transaction just wrote.
pub(crate) fn unavailable() -> AppError {
    ApplicationError::database_unavailable().into()
}

/// A database result under the job policy: a constraint violation keeps its class, every other
/// failure is "Local database is unavailable".
pub(crate) fn db<T>(result: rusqlite::Result<T>) -> Result<T, AppError> {
    result.map_err(|error| details_app_error(SqliteError::from(error)))
}

/// Runs `work` on the transaction's connection. A failure of `work` is already an [`AppError`].
pub(crate) fn run<T>(
    tx: &SqliteTransaction,
    work: impl FnOnce(&Connection) -> Result<T, AppError>,
) -> Result<T, AppError> {
    tx.run(|connection| Ok(work(connection)))
        .map_err(details_app_error)?
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
