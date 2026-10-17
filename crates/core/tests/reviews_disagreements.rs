//! Disagreement creation, dedupe identifiers and import validation.
//!
//! The vectors are the `create_match_disagreement` records for reviews against synthetic reports, the
//! `disagreement_fingerprint` identifiers for reason and code orderings, duplicates and policy
//! versions, and the `validate_match_disagreement` envelopes of stored records.

mod reviews_support;

use jobniac_core::reviews::{
    EvidenceDefectCode, POLICY_VERSION, Reason, create_match_disagreement, create_match_review,
    disagreement_fingerprint, validate_match_disagreement,
};
use reviews_support::{
    array, now_of, object, outcome_json, parse, report_id_of, reports_of, text, uuid,
};

const DISAGREEMENTS: &str = include_str!("golden/reviews-disagreements.json");

#[test]
fn disagreements_equal_the_golden_records() {
    let document = parse(DISAGREEMENTS);
    let now = now_of(&document);
    let reports = reports_of(&document);
    let cases = array(&document, "creations");
    assert_eq!(
        cases.len(),
        9,
        "the disagreement creation vector count drifted"
    );
    for case in cases {
        let letter = text(case, "report");
        let report = match reports
            .iter()
            .find(|report| report.id == report_id_of(letter))
        {
            Some(report) => report,
            None => panic!("vector report {letter} is missing"),
        };
        let review = match create_match_review(report, object(&case["draft"]), now) {
            Ok(review) => review,
            Err(error) => panic!("{}: vector draft is refused: {error}", text(case, "name")),
        };
        assert_eq!(
            review.id.to_string(),
            text(case, "reviewId"),
            "{}",
            text(case, "name")
        );
        let disagreement = create_match_disagreement(report, &review, now);
        let actual = match serde_json::to_value(&disagreement) {
            Ok(value) => value,
            Err(error) => panic!("disagreement does not serialize: {error}"),
        };
        assert_eq!(actual, case["disagreement"], "{}", text(case, "name"));
    }
}

#[test]
fn disagreement_ids_equal_the_golden_ids_for_any_ordering() {
    let document = parse(DISAGREEMENTS);
    let cases = array(&document, "fingerprints");
    assert_eq!(
        cases.len(),
        13,
        "the disagreement identifier vector count drifted"
    );
    for case in cases {
        let reasons: Vec<Reason> = array(case, "reasons")
            .iter()
            .map(|item| match item.as_str().and_then(Reason::parse) {
                Some(reason) => reason,
                None => panic!("vector reason is unknown: {item}"),
            })
            .collect();
        let codes: Vec<EvidenceDefectCode> = array(case, "defectCodes")
            .iter()
            .map(
                |item| match item.as_str().and_then(EvidenceDefectCode::parse) {
                    Some(code) => code,
                    None => panic!("vector defect code is unknown: {item}"),
                },
            )
            .collect();
        let policy = case["policyVersion"].as_str().unwrap_or(POLICY_VERSION);
        let actual =
            disagreement_fingerprint(uuid(text(case, "reportId")), &reasons, &codes, policy);
        assert_eq!(
            actual.to_string(),
            text(case, "id"),
            "{}",
            text(case, "name")
        );
    }
}

#[test]
fn stored_disagreements_import_under_the_documented_rules() {
    let document = parse(DISAGREEMENTS);
    let cases = array(&document, "validations");
    assert_eq!(
        cases.len(),
        29,
        "the disagreement validation vector count drifted"
    );
    for case in cases {
        let actual = outcome_json(validate_match_disagreement(object(&case["input"])));
        assert_eq!(actual, case["result"], "{}", text(case, "name"));
    }
}
