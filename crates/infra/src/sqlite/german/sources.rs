//! SQLite read ports of the German generation: the job aggregate and the personal-details
//! envelope (read only). The profile graph read is shared with the generic generation.

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_core::german::generation::{JobAggregatePort, PersonalDetailsPort};
use jobniac_core::jobs::model::JobAggregate;
use jobniac_core::jobs::repository::JobRepository;
use jobniac_core::profiles::personal_details::PersonalDetailsEnvelope;
use jobniac_core::profiles::repository::PersonalDetailsRepository;
use jobniac_core::transactions::TransactionOptions;
use uuid::Uuid;

use crate::sqlite::jobs::SqliteJobRepository;
use crate::sqlite::profiles::SqliteProfileRepository;
use crate::sqlite::unit_of_work::SqliteUnitOfWork;

/// The German source reads over one unit of work (read-only transactions).
#[derive(Debug, Clone)]
pub struct SqliteGermanSources {
    uow: SqliteUnitOfWork,
}

impl SqliteGermanSources {
    /// Source reads over `uow`.
    #[must_use]
    pub const fn new(uow: SqliteUnitOfWork) -> Self {
        Self { uow }
    }

    fn read_only() -> TransactionOptions {
        TransactionOptions {
            read_only: true,
            requires_new: false,
        }
    }
}

#[async_trait]
impl JobAggregatePort for SqliteGermanSources {
    async fn find_job(&self, job_id: Uuid) -> Result<Option<JobAggregate>, AppError> {
        self.uow
            .transaction::<Option<JobAggregate>, AppError>(Self::read_only(), async |tx| {
                SqliteJobRepository.get_job(tx, job_id).await
            })
            .await
    }
}

#[async_trait]
impl PersonalDetailsPort for SqliteGermanSources {
    async fn find_personal_details(
        &self,
        profile_id: Uuid,
    ) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
        self.uow
            .transaction::<Option<PersonalDetailsEnvelope>, AppError>(
                Self::read_only(),
                async |tx| {
                    SqliteProfileRepository
                        .get_personal_details(tx, profile_id)
                        .await
                },
            )
            .await
    }
}
