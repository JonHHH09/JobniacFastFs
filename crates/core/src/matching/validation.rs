//! Validators for persisted match components and evidence.
//!
//! The values are the JSON records stored with an immutable report. Each validator checks the
//! record's shape, then reconstructs the typed record, so imported history is served under the same
//! rules as freshly scored history. Every failure is a [`AppError::DomainValue`] whose message names
//! the rule and never echoes the input.

use serde_json::{Map, Value};

use crate::errors::AppError;

use super::privacy::{
    has_sensitive_skill_text, has_sensitive_text, is_safe_label, is_safe_skill,
    trim_control_and_space,
};
use super::report::{Component, ComponentStatus, MatchComponent, MatchEvidence};

const COMPONENT_FIELDS: [&str; 4] = ["component", "earnedPoints", "availablePoints", "status"];
const EVIDENCE_FIELDS: [&str; 5] = [
    "component",
    "status",
    "sourceType",
    "fact",
    "outcomeChangingDefect",
];
const INVALID_COMPONENT: &str = "invalid persisted match component";
const POINTS_OUT_OF_RANGE: &str = "component score must be within its configured available points";
const INVALID_EVIDENCE: &str = "invalid persisted match evidence";
const SOURCE_MESSAGE: &str = "sourceType must be a normalized safe label";
const TEXT_MESSAGE: &str = "fact must be normalized privacy-safe text";
/// Largest free-text fact, in code points and in UTF-16 units.
const MAX_FACT_LENGTH: usize = 500;

/// Reconstructs a persisted component record with its configured point bounds.
///
/// `earnedPoints` defaults to 0 when absent. Both point values must be JSON integers (a boolean or a
/// float is refused), `availablePoints` must equal the component's weight and `earnedPoints` must lie
/// between 0 and that weight.
///
/// # Errors
///
/// `DomainValue` with `invalid persisted match component` for an unknown key, component or status,
/// or `component score must be within its configured available points` for a point violation.
pub fn validate_match_component(value: &Value) -> Result<MatchComponent, AppError> {
    let object = record(value, INVALID_COMPONENT, &COMPONENT_FIELDS)?;
    let component = named(object, "component").and_then(Component::parse);
    let status = named(object, "status").and_then(ComponentStatus::parse);
    let (Some(component), Some(status)) = (component, status) else {
        return Err(domain(INVALID_COMPONENT));
    };
    let earned = match object.get("earnedPoints") {
        None => Some(0),
        Some(points) => points.as_i64(),
    };
    let available = object.get("availablePoints").and_then(Value::as_i64);
    let (Some(earned), Some(available)) = (earned, available) else {
        return Err(domain(POINTS_OUT_OF_RANGE));
    };
    if available != component.available_points() || earned < 0 || earned > available {
        return Err(domain(POINTS_OUT_OF_RANGE));
    }
    Ok(MatchComponent {
        component,
        earned_points: earned,
        available_points: available,
        status,
    })
}

