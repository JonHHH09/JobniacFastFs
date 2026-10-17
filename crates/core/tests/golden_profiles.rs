//! Profile aggregate golden vectors, computed from synthetic inputs only.
//!
//! Covered: write-request canonicalization and graph build (with counter identifiers), search
//! postings, project patches over the in-memory repository, canonical link URLs, profile search
//! (scores, ordering, matched fields and the envelope), personal-details validation and
//! expected-revision validation.

mod profile_support;

use std::collections::HashSet;
use std::sync::Arc;

use jobniac_core::clock::{FixedClock, parse_instant};
use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::pagination::SearchCandidates;
use jobniac_core::profiles::personal_details::canonical_personal_details;
use jobniac_core::profiles::search::ProfileSearchCandidate;
use jobniac_core::profiles::{
    ProfileGraph, ProfileIdentity, ProfileSearchService, ProfileService, build_profile_graph,
    canonical_profile_link_url, canonicalize_profile_write, expected_revision,
    profile_search_terms,
};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use profile_support::{MemoryProfiles, MemorySearch, MemoryUow};

const WRITE: &str = include_str!("golden/profile-write.json");
const PATCH: &str = include_str!("golden/profile-patch.json");
const LINKS: &str = include_str!("golden/profile-links.json");
const SEARCH: &str = include_str!("golden/profile-search.json");
const PERSONAL: &str = include_str!("golden/personal-details.json");
const NOW: &str = "2026-09-28T12:30:15.123Z";

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

fn name_of(case: &Value) -> &str {
    case.get("name")
        .and_then(Value::as_str)
        .unwrap_or("<unnamed>")
}

fn to_value<T: Serialize>(value: &T) -> Value {
    match serde_json::to_value(value) {
        Ok(json) => json,
        Err(error) => panic!("value does not serialize: {error}"),
    }
}

fn envelope(error: &AppError) -> Value {
    to_value(&error_envelope(error))
}

fn instant(text: &str) -> time::OffsetDateTime {
    match parse_instant(text) {
        Ok(value) => value,
        Err(error) => panic!("bad instant {text}: {error}"),
    }
}

/// Every string under an `id` key of a JSON document (the identifiers the request supplied).
fn supplied_ids(document: &Value, found: &mut HashSet<String>) {
    match document {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "id" {
                    if let Some(text) = value.as_str() {
                        found.insert(text.to_owned());
                    }
                } else {
                    supplied_ids(value, found);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| supplied_ids(item, found)),
        _ => {}
    }
}

/// Replaces the identifiers that the request did not supply by `<new>`, as the vector generator does.
fn mask_new_technology_ids(result: &mut Value, known: &HashSet<String>) {
    let Some(technologies) = result["project"]["technologies"].as_array_mut() else {
        return;
    };
    for technology in technologies {
        let supplied = technology["id"]
            .as_str()
            .is_some_and(|id| known.contains(id));
        if !supplied {
            technology["id"] = json!("<new>");
        }
    }
}

#[test]
fn write_requests_canonicalize_and_build_like_the_golden_vectors() {
    let document = parse(WRITE);
    let now = instant(NOW);
    let mut checked = 0;
    for case in array(&document, "cases") {
        let name = name_of(case);
        match canonicalize_profile_write(&case["request"]) {
            Err(error) => {
                assert_eq!(
                    case["ok"],
                    json!(false),
                    "{name}: Rust refuses, the vector accepts"
                );
                assert_eq!(envelope(&error), case["error"], "{name}: error envelope");
            }
            Ok(draft) => {
                assert_eq!(
                    case["ok"],
                    json!(true),
                    "{name}: the vector refuses, Rust accepts"
                );
                let mut next = 0_u128;
                let mut counter = || {
                    next += 1;
                    Uuid::from_u128(next)
                };
                let profile_id = counter();
                let graph: ProfileGraph =
                    build_profile_graph(&draft, profile_id, now, now, 0, &mut counter);
                assert_eq!(to_value(&graph), case["graph"], "{name}: graph");
                let postings: Vec<Value> = profile_search_terms(&graph)
                    .iter()
                    .map(|term| json!([term.field_key, term.term, term.weight]))
                    .collect();
                assert_eq!(Value::Array(postings), case["postings"], "{name}: postings");
            }
        }
        checked += 1;
    }
    assert!(
        checked > 50,
        "the write vector set shrank to {checked} cases"
    );
}

#[tokio::test]
async fn project_patches_match_the_golden_in_memory_flow() {
    let document = parse(PATCH);
    let base = document["base"].clone();
    let mut known = HashSet::new();
    supplied_ids(&base, &mut known);
    let draft = match canonicalize_profile_write(&base) {
        Ok(draft) => draft,
        Err(error) => panic!("base request refused: {}", envelope(&error)),
    };
    let mut seeded_next = 0_u128;
    let mut seeded_counter = || {
        seeded_next += 1;
        Uuid::from_u128(seeded_next)
    };
    let profile_id = seeded_counter();
    let now = instant(NOW);
    let base_graph = build_profile_graph(&draft, profile_id, now, now, 0, &mut seeded_counter);
    assert_eq!(to_value(&base_graph), document["baseGraph"], "base graph");

    for case in array(&document, "cases") {
        let name = name_of(case);
        let repository = MemoryProfiles::new();
        repository.seed(base_graph.clone());
        let service = ProfileService::new(MemoryUow, repository, Arc::new(FixedClock::new(now)));
        match service.update_profile_project(&case["request"]).await {
            Err(error) => {
                assert_eq!(
                    case["ok"],
                    json!(false),
                    "{name}: Rust refuses, the vector accepts"
                );
                assert_eq!(envelope(&error), case["error"], "{name}: error envelope");
            }
            Ok(result) => {
                assert_eq!(
                    case["ok"],
                    json!(true),
                    "{name}: the vector refuses, Rust accepts"
                );
                let mut actual = to_value(&result);
                mask_new_technology_ids(&mut actual, &known);
                assert_eq!(actual, case["result"], "{name}: patch result");
            }
        }
    }
}

