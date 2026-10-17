//! The deterministic-v1 scorer against its golden vectors.
//!
//! Three sources feed this file: the upstream golden reports (`matching-source-golden.json`, emitted
//! by executing the upstream Java scorer), the scorer vectors (`matching-scores.json`), and the
//! explicit technical-floor and experience cases. Every report is compared as its full JSON form.

mod matching_support;

use jobniac_core::clock::parse_instant;
use jobniac_core::errors::AppError;
use jobniac_core::matching::{JobGraph, ProfileGraph, score_match};
use serde_json::{Value, json};

use matching_support::{array, error_value, job_graph, outcome_value, parse, profile_graph, text};

const GOLDEN: &str = include_str!("golden/matching-source-golden.json");
const SCORES: &str = include_str!("golden/matching-scores.json");
const NOW: &str = "2026-10-02T12:00:00.123456Z";

fn now_of(text: &str) -> time::OffsetDateTime {
    match parse_instant(text) {
        Ok(instant) => instant,
        Err(error) => panic!("vector instant is refused: {error}"),
    }
}

fn profile(skills: &[&str], has_experience: bool) -> ProfileGraph {
    ProfileGraph {
        id: "11111111-1111-4111-8111-111111111111".to_owned(),
        updated_at: "2025-01-02T03:04:05.123456Z".to_owned(),
        skills: skills.iter().map(|skill| (*skill).to_owned()).collect(),
        has_experience,
    }
}

fn job(skills: &[&str], requirement: Option<&str>, seniority: Option<&str>) -> JobGraph {
    JobGraph {
        id: "22222222-2222-4222-8222-222222222222".to_owned(),
        updated_at: "2025-02-03T04:05:06.654321Z".to_owned(),
        experience_requirement: requirement.map(str::to_owned),
        seniority: seniority.map(str::to_owned),
        skills: skills.iter().map(|skill| (*skill).to_owned()).collect(),
    }
}

#[test]
fn upstream_golden_reports_are_reproduced_exactly() {
    let document = parse(GOLDEN);
    let vectors = array(&document, "vectors");
    assert_eq!(vectors.len(), 5, "the upstream golden vector count drifted");
    for vector in vectors {
        let name = text(vector, "name");
        let profile = profile_graph(&vector["input"]["profile"]);
        let job = job_graph(&vector["input"]["job"]);
        let now = now_of(text(&vector["input"], "now"));
        let actual = score_match(&profile, &job, now);
        assert_eq!(
            outcome_value(actual),
            json!({ "ok": vector["report"] }),
            "{name}"
        );
    }
}

#[test]
fn golden_scorer_vectors_are_reproduced_exactly() {
    let document = parse(SCORES);
    let cases = array(&document, "cases");
    assert_eq!(cases.len(), 225, "the scorer vector count drifted");
    let mut successes = 0_usize;
    let mut refusals = 0_usize;
    for case in cases {
        let name = text(case, "name");
        let input = &case["input"];
        let profile: ProfileGraph = profile_graph(&input["profile"]);
        let job: JobGraph = job_graph(&input["job"]);
        let now = now_of(text(input, "now"));
        let actual: Value = outcome_value(score_match(&profile, &job, now));
        assert_eq!(actual, case["result"], "{name}");
        if case["result"].get("ok").is_some() {
            successes += 1;
        } else {
            refusals += 1;
        }
    }
    assert!(successes > 150, "too few successful scores: {successes}");
    assert!(refusals > 20, "too few refusals: {refusals}");
}

#[test]
fn technical_floor_precedes_the_overall_rounding() {
    // (matches, total, expected overall score), as in the recorded cases; confidence is 40 and the
    // technical earned points are matches * 40 // total.
    let cases = [(0, 1, 0), (1, 3, 33), (2, 3, 65), (3, 4, 75), (1, 2, 50)];
    let now = now_of(NOW);
    for (matches, total, expected) in cases {
        let names: Vec<String> = (0..total).map(|index| format!("skill-{index}")).collect();
        let skills: Vec<&str> = names.iter().map(String::as_str).collect();
        let report = match score_match(
            &profile(&skills[..matches], false),
            &job(&skills, None, None),
            now,
        ) {
            Ok(report) => report,
            Err(error) => panic!("technical-floor case refused: {error}"),
        };
        assert_eq!(report.overall_score, expected, "{matches} of {total}");
        assert_eq!(report.confidence, 40);
        assert_eq!(
            report.components[0].earned_points,
            i64::try_from(matches * 40 / total).unwrap_or(-1)
        );
        assert!(!report.blocker_mismatch);
    }
}

#[test]
fn experience_needs_a_requirement_and_a_dated_row() {
    let now = now_of(NOW);
    let without_requirement = score_match(&profile(&[], true), &job(&[], None, None), now);
    assert_eq!(without_requirement.map(|report| report.confidence), Ok(0));
    let without_experience =
        score_match(&profile(&[], false), &job(&[], Some("5 years"), None), now);
    assert_eq!(without_experience.map(|report| report.confidence), Ok(0));
    let report = match score_match(&profile(&[], true), &job(&[], None, Some("Senior")), now) {
        Ok(report) => report,
        Err(error) => panic!("experience case refused: {error}"),
    };
    assert_eq!(report.overall_score, 52);
    assert_eq!(report.confidence, 25);
    assert_eq!(report.components[1].earned_points, 13);
    assert_eq!(report.components[1].available_points, 25);
    assert_eq!(report.evidence.len(), 1);
    assert_eq!(report.evidence[0].fact, "profile contains dated experience");
}

#[test]
fn an_unsafe_job_skill_is_refused_with_the_fixed_message() {
    let result = score_match(
        &profile(&[], false),
        &job(&["Java\u{a0}"], None, None),
        now_of(NOW),
    );
    assert_eq!(
        result,
        Err(AppError::DomainValue(
            "fact must be a normalized privacy-safe skill".to_owned()
        ))
    );
    assert_eq!(
        error_value(&AppError::Internal),
        json!({"error": {"kind": "Internal"}})
    );
}

#[test]
fn report_json_keeps_the_golden_key_order() {
    let report = match score_match(
        &profile(&["java"], false),
        &job(&["java"], None, None),
        now_of(NOW),
    ) {
        Ok(report) => report,
        Err(error) => panic!("scored pair refused: {error}"),
    };
    let rendered = match serde_json::to_string(&report) {
        Ok(text) => text,
        Err(error) => panic!("report does not serialize: {error}"),
    };
    let keys = [
        "\"id\"",
        "\"profileId\"",
        "\"jobId\"",
        "\"profileRevision\"",
        "\"jobRevision\"",
        "\"algorithmVersion\"",
        "\"overallScore\"",
        "\"confidence\"",
        "\"outcome\"",
        "\"blockerMismatch\"",
        "\"components\"",
        "\"evidence\"",
        "\"createdAt\"",
    ];
    let mut cursor = 0;
    for key in keys {
        match rendered[cursor..].find(key) {
            Some(offset) => cursor += offset + key.len(),
            None => panic!("key {key} is missing or out of order in {rendered}"),
        }
    }
}
