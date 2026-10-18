//! SQLite adapter of the matching port.
//!
//! [`SqliteMatchingRepository`] implements [`jobniac_core::matching::MatchingRepository`] over
//! [`SqliteUnitOfWork`]. Each port method is one transaction of the unit of work.
//!
//! * [`store`]: the profile and job reads of a pair, the insert-only report store and the report views.
//! * [`pages`]: the batch page of candidate jobs and the filtered report history.
//! * [`value`]: the failure type of a transaction body, the JSON column reader and the page builder.
//!
//! # Transactions
//!
//! * `analyze_pair` is one read-write scope. A write scope is `BEGIN IMMEDIATE` on the single
//!   writer, so the profile, the job and the report insert are read and written from one state,
//!   and there is no serialization failure to retry (the PostgreSQL adapter retried 40001 up to
//!   [`jobniac_core::matching::SNAPSHOT_ATTEMPTS`] times). A busy lock is retried by the unit of work.
//! * `load_batch` and the reads run in a read-only scope, so their rows share one WAL snapshot.
//! * `save_report` and the other writes run in a read-write scope.

pub mod pages;
pub mod store;
pub mod value;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_core::matching::{
    JobGraph, MatchReport, MatchView, MatchingRepository, PairScorer, ProfileGraph,
};
use jobniac_core::pagination::{Page, PageRequest};
use jobniac_core::transactions::TransactionOptions;
use uuid::Uuid;

use crate::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};
use value::{Failure, run};

/// The repository adapter of the matching port. It holds the unit of work, not a connection.
#[derive(Debug, Clone)]
pub struct SqliteMatchingRepository {
    uow: SqliteUnitOfWork,
}

impl SqliteMatchingRepository {
    /// A repository over `uow`.
    pub fn new(uow: SqliteUnitOfWork) -> Self {
        Self { uow }
    }
}

/// The options of a read-only scope.
fn read_only() -> TransactionOptions {
    TransactionOptions {
        read_only: true,
        ..TransactionOptions::default()
    }
}

#[async_trait]
impl MatchingRepository for SqliteMatchingRepository {
    async fn analyze_pair(
        &self,
        profile_id: Uuid,
        job_id: Uuid,
        scorer: &PairScorer<'_>,
    ) -> Result<MatchReport, AppError> {
        self.uow
            .transaction(
                TransactionOptions::default(),
                async |tx: &mut SqliteTransaction| -> Result<MatchReport, Failure> {
                    let (profile, job) = run(tx, |connection| {
                        let profile = store::load_profile(connection, profile_id)?;
                        let job = store::load_job(connection, job_id)?;
                        Ok((profile, job))
                    })?;
                    let report = scorer(&profile, &job)?;
                    store::save_report_in(tx, &report)
                },
            )
            .await
            .map_err(AppError::from)
    }

    async fn load_batch(
        &self,
        profile_id: Uuid,
        request: &PageRequest<'_>,
    ) -> Result<(ProfileGraph, Page<JobGraph>), AppError> {
        self.uow
            .transaction(
                read_only(),
                async |tx: &mut SqliteTransaction| -> Result<(ProfileGraph, Page<JobGraph>), Failure> {
                    pages::load_batch(tx, profile_id, request)
                },
            )
            .await
            .map_err(AppError::from)
    }

    async fn save_report(&self, report: MatchReport) -> Result<MatchReport, AppError> {
        self.uow
            .transaction(
                TransactionOptions::default(),
                async |tx: &mut SqliteTransaction| -> Result<MatchReport, Failure> {
                    store::save_report_in(tx, &report)
                },
            )
            .await
            .map_err(AppError::from)
    }

    async fn get_report(&self, report_id: Uuid) -> Result<Option<MatchView>, AppError> {
        self.uow
            .transaction(
                read_only(),
                async |tx: &mut SqliteTransaction| -> Result<Option<MatchView>, Failure> {
                    store::get_report(tx, report_id)
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
                async |tx: &mut SqliteTransaction| -> Result<Page<MatchView>, Failure> {
                    pages::list_reports(tx, profile_id, job_id, request)
                },
            )
            .await
            .map_err(AppError::from)
    }
}
