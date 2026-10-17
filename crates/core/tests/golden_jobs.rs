//! Job service golden vectors, computed from synthetic inputs only.
//!
//! Covered: the text rules (input text hash, normalized keys, fingerprints, skill split and merge,
//! skill and experience extraction, title derivation), `postedAt` admission, the capture, update and
//! read scenarios over the in-memory repository (responses, errors and the order of repository
//! calls), and the search scenarios (scoring, ordering, the envelope, validation and the arguments of
//! the repository call).

mod job_support;

use std::sync::Arc;

use jobniac_core::clock::format_instant;
use jobniac_core::errors::error_envelope;
use jobniac_core::jobs::{
    JobAggregate, JobSearchService, JobService, canonical_whitespace, derive_title,
    extract_experience, extract_skills, fingerprint, input_text_hash, merge_skills, normalized_key,
    optional_instant, split_skill_list,
};
use jobniac_core::pagination::{CursorCodec, CursorKey, SearchCandidates};
use serde::Serialize;
use serde_json::{Value, json};

use job_support::{CounterIds, MemoryJobs, MemorySearch, MemoryUow, fixed_clock};

const TEXT: &str = include_str!("golden/jobs-text.json");
const SCENARIOS: &str = include_str!("golden/jobs-scenarios.json");
const SEARCH: &str = include_str!("golden/jobs-search.json");
const SECRET: &str = "golden-cursor-secret-0123456789abcdef";

fn parse(text: &str) -> Value {
    match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => panic!("vector does not parse: {error}"),
    }
}

fn array<'a>(document: &'a Value, key: &str) -> &'a [Value] {
    match document[key].as_array() {
        Some(items) => items,
        None => panic!("missing array {key}"),
    }
}

fn text_of(case: &Value, key: &str) -> String {
    match case[key].as_str() {
        Some(value) => value.to_owned(),
        None => panic!("missing string {key} in {case}"),
    }
}

fn opt_text(case: &Value, key: &str) -> Option<String> {
    case[key].as_str().map(str::to_owned)
}

fn to_value<T: Serialize>(value: &T) -> Value {
    match serde_json::to_value(value) {
        Ok(json) => json,
        Err(error) => panic!("value does not serialize: {error}"),
    }
}

fn codec() -> CursorCodec {
    match CursorKey::from_secret(SECRET) {
        Ok(key) => CursorCodec::new(key),
        Err(error) => panic!("test secret is refused: {error}"),
    }
}

#[test]
fn input_text_hash_matches_the_golden_vectors() {
    let document = parse(TEXT);
    for case in array(&document, "inputTextHash") {
        let text = text_of(case, "text");
        assert_eq!(
            input_text_hash(&text),
            text_of(case, "sha256"),
            "hash of {text:?}"
        );
    }
}

#[test]
fn canonical_whitespace_collapses_only_pattern_whitespace() {
    assert_eq!(
        canonical_whitespace("  A\u{a0}\u{a0} B\t\nC  "),
        "a\u{a0}\u{a0} b c"
    );
}

#[test]
fn canonical_whitespace_keeps_the_code_points_the_recorded_run_does_not_lowercase() {
    // Unicode 17.0 gives these a lowercase mapping; the recorded run (Unicode 16.0.0) leaves them unchanged,
    // so the input-text hash and the description of the fingerprint must leave them unchanged too.
    assert_eq!(
        canonical_whitespace("\u{a7ce}\u{a7d2}\u{a7d4}\u{a7f1} A\u{16ea0}\u{16eb8}"),
        "\u{a7ce}\u{a7d2}\u{a7d4}\u{a7f1} a\u{16ea0}\u{16eb8}"
    );
    assert_eq!(
        input_text_hash("A\u{a7ce}"),
        input_text_hash("a\u{a7ce}"),
        "the hash is over the lower-cased text"
    );
    assert_ne!(input_text_hash("\u{a7ce}"), input_text_hash("\u{a7cf}"));
}

