//! The divergence-v1 policy: which reasons a review raises against its report, and the disagreement
//! that records them.
//!
//! The thresholds are inclusive and integer-exact. Nothing here changes the report.

use std::collections::{BTreeSet, HashMap};

use time::OffsetDateTime;

use crate::clock::truncate_to_micros;
use crate::matching::{Component, MatchReport};

use super::fingerprint::disagreement_fingerprint;
use super::model::{
    DisagreementStatus, EvidenceDefectCode, MatchDisagreement, MatchReview, POLICY_VERSION, Reason,
};

/// The overall score difference that raises [`Reason::OverallDelta`] (inclusive).
const OVERALL_DELTA_POINTS: i64 = 15;
/// The component difference, as a share of its available points, that raises
/// [`Reason::ComponentDelta`] (inclusive): 40 percent.
const COMPONENT_DELTA_PERCENT: i64 = 40;

/// The reasons of a review against its report, in canonical order.
///
/// Components are compared with the first report component of the same name. Each review component
/// is checked on its own, so a duplicate name in the review is checked against the same baseline.
pub fn divergence_reasons(report: &MatchReport, review: &MatchReview) -> Vec<Reason> {
    let mut reasons = BTreeSet::new();
    if (report.overall_score - review.overall_score).abs() >= OVERALL_DELTA_POINTS {
        reasons.insert(Reason::OverallDelta);
    }
    if report.outcome != review.outcome {
        reasons.insert(Reason::OutcomeMismatch);
    }
    if report.blocker_mismatch != review.blocker_mismatch {
        reasons.insert(Reason::BlockerMismatch);
    }
    let mut baseline: HashMap<Component, i64> = HashMap::new();
    for component in &report.components {
        baseline
            .entry(component.component)
            .or_insert(component.earned_points);
    }
    for component in &review.components {
        if let Some(previous) = baseline.get(&component.component) {
            let delta = (previous - component.earned_points).abs();
            if delta * 100 >= component.available_points * COMPONENT_DELTA_PERCENT {
                reasons.insert(Reason::ComponentDelta);
            }
        }
    }
    if review
        .evidence
        .iter()
        .any(|item| item.outcome_changing_defect)
    {
        reasons.insert(Reason::EvidenceDefect);
    }
    reasons.into_iter().collect()
}

/// The disagreement a review creates against its report, or `None` when no reason applies.
///
/// The defect codes are the distinct facts of the outcome-changing evidence that are defect codes. The
/// new disagreement is `PENDING_ESCALATION`, with no linked issue; `created_at` and `updated_at` are
/// `now` truncated to microseconds.
pub fn create_match_disagreement(
    report: &MatchReport,
    review: &MatchReview,
    now: OffsetDateTime,
) -> Option<MatchDisagreement> {
    let reasons = divergence_reasons(report, review);
    if reasons.is_empty() {
        return None;
    }
    let codes: BTreeSet<EvidenceDefectCode> = review
        .evidence
        .iter()
        .filter(|item| item.outcome_changing_defect)
        .filter_map(|item| EvidenceDefectCode::parse(&item.fact))
        .collect();
    let evidence_defect_codes: Vec<EvidenceDefectCode> = codes.into_iter().collect();
    let id = disagreement_fingerprint(report.id, &reasons, &evidence_defect_codes, POLICY_VERSION);
    let now = truncate_to_micros(now);
    Some(MatchDisagreement {
        id,
        report_id: report.id,
        review_id: review.id,
        policy_version: POLICY_VERSION.to_owned(),
        reasons,
        evidence_defect_codes,
        status: DisagreementStatus::PendingEscalation,
        linear_issue_id: None,
        created_at: now,
        updated_at: now,
    })
}
