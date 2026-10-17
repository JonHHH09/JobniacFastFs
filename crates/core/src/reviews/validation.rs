//! Normalisation of a submitted review and import validation of stored reviews and disagreements.
//!
//! Every check runs in a fixed order, so the first failing field and its message are deterministic.
//! Failures are [`AppError::DomainValue`] with messages that name the rule and never echo
//! the input.

use std::collections::BTreeSet;

use jobniac_text::java::java_is_blank;
use serde_json::{Map, Value};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::clock::{parse_instant, truncate_to_micros};
use crate::errors::AppError;
use crate::matching::privacy::{has_sensitive_text, is_safe_label, trim_control_and_space};
use crate::matching::{MatchReport, validate_match_component, validate_match_evidence};

use super::fingerprint::review_fingerprint;
use super::model::{
    DisagreementStatus, EvidenceDefectCode, MatchDisagreement, MatchReview, Reason, Summary,
    parse_outcome,
};
use super::request::required_id;

const GROUNDING_MESSAGE: &str =
    "advisory evidence must reuse a deterministic fact or an allowed defect code";

/// Normalizes a submitted review and stamps it against `report`.
///
/// The review's evidence facts must each be a fact of the report or an allowed defect code. The
/// identifier is the content address of the normalized fields; `created_at` is `now` truncated to
/// microseconds.
///
/// # Errors
///
/// `DomainValue` with the message of the first failing field, or the grounding message.
pub fn create_match_review(
    report: &MatchReport,
    draft: &Map<String, Value>,
    now: OffsetDateTime,
) -> Result<MatchReview, AppError> {
    let mut review = review_fields(draft)?;
    let baseline: BTreeSet<&str> = report
        .evidence
        .iter()
        .map(|item| item.fact.as_str())
        .collect();
    let grounded = review.evidence.iter().all(|item| {
        baseline.contains(item.fact.as_str()) || EvidenceDefectCode::parse(&item.fact).is_some()
    });
    if !grounded {
        return Err(domain(GROUNDING_MESSAGE));
    }
    review.id = review_fingerprint(&review);
    review.created_at = truncate_to_micros(now);
    Ok(review)
}

/// Reconstructs a stored review under the same normalization rules.
///
/// The stored identifier is trusted and not recomputed. Evidence is not checked against a report,
/// because no report is available on import.
///
/// # Errors
///
/// `DomainValue` for a missing or malformed `id`, `reportId`, label, score, outcome, summary,
/// component or evidence record, or a `createdAt` that is not an instant.
pub fn validate_match_review(value: &Map<String, Value>) -> Result<MatchReview, AppError> {
    let id = required_id(value.get("id"), "reviewId")?;
    let mut review = review_fields(value)?;
    review.id = id;
    review.created_at = instant_field(value, "createdAt")?;
    Ok(review)
}

/// Reconstructs a stored disagreement.
///
/// Reasons and defect codes are returned sorted and distinct. The linked issue is kept as given (it is
/// not stripped on import, unlike the service's status change). The policy version is checked as a
/// label only, not against `divergence-v1`.
///
/// # Errors
///
/// `DomainValue` for the first failing field, in a fixed order.
pub fn validate_match_disagreement(
    value: &Map<String, Value>,
) -> Result<MatchDisagreement, AppError> {
    let reasons = disagreement_reasons(value.get("reasons"))?;
    let evidence_defect_codes = disagreement_codes(value.get("evidenceDefectCodes"))?;
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .and_then(DisagreementStatus::parse)
        .ok_or_else(|| domain("invalid match disagreement status"))?;
    let linear_issue_id = match value.get("linearIssueId") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if !java_is_blank(text) => Some(text.clone()),
        Some(_) => return Err(domain("linearIssueId must not be blank")),
    };
    let id = required_id(value.get("id"), "disagreementId")?;
    let report_id = required_id(value.get("reportId"), "reportId")?;
    let review_id = required_id(value.get("reviewId"), "reviewId")?;
    let policy_version = label(value.get("policyVersion"), "policyVersion")?;
    let created_at = instant_field(value, "createdAt")?;
    let updated_at = instant_field(value, "updatedAt")?;
    Ok(MatchDisagreement {
        id,
        report_id,
        review_id,
        policy_version,
        reasons,
        evidence_defect_codes,
        status,
        linear_issue_id,
        created_at,
        updated_at,
    })
}

