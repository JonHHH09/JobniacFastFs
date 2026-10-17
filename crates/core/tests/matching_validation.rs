//! Envelopes of the persisted-record validators.
//!
//! `golden/matching-validation.json` records the `validate_match_component` and
//! `validate_match_evidence` results for the same inputs. The explicit cases are the refusal list for
//! imported evidence.

mod matching_support;

use jobniac_core::errors::AppError;
use jobniac_core::matching::{validate_match_component, validate_match_evidence};
use matching_support::{array, outcome_value, parse};
use serde_json::json;

const VALIDATION: &str = include_str!("golden/matching-validation.json");

#[test]
fn component_and_evidence_envelopes_match_the_golden_vectors() {
    let document = parse(VALIDATION);
    let components = array(&document, "components");
    assert!(components.len() >= 20, "the component vector count drifted");
    for case in components {
        let actual = outcome_value(validate_match_component(&case["input"]));
        assert_eq!(actual, case["result"], "component {}", case["input"]);
    }
    let evidence = array(&document, "evidence");
    assert!(evidence.len() >= 30, "the evidence vector count drifted");
    for case in evidence {
        let actual = outcome_value(validate_match_evidence(&case["input"]));
        assert_eq!(actual, case["result"], "evidence {}", case["input"]);
    }
}

#[test]
fn persisted_evidence_with_sensitive_text_is_refused() {
    let base = |source: &str, fact: &str| {
        json!({
            "component": "TECHNICAL",
            "status": "MATCH",
            "sourceType": source,
            "fact": fact,
            "outcomeChangingDefect": false,
        })
    };
    for fact in [
        "password secret",
        "localhost",
        "127.0.0.1",
        "10.0.0.1",
        "192.168.0.1",
        "172.16.0.1",
        "example.internal",
        "+355 123456789",
        "reasoning trace",
    ] {
        assert!(
            matches!(
                validate_match_evidence(&base("notes", fact)),
                Err(AppError::DomainValue(_))
            ),
            "{fact}"
        );
    }
    for (source, fact) in [
        ("normalized_skill", "secret"),
        ("notes", "email@example.test"),
    ] {
        assert!(
            matches!(
                validate_match_evidence(&base(source, fact)),
                Err(AppError::DomainValue(_))
            ),
            "{source} {fact}"
        );
    }
}

#[test]
fn a_component_score_is_bounded_by_its_weight() {
    let ok = validate_match_component(&json!({
        "component": "HARD_REQUIREMENTS",
        "earnedPoints": 10,
        "availablePoints": 10,
        "status": "MATCH",
    }));
    assert!(ok.is_ok());
    let refused = validate_match_component(&json!({
        "component": "HARD_REQUIREMENTS",
        "earnedPoints": 11,
        "availablePoints": 10,
        "status": "MATCH",
    }));
    assert_eq!(
        refused,
        Err(AppError::DomainValue(
            "component score must be within its configured available points".to_owned()
        ))
    );
}
