//! Review submission validation and the service use cases.
//!
//! The vectors are the envelopes of `create_match_review`, `validate_match_review` and
//! `MatchReviewService` over an in-memory repository, on synthetic reports only. Each Rust result is
//! compared with the recorded result as JSON, so the message, the code, the details and the order of
//! the checks are all pinned. Each service step also pins the repository calls it reached (none for
//! an invalid request), the instant it ran at, and the default page limit of 20.

mod reviews_support;

use std::sync::Arc;

use jobniac_core::clock::format_instant;
use jobniac_core::reviews::{
    create_match_review, validate_match_disagreement, validate_match_review,
};
use reviews_support::{
    FakeRepository, SteppedClock, array, instant, now_of, object, outcome_json, parse, report_from,
    report_id_of, reports_of, stepped_service, take_calls, text,
};
use serde_json::{Value, json};

const VALIDATION: &str = include_str!("golden/reviews-validation.json");
const DISAGREEMENTS: &str = include_str!("golden/reviews-disagreements.json");

#[test]
fn submitted_reviews_normalize_or_refuse_like_the_golden_vectors() {
    let document = parse(VALIDATION);
    let now = now_of(&document);
    let reports = reports_of(&document);
    let cases = array(&document, "create");
    assert_eq!(cases.len(), 82, "the create vector count drifted");
    for case in cases {
        let letter = text(case, "report");
        let report = match reports
            .iter()
            .find(|report| report.id == report_id_of(letter))
        {
            Some(report) => report,
            None => panic!("vector report {letter} is missing"),
        };
        let actual = outcome_json(create_match_review(report, object(&case["draft"]), now));
        assert_eq!(actual, case["result"], "{}", text(case, "name"));
    }
}

#[test]
fn stored_reviews_import_under_the_same_rules_without_grounding() {
    let document = parse(VALIDATION);
    let cases = array(&document, "import");
    assert_eq!(cases.len(), 14, "the import vector count drifted");
    for case in cases {
        let actual = outcome_json(validate_match_review(object(&case["input"])));
        assert_eq!(actual, case["result"], "{}", text(case, "name"));
    }
}

#[test]
fn the_report_rebuilt_from_the_vector_keeps_its_scores_and_facts() {
    let document = parse(VALIDATION);
    let reports = array(&document, "reports");
    let report = report_from(&reports[0]);
    assert_eq!(report.overall_score, 57);
    assert_eq!(report.components.len(), 5);
    assert!(!report.evidence.is_empty());
}

#[tokio::test]
async fn the_service_sequence_matches_the_golden_vectors() {
    let document = parse(VALIDATION);
    let clock = Arc::new(SteppedClock::new(now_of(&document)));
    let repository = FakeRepository::new(reports_of(&document));
    let calls = repository.log();
    let service = stepped_service(repository, &clock);
    let steps = array(&document, "service");
    assert_eq!(steps.len(), 39, "the service sequence length drifted");
    for step in steps {
        let name = text(step, "name");
        clock.set(instant(text(step, "now")));
        take_calls(&calls);
        let request = &step["request"];
        let actual = match text(step, "operation") {
            "submit_match_review" => outcome_json(service.submit_match_review(request).await),
            "get_match_review" => outcome_json(service.get_match_review(request).await),
            "list_match_reviews" => outcome_json(service.list_match_reviews(request).await),
            "list_match_disagreements" => {
                outcome_json(service.list_match_disagreements(request).await)
            }
            "acknowledge_match_disagreement" => {
                outcome_json(service.acknowledge_match_disagreement(request).await)
            }
            "link_match_disagreement" => {
                outcome_json(service.link_match_disagreement(request).await)
            }
            other => panic!("unknown operation {other}"),
        };
        assert_eq!(
            Value::Array(take_calls(&calls)),
            step["calls"],
            "repository calls of {name}"
        );
        assert_eq!(actual, step["result"], "{name}");
    }
}

#[test]
fn a_resubmission_returns_the_stored_rows_and_keeps_the_linked_issue() {
    let document = parse(VALIDATION);
    let steps = array(&document, "service");
    let result_of = |name: &str| -> Value {
        match steps.iter().find(|step| text(step, "name") == name) {
            Some(step) => step["result"]["ok"].clone(),
            None => panic!("service step {name} is missing"),
        }
    };
    let first = result_of("submit valid");
    assert_eq!(
        result_of("submit again later returns the stored review and disagreement"),
        first,
        "a resubmission returns the stored review, with its original creation time"
    );
    let linked = result_of("link with an over-long issue is stored (no core cap)");
    let later = result_of("resubmit with another reviewer keeps the linked disagreement");
    assert_eq!(
        later["disagreement"], linked,
        "the disagreement keeps its status, issue and original review"
    );
    assert_ne!(
        later["review"]["id"], first["review"]["id"],
        "other reviewer metadata is another review"
    );
    assert_eq!(later["review"]["createdAt"], "2026-10-04T12:00:00.123456Z");
}

#[test]
fn stored_times_are_instants_truncated_to_microseconds_by_the_rust_rule() {
    // The typed record requires an instant and keeps microseconds. The read path always passes formatted
    // instants, so only a direct import can reach this rule.
    let document = parse(DISAGREEMENTS);
    let Some(case) = array(&document, "validations").first() else {
        panic!("the disagreement import vectors are missing");
    };
    let stored = object(&case["input"]).clone();
    let mut overprecise = stored.clone();
    overprecise.insert(
        "createdAt".to_owned(),
        json!("2026-10-02T12:00:00.123456789Z"),
    );
    let record = match validate_match_disagreement(&overprecise) {
        Ok(record) => record,
        Err(error) => panic!("an over-precise instant is refused: {error}"),
    };
    assert_eq!(
        format_instant(record.created_at),
        "2026-10-02T12:00:00.123456Z"
    );
    let mut bad = stored.clone();
    bad.insert("updatedAt".to_owned(), json!("not-an-instant"));
    assert_eq!(
        outcome_json(validate_match_disagreement(&bad))["error"]["message"],
        "updatedAt must be a valid instant"
    );
    let mut missing = stored;
    missing.remove("createdAt");
    assert_eq!(
        outcome_json(validate_match_disagreement(&missing))["error"]["message"],
        "createdAt must be a valid instant"
    );
}
