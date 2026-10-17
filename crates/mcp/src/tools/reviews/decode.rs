//! Argument decoding of the review tools.
//!
//! The enumerations are closed sets and the flags are strict booleans, so both are checked here,
//! as the SDK checks them before the tool runs; the service repeats the same checks (defence in
//! depth). Every argument is decoded key by key in declaration order, and the request handed to
//! the service keeps only the declared keys. The nested `components` and `evidence` items are
//! normalized the same way.

use serde_json::{Map, Value};

use crate::args::{
    ArgDecoder, ArgErrors, ArgIssue, OPTIONAL_NULLABLE, REQUIRED, REQUIRED_NULLABLE, declared_keys,
    domain_int, list_of, literal_str, strict_bool, strict_string, typed_dict,
};
use crate::registry::Arguments;

/// The `Component` literal set, in declaration order.
pub const COMPONENTS: &[&str] = &[
    "TECHNICAL",
    "EXPERIENCE_SENIORITY",
    "DOMAIN",
    "DELIVERY",
    "HARD_REQUIREMENTS",
];
/// The `Status` literal set, in declaration order.
pub const STATUSES: &[&str] = &["MATCH", "PARTIAL", "MISMATCH", "UNKNOWN"];
/// The `Outcome` literal set, in declaration order.
pub const OUTCOMES: &[&str] = &[
    "STRONG_MATCH",
    "PARTIAL_MATCH",
    "WEAK_MATCH",
    "INSUFFICIENT_EVIDENCE",
];

/// The declared keys of `ReviewRequest`.
const REVIEW_KEYS: &[&str] = &[
    "reportId",
    "reviewer",
    "model",
    "reviewVersion",
    "overallScore",
    "outcome",
    "blockerMismatch",
    "components",
    "evidence",
    "summary",
];
/// The declared keys of `ComponentInput`.
const COMPONENT_KEYS: &[&str] = &["component", "earnedPoints", "availablePoints", "status"];
/// The declared keys of `EvidenceInput`.
const EVIDENCE_KEYS: &[&str] = &[
    "component",
    "status",
    "sourceType",
    "fact",
    "outcomeChangingDefect",
];
/// The declared keys of `IdRequest`.
const ID_KEYS: &[&str] = &["id"];
/// The declared keys of `ReviewFilter`.
const FILTER_KEYS: &[&str] = &["reportId", "limit", "cursor"];
/// The declared keys of `DisagreementRequest`.
const DISAGREEMENT_KEYS: &[&str] = &["disagreementId", "linearIssueId"];

/// Decode the `request` of `submit_match_review` (`ReviewRequest`: all ten keys required, each
/// nullable where its type allows `null`).
pub fn submit_match_review(arguments: &Arguments) -> Result<Value, ArgErrors> {
    let mut decoder = ArgDecoder::new("submit_match_review");
    let request = decoder.field(arguments, "", "request", REQUIRED, review_request);
    decoder.check()?;
    Ok(request.into_value().unwrap_or(Value::Null))
}

/// Decode the `request` of `get_match_review` (`IdRequest`: `id` required and nullable).
pub fn get_match_review(arguments: &Arguments) -> Result<Value, ArgErrors> {
    let mut decoder = ArgDecoder::new("get_match_review");
    let request = decoder.field(arguments, "", "request", REQUIRED, id_request);
    decoder.check()?;
    Ok(request.into_value().unwrap_or(Value::Null))
}

/// Decode the `request` of `list_match_reviews` (`ReviewFilter`: every key optional and nullable).
pub fn list_match_reviews(arguments: &Arguments) -> Result<Value, ArgErrors> {
    review_filter("list_match_reviews", arguments)
}

/// Decode the `request` of `list_match_disagreements` (`ReviewFilter`, as `list_match_reviews`).
pub fn list_match_disagreements(arguments: &Arguments) -> Result<Value, ArgErrors> {
    review_filter("list_match_disagreements", arguments)
}

/// Decode the `request` of `acknowledge_match_disagreement` (`DisagreementRequest`: both keys
/// required and nullable).
pub fn acknowledge_match_disagreement(arguments: &Arguments) -> Result<Value, ArgErrors> {
    disagreement_request("acknowledge_match_disagreement", arguments)
}

/// Decode the `request` of `link_match_disagreement` (`DisagreementRequest`, as acknowledge).
pub fn link_match_disagreement(arguments: &Arguments) -> Result<Value, ArgErrors> {
    disagreement_request("link_match_disagreement", arguments)
}

/// The shared decoding of the two listing tools, which take the same `ReviewFilter`.
fn review_filter(tool: &'static str, arguments: &Arguments) -> Result<Value, ArgErrors> {
    let mut decoder = ArgDecoder::new(tool);
    let request = decoder.field(arguments, "", "request", REQUIRED, filter_request);
    decoder.check()?;
    Ok(request.into_value().unwrap_or(Value::Null))
}

