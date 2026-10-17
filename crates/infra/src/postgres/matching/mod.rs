//! PostgreSQL adapter of the matching port.
//!
//! [`PgMatchingRepository`] implements [`jobniac_core::matching::MatchingRepository`] over
//! [`PgUnitOfWork`]. Each port method is one transaction of the unit of work.
//!
//! * [`store`]: the profile and job reads of a pair, the insert-only report store and the report views.
//! * [`pages`]: the batch page of candidate jobs and the filtered report history.
//! * [`value`]: the failure type of a transaction body, the JSONB column reader and the page builder.
//!
//! # Transactions
//!
//! * `analyze_pair` runs at REPEATABLE READ and read-write. It is the only operation that retries:
//!   a serialization failure (SQLSTATE 40001) starts the transaction again while
//!   [`retries_serialization_failure`] allows it, and the last failure is "Local database is
//!   unavailable". The attempt is set by `SET TRANSACTION`, the first statement of the transaction.
//! * `load_batch` and the reads run in a read-only REPEATABLE READ scope, so their rows share one
//!   snapshot.
//! * `save_report` and the other writes use the server default (READ COMMITTED).
//!
//! # Sessions
//!
//! The pool of [`PgMatchingRepository`] is named `jobniac-matching`.
//! Connections are loopback only and refused under `PGSERVICE`, as the shared pool enforces.

pub mod pages;
pub mod store;
pub mod value;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_core::matching::{
    JobGraph, MatchReport, MatchView, MatchingRepository, PairScorer, ProfileGraph,
    SNAPSHOT_ATTEMPTS, retries_serialization_failure,
};
use jobniac_core::pagination::{Page, PageRequest};
use jobniac_core::transactions::TransactionOptions;
use uuid::Uuid;

use crate::postgres::unit_of_work::{PgTransaction, PgUnitOfWork};
use value::{Failure, unavailable};

/// `application_name` of the matching sessions.
pub const MATCHING_APPLICATION_NAME: &str = "jobniac-matching";

/// Sets the isolation level of the current transaction. It must be the first statement of it.
const REPEATABLE_READ_SQL: &str = "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ";

/// The repository adapter of the matching port. It holds the unit of work, not a session.
#[derive(Debug, Clone)]
pub struct PgMatchingRepository {
    uow: PgUnitOfWork,
}

impl PgMatchingRepository {
    /// A repository over `uow`. Build the unit of work with a pool named
    /// [`MATCHING_APPLICATION_NAME`] so the sessions carry that name.
    pub fn new(uow: PgUnitOfWork) -> Self {
        Self { uow }
    }
}

/// The options of a read-only scope (REPEATABLE READ, read-only).
fn read_only() -> TransactionOptions {
    TransactionOptions {
        read_only: true,
        ..TransactionOptions::default()
    }
}

#[async_trait]
impl MatchingRepository for PgMatchingRepository {
    async fn analyze_pair(
        &self,
        profile_id: Uuid,
        job_id: Uuid,
        scorer: &PairScorer<'_>,
    ) -> Result<MatchReport, AppError> {
        for attempt in 0..SNAPSHOT_ATTEMPTS {
            let outcome = self
                .uow
                .transaction(
                    TransactionOptions::default(),
                    async |tx: &mut PgTransaction| -> Result<MatchReport, Failure> {
                        tx.client().batch_execute(REPEATABLE_READ_SQL).await?;
                        let profile = store::load_profile(tx.client(), profile_id).await?;
                        let job = store::load_job(tx.client(), job_id).await?;
                        let report = scorer(&profile, &job)?;
                        store::save_report(tx.client(), &report).await
                    },
                )
                .await;
            match outcome {
                Ok(report) => return Ok(report),
                Err(Failure::Serialization) if retries_serialization_failure(attempt, true) => {}
                Err(failure) => return Err(AppError::from(failure)),
            }
        }
        Err(unavailable())
    }

    async fn load_batch(
        &self,
        profile_id: Uuid,
        request: &PageRequest<'_>,
    ) -> Result<(ProfileGraph, Page<JobGraph>), AppError> {
        self.uow
            .transaction(
                read_only(),
                async |tx: &mut PgTransaction| -> Result<(ProfileGraph, Page<JobGraph>), Failure> {
                    pages::load_batch(tx.client(), profile_id, request).await
                },
            )
            .await
            .map_err(AppError::from)
    }

    async fn save_report(&self, report: MatchReport) -> Result<MatchReport, AppError> {
        self.uow
            .transaction(
                TransactionOptions::default(),
                async |tx: &mut PgTransaction| -> Result<MatchReport, Failure> {
                    store::save_report(tx.client(), &report).await
                },
            )
            .await
            .map_err(AppError::from)
    }

    async fn get_report(&self, report_id: Uuid) -> Result<Option<MatchView>, AppError> {
        self.uow
            .transaction(
                read_only(),
                async |tx: &mut PgTransaction| -> Result<Option<MatchView>, Failure> {
                    store::get_report(tx.client(), report_id).await
                },
            )
            .await
            .map_err(AppError::from)
    }

    async fn list_reports(
        &self,
        profile_id: Option<Uuid>,
        job_id: Option<Uuid>,
        request: &PageRequest<'_>,
    ) -> Result<Page<MatchView>, AppError> {
        self.uow
            .transaction(
                read_only(),
                async |tx: &mut PgTransaction| -> Result<Page<MatchView>, Failure> {
                    pages::list_reports(tx.client(), profile_id, job_id, request).await
                },
            )
            .await
            .map_err(AppError::from)
    }
}
