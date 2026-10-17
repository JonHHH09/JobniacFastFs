//! IPC mirror of the matching and advisory-review results.
//!
//! Field names are camelCase. Revisions and instants are the `format_instant` text, and every
//! other `i64` that may exceed 2^53 crosses as a decimal string. Scores, points and row
//! counts are bounded (scores 0..100, at most 1000 rows) and cross as numbers, as the job DTO does for
//! its counts. Enumerations cross as their wire strings; an unknown outcome has no label in the
//! UI. Projections are capped as the screens show them: 100 components, 1000 evidence
//! rows, 100 reasons and 1000 defect codes. The totals and the truncation flags say how many rows the
//! core holds. Raw inputs, embeddings and provenance never cross IPC.

use jobniac_core::clock::format_instant;
use jobniac_core::matching::{
    BatchAnalysis, MatchComponent, MatchEvidence, MatchReport, MatchView, Outcome, ReportList,
};
use jobniac_core::reviews::{
    DisagreementList, MatchDisagreement, MatchReview, ReviewList, ReviewSubmission,
};
use serde::Serialize;
use specta::Type;

/// Components a report or review shows at most.
pub const MAX_COMPONENTS: usize = 100;
/// Evidence rows a report or review shows at most.
pub const MAX_EVIDENCE: usize = 1000;
/// Reasons a disagreement shows at most.
pub const MAX_REASONS: usize = 100;
/// Defect codes a disagreement shows at most.
pub const MAX_DEFECT_CODES: usize = 1000;

/// A count as a number. A length never exceeds `u32::MAX` in practice; the clamp keeps it total.
fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// A value of the core outside the range its invariant allows. The whole response is refused: a bad
/// score or point is never shown as another number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfRange;

impl std::fmt::Display for OutOfRange {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a core value is outside its range")
    }
}

impl std::error::Error for OutOfRange {}

/// A non-negative point count as a number. A negative value is an invariant break, not a zero.
fn points(value: i64) -> Result<u32, OutOfRange> {
    u32::try_from(value).map_err(|_| OutOfRange)
}

/// A score or confidence as a number. The core keeps these in 0..=100; anything else is refused.
fn score(value: i64) -> Result<u32, OutOfRange> {
    points(value).and_then(|value| {
        if value <= 100 {
            Ok(value)
        } else {
            Err(OutOfRange)
        }
    })
}

/// The wire name of an outcome band.
fn outcome_text(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::StrongMatch => "STRONG_MATCH",
        Outcome::PartialMatch => "PARTIAL_MATCH",
        Outcome::WeakMatch => "WEAK_MATCH",
        Outcome::InsufficientEvidence => "INSUFFICIENT_EVIDENCE",
    }
}

/// One scored component of a report or of a review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchComponentDto {
    /// The component name, such as `TECHNICAL`.
    pub component: String,
    /// Points earned, from 0 to `availablePoints`.
    pub earned_points: u32,
    /// The configured weight of the component.
    pub available_points: u32,
    /// The status name, such as `PARTIAL`.
    pub status: String,
}

impl TryFrom<&MatchComponent> for MatchComponentDto {
    type Error = OutOfRange;

    fn try_from(component: &MatchComponent) -> Result<Self, OutOfRange> {
        Ok(Self {
            component: component.component.as_str().to_owned(),
            earned_points: points(component.earned_points)?,
            available_points: points(component.available_points)?,
            status: component.status.as_str().to_owned(),
        })
    }
}

/// The projected components of a report or a review: the first 100 rows, each validated.
fn project_components(rows: &[MatchComponent]) -> Result<Vec<MatchComponentDto>, OutOfRange> {
    rows.iter()
        .take(MAX_COMPONENTS)
        .map(MatchComponentDto::try_from)
        .collect()
}

/// The projected evidence of a report or a review: the first 1000 rows, each converted.
fn project_evidence(rows: &[MatchEvidence]) -> Vec<MatchEvidenceDto> {
    rows.iter()
        .take(MAX_EVIDENCE)
        .map(MatchEvidenceDto::from)
        .collect()
}

/// One evidence row. The source type is a fixed label (`normalized_skill`, `advisory_defect`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchEvidenceDto {
    /// The component the evidence supports.
    pub component: String,
    /// The status the evidence supports.
    pub status: String,
    /// The source label.
    pub source_type: String,
    /// The fact text (a canonical skill or a fixed sentence).
    pub fact: String,
    /// Whether the evidence changes the outcome.
    pub outcome_changing_defect: bool,
}