#[test]
fn normalized_keys_match_the_golden_vectors() {
    let document = parse(TEXT);
    for case in array(&document, "normalizedKey") {
        let value = opt_text(case, "value");
        assert_eq!(
            normalized_key(value.as_deref()),
            text_of(case, "key"),
            "key of {value:?}"
        );
    }
}

#[test]
fn fingerprints_match_the_golden_vectors() {
    let document = parse(TEXT);
    for case in array(&document, "fingerprint") {
        let fields = (
            text_of(case, "title"),
            opt_text(case, "company"),
            opt_text(case, "location"),
            text_of(case, "description"),
            opt_text(case, "url"),
        );
        assert_eq!(
            fingerprint(
                &fields.0,
                fields.1.as_deref(),
                fields.2.as_deref(),
                &fields.3,
                fields.4.as_deref()
            ),
            text_of(case, "fingerprint"),
            "fingerprint of {fields:?}"
        );
    }
}

#[test]
fn skill_lists_match_the_golden_vectors() {
    let document = parse(TEXT);
    for case in array(&document, "skillSplit") {
        let value = text_of(case, "value");
        assert_eq!(
            split_skill_list(&value),
            strings(&case["parts"]),
            "{value:?}"
        );
    }
    for case in array(&document, "mergeSkills") {
        assert_eq!(
            merge_skills(&strings(&case["explicit"]), &strings(&case["extracted"])),
            strings(&case["merged"]),
            "merge of {case}"
        );
    }
    for case in array(&document, "extractSkills") {
        let text = text_of(case, "text");
        assert_eq!(extract_skills(&text), strings(&case["skills"]), "{text:?}");
    }
}

#[test]
fn experience_and_titles_match_the_golden_vectors() {
    let document = parse(TEXT);
    for case in array(&document, "extractExperience") {
        let text = text_of(case, "text");
        assert_eq!(
            extract_experience(&text),
            opt_text(case, "experience"),
            "{text:?}"
        );
    }
    for case in array(&document, "deriveTitle") {
        let text = text_of(case, "text");
        assert_eq!(derive_title(&text), text_of(case, "title"), "{text:?}");
    }
}

#[test]
fn posted_at_admits_hour_24_only_as_the_next_midnight() {
    let admitted = |value: &str| match optional_instant(&json!({ "postedAt": value }), "postedAt") {
        Ok(Some(instant)) => format_instant(instant),
        other => panic!("{value} refused: {other:?}"),
    };
    assert_eq!(admitted("2026-12-31T24:00+00:00"), "2027-01-01T00:00:00Z");
    assert_eq!(
        admitted("2026-09-29T24:00:00.000+02:00"),
        "2026-09-29T22:00:00Z"
    );
    assert_eq!(admitted("0001-01-01T24:00+00:00"), "0001-01-02T00:00:00Z");
    for refused in [
        "9999-12-31T24:00+00:00",
        "2026-09-29T24:00:01+02:00",
        "2026-09-29T24:30+02:00",
    ] {
        assert!(
            optional_instant(&json!({ "postedAt": refused }), "postedAt").is_err(),
            "{refused} must be refused"
        );
    }
}

#[test]
fn posted_at_admission_matches_the_golden_vectors() {
    let document = parse(TEXT);
    for case in array(&document, "postedAt") {
        let value = case["value"].clone();
        let outcome = optional_instant(&json!({ "postedAt": value.clone() }), "postedAt");
        match (outcome, case.get("result")) {
            (Ok(Some(instant)), Some(expected)) => {
                assert_eq!(
                    Value::String(format_instant(instant)),
                    *expected,
                    "instant of {value}"
                );
            }
            (Err(error), None) => {
                assert_eq!(
                    to_value(&error_envelope(&error)),
                    case["error"],
                    "error of {value}"
                );
            }
            (outcome, expected) => panic!("{value}: {outcome:?} against {expected:?}"),
        }
    }
}

