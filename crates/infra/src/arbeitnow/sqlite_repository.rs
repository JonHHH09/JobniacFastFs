//! SQLite adapter of the trusted Arbeitnow import.
//!
//! The SQL is the existing job-link SQL of [`crate::sqlite::jobs`]; this adapter only owns the
//! transaction of each call, so a lookup or an import commits or rolls back as one unit: a failed
//! provenance insert leaves no job, skill or provenance row behind.

use async_trait::async_trait;
use jobniac_core::arbeitnow::import::TrustedImportRepository;
use jobniac_core::errors::{AppError, ApplicationError, error_envelope};
use jobniac_core::job_links::service::JobLinkRepository;
use jobniac_core::jobs::JobAggregate;
use jobniac_core::transactions::TransactionOptions;

use crate::sqlite::jobs::SqliteJobRepository;
use crate::sqlite::unit_of_work::SqliteUnitOfWork;

/// The import repository over a unit of work. Cheap to clone; clones share the pool.
#[derive(Clone)]
pub struct SqliteArbeitnowImportRepository {
    uow: SqliteUnitOfWork,
}

impl std::fmt::Debug for SqliteArbeitnowImportRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteArbeitnowImportRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteArbeitnowImportRepository {
    /// A repository whose calls each run in their own transaction of `uow`.
    #[must_use]
    pub fn new(uow: SqliteUnitOfWork) -> Self {
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
impl TrustedImportRepository for SqliteArbeitnowImportRepository {
    async fn find_by_normalized_url(
        &self,
        url: &str,
    ) -> Result<Option<JobAggregate>, ApplicationError> {
        self.uow
            .transaction::<_, AppError>(TransactionOptions::default(), async |tx| {
                SqliteJobRepository.find_by_normalized_url(tx, url).await
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
                SqliteJobRepository.create_link_job(tx, &graph).await
            })
            .await
            .map_err(|error| application_error(&error))
    }
}