/// Reconstructs a persisted evidence record under the source privacy rules.
///
/// `outcomeChangingDefect` defaults to `false` and must be a boolean. `sourceType` must be a safe
/// label, judged on the original text (sensitive words are refused) and on the stripped text (which
/// must be a safe label). The fact is stripped. A `normalized_skill` fact must be a privacy-safe
/// skill; any other fact must be non-empty, at most 500 code points and 500 UTF-16 units, free of
/// line breaks and free of sensitive text. The skill fact is not canonicalized on import.
///
/// # Errors
///
/// `DomainValue` with the message of the failed rule, never the input.
pub fn validate_match_evidence(value: &Value) -> Result<MatchEvidence, AppError> {
    let object = record(value, INVALID_EVIDENCE, &EVIDENCE_FIELDS)?;
    let defect = match object.get("outcomeChangingDefect") {
        None => false,
        Some(flag) => flag.as_bool().ok_or_else(|| domain(INVALID_EVIDENCE))?,
    };
    let component = named(object, "component").and_then(Component::parse);
    let status = named(object, "status").and_then(ComponentStatus::parse);
    let (Some(component), Some(status)) = (component, status) else {
        return Err(domain(INVALID_EVIDENCE));
    };
    let original_source = object
        .get("sourceType")
        .and_then(Value::as_str)
        .ok_or_else(|| domain(SOURCE_MESSAGE))?;
    let source = trim_control_and_space(original_source);
    if !is_safe_label(source) || has_sensitive_text(original_source) {
        return Err(domain(SOURCE_MESSAGE));
    }
    let fact = object
        .get("fact")
        .and_then(Value::as_str)
        .map_or("", trim_control_and_space);
    if source == "normalized_skill" {
        if !is_safe_skill(fact) || has_sensitive_skill_text(fact) {
            return Err(domain(super::privacy::SKILL_MESSAGE));
        }
    } else if !is_safe_free_text(fact) {
        return Err(domain(TEXT_MESSAGE));
    }
    Ok(MatchEvidence {
        component,
        status,
        source_type: source.to_owned(),
        fact: fact.to_owned(),
        outcome_changing_defect: defect,
    })
}

fn is_safe_free_text(fact: &str) -> bool {
    !fact.is_empty()
        && fact.chars().count() <= MAX_FACT_LENGTH
        && fact.encode_utf16().count() <= MAX_FACT_LENGTH
        && !fact.contains(['\n', '\r'])
        && !has_sensitive_text(fact)
}

/// The record as an object whose keys are all in `fields`.
fn record<'a>(
    value: &'a Value,
    message: &str,
    fields: &[&str],
) -> Result<&'a Map<String, Value>, AppError> {
    match value.as_object() {
        Some(object) if object.keys().all(|key| fields.contains(&key.as_str())) => Ok(object),
        _ => Err(domain(message)),
    }
}

fn named<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn domain(message: &str) -> AppError {
    AppError::DomainValue(message.to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn component_defaults_and_bounds() {
        let valid = validate_match_component(&json!({
            "component": "TECHNICAL", "availablePoints": 40, "status": "PARTIAL"
        }));
        assert_eq!(valid.map(|component| component.earned_points), Ok(0));
        let bad_weight = validate_match_component(&json!({
            "component": "DOMAIN", "earnedPoints": 1, "availablePoints": 10, "status": "MATCH"
        }));
        assert_eq!(bad_weight, Err(domain(POINTS_OUT_OF_RANGE)));
        let boolean = validate_match_component(&json!({
            "component": "DOMAIN", "earnedPoints": true, "availablePoints": 15, "status": "MATCH"
        }));
        assert_eq!(boolean, Err(domain(POINTS_OUT_OF_RANGE)));
        let extra = validate_match_component(&json!({
            "component": "DOMAIN", "availablePoints": 15, "status": "MATCH", "note": "x"
        }));
        assert_eq!(extra, Err(domain(INVALID_COMPONENT)));
    }

    #[test]
    fn evidence_defaults_and_source_rules() {
        let evidence = validate_match_evidence(&json!({
            "component": "TECHNICAL", "status": "MATCH", "sourceType": " notes ",
            "fact": "  reviewed skill list  "
        }));
        let evidence = evidence.map(|value| (value.source_type, value.fact));
        assert_eq!(
            evidence,
            Ok(("notes".to_owned(), "reviewed skill list".to_owned()))
        );
        let secret_source = validate_match_evidence(&json!({
            "component": "TECHNICAL", "status": "MATCH", "sourceType": "api-key",
            "fact": "text"
        }));
        assert_eq!(secret_source, Err(domain(SOURCE_MESSAGE)));
        let defect = validate_match_evidence(&json!({
            "component": "TECHNICAL", "status": "MATCH", "sourceType": "notes",
            "fact": "text", "outcomeChangingDefect": "yes"
        }));
        assert_eq!(defect, Err(domain(INVALID_EVIDENCE)));
    }
}