impl From<&MatchEvidence> for MatchEvidenceDto {
    fn from(evidence: &MatchEvidence) -> Self {
        Self {
            component: evidence.component.as_str().to_owned(),
            status: evidence.status.as_str().to_owned(),
            source_type: evidence.source_type.clone(),
            fact: evidence.fact.clone(),
            outcome_changing_defect: evidence.outcome_changing_defect,
        }
    }
}

/// An immutable deterministic report. Components and evidence are capped; see
/// [`MatchReportViewDto`] for the totals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchReportDto {
    /// Content-addressed report identifier, lowercase.
    pub id: String,
    /// The profile the report scores.
    pub profile_id: String,
    /// The job the report scores.
    pub job_id: String,
    /// The profile revision the report was computed from (instant text).
    pub profile_revision: String,
    /// The job revision the report was computed from (instant text).
    pub job_revision: String,
    /// The algorithm identifier, `deterministic-v1`.
    pub algorithm_version: String,
    /// The outcome band, as its wire name.
    pub outcome: String,
    /// When the report was computed (instant text).
    pub created_at: String,
    /// Overall score, 0 to 100.
    pub overall_score: u32,
    /// Evidence coverage, 0 to 100.
    pub confidence: u32,
    /// Whether a blocking mismatch was found.
    pub blocker_mismatch: bool,
    /// The components, at most 100.
    pub components: Vec<MatchComponentDto>,
    /// The evidence, at most 1000.
    pub evidence: Vec<MatchEvidenceDto>,
}

impl TryFrom<&MatchReport> for MatchReportDto {
    type Error = OutOfRange;

    fn try_from(report: &MatchReport) -> Result<Self, OutOfRange> {
        Ok(Self {
            id: report.id.to_string(),
            profile_id: report.profile_id.to_string(),
            job_id: report.job_id.to_string(),
            profile_revision: format_instant(report.profile_revision),
            job_revision: format_instant(report.job_revision),
            algorithm_version: report.algorithm_version.clone(),
            outcome: outcome_text(report.outcome).to_owned(),
            created_at: format_instant(report.created_at),
            overall_score: score(report.overall_score)?,
            confidence: score(report.confidence)?,
            blocker_mismatch: report.blocker_mismatch,
            components: project_components(&report.components)?,
            evidence: project_evidence(&report.evidence),
        })
    }
}

/// A report with its staleness and the projection totals (the `{report, stale}` view).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchReportViewDto {
    /// Whether a source revision changed after the report was computed.
    pub stale: bool,
    /// The report.
    pub report: MatchReportDto,
    /// Whether the report has more components than the projection shows.
    pub components_truncated: bool,
    /// The number of components the report holds.
    pub components_total_count: u32,
    /// Whether the report has more evidence rows than the projection shows.
    pub evidence_truncated: bool,
    /// The number of evidence rows the report holds.
    pub evidence_total_count: u32,
}

impl TryFrom<&MatchView> for MatchReportViewDto {
    type Error = OutOfRange;

    fn try_from(view: &MatchView) -> Result<Self, OutOfRange> {
        let components = view.report.components.len();
        let evidence = view.report.evidence.len();
        Ok(Self {
            stale: view.stale,
            report: MatchReportDto::try_from(&view.report)?,
            components_truncated: components > MAX_COMPONENTS,
            components_total_count: count(components),
            evidence_truncated: evidence > MAX_EVIDENCE,
            evidence_total_count: count(evidence),
        })
    }
}

/// A job of a batch page that could not be analyzed. The copy comes from the message id; the
/// service text is never sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchFailureDto {
    /// The profile of the batch.
    pub profile_id: String,
    /// The job that failed.
    pub job_id: String,
}

/// One batch page: the reports that succeeded, the jobs that failed, and the cursor of the next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchBatchPageDto {
    /// The analyzed jobs of the page, in page order.
    pub succeeded: Vec<MatchReportViewDto>,
    /// The jobs of the page that failed, in page order.
    pub failed: Vec<MatchFailureDto>,
    /// The cursor of the next page; null when there is none.
    pub next_cursor: Option<String>,
}

