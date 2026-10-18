//! SQLite adapter of the advisory review port.
//!
//! [`SqliteMatchReviewRepository`] implements [`jobniac_core::reviews::MatchReviewRepository`] over
//! [`SqliteUnitOfWork`]. It reuses the matching repository's error policy: a database failure is
//! "Local database is unavailable" unless it is an integrity violation.
//!
//! * [`store`]: the submission transaction, the single reads and the status change.
//! * [`pages`]: the keyset history of reviews and disagreements.
//!
//! Only the disagreement's status, linked issue and `updated_at` change after insert. Reviews are
//! insert-only (the database rejects an update).

pub mod pages;
pub mod store;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_core::pagination::{Page, PageRequest};
use jobniac_core::reviews::{
    DisagreementFactory, DisagreementStatus, MatchDisagreement, MatchReview, MatchReviewRepository,
    ReviewFactory, ReviewSubmission,
};
use jobniac_core::transactions::TransactionOptions;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::sqlite::matching::value::Failure;
use crate::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};

/// The repository adapter of the review port. It holds the unit of work, not a connection.
#[derive(Debug, Clone)]
pub struct SqliteMatchReviewRepository {
    uow: SqliteUnitOfWork,
}

impl SqliteMatchReviewRepository {
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
impl MatchReviewRepository for SqliteMatchReviewRepository {
    async fn submit_review(
        &self,
        report_id: Uuid,
        review_factory: &ReviewFactory<'_>,
        disagreement_factory: &DisagreementFactory<'_>,
    ) -> Result<ReviewSubmission, AppError> {
        self.uow
            .transaction(
                TransactionOptions::default(),
                async |tx: &mut SqliteTransaction| -> Result<ReviewSubmission, Failure> {
                    store::submit(tx, report_id, review_factory, disagreement_factory)
                },
            )
            .await
            .map_err(AppError::from)
    }

    async fn get_review(&self, review_id: Uuid) -> Result<Option<MatchReview>, AppError> {
        self.uow
            .transaction(
                read_only(),
                async |tx: &mut SqliteTransaction| -> Result<Option<MatchReview>, Failure> {
                    store::get_review(tx, review_id)
                },
            )
            .await
            .map_err(AppError::from)
    }

    async fn list_reviews(
        &self,
        report_id: Option<Uuid>,
        request: &PageRequest<'_>,
    ) -> Result<Page<MatchReview>, AppError> {
        self.uow
            .transaction(
                read_only(),
                async |tx: &mut SqliteTransaction| -> Result<Page<MatchReview>, Failure> {
                    pages::list_reviews(tx, report_id, request)
                },
            )
            .await
            .map_err(AppError::from)
    }

    async fn list_disagreements(
        &self,
        report_id: Option<Uuid>,
        request: &PageRequest<'_>,
    ) -> Result<Page<MatchDisagreement>, AppError> {
        self.uow
            .transaction(
                read_only(),
                async |tx: &mut SqliteTransaction| -> Result<Page<MatchDisagreement>, Failure> {
                    pages::list_disagreements(tx, report_id, request)
                },
            )
            .await
            .map_err(AppError::from)
    }

    async fn change_disagreement(
        &self,
        disagreement_id: Uuid,
        status: DisagreementStatus,
        issue: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<Option<MatchDisagreement>, AppError> {
        self.uow
            .transaction(
                TransactionOptions::default(),
                async |tx: &mut SqliteTransaction| -> Result<Option<MatchDisagreement>, Failure> {
                    store::change_disagreement(tx, disagreement_id, status, issue, now)
                },
            )
            .await
            .map_err(AppError::from)
    }
}