/// The shared decoding of the two disagreement tools, which take the same `DisagreementRequest`.
fn disagreement_request(tool: &'static str, arguments: &Arguments) -> Result<Value, ArgErrors> {
    let mut decoder = ArgDecoder::new(tool);
    let request = decoder.field(arguments, "", "request", REQUIRED, disagreement);
    decoder.check()?;
    Ok(request.into_value().unwrap_or(Value::Null))
}

/// `ReviewRequest`, with its nested item lists normalized.
fn review_request(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "reportId", REQUIRED_NULLABLE, strict_string);
        decoder.field(object, parent, "reviewer", REQUIRED_NULLABLE, strict_string);
        decoder.field(object, parent, "model", REQUIRED_NULLABLE, strict_string);
        decoder.field(
            object,
            parent,
            "reviewVersion",
            REQUIRED_NULLABLE,
            strict_string,
        );
        decoder.field(
            object,
            parent,
            "overallScore",
            REQUIRED_NULLABLE,
            domain_int,
        );
        decoder.field(object, parent, "outcome", REQUIRED_NULLABLE, outcome);
        decoder.field(
            object,
            parent,
            "blockerMismatch",
            REQUIRED_NULLABLE,
            strict_bool,
        );
        decoder.field(
            object,
            parent,
            "components",
            REQUIRED_NULLABLE,
            |location, value, issues| list_of(location, value, issues, component_input),
        );
        decoder.field(
            object,
            parent,
            "evidence",
            REQUIRED_NULLABLE,
            |location, value, issues| list_of(location, value, issues, evidence_input),
        );
        decoder.field(object, parent, "summary", REQUIRED_NULLABLE, strict_string);
    })?;
    Some(normalized_review(value))
}

/// `IdRequest`.
fn id_request(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "id", REQUIRED_NULLABLE, strict_string);
    })?;
    Some(declared(value, ID_KEYS))
}

/// `ReviewFilter`: every key is optional and nullable.
fn filter_request(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "reportId", OPTIONAL_NULLABLE, strict_string);
        decoder.field(object, parent, "limit", OPTIONAL_NULLABLE, domain_int);
        decoder.field(object, parent, "cursor", OPTIONAL_NULLABLE, strict_string);
    })?;
    Some(declared(value, FILTER_KEYS))
}

/// `DisagreementRequest`: both keys are required and nullable.
fn disagreement(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Value> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(
            object,
            parent,
            "disagreementId",
            REQUIRED_NULLABLE,
            strict_string,
        );
        decoder.field(
            object,
            parent,
            "linearIssueId",
            REQUIRED_NULLABLE,
            strict_string,
        );
    })?;
    Some(declared(value, DISAGREEMENT_KEYS))
}

/// `ComponentInput`: every key is required and nullable where its type allows it.
fn component_input(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "component", REQUIRED_NULLABLE, component);
        decoder.field(
            object,
            parent,
            "earnedPoints",
            REQUIRED_NULLABLE,
            domain_int,
        );
        decoder.field(
            object,
            parent,
            "availablePoints",
            REQUIRED_NULLABLE,
            domain_int,
        );
        decoder.field(object, parent, "status", REQUIRED_NULLABLE, status);
    })
}

/// `EvidenceInput`: every key is required and nullable where its type allows it.
fn evidence_input(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "component", REQUIRED_NULLABLE, component);
        decoder.field(object, parent, "status", REQUIRED_NULLABLE, status);
        decoder.field(
            object,
            parent,
            "sourceType",
            REQUIRED_NULLABLE,
            strict_string,
        );
        decoder.field(object, parent, "fact", REQUIRED_NULLABLE, strict_string);
        decoder.field(
            object,
            parent,
            "outcomeChangingDefect",
            REQUIRED_NULLABLE,
            strict_bool,
        );
    })
}

/// A `Component` value.
fn component(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    literal_str(location, value, issues, COMPONENTS).map(|_| ())
}

/// A `Status` value.
fn status(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    literal_str(location, value, issues, STATUSES).map(|_| ())
}

/// An `Outcome` value.
fn outcome(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    literal_str(location, value, issues, OUTCOMES).map(|_| ())
}

/// The declared keys of a `TypedDict` argument that has passed its shape check.
fn declared(value: &Value, keys: &[&str]) -> Value {
    value.as_object().map_or(Value::Null, |object| {
        Value::Object(declared_keys(object, keys))
    })
}