#[test]
fn link_urls_canonicalize_like_the_golden_vectors() {
    let document = parse(LINKS);
    for case in array(&document, "cases") {
        let raw = case["raw"].as_str().unwrap_or_default();
        assert_eq!(
            canonical_profile_link_url(raw),
            case["canonical"].as_str().unwrap_or_default(),
            "raw {raw:?}"
        );
    }
}

fn identity_from(value: &Value) -> ProfileIdentity {
    let id = match value["id"].as_str().map(Uuid::parse_str) {
        Some(Ok(id)) => id,
        _ => panic!("candidate id is not a canonical UUID: {value}"),
    };
    ProfileIdentity::new(
        id,
        value["fullName"].as_str().unwrap_or_default().to_owned(),
        value["email"].as_str().unwrap_or_default().to_owned(),
        value["summary"].as_str().map(str::to_owned),
        instant(value["createdAt"].as_str().unwrap_or_default()),
        instant(value["updatedAt"].as_str().unwrap_or_default()),
        value["revision"].as_i64().unwrap_or_default(),
    )
}

fn candidates_from(scenario: &Value) -> SearchCandidates<ProfileSearchCandidate> {
    let items = array(scenario, "candidates")
        .iter()
        .map(|candidate| ProfileSearchCandidate {
            profile: identity_from(&candidate["profile"]),
            score: candidate["score"].as_i64().unwrap_or_default(),
            field_keys: array(candidate, "fieldKeys")
                .iter()
                .map(|key| key.as_str().unwrap_or_default().to_owned())
                .collect(),
        })
        .collect();
    match SearchCandidates::new(
        scenario["matchedCount"].as_i64().unwrap_or_default(),
        scenario["hasMore"].as_bool().unwrap_or_default(),
        items,
    ) {
        Ok(candidates) => candidates,
        Err(error) => panic!("vector candidates violate the invariant: {error}"),
    }
}

#[tokio::test]
async fn profile_search_orders_and_formats_like_the_golden_vectors() {
    let document = parse(SEARCH);
    for case in array(&document, "cases") {
        let name = name_of(case);
        let candidates = candidates_from(&case["scenario"]);
        let service = ProfileSearchService::new(MemoryUow, MemorySearch::new(candidates));
        match service.search_profiles(&case["request"]).await {
            Err(error) => {
                assert_eq!(
                    case["ok"],
                    json!(false),
                    "{name}: Rust refuses, the vector accepts"
                );
                assert_eq!(envelope(&error), case["error"], "{name}: error envelope");
            }
            Ok(response) => {
                assert_eq!(
                    case["ok"],
                    json!(true),
                    "{name}: the vector refuses, Rust accepts"
                );
                assert_eq!(to_value(&response), case["response"], "{name}: response");
            }
        }
    }
}

#[test]
fn personal_details_and_revisions_validate_like_the_golden_vectors() {
    let document = parse(PERSONAL);
    for case in array(&document, "requests") {
        let name = name_of(case);
        match canonical_personal_details(&case["request"]) {
            Err(error) => {
                assert_eq!(
                    case["ok"],
                    json!(false),
                    "{name}: Rust refuses, the vector accepts"
                );
                assert_eq!(envelope(&error), case["error"], "{name}: error envelope");
            }
            Ok(input) => {
                assert_eq!(
                    case["ok"],
                    json!(true),
                    "{name}: the vector refuses, Rust accepts"
                );
                let birth = input.date_of_birth.map(|date| {
                    format!(
                        "{:04}-{:02}-{:02}",
                        date.year(),
                        u8::from(date.month()),
                        date.day()
                    )
                });
                assert_eq!(json!(birth), case["dateOfBirth"], "{name}: dateOfBirth");
                assert_eq!(
                    json!(input.nationality),
                    case["nationality"],
                    "{name}: nationality"
                );
            }
        }
    }
    for case in array(&document, "revisions") {
        let name = name_of(case);
        match expected_revision(Some(&case["value"])) {
            Err(error) => {
                assert_eq!(
                    case["ok"],
                    json!(false),
                    "{name}: Rust refuses, the vector accepts"
                );
                assert_eq!(envelope(&error), case["error"], "{name}: error envelope");
            }
            Ok(revision) => {
                assert_eq!(
                    case["ok"],
                    json!(true),
                    "{name}: the vector refuses, Rust accepts"
                );
                assert_eq!(json!(revision), case["revision"], "{name}: revision");
            }
        }
    }
}
