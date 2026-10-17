//! PostgreSQL adapter of the trusted Arbeitnow import.
//!
//! The SQL is the existing job-link SQL of [`crate::postgres::jobs::graph`]; this adapter only owns
//! the transaction of each call, so a lookup or an import commits or rolls back as one unit: a
//! failed provenance insert leaves no job, skill or provenance row behind.

use async_trait::async_trait;
use jobniac_core::arbeitnow::import::TrustedImportRepository;
use jobniac_core::errors::{AppError, ApplicationError, error_envelope};
use jobniac_core::jobs::JobAggregate;
use jobniac_core::transactions::TransactionOptions;

use crate::postgres::jobs::graph;
use crate::postgres::unit_of_work::PgUnitOfWork;

/// The import repository over a unit of work. Cheap to clone; clones share the pool.
#[derive(Clone)]
pub struct PostgresArbeitnowImportRepository {
    uow: PgUnitOfWork,
}

impl std::fmt::Debug for PostgresArbeitnowImportRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresArbeitnowImportRepository")
            .finish_non_exhaustive()
    }
}

impl PostgresArbeitnowImportRepository {
    /// A repository whose calls each run in their own transaction of `uow`.
    #[must_use]
    pub fn new(uow: PgUnitOfWork) -> Self {
        Self { uow }
    }
}

/// The caller-safe application error of a persistence failure: the envelope of the failure
/// (database unavailable, persistence constraint, internal), never SQL or value text.
fn application_error(error: &AppError) -> ApplicationError {
    let envelope = error_envelope(error);
    ApplicationError::new(envelope.code, Some(&envelope.message), envelope.details)
}

#[async_trait]
impl TrustedImportRepository for PostgresArbeitnowImportRepository {
    async fn find_by_normalized_url(
        &self,
        url: &str,
    ) -> Result<Option<JobAggregate>, ApplicationError> {
        self.uow
            .transaction::<_, AppError>(TransactionOptions::default(), async |tx| {
                graph::find_by_normalized_url(tx.client(), url).await
            })
            .await
            .map_err(|error| application_error(&error))
    }

    async fn create_link_job(
        &self,
        graph: JobAggregate,
    ) -> Result<(JobAggregate, bool), ApplicationError> {
        self.uow
            .transaction::<_, AppError>(TransactionOptions::default(), async |tx| {
                graph::create_link_job(tx.client(), &graph).await
            })
            .await
            .map_err(|error| application_error(&error))
    }
}
