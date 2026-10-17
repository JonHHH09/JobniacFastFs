//! Records and closed vocabularies of the advisory review layer.
//!
//! Reviews are separate from the deterministic reports: a review is a reviewer's scored assessment of
//! a report, and a disagreement records how it diverges (policy `divergence-v1`). The enums keep a
//! declaration order where sorting or filtering depends on it, so the derived `Ord` matches the
//! wire ordering.

use serde::{Serialize, Serializer};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::clock::format_instant;
use crate::matching::{MatchComponent, MatchEvidence, Outcome};

/// The divergence policy every disagreement is created under.
pub const POLICY_VERSION: &str = "divergence-v1";

/// The advisory summary of a review. Free text is never stored; only these codes are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Summary {
    /// The reviewer agrees with the report.
    ReviewConsistent,
    /// The reviewer proposes a different score.
    ScoreAdjustment,
    /// The reviewer proposes a different outcome.
    OutcomeAdjustment,
    /// The reviewer found a defect in the evidence.
    EvidenceDefectIdentified,
}

impl Summary {
    /// Every summary code, in declaration order.
    pub const ALL: [Self; 4] = [
        Self::ReviewConsistent,
        Self::ScoreAdjustment,
        Self::OutcomeAdjustment,
        Self::EvidenceDefectIdentified,
    ];

    /// The wire code of the summary.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReviewConsistent => "review_consistent",
            Self::ScoreAdjustment => "score_adjustment",
            Self::OutcomeAdjustment => "outcome_adjustment",
            Self::EvidenceDefectIdentified => "evidence_defect_identified",
        }
    }

    /// The summary named by its wire code, or `None`.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|summary| summary.as_str() == text)
    }
}

/// Advisory defect codes a reviewer may attach as evidence.
///
/// The declaration order is alphabetical by wire code, so the derived `Ord` equals the sorted
/// order of the strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceDefectCode {
    /// The outcome calibration is off.
    OutcomeCalibrationIssue,
    /// The requirement has no provenance.
    RequirementProvenanceMissing,
    /// Structured evidence is wrong.
    StructuredEvidenceIncorrect,
    /// Structured evidence is absent.
    StructuredEvidenceMissing,
}

impl EvidenceDefectCode {
    /// Every defect code, in wire-code order.
    pub const ALL: [Self; 4] = [
        Self::OutcomeCalibrationIssue,
        Self::RequirementProvenanceMissing,
        Self::StructuredEvidenceIncorrect,
        Self::StructuredEvidenceMissing,
    ];

    /// The wire code of the defect.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OutcomeCalibrationIssue => "outcome_calibration_issue",
            Self::RequirementProvenanceMissing => "requirement_provenance_missing",
            Self::StructuredEvidenceIncorrect => "structured_evidence_incorrect",
            Self::StructuredEvidenceMissing => "structured_evidence_missing",
        }
    }

    /// The defect named by its wire code, or `None`.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|code| code.as_str() == text)
    }
}

/// The divergence reasons of policy `divergence-v1`, in the canonical order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Reason {
    /// The overall scores differ by at least 15 points.
    OverallDelta,
    /// The outcome bands differ.
    OutcomeMismatch,
    /// The blocker flags differ.
    BlockerMismatch,
    /// A component differs by at least 40% of its available points.
    ComponentDelta,
    /// The review marks evidence as changing the outcome.
    EvidenceDefect,
}

impl Reason {
    /// Every reason, in canonical order.
    pub const ALL: [Self; 5] = [
        Self::OverallDelta,
        Self::OutcomeMismatch,
        Self::BlockerMismatch,
        Self::ComponentDelta,
        Self::EvidenceDefect,
    ];

    /// The wire name of the reason.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OverallDelta => "OVERALL_DELTA",
            Self::OutcomeMismatch => "OUTCOME_MISMATCH",
            Self::BlockerMismatch => "BLOCKER_MISMATCH",
            Self::ComponentDelta => "COMPONENT_DELTA",
            Self::EvidenceDefect => "EVIDENCE_DEFECT",
        }
    }

    /// The reason named by its wire name, or `None`.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.as_str() == text)
    }
}

/// Status of a disagreement. The service creates `PendingEscalation` only and moves a disagreement to
/// `Acknowledged` or `Linked`; `Open` is never written by the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DisagreementStatus {
    /// Open for review (not written by the service).
    Open,
    /// Waiting for escalation: the status of a new disagreement.
    PendingEscalation,
    /// Acknowledged by a person.
    Acknowledged,
    /// Linked to an external issue reference.
    Linked,
}

impl DisagreementStatus {
    /// The wire name of the status.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::PendingEscalation => "PENDING_ESCALATION",
            Self::Acknowledged => "ACKNOWLEDGED",
            Self::Linked => "LINKED",
        }
    }

    /// The status named by its wire name, or `None`.
    pub fn parse(text: &str) -> Option<Self> {
        [
            Self::Open,
            Self::PendingEscalation,
            Self::Acknowledged,
            Self::Linked,
        ]
        .into_iter()
        .find(|status| status.as_str() == text)
    }
}