/// The shared normalization of a submitted or stored review. The identifier
/// and creation instant are placeholders; the caller stamps them.
fn review_fields(values: &Map<String, Value>) -> Result<MatchReview, AppError> {
    let overall_score = values
        .get("overallScore")
        .and_then(Value::as_i64)
        .filter(|score| (0..=100).contains(score))
        .ok_or_else(|| domain("overallScore must be 0-100"))?;
    let blocker_mismatch = values
        .get("blockerMismatch")
        .and_then(Value::as_bool)
        .ok_or_else(|| domain("blockerMismatch must be a boolean"))?;
    let outcome = values
        .get("outcome")
        .and_then(Value::as_str)
        .and_then(parse_outcome)
        .ok_or_else(|| domain("outcome must be a valid match outcome"))?;
    let summary = values
        .get("summary")
        .and_then(Value::as_str)
        .map(trim_control_and_space)
        .and_then(Summary::parse)
        .ok_or_else(|| domain("summary must be an allowed advisory summary code"))?;
    let report_id = required_id(values.get("reportId"), "reportId")?;
    let reviewer = label(values.get("reviewer"), "reviewer")?;
    let model = label(values.get("model"), "model")?;
    let review_version = label(values.get("reviewVersion"), "reviewVersion")?;
    let components = records(
        values.get("components"),
        validate_match_component,
        "components",
    )?;
    let evidence = records(values.get("evidence"), validate_match_evidence, "evidence")?;
    Ok(MatchReview {
        id: Uuid::nil(),
        report_id,
        reviewer,
        model,
        review_version,
        overall_score,
        outcome,
        blocker_mismatch,
        components,
        evidence,
        summary,
        created_at: OffsetDateTime::UNIX_EPOCH,
    })
}

/// A safe label, stripped of the Java trim set. The sensitive-text check runs on the
/// original text.
fn label(value: Option<&Value>, field: &str) -> Result<String, AppError> {
    let message = format!("{field} must be a normalized safe label");
    let text = value
        .and_then(Value::as_str)
        .ok_or_else(|| domain(&message))?;
    let stripped = trim_control_and_space(text);
    if is_safe_label(stripped) && !has_sensitive_text(text) {
        Ok(stripped.to_owned())
    } else {
        Err(domain(&message))
    }
}

/// A list of records, each validated by `validate`. An absent or null list is empty. Every item
/// failure is reported as `invalid match review <field>`, including the `DomainValue` messages of the
/// item validators.
fn records<T>(
    value: Option<&Value>,
    validate: fn(&Value) -> Result<T, AppError>,
    field: &str,
) -> Result<Vec<T>, AppError> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                validate(item).map_err(|_| domain(&format!("invalid match review {field}")))
            })
            .collect(),
        Some(_) => Err(domain(&format!("{field} must be a list"))),
    }
}

fn disagreement_reasons(value: Option<&Value>) -> Result<Vec<Reason>, AppError> {
    let message = "reasons must not be empty or contain unknown reasons";
    let items = match value {
        Some(Value::Array(items)) if !items.is_empty() => items,
        _ => return Err(domain(message)),
    };
    let mut reasons = BTreeSet::new();
    for item in items {
        let reason = item
            .as_str()
            .and_then(Reason::parse)
            .ok_or_else(|| domain(message))?;
        reasons.insert(reason);
    }
    Ok(reasons.into_iter().collect())
}

fn disagreement_codes(value: Option<&Value>) -> Result<Vec<EvidenceDefectCode>, AppError> {
    let message = "evidenceDefectCodes must contain only allowed advisory defect codes";
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut codes = BTreeSet::new();
            for item in items {
                let code = item
                    .as_str()
                    .and_then(EvidenceDefectCode::parse)
                    .ok_or_else(|| domain(message))?;
                codes.insert(code);
            }
            Ok(codes.into_iter().collect())
        }
        Some(_) => Err(domain(message)),
    }
}

fn instant_field(value: &Map<String, Value>, key: &str) -> Result<OffsetDateTime, AppError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .and_then(|text| parse_instant(text).ok())
        .map(truncate_to_micros)
        .ok_or_else(|| domain(&format!("{key} must be a valid instant")))
}

fn domain(message: &str) -> AppError {
    AppError::DomainValue(message.to_owned())
}