impl TryFrom<&BatchAnalysis> for MatchBatchPageDto {
    type Error = OutOfRange;

    fn try_from(batch: &BatchAnalysis) -> Result<Self, OutOfRange> {
        Ok(Self {
            succeeded: batch
                .succeeded
                .iter()
                .map(MatchReportViewDto::try_from)
                .collect::<Result<_, _>>()?,
            failed: batch
                .failed
                .iter()
                .map(|failure| MatchFailureDto {
                    profile_id: failure.profile_id.clone(),
                    job_id: failure.job_id.clone(),
                })
                .collect(),
            next_cursor: batch.next_cursor.clone(),
        })
    }
}

/// One page of stored reports, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchHistoryPageDto {
    /// The reports of the page.
    pub reports: Vec<MatchReportViewDto>,
    /// The cursor of the next page; null when there is none.
    pub next_cursor: Option<String>,
}

impl TryFrom<&ReportList> for MatchHistoryPageDto {
    type Error = OutOfRange;

    fn try_from(list: &ReportList) -> Result<Self, OutOfRange> {
        Ok(Self {
            reports: list
                .reports
                .iter()
                .map(MatchReportViewDto::try_from)
                .collect::<Result<_, _>>()?,
            next_cursor: list.next_cursor.clone(),
        })
    }
}

/// An advisory review, separate from the report it assesses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ReviewViewDto {
    /// Content-addressed review identifier, lowercase.
    pub id: String,
    /// The report the review assesses.
    pub report_id: String,
    /// The reviewer label.
    pub reviewer: String,
    /// The model label.
    pub model: String,
    /// The review version label.
    pub review_version: String,
    /// The reviewer's outcome band, as its wire name.
    pub outcome: String,
    /// The advisory summary code, as its wire name.
    pub summary: String,
    /// When the review was created (instant text).
    pub created_at: String,
    /// The reviewer's overall score, 0 to 100.
    pub overall_score: u32,
    /// Whether the reviewer found a blocking mismatch.
    pub blocker_mismatch: bool,
    /// The components, at most 100.
    pub components: Vec<MatchComponentDto>,
    /// The evidence, at most 1000.
    pub evidence: Vec<MatchEvidenceDto>,
    /// The number of components the review holds.
    pub components_total_count: u32,
    /// Whether the review has more components than the projection shows.
    pub components_truncated: bool,
    /// The number of evidence rows the review holds.
    pub evidence_total_count: u32,
    /// Whether the review has more evidence rows than the projection shows.
    pub evidence_truncated: bool,
}

impl TryFrom<&MatchReview> for ReviewViewDto {
    type Error = OutOfRange;

    fn try_from(review: &MatchReview) -> Result<Self, OutOfRange> {
        let components = review.components.len();
        let evidence = review.evidence.len();
        Ok(Self {
            id: review.id.to_string(),
            report_id: review.report_id.to_string(),
            reviewer: review.reviewer.clone(),
            model: review.model.clone(),
            review_version: review.review_version.clone(),
            outcome: outcome_text(review.outcome).to_owned(),
            summary: review.summary.as_str().to_owned(),
            created_at: format_instant(review.created_at),
            overall_score: score(review.overall_score)?,
            blocker_mismatch: review.blocker_mismatch,
            components: project_components(&review.components)?,
            evidence: project_evidence(&review.evidence),
            components_total_count: count(components),
            components_truncated: components > MAX_COMPONENTS,
            evidence_total_count: count(evidence),
            evidence_truncated: evidence > MAX_EVIDENCE,
        })
    }
}

/// A divergence between a report and a review, with its status and the codes the screen shows raw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DisagreementViewDto {
    /// The dedupe key of the disagreement.
    pub id: String,
    /// The report the disagreement concerns.
    pub report_id: String,
    /// The review that created the disagreement.
    pub review_id: String,
    /// The divergence policy version, `divergence-v1`.
    pub policy_version: String,
    /// The workflow status, as its wire name.
    pub status: String,
    /// When the disagreement was first created (instant text).
    pub created_at: String,
    /// When the status or link last changed (instant text).
    pub updated_at: String,
    /// The linked issue reference, or null when none was recorded. Opaque, never looked up.
    pub issue_reference: Option<String>,
    /// The reasons, at most 100.
    pub reasons: Vec<String>,
    /// The defect codes, at most 1000.
    pub evidence_defect_codes: Vec<String>,
    /// The number of reasons the disagreement holds.
    pub reasons_total_count: u32,
    /// Whether the disagreement has more reasons than the projection shows.
    pub reasons_truncated: bool,
    /// The number of defect codes the disagreement holds.
    pub defects_total_count: u32,
    /// Whether the disagreement has more defect codes than the projection shows.
    pub defects_truncated: bool,
}

