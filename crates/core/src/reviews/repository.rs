//! Persistence port of the advisory reviews.
//!
//! As in the matching port, each method is one atomic unit and the repository opens its own
//! transaction. The submission is a single transaction: it loads the report, runs the review factory
//! on it, stores the review and then runs the disagreement factory on the stored review. The adapter
//! owns the transactions and the lock order; the factories are pure.

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::errors::AppError;
use crate::matching::MatchReport;
use crate::pagination::{Page, PageRequest};

use super::model::{DisagreementStatus, MatchDisagreement, MatchReview, ReviewSubmission};

/// Builds the review of a submission from the report it assesses. Failures
/// are validation failures of the submitted fields.
pub type ReviewFactory<'a> = dyn Fn(&MatchReport) -> Result<MatchReview, AppError> + Sync + 'a;

/// Builds the disagreement of a stored review, if the review diverges.
pub type DisagreementFactory<'a> =
    dyn Fn(&MatchReport, &MatchReview) -> Option<MatchDisagreement> + Sync + 'a;

/// Storage of the advisory reviews and their disagreements.
///
/// A review is insert-only and keyed by its fingerprint: a resubmitted review returns the stored row
/// with its original `created_at`. A disagreement is keyed by its dedupe key: a resubmission returns
/// the stored row with its current status and linked issue, and its original review. Only the status,
/// the linked issue and `updated_at` of a disagreement change after insert.
#[async_trait]
pub trait MatchReviewRepository: Send + Sync {
    /// Loads the report, runs `review_factory` on it, stores the review and then runs
    /// `disagreement_factory` on the stored review. The report is not found, the factory fails or the
    /// store fails, and nothing is written.
    ///
    /// # Errors
    ///
    /// `DomainValue` `match report not found: <id>` when the report does not exist, the factory's
    /// validation failure, or a repository error.
    async fn submit_review(
        &self,
        report_id: Uuid,
        review_factory: &ReviewFactory<'_>,
        disagreement_factory: &DisagreementFactory<'_>,
    ) -> Result<ReviewSubmission, AppError>;

    /// The stored review with `review_id`, or `None` when it does not exist.
    async fn get_review(&self, review_id: Uuid) -> Result<Option<MatchReview>, AppError>;

    /// One page of reviews, filtered by report when given, newest first.
    async fn list_reviews(
        &self,
        report_id: Option<Uuid>,
        request: &PageRequest<'_>,
    ) -> Result<Page<MatchReview>, AppError>;

    /// One page of disagreements, filtered by report when given, newest first.
    async fn list_disagreements(
        &self,
        report_id: Option<Uuid>,
        request: &PageRequest<'_>,
    ) -> Result<Page<MatchDisagreement>, AppError>;

    /// Sets the status of a disagreement. A missing `issue` keeps the stored one. Returns the changed
    /// row, or `None` when the disagreement does not exist.
    async fn change_disagreement(
        &self,
        disagreement_id: Uuid,
        status: DisagreementStatus,
        issue: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<Option<MatchDisagreement>, AppError>;
}
