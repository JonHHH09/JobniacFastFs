//! PostgreSQL adapter of the advisory review port.
//!
//! [`PgMatchReviewRepository`] implements [`jobniac_core::reviews::MatchReviewRepository`] over
//! [`PgUnitOfWork`]. It reuses the matching repository's error policy, so the sessions
//! are the matching sessions (see [`crate::postgres::matching::MATCHING_APPLICATION_NAME`]) and a
//! database failure is "Local database is unavailable" unless it is an integrity violation.
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

use crate::postgres::matching::value::Failure;
use crate::postgres::unit_of_work::{PgTransaction, PgUnitOfWork};

/// The repository adapter of the review port. It holds the unit of work, not a session.
#[derive(Debug, Clone)]
pub struct PgMatchReviewRepository {
    uow: PgUnitOfWork,
}

impl PgMatchReviewRepository {
    /// A repository over `uow`. Build the unit of work with the matching pool (see the module
    /// documentation).
    pub fn new(uow: PgUnitOfWork) -> Self {
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
impl MatchReviewRepository for PgMatchReviewRepository {
    async fn submit_review(
        &self,
        report_id: Uuid,
        review_factory: &ReviewFactory<'_>,
        disagreement_factory: &DisagreementFactory<'_>,
    ) -> Result<ReviewSubmission, AppError> {
        self.uow
            .transaction(
                TransactionOptions::default(),
                async |tx: &mut PgTransaction| -> Result<ReviewSubmission, Failure> {
                    store::submit(tx.client(), report_id, review_factory, disagreement_factory)
                        .await
                },
            )
            .await
            .map_err(AppError::from)
    }

    async fn get_review(&self, review_id: Uuid) -> Result<Option<MatchReview>, AppError> {
        self.uow
            .transaction(
                read_only(),
                async |tx: &mut PgTransaction| -> Result<Option<MatchReview>, Failure> {
                    store::get_review(tx.client(), review_id).await
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
                async |tx: &mut PgTransaction| -> Result<Page<MatchReview>, Failure> {
                    pages::list_reviews(tx.client(), report_id, request).await
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
                async |tx: &mut PgTransaction| -> Result<Page<MatchDisagreement>, Failure> {
                    pages::list_disagreements(tx.client(), report_id, request).await
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
                async |tx: &mut PgTransaction| -> Result<Option<MatchDisagreement>, Failure> {
                    store::change_disagreement(tx.client(), disagreement_id, status, issue, now)
                        .await
                },
            )
            .await
            .map_err(AppError::from)
    }
}