/// An immutable advisory review.
///
/// The identifier is content-addressed (see [`crate::reviews::review_fingerprint`]); `created_at` is
/// not part of it. Field order is fixed, so the JSON reads the same.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchReview {
    /// Content-addressed review identifier.
    pub id: Uuid,
    /// The report the review assesses.
    pub report_id: Uuid,
    /// Reviewer label (a normalized safe label).
    pub reviewer: String,
    /// Model label (a normalized safe label).
    pub model: String,
    /// Review version label (a normalized safe label).
    pub review_version: String,
    /// The reviewer's overall score, from 0 to 100.
    pub overall_score: i64,
    /// The reviewer's outcome band.
    #[serde(serialize_with = "serialize_outcome")]
    pub outcome: Outcome,
    /// Whether the reviewer found a blocking mismatch.
    pub blocker_mismatch: bool,
    /// The reviewer's components, in submission order (duplicates are kept).
    pub components: Vec<MatchComponent>,
    /// The reviewer's evidence, in submission order.
    pub evidence: Vec<MatchEvidence>,
    /// The advisory summary code.
    pub summary: Summary,
    /// When the review was created (truncated to microseconds).
    #[serde(serialize_with = "serialize_instant")]
    pub created_at: OffsetDateTime,
}

/// A divergence between a report and a review.
///
/// The identifier is the dedupe key of the reasons and defect codes; it does not depend on the review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchDisagreement {
    /// Dedupe key (see [`crate::reviews::disagreement_fingerprint`]).
    pub id: Uuid,
    /// The report the disagreement concerns.
    pub report_id: Uuid,
    /// The review that created the disagreement.
    pub review_id: Uuid,
    /// The divergence policy version.
    pub policy_version: String,
    /// The reasons, in canonical order.
    pub reasons: Vec<Reason>,
    /// The defect codes of the evidence, sorted and distinct.
    pub evidence_defect_codes: Vec<EvidenceDefectCode>,
    /// The workflow status.
    #[serde(serialize_with = "serialize_status")]
    pub status: DisagreementStatus,
    /// The linked issue reference, when one was given (opaque, never looked up).
    pub linear_issue_id: Option<String>,
    /// When the disagreement was first created.
    #[serde(serialize_with = "serialize_instant")]
    pub created_at: OffsetDateTime,
    /// When the status or link last changed.
    #[serde(serialize_with = "serialize_instant")]
    pub updated_at: OffsetDateTime,
}

/// The result of a review submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSubmission {
    /// The stored review.
    pub review: MatchReview,
    /// The disagreement the review created, or the existing one with the same dedupe key.
    pub disagreement: Option<MatchDisagreement>,
}

/// One page of reviews.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewList {
    /// The reviews of the page, newest first.
    pub reviews: Vec<MatchReview>,
    /// The cursor of the next page, if any.
    pub next_cursor: Option<String>,
}

/// One page of disagreements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisagreementList {
    /// The disagreements of the page, newest first.
    pub disagreements: Vec<MatchDisagreement>,
    /// The cursor of the next page, if any.
    pub next_cursor: Option<String>,
}

/// The wire name of an outcome, as it is stored and compared.
pub(crate) const fn outcome_text(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::StrongMatch => "STRONG_MATCH",
        Outcome::PartialMatch => "PARTIAL_MATCH",
        Outcome::WeakMatch => "WEAK_MATCH",
        Outcome::InsufficientEvidence => "INSUFFICIENT_EVIDENCE",
    }
}

/// The outcome named by its wire name, or `None`.
pub(crate) fn parse_outcome(text: &str) -> Option<Outcome> {
    [
        Outcome::StrongMatch,
        Outcome::PartialMatch,
        Outcome::WeakMatch,
        Outcome::InsufficientEvidence,
    ]
    .into_iter()
    .find(|outcome| outcome_text(*outcome) == text)
}

fn serialize_outcome<S: Serializer>(value: &Outcome, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(outcome_text(*value))
}

fn serialize_status<S: Serializer>(
    value: &DisagreementStatus,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(value.as_str())
}

fn serialize_instant<S: Serializer>(
    value: &OffsetDateTime,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format_instant(*value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_order_is_the_canonical_order() {
        let mut shuffled = vec![
            Reason::EvidenceDefect,
            Reason::OverallDelta,
            Reason::ComponentDelta,
            Reason::BlockerMismatch,
            Reason::OutcomeMismatch,
        ];
        shuffled.sort();
        assert_eq!(shuffled, Reason::ALL.to_vec());
        assert_eq!(
            Reason::parse("COMPONENT_DELTA"),
            Some(Reason::ComponentDelta)
        );
        assert_eq!(Reason::parse("component_delta"), None);
    }

    #[test]
    fn defect_code_order_is_the_sorted_wire_order() {
        let mut names: Vec<&str> = EvidenceDefectCode::ALL
            .iter()
            .map(|code| code.as_str())
            .collect();
        let declared = names.clone();
        names.sort_unstable();
        assert_eq!(names, declared);
        assert!(
            EvidenceDefectCode::ALL
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
    }

    #[test]
    fn wire_names_round_trip() {
        for summary in Summary::ALL {
            assert_eq!(Summary::parse(summary.as_str()), Some(summary));
        }
        assert_eq!(Summary::parse(" score_adjustment"), None);
        for code in EvidenceDefectCode::ALL {
            assert_eq!(EvidenceDefectCode::parse(code.as_str()), Some(code));
        }
        for outcome in [
            Outcome::StrongMatch,
            Outcome::PartialMatch,
            Outcome::WeakMatch,
            Outcome::InsufficientEvidence,
        ] {
            assert_eq!(parse_outcome(outcome_text(outcome)), Some(outcome));
        }
        assert_eq!(
            DisagreementStatus::parse("LINKED"),
            Some(DisagreementStatus::Linked)
        );
        assert_eq!(DisagreementStatus::parse("linked"), None);
    }
}
