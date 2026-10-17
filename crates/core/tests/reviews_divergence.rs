//! Divergence-v1 reasons.
//!
//! The vectors are the `divergence_reasons` results for reviews against synthetic reports, at
//! the inclusive thresholds: an overall delta of 15 points, a component delta of 40 percent of its
//! available points, the first report component of a duplicated name as the baseline, and the
//! outcome-changing evidence flag. The upstream Java golden vectors are checked in
//! `reviews_upstream_golden.rs`.

mod reviews_support;

use jobniac_core::reviews::{create_match_review, divergence_reasons};
use reviews_support::{array, now_of, object, parse, report_id_of, reports_of, text};

const DIVERGENCE: &str = include_str!("golden/reviews-divergence.json");

#[test]
fn divergence_reasons_equal_the_golden_reasons() {
    let document = parse(DIVERGENCE);
    let now = now_of(&document);
    let reports = reports_of(&document);
    let cases = array(&document, "cases");
    assert_eq!(cases.len(), 20, "the divergence vector count drifted");
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
        let actual = match serde_json::to_value(divergence_reasons(report, &review)) {
            Ok(value) => value,
            Err(error) => panic!("reasons do not serialize: {error}"),
        };
        assert_eq!(actual, case["reasons"], "{}", text(case, "name"));
    }
}