fn strings(value: &Value) -> Vec<String> {
    match value.as_array() {
        Some(items) => items
            .iter()
            .map(|item| match item.as_str() {
                Some(text) => text.to_owned(),
                None => panic!("not a string: {item}"),
            })
            .collect(),
        None => panic!("not an array: {value}"),
    }
}

#[tokio::test]
async fn capture_update_and_read_scenarios_match_the_golden_vectors() {
    let document = parse(SCENARIOS);
    let ids = Arc::new(CounterIds::default());
    for scenario in array(&document, "scenarios") {
        let name = text_of(scenario, "name");
        ids.reset();
        let repo = MemoryJobs::new();
        for (index, step) in array(scenario, "steps").iter().enumerate() {
            let label = format!("{name} step {index}");
            let op = text_of(step, "op");
            let request = step["request"].clone();
            let expect = &step["expect"];
            repo.lose_races(usize::from(
                step.get("loseRace") == Some(&Value::Bool(true)),
            ));
            let clock = fixed_clock(&text_of(step, "clock"));
            let service =
                JobService::with_ids(MemoryUow, repo.clone(), clock, codec(), ids.clone());
            let outcome = match op.as_str() {
                "capture" => service
                    .add_job_from_text(&request)
                    .await
                    .map(|result| to_value(&result)),
                "update" => service.update_job(&request).await.map(|job| to_value(&job)),
                "get" => {
                    let job_id = text_of(&request, "jobId");
                    service.get_job(&job_id).await.map(|job| to_value(&job))
                }
                other => panic!("unknown op {other}"),
            };
            match outcome {
                Ok(value) => {
                    assert!(expect.get("error").is_none(), "{label}: expected an error");
                    assert_eq!(value, expect["result"], "{label}: result");
                }
                Err(error) => {
                    assert_eq!(
                        to_value(&error_envelope(&error)),
                        expect["error"],
                        "{label}: error envelope"
                    );
                }
            }
            let calls: Vec<String> = repo.take_calls().into_iter().map(str::to_owned).collect();
            assert_eq!(
                calls,
                strings(&expect["repoCalls"]),
                "{label}: repository calls"
            );
        }
    }
}

#[tokio::test]
async fn search_scenarios_match_the_golden_vectors() {
    let document = parse(SEARCH);
    for scenario in array(&document, "scenarios") {
        let name = text_of(scenario, "name");
        let candidates = &scenario["candidates"];
        let items: Vec<JobAggregate> = array(candidates, "aggregates")
            .iter()
            .map(|item| match serde_json::from_value(item.clone()) {
                Ok(aggregate) => aggregate,
                Err(error) => panic!("{name}: candidate does not parse: {error}"),
            })
            .collect();
        let matched = match candidates["matchedCount"].as_i64() {
            Some(count) => count,
            None => panic!("{name}: matchedCount"),
        };
        let has_more = candidates["hasMore"] == Value::Bool(true);
        let candidates = match SearchCandidates::new(matched, has_more, items) {
            Ok(candidates) => candidates,
            Err(error) => panic!("{name}: candidates refused: {error}"),
        };
        let repo = MemorySearch::with_candidates(candidates);
        let service = JobSearchService::new(MemoryUow, repo.clone());
        let outcome = service.search_jobs(&scenario["request"]).await;
        match outcome {
            Ok(response) => {
                assert_eq!(
                    to_value(&response),
                    scenario["expect"]["result"],
                    "{name}: response"
                );
            }
            Err(error) => {
                assert_eq!(
                    to_value(&error_envelope(&error)),
                    scenario["expect"]["error"],
                    "{name}: error envelope"
                );
            }
        }
        let calls: Vec<String> = std::mem::take(&mut *job_support::locked(&repo.store.calls))
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(calls, strings(&scenario["repoCalls"]), "{name}: calls");
        let arguments = job_support::locked(&repo.store.arguments)
            .clone()
            .map(|(tokens, limit)| json!({ "queryTokens": tokens, "limit": limit }))
            .unwrap_or(Value::Null);
        assert_eq!(arguments, scenario["searchArguments"], "{name}: arguments");
    }
}