impl From<&MatchDisagreement> for DisagreementViewDto {
    fn from(disagreement: &MatchDisagreement) -> Self {
        let reasons = disagreement.reasons.len();
        let defects = disagreement.evidence_defect_codes.len();
        Self {
            id: disagreement.id.to_string(),
            report_id: disagreement.report_id.to_string(),
            review_id: disagreement.review_id.to_string(),
            policy_version: disagreement.policy_version.clone(),
            status: disagreement.status.as_str().to_owned(),
            created_at: format_instant(disagreement.created_at),
            updated_at: format_instant(disagreement.updated_at),
            issue_reference: disagreement.linear_issue_id.clone(),
            reasons: disagreement
                .reasons
                .iter()
                .take(MAX_REASONS)
                .map(|reason| reason.as_str().to_owned())
                .collect(),
            evidence_defect_codes: disagreement
                .evidence_defect_codes
                .iter()
                .take(MAX_DEFECT_CODES)
                .map(|code| code.as_str().to_owned())
                .collect(),
            reasons_total_count: count(reasons),
            reasons_truncated: reasons > MAX_REASONS,
            defects_total_count: count(defects),
            defects_truncated: defects > MAX_DEFECT_CODES,
        }
    }
}

/// The result of a submission: the stored review and the disagreement it created or reused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSubmitResultDto {
    /// The stored review.
    pub review: ReviewViewDto,
    /// The disagreement, or null when the review agrees with the report.
    pub disagreement: Option<DisagreementViewDto>,
}

impl TryFrom<&ReviewSubmission> for ReviewSubmitResultDto {
    type Error = OutOfRange;

    fn try_from(submission: &ReviewSubmission) -> Result<Self, OutOfRange> {
        Ok(Self {
            review: ReviewViewDto::try_from(&submission.review)?,
            disagreement: submission
                .disagreement
                .as_ref()
                .map(DisagreementViewDto::from),
        })
    }
}

/// One page of reviews, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPageDto {
    /// The reviews of the page.
    pub reviews: Vec<ReviewViewDto>,
    /// The cursor of the next page; null when there is none.
    pub next_cursor: Option<String>,
}

impl TryFrom<&ReviewList> for ReviewPageDto {
    type Error = OutOfRange;

    fn try_from(list: &ReviewList) -> Result<Self, OutOfRange> {
        Ok(Self {
            reviews: list
                .reviews
                .iter()
                .map(ReviewViewDto::try_from)
                .collect::<Result<_, _>>()?,
            next_cursor: list.next_cursor.clone(),
        })
    }
}

/// One page of disagreements, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DisagreementPageDto {
    /// The disagreements of the page.
    pub disagreements: Vec<DisagreementViewDto>,
    /// The cursor of the next page; null when there is none.
    pub next_cursor: Option<String>,
}

impl From<&DisagreementList> for DisagreementPageDto {
    fn from(list: &DisagreementList) -> Self {
        Self {
            disagreements: list
                .disagreements
                .iter()
                .map(DisagreementViewDto::from)
                .collect(),
            next_cursor: list.next_cursor.clone(),
        }
    }
}