/// The review request with its declared keys only; `components` and `evidence` items keep only
/// their declared keys too. Only called once the shape check has passed.
fn normalized_review(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return Value::Null;
    };
    let mut picked = declared_keys(object, REVIEW_KEYS);
    replace_items(&mut picked, "components", COMPONENT_KEYS);
    replace_items(&mut picked, "evidence", EVIDENCE_KEYS);
    Value::Object(picked)
}

/// Replace each item of the list `key` with its declared keys, when the list is present.
fn replace_items(picked: &mut Map<String, Value>, key: &str, keys: &[&str]) {
    let items = match picked.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_object().map_or_else(
                    || item.clone(),
                    |object| Value::Object(declared_keys(object, keys)),
                )
            })
            .collect::<Vec<Value>>(),
        _ => return,
    };
    picked.insert(key.to_owned(), Value::Array(items));
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::args::IssueKind;

    fn arguments(value: Value) -> Arguments {
        match value {
            Value::Object(map) => map,
            _ => Map::new(),
        }
    }

    fn kinds(errors: &ArgErrors) -> Vec<(String, IssueKind)> {
        errors
            .issues()
            .iter()
            .map(|issue| (issue.location().to_owned(), issue.kind()))
            .collect()
    }

    fn full_review() -> Value {
        json!({
            "reportId": "00000000-0000-4000-8000-0000000000ee",
            "reviewer": "synthetic-reviewer",
            "model": null,
            "reviewVersion": "v1",
            "overallScore": 80,
            "outcome": "PARTIAL_MATCH",
            "blockerMismatch": false,
            "components": [
                {"component": "TECHNICAL", "earnedPoints": 10, "availablePoints": 20,
                 "status": "PARTIAL", "extra": "dropped"}
            ],
            "evidence": [
                {"component": null, "status": "MATCH", "sourceType": "skill",
                 "fact": "Rust", "outcomeChangingDefect": null}
            ],
            "summary": null,
            "unknown": 1
        })
    }

    #[test]
    fn a_full_review_is_admitted_and_normalized() {
        let decoded = submit_match_review(&arguments(json!({ "request": full_review() })));
        let Ok(request) = decoded else {
            panic!("a complete review must be admitted");
        };
        assert!(request.get("unknown").is_none());
        assert!(
            request["components"][0].get("extra").is_none(),
            "item keys outside the TypedDict are dropped"
        );
        assert_eq!(request["components"][0]["component"], "TECHNICAL");
        assert_eq!(request["evidence"][0]["component"], Value::Null);
        assert_eq!(request["summary"], Value::Null);
    }

    #[test]
    fn every_key_of_the_review_is_required() {
        let rejected = submit_match_review(&arguments(json!({
            "request": {"reportId": null}
        })));
        let Err(errors) = rejected else {
            panic!("a review without its keys must be rejected");
        };
        assert_eq!(errors.issues().len(), 9);
        assert!(
            errors
                .issues()
                .iter()
                .all(|issue| issue.kind() == IssueKind::Missing)
        );
    }

    #[test]
    fn enumerations_and_strict_flags_are_checked_in_declaration_order() {
        let mut review = full_review();
        review["outcome"] = json!("WEAK");
        review["blockerMismatch"] = json!(0);
        review["components"][0]["component"] = json!("SALARY");
        review["evidence"][0]["status"] = json!(1);
        let rejected = submit_match_review(&arguments(json!({ "request": review })));
        let Err(errors) = rejected else {
            panic!("bad enumerations and flags must be rejected");
        };
        assert_eq!(
            kinds(&errors),
            vec![
                (
                    "request.outcome".to_owned(),
                    IssueKind::LiteralError(OUTCOMES)
                ),
                ("request.blockerMismatch".to_owned(), IssueKind::BoolType),
                (
                    "request.components.0.component".to_owned(),
                    IssueKind::LiteralError(COMPONENTS)
                ),
                (
                    "request.evidence.0.status".to_owned(),
                    IssueKind::LiteralError(STATUSES)
                ),
            ]
        );
    }

    #[test]
    fn disagreement_and_identifier_requests_keep_declared_keys() {
        assert_eq!(
            acknowledge_match_disagreement(&arguments(json!({
                "request": {"disagreementId": "d", "linearIssueId": null, "x": 1}
            })))
            .ok(),
            Some(json!({"disagreementId": "d", "linearIssueId": null}))
        );
        assert!(
            link_match_disagreement(&arguments(json!({"request": {"disagreementId": null}})))
                .is_err()
        );
        assert!(get_match_review(&arguments(json!({"request": {"id": null}}))).is_ok());
        assert!(list_match_reviews(&arguments(json!({"request": {}}))).is_ok());
        assert!(list_match_disagreements(&arguments(json!({"request": null}))).is_err());
    }
}
