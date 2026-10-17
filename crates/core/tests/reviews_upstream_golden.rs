//! The upstream golden vectors of the match review divergence policy.
//!
//! `golden/reviews-source-golden.json` holds vectors produced by executing the upstream domain source
//! at the commit named in the file. Its inputs are synthetic. For each of the eight vectors the
//! baseline report is rebuilt, the review is created from the vector's draft at the vector's instant,
//! and the review, the divergence reasons and the disagreement must equal the vector. The baseline
//! report must be unchanged by the review.

mod reviews_support;

use jobniac_core::reviews::{
    POLICY_VERSION, create_match_disagreement, create_match_review, divergence_reasons,
};
use reviews_support::{array, instant, object, parse, report_from, text};
use serde_json::Value;

const UPSTREAM: &str = include_str!("golden/reviews-source-golden.json");

#[test]
fn upstream_golden_vectors_reproduce_review_reasons_and_disagreement() {
    let document = parse(UPSTREAM);
    assert_eq!(text(&document, "policyVersion"), POLICY_VERSION);
    let vectors = array(&document, "vectors");
    assert_eq!(vectors.len(), 8, "the upstream golden vector count drifted");
    for vector in vectors {
        let name = text(vector, "name");
        let input = &vector["input"];
        let report = report_from(&input["report"]);
        let now = instant(text(input, "now"));
        let review = match create_match_review(&report, object(&input["review"]), now) {
            Ok(review) => review,
            Err(error) => panic!("{name}: the upstream draft is refused: {error}"),
        };
        assert_eq!(to_value(&review), vector["review"], "review of {name}");
        assert_eq!(
            to_value(&divergence_reasons(&report, &review)),
            vector["reasons"],
            "reasons of {name}"
        );
        assert_eq!(
            to_value(&create_match_disagreement(&report, &review, now)),
            vector["disagreement"],
            "disagreement of {name}"
        );
        let baseline = report_from(&vector["report"]);
        assert_eq!(
            (
                baseline.overall_score,
                baseline.outcome,
                baseline.blocker_mismatch
            ),
            (
                report.overall_score,
                report.outcome,
                report.blocker_mismatch
            ),
            "the baseline report of {name} is changed by the review"
        );
    }
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    match serde_json::to_value(value) {
        Ok(value) => value,
        Err(error) => panic!("the vector value does not serialize: {error}"),
    }
}
