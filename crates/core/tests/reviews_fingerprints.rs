//! Review identifiers.
//!
//! The review identifier is the MD5 name-based UUIDv3 of a length-prefixed identity string built from
//! the normalized review. The vectors record the identifiers for reordered, duplicated and
//! non-ASCII inputs, so the component and evidence sort keys, the stable sort for duplicate names and
//! the encoding are all pinned byte for byte.

mod reviews_support;

use jobniac_core::reviews::{create_match_review, review_fingerprint};
use reviews_support::{array, now_of, object, parse, report_id_of, reports_of, text};

const FINGERPRINTS: &str = include_str!("golden/reviews-fingerprints.json");

#[test]
fn review_ids_equal_the_golden_ids_byte_for_byte() {
    let document = parse(FINGERPRINTS);
    let now = now_of(&document);
    let reports = reports_of(&document);
    let cases = array(&document, "reviews");
    assert_eq!(
        cases.len(),
        24,
        "the review identifier vector count drifted"
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
            text(case, "id"),
            "{}",
            text(case, "name")
        );
        assert_eq!(
            review_fingerprint(&review).to_string(),
            text(case, "id"),
            "{}",
            text(case, "name")
        );
    }
}

#[test]
fn reordered_components_and_evidence_share_an_identifier() {
    let document = parse(FINGERPRINTS);
    let cases = array(&document, "reviews");
    let id_of = |name: &str| -> &str {
        match cases.iter().find(|case| text(case, "name") == name) {
            Some(case) => text(case, "id"),
            None => panic!("vector case {name} is missing"),
        }
    };
    assert_eq!(id_of("base"), id_of("components reversed"));
    assert_eq!(id_of("two evidence"), id_of("two evidence reversed"));
    assert_eq!(
        id_of("astral and bmp facts"),
        id_of("astral and bmp facts reversed")
    );
    assert_ne!(id_of("base"), id_of("five components shuffled"));
}

#[test]
fn duplicate_component_names_keep_their_input_order_in_the_identifier() {
    let document = parse(FINGERPRINTS);
    let cases = array(&document, "reviews");
    let id_of = |name: &str| -> &str {
        match cases.iter().find(|case| text(case, "name") == name) {
            Some(case) => text(case, "id"),
            None => panic!("vector case {name} is missing"),
        }
    };
    assert_ne!(
        id_of("duplicate names"),
        id_of("duplicate names reversed"),
        "P-13: duplicate names are sorted by name only"
    );
}
