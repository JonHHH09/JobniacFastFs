//! SQLite adapters of the read and link ports of the generation use cases: the profile graph read of
//! `GenericGenerationService` and the profile resume link replacement.

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::GeneratedReservation;
use jobniac_core::generated_assets::{IntentReservation, ProfileResumeDocument, ReplaceResult};
use jobniac_core::generation::generic::{ProfileGraphPort, ProfileResumeLinkPort};
use jobniac_core::profiles::model::ProfileGraph;
use jobniac_core::profiles::repository::ProfileRepository;
use jobniac_core::transactions::TransactionOptions;
use time::OffsetDateTime;
use uuid::Uuid;

use super::profile_links::SqliteProfileResumeLinks;
use crate::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};

/// The profile graph read over the profile unit of work (read only).
///
/// The profile repository is a parameter because it belongs to the profile adapters; compose it
/// with the SQLite profile repository (`Tx = SqliteTransaction`).
#[derive(Debug, Clone)]
pub struct SqliteProfileGraphs<R> {
    uow: SqliteUnitOfWork,
    repository: R,
}

impl<R> SqliteProfileGraphs<R> {
    /// A graph reader over `uow` that reads through `repository`.
    #[must_use]
    pub const fn new(uow: SqliteUnitOfWork, repository: R) -> Self {
        Self { uow, repository }
    }
}

#[async_trait]
impl<R> ProfileGraphPort for SqliteProfileGraphs<R>
where
    R: ProfileRepository<Tx = SqliteTransaction>,
{
    async fn find_profile(&self, profile_id: Uuid) -> Result<Option<ProfileGraph>, AppError> {
        let read_only = TransactionOptions {
            read_only: true,
            requires_new: false,
        };
        self.uow
            .transaction::<Option<ProfileGraph>, AppError>(read_only, async |tx| {
                self.repository.get_profile(tx, profile_id).await
            })
            .await
    }
}

#[async_trait]
impl ProfileResumeLinkPort for SqliteProfileResumeLinks {
    async fn replace_link(
        &self,
        link: &ProfileResumeDocument,
        binding: &GeneratedReservation,
        expected_revision: i64,
        expected_updated_at: OffsetDateTime,
        claim: Option<&IntentReservation>,
    ) -> Result<ReplaceResult, AppError> {
        self.replace(link, binding, expected_revision, expected_updated_at, claim)
            .await
    }
}
