//! Ports of the job use cases.
//!
//! As in [`crate::profiles::repository`], the use cases never see a connection: each repository call
//! receives the transaction handle of a [`UnitOfWork`] scope, and the port uses `async_trait` so that
//! it stays object-safe. The SQLite adapter implements the ports over its own transaction type.
//! The unit of work and the read-only and read-write options are shared with the profile aggregate.

use async_trait::async_trait;
use uuid::Uuid;

use crate::errors::AppError;
use crate::pagination::{Page, PageRequest, SearchCandidates};

use super::model::{JobAggregate, JobRecord};

/// Persistence port for the job aggregate.
#[async_trait]
pub trait JobRepository: Send + Sync {
    /// The transaction handle of the adapter.
    type Tx: Send;

    /// The aggregate whose text provenance has `input_text_hash`, or `None`.
    async fn find_by_text_hash(
        &self,
        tx: &mut Self::Tx,
        input_text_hash: &str,
    ) -> Result<Option<JobAggregate>, AppError>;

    /// The complete aggregate of a job, or `None` when it does not exist.
    async fn get_job(
        &self,
        tx: &mut Self::Tx,
        job_id: Uuid,
    ) -> Result<Option<JobAggregate>, AppError>;

    /// One page of job rows for `request`, in the keyset order of the adapter.
    async fn list_jobs(
        &self,
        tx: &mut Self::Tx,
        request: &PageRequest<'_>,
    ) -> Result<Page<JobRecord>, AppError>;

    /// Writes a new text job. Returns the stored aggregate and `true`, or, when an existing job
    /// already has the same canonical fingerprint, that job and `false` (nothing is written).
    async fn create_text_job(
        &self,
        tx: &mut Self::Tx,
        graph: &JobAggregate,
    ) -> Result<(JobAggregate, bool), AppError>;

    /// The aggregate whose canonical fingerprint is `fingerprint`, or `None`.
    async fn find_by_canonical_fingerprint(
        &self,
        tx: &mut Self::Tx,
        fingerprint: &str,
    ) -> Result<Option<JobAggregate>, AppError>;

    /// Replaces the job row and its skills when the stored revision equals `expected_revision`.
    /// Returns the stored aggregate, or `None` on a revision mismatch so the caller reports a
    /// conflict.
    async fn update_job_aggregate(
        &self,
        tx: &mut Self::Tx,
        graph: &JobAggregate,
        expected_revision: i64,
    ) -> Result<Option<JobAggregate>, AppError>;
}

/// Port for bounded job search.
#[async_trait]
pub trait JobSearchRepository: Send + Sync {
    /// The transaction handle of the adapter.
    type Tx: Send;

    /// The bounded candidate aggregates that match `query_tokens`, hydrated with their skills.
    /// `limit` is the validated caller limit; the adapter bounds the candidate sample itself.
    async fn search_job_candidates(
        &self,
        tx: &mut Self::Tx,
        query_tokens: &[String],
        limit: usize,
    ) -> Result<SearchCandidates<JobAggregate>, AppError>;
}