/// Which comparison and review screens have their services composed. Both follow the local database; a missing one is the unconfigured state, not a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchCapabilitiesDto {
    /// Whether the matching service is configured.
    pub matching: bool,
    /// Whether the advisory-review service is configured.
    pub reviews: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobniac_core::matching::{Component, ComponentStatus};

    /// Any error a test body can raise: serialization or a refused core value.
    type BoxError = Box<dyn std::error::Error>;
    use jobniac_core::reviews::{DisagreementStatus, EvidenceDefectCode, Reason, Summary};
    use serde_json::json;
    use time::OffsetDateTime;
    use uuid::Uuid;

    // Synthetic identifiers only.
    const REPORT_ID: Uuid = Uuid::from_u128(0xd1);
    const PROFILE_ID: Uuid = Uuid::from_u128(0xd2);
    const JOB_ID: Uuid = Uuid::from_u128(0xd3);
    const REVIEW_ID: Uuid = Uuid::from_u128(0xd4);

    fn component(index: usize) -> MatchComponent {
        MatchComponent {
            component: Component::Technical,
            earned_points: 30,
            available_points: 40,
            status: if index.is_multiple_of(2) {
                ComponentStatus::Partial
            } else {
                ComponentStatus::Match
            },
        }
    }

    fn evidence(index: usize) -> MatchEvidence {
        MatchEvidence {
            component: Component::Technical,
            status: ComponentStatus::Match,
            source_type: "normalized_skill".to_owned(),
            fact: format!("synthetic skill {index}"),
            outcome_changing_defect: false,
        }
    }

    fn report(components: usize, evidence_rows: usize) -> MatchReport {
        let when = OffsetDateTime::UNIX_EPOCH;
        MatchReport {
            id: REPORT_ID,
            profile_id: PROFILE_ID,
            job_id: JOB_ID,
            profile_revision: when,
            job_revision: when,
            algorithm_version: "deterministic-v1".to_owned(),
            overall_score: 62,
            confidence: 80,
            outcome: Outcome::PartialMatch,
            blocker_mismatch: false,
            components: (0..components).map(component).collect(),
            evidence: (0..evidence_rows).map(evidence).collect(),
            created_at: when,
        }
    }

    fn review(components: usize) -> MatchReview {
        MatchReview {
            id: REVIEW_ID,
            report_id: REPORT_ID,
            reviewer: "synthetic-reviewer".to_owned(),
            model: "synthetic-model".to_owned(),
            review_version: "v1".to_owned(),
            overall_score: 55,
            outcome: Outcome::WeakMatch,
            blocker_mismatch: true,
            components: (0..components).map(component).collect(),
            evidence: Vec::new(),
            summary: Summary::ScoreAdjustment,
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn disagreement(reasons: usize) -> MatchDisagreement {
        let when = OffsetDateTime::UNIX_EPOCH;
        MatchDisagreement {
            id: REVIEW_ID,
            report_id: REPORT_ID,
            review_id: REVIEW_ID,
            policy_version: "divergence-v1".to_owned(),
            reasons: vec![Reason::OverallDelta; reasons],
            evidence_defect_codes: vec![EvidenceDefectCode::StructuredEvidenceMissing],
            status: DisagreementStatus::Open,
            linear_issue_id: None,
            created_at: when,
            updated_at: when,
        }
    }

    #[test]
    fn a_report_keeps_revisions_as_instant_text_and_uses_wire_names() -> Result<(), BoxError> {
        let view = MatchView {
            report: report(2, 1),
            stale: true,
        };
        let json = serde_json::to_value(MatchReportViewDto::try_from(&view)?)?;
        assert_eq!(json["stale"], true);
        assert_eq!(json["report"]["profileRevision"], "1970-01-01T00:00:00Z");
        assert_eq!(json["report"]["algorithmVersion"], "deterministic-v1");
        assert_eq!(json["report"]["outcome"], "PARTIAL_MATCH");
        assert_eq!(json["report"]["overallScore"], 62);
        assert_eq!(json["report"]["components"][0]["component"], "TECHNICAL");
        assert_eq!(json["report"]["components"][0]["status"], "PARTIAL");
        assert_eq!(json["report"]["components"][1]["status"], "MATCH");
        assert_eq!(
            json["report"]["evidence"][0]["sourceType"],
            "normalized_skill"
        );
        assert_eq!(
            json["report"]["evidence"][0]["outcomeChangingDefect"],
            false
        );
        assert_eq!(json["componentsTotalCount"], 2);
        assert_eq!(json["componentsTruncated"], false);
        Ok(())
    }

    #[test]
    fn projections_are_capped_and_the_totals_say_so() -> Result<(), BoxError> {
        let view = MatchView {
            report: report(MAX_COMPONENTS + 1, MAX_EVIDENCE + 2),
            stale: false,
        };
        let dto = MatchReportViewDto::try_from(&view)?;
        assert_eq!(dto.report.components.len(), MAX_COMPONENTS);
        assert_eq!(dto.report.evidence.len(), MAX_EVIDENCE);
        assert!(dto.components_truncated);
        assert_eq!(dto.components_total_count, 101);
        assert!(dto.evidence_truncated);
        assert_eq!(dto.evidence_total_count, 1002);
        Ok(())
    }

    #[test]
    fn a_length_exactly_at_the_cap_is_not_truncated() -> Result<(), BoxError> {
        let view = MatchView {
            report: report(MAX_COMPONENTS, 0),
            stale: false,
        };
        let dto = MatchReportViewDto::try_from(&view)?;
        assert!(!dto.components_truncated);
        assert_eq!(dto.components_total_count, 100);
        Ok(())
    }

    #[test]
    fn a_negative_or_out_of_range_score_refuses_the_whole_response() {
        let mut bad = report(1, 0);
        bad.overall_score = -1;
        assert_eq!(
            MatchReportViewDto::try_from(&MatchView {
                report: bad.clone(),
                stale: false,
            })
            .err(),
            Some(OutOfRange)
        );
        bad.overall_score = 101;
        assert!(MatchReportDto::try_from(&bad).is_err());
        // A negative point count is an invariant break as well, never a zero.
        assert_eq!(points(-3), Err(OutOfRange));
        assert_eq!(score(100), Ok(100));
    }

    #[test]
    fn a_review_keeps_the_wire_summary_and_the_outcome_band() -> Result<(), BoxError> {
        let dto = ReviewViewDto::try_from(&review(1))?;
        let json = serde_json::to_value(&dto)?;
        assert_eq!(json["summary"], "score_adjustment");
        assert_eq!(json["outcome"], "WEAK_MATCH");
        assert_eq!(json["blockerMismatch"], true);
        assert_eq!(json["reviewVersion"], "v1");
        assert_eq!(json["componentsTotalCount"], 1);
        assert_eq!(json["evidenceTotalCount"], 0);
        Ok(())
    }

    #[test]
    fn a_disagreement_keeps_the_issue_reference_as_null_until_linked()
    -> Result<(), serde_json::Error> {
        let mut linked = disagreement(1);
        linked.status = DisagreementStatus::Linked;
        linked.linear_issue_id = Some("SYN-1".to_owned());
        let dto = DisagreementViewDto::from(&disagreement(1));
        let json = serde_json::to_value(&dto)?;
        assert_eq!(json["status"], "OPEN");
        assert_eq!(json["issueReference"], serde_json::Value::Null);
        assert_eq!(json["reasons"], json!(["OVERALL_DELTA"]));
        assert_eq!(
            json["evidenceDefectCodes"],
            json!(["structured_evidence_missing"])
        );
        assert_eq!(
            DisagreementViewDto::from(&linked)
                .issue_reference
                .as_deref(),
            Some("SYN-1")
        );
        Ok(())
    }

    #[test]
    fn disagreement_reasons_are_capped_with_totals() {
        let dto = DisagreementViewDto::from(&disagreement(MAX_REASONS + 3));
        assert_eq!(dto.reasons.len(), MAX_REASONS);
        assert!(dto.reasons_truncated);
        assert_eq!(dto.reasons_total_count, 103);
        assert!(!dto.defects_truncated);
        assert_eq!(dto.defects_total_count, 1);
    }

    #[test]
    fn a_submission_without_a_disagreement_serializes_null() -> Result<(), BoxError> {
        let submission = ReviewSubmission {
            review: review(0),
            disagreement: None,
        };
        let json = serde_json::to_value(ReviewSubmitResultDto::try_from(&submission)?)?;
        assert_eq!(json["disagreement"], serde_json::Value::Null);
        assert_eq!(json["review"]["id"], REVIEW_ID.to_string());
        Ok(())
    }

    #[test]
    fn list_pages_keep_the_cursor_and_the_rows() -> Result<(), BoxError> {
        let history = ReportList {
            reports: vec![MatchView {
                report: report(0, 0),
                stale: false,
            }],
            next_cursor: Some("cursor-2".to_owned()),
        };
        let json = serde_json::to_value(MatchHistoryPageDto::try_from(&history)?)?;
        assert_eq!(json["nextCursor"], "cursor-2");
        assert_eq!(json["reports"][0]["stale"], false);
        let empty = ReviewList {
            reviews: Vec::new(),
            next_cursor: None,
        };
        let json = serde_json::to_value(ReviewPageDto::try_from(&empty)?)?;
        assert_eq!(json["nextCursor"], serde_json::Value::Null);
        Ok(())
    }
}
