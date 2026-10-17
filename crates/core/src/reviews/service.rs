//! The advisory review use cases: submit a review, read one, list the
//! reviews and the disagreements, acknowledge a disagreement and link it to an issue reference.
//!
//! The requests arrive as JSON objects. Each use case reads its fields in a fixed order, so the
//! first failing field and its message are deterministic. Nothing is loaded or written for an
//! invalid request, except that the report is read first (a submission for an
//! unknown report reports the missing report before it validates the review fields).

use std::sync::Arc;

use jobniac_text::java::java_is_blank;
use serde_json::Value;
use uuid::Uuid;

use crate::clock::Clock;
use crate::errors::AppError;
use crate::matching::MatchReport;
use crate::matching::privacy::trim_control_and_space;
use crate::pagination::CursorCodec;

use super::divergence::create_match_disagreement;
use super::model::{
    DisagreementList, DisagreementStatus, MatchDisagreement, MatchReview, ReviewList,
    ReviewSubmission,
};
use super::repository::MatchReviewRepository;
use super::request::{is_absent, optional_id, page_request_for, request_object, required_id};
use super::validation::create_match_review;

const REVIEW_SCOPE: &str = "match-reviews";
const DISAGREEMENT_SCOPE: &str = "match-disagreements";
const REQUIRED_FIELDS_MESSAGE: &str = "overallScore and blockerMismatch are required";
const BLANK_ISSUE_MESSAGE: &str = "linearIssueId must not be blank";

/// The advisory review use cases over a repository, a cursor codec and a clock.
#[derive(Debug)]
pub struct MatchReviewService<R> {
    repository: R,
    codec: CursorCodec,
    clock: Arc<dyn Clock>,
}

impl<R: MatchReviewRepository> MatchReviewService<R> {
    /// A service over `repository`, signing cursors with `codec` and stamping rows with `clock`.
    pub fn new(repository: R, codec: CursorCodec, clock: Arc<dyn Clock>) -> Self {
        Self {
            repository,
            codec,
            clock,
        }
    }

    /// Stores a review of a report and, when the review diverges, its disagreement.
    ///
    /// # Errors
    ///
    /// `DomainValue` for the request object, a missing or malformed `reportId`, a missing overall score
    /// or blocker flag, an unknown report (`match report not found: <id>`), or a review field that fails
    /// validation. The repository error is returned for a failed write.
    pub async fn submit_match_review(&self, request: &Value) -> Result<ReviewSubmission, AppError> {
        let values = request_object(request)?;
        let report_id = required_id(values.get("reportId"), "reportId")?;
        if is_absent(values.get("overallScore")) || is_absent(values.get("blockerMismatch")) {
            return Err(AppError::DomainValue(REQUIRED_FIELDS_MESSAGE.to_owned()));
        }
        let now = self.clock.now();
        let review_factory = |report: &MatchReport| create_match_review(report, values, now);
        let disagreement_factory = |report: &MatchReport, review: &MatchReview| {
            create_match_disagreement(report, review, now)
        };
        self.repository
            .submit_review(report_id, &review_factory, &disagreement_factory)
            .await
    }

    /// One stored review.
    ///
    /// # Errors
    ///
    /// `DomainValue` `reviewId ...` for a missing or malformed `id`, `match review not found: <id>` when
    /// the review does not exist, or a repository error.
    pub async fn get_match_review(&self, request: &Value) -> Result<MatchReview, AppError> {
        let review_id = required_id(request_object(request)?.get("id"), "reviewId")?;
        self.repository
            .get_review(review_id)
            .await?
            .ok_or_else(|| AppError::DomainValue(format!("match review not found: {review_id}")))
    }

    /// One page of reviews, optionally filtered by report.
    ///
    /// # Errors
    ///
    /// `DomainValue` for an invalid `reportId` or `limit`, the cursor error for a malformed or foreign
    /// cursor, or a repository error.
    pub async fn list_match_reviews(&self, request: &Value) -> Result<ReviewList, AppError> {
        let values = request_object(request)?;
        let report_id = optional_id(values.get("reportId"), "reportId")?;
        let page_request =
            page_request_for(&self.codec, values, REVIEW_SCOPE, &report_filter(report_id))?;
        let page = self
            .repository
            .list_reviews(report_id, &page_request)
            .await?;
        Ok(ReviewList {
            reviews: page.items,
            next_cursor: page.next_cursor,
        })
    }

    /// One page of disagreements, optionally filtered by report.
    ///
    /// # Errors
    ///
    /// `DomainValue` for an invalid `reportId` or `limit`, the cursor error for a malformed or foreign
    /// cursor, or a repository error.
    pub async fn list_match_disagreements(
        &self,
        request: &Value,
    ) -> Result<DisagreementList, AppError> {
        let values = request_object(request)?;
        let report_id = optional_id(values.get("reportId"), "reportId")?;
        let page_request = page_request_for(
            &self.codec,
            values,
            DISAGREEMENT_SCOPE,
            &report_filter(report_id),
        )?;
        let page = self
            .repository
            .list_disagreements(report_id, &page_request)
            .await?;
        Ok(DisagreementList {
            disagreements: page.items,
            next_cursor: page.next_cursor,
        })
    }

    /// Marks a disagreement as acknowledged. A linked issue is optional; when given it is stored
    /// (stripped of the Java trim set), and when absent the stored one is kept.
    ///
    /// # Errors
    ///
    /// `DomainValue` for a blank or non-string `linearIssueId`, a missing or malformed `disagreementId`,
    /// or `match disagreement not found: <id>`.
    pub async fn acknowledge_match_disagreement(
        &self,
        request: &Value,
    ) -> Result<MatchDisagreement, AppError> {
        self.change(request, DisagreementStatus::Acknowledged, false)
            .await
    }

    /// Marks a disagreement as linked to an issue reference. The issue is required and is stored as an
    /// opaque reference only; it is never looked up.
    ///
    /// # Errors
    ///
    /// As [`MatchReviewService::acknowledge_match_disagreement`], with the issue required.
    pub async fn link_match_disagreement(
        &self,
        request: &Value,
    ) -> Result<MatchDisagreement, AppError> {
        self.change(request, DisagreementStatus::Linked, true).await
    }

    async fn change(
        &self,
        request: &Value,
        status: DisagreementStatus,
        required_issue: bool,
    ) -> Result<MatchDisagreement, AppError> {
        let values = request_object(request)?;
        let issue = match values.get("linearIssueId") {
            None | Some(Value::Null) => {
                if required_issue {
                    return Err(AppError::DomainValue(BLANK_ISSUE_MESSAGE.to_owned()));
                }
                None
            }
            Some(Value::String(text)) if !java_is_blank(text) => {
                Some(trim_control_and_space(text).to_owned())
            }
            Some(_) => return Err(AppError::DomainValue(BLANK_ISSUE_MESSAGE.to_owned())),
        };
        let identifier = required_id(values.get("disagreementId"), "disagreementId")?;
        let changed = self
            .repository
            .change_disagreement(identifier, status, issue.as_deref(), self.clock.now())
            .await?;
        changed.ok_or_else(|| {
            AppError::DomainValue(format!("match disagreement not found: {identifier}"))
        })
    }
}

/// The identity of a list filter, (`report=<id>` or `report=null`).
fn report_filter(report_id: Option<Uuid>) -> String {
    match report_id {
        None => "report=null".to_owned(),
        Some(identifier) => format!("report={identifier}"),
    }
}
