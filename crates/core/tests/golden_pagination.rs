//! Cursor golden vectors. The Rust codec must produce byte-identical cursors for the same inputs
//! and make the same accept or reject decision for every decoded or requested cursor.
//!
//! A case marked `d16_tightened` is a UUID spelling (braced, URN or compact) that the recorded
//! decoder accepts but that is refused here. For those the Rust decode must refuse the cursor.

use jobniac_core::clock::{format_instant, parse_instant};
use jobniac_core::env::Environment;
use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::pagination::{
    CURSOR_SECRET_ENV, Cursor, CursorCodec, CursorKey, SearchCandidates, keyset_page,
};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

const VECTORS: &str = include_str!("golden/cursors.json");

fn load() -> Value {
    match serde_json::from_str(VECTORS) {
        Ok(value) => value,
        Err(error) => panic!("cursors.json does not parse: {error}"),
    }
}

fn array<'a>(document: &'a Value, key: &str) -> &'a [Value] {
    match document[key].as_array() {
        Some(items) => items,
        None => panic!("missing array {key}"),
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    match value[key].as_str() {
        Some(found) => found,
        None => panic!("missing string field {key} in {value}"),
    }
}

fn instant(value: &Value, key: &str) -> OffsetDateTime {
    match parse_instant(text(value, key)) {
        Ok(parsed) => parsed,
        Err(error) => panic!("vector instant {key} rejected: {error}"),
    }
}

fn uuid_of(value: &Value, key: &str) -> Uuid {
    match Uuid::parse_str(text(value, key)) {
        Ok(parsed) => parsed,
        Err(error) => panic!("vector uuid {key} rejected: {error}"),
    }
}

fn envelope(error: &AppError) -> Value {
    match serde_json::to_value(error_envelope(error)) {
        Ok(value) => value,
        Err(error) => panic!("envelope does not serialize: {error}"),
    }
}

fn environment_of(env: &Value) -> Environment {
    let pairs: Vec<(String, String)> = env
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_owned()))
                .collect()
        })
        .unwrap_or_default();
    Environment::from_pairs(pairs)
}

fn fixture_codec(document: &Value) -> CursorCodec {
    let environment =
        Environment::from_pairs([(CURSOR_SECRET_ENV, text(document, "fixtureSecret"))]);
    match CursorKey::from_environment(&environment) {
        Ok(key) => CursorCodec::new(key),
        Err(error) => panic!("fixture key rejected: {error}"),
    }
}

fn cursor_json(cursor: &Cursor) -> Value {
    serde_json::json!({
        "scope": cursor.scope,
        "fingerprint": cursor.fingerprint,
        "snapshotAt": format_instant(cursor.snapshot_at),
        "createdAt": format_instant(cursor.created_at),
        "id": cursor.id.to_string(),
    })
}

#[test]
fn configured_keys_match_the_golden_vectors() {
    let document = load();
    for case in array(&document, "keyCases") {
        let label = text(case, "label");
        let environment = environment_of(&case["env"]);
        let key = CursorKey::from_environment(&environment);
        if case["ok"] == Value::Bool(false) {
            match key {
                Ok(_) => panic!("{label}: the vector refuses the key, Rust accepts it"),
                Err(error) => assert_eq!(error.to_string(), text(case, "error"), "{label}"),
            }
            continue;
        }
        let codec = match key {
            Ok(key) => CursorCodec::new(key),
            Err(error) => panic!("{label}: the vector accepts the key, Rust refuses it: {error}"),
        };
        let sample = codec.page_request(None, None, "jobs", "sample");
        let request = match sample {
            Ok(request) => request,
            Err(error) => panic!("{label}: sample request refused: {error:?}"),
        };
        if case["random"] == Value::Bool(true) {
            let encoded = request.next_cursor(
                instant(&document, "snapshotAt"),
                instant(&document, "createdAt"),
                uuid_of(&document, "id"),
            );
            assert!(encoded.is_ok(), "{label}: random key cannot sign");
            continue;
        }
        let encoded = request.next_cursor(
            instant(&document, "snapshotAt"),
            instant(&document, "createdAt"),
            uuid_of(&document, "id"),
        );
        match encoded {
            Ok(encoded) => assert_eq!(encoded, text(case, "sample"), "{label}"),
            Err(error) => panic!("{label}: signing failed: {error}"),
        }
    }
}

#[test]
fn next_cursor_is_byte_identical_to_golden() {
    let document = load();
    let codec = fixture_codec(&document);
    for case in array(&document, "nextCursor") {
        let encoded = codec.next_cursor(
            text(case, "scope"),
            text(case, "fingerprint"),
            instant(case, "snapshotAt"),
            instant(case, "createdAt"),
            uuid_of(case, "id"),
        );
        match encoded {
            Ok(encoded) => assert_eq!(
                encoded,
                text(case, "encoded"),
                "scope {:?}",
                text(case, "scope")
            ),
            Err(error) => panic!("signing failed: {error}"),
        }
    }
}

#[test]
fn decode_decisions_match_the_golden_vectors() {
    let document = load();
    let codec = fixture_codec(&document);
    for case in array(&document, "decode") {
        let label = text(case, "label");
        let recorded = &case["recorded"];
        let encoded = case["encoded"].as_str();
        let rust = codec.decode(encoded);
        if case["d16_tightened"] == Value::Bool(true) {
            assert_eq!(
                recorded["ok"],
                Value::Bool(true),
                "{label}: the recorded decoder must accept this spelling for the tightened case to apply"
            );
            assert!(rust.is_err(), "{label}: this spelling must be refused");
            continue;
        }
        match rust {
            Ok(None) => assert_eq!(recorded["ok"], Value::Bool(true), "{label}"),
            Ok(Some(cursor)) => {
                assert_eq!(
                    recorded["ok"],
                    Value::Bool(true),
                    "{label}: Rust accepts, the vector refuses"
                );
                assert_eq!(cursor_json(&cursor), recorded["cursor"], "{label}");
            }
            Err(error) => {
                assert_eq!(
                    recorded["ok"],
                    Value::Bool(false),
                    "{label}: Rust refuses, the vector accepts"
                );
                assert_eq!(envelope(&error), recorded["error"], "{label}");
            }
        }
    }
}

#[test]
fn page_requests_match_the_golden_vectors() {
    let document = load();
    let codec = fixture_codec(&document);
    let snapshot = instant(&document, "snapshotAt");
    let created = instant(&document, "createdAt");
    let item = uuid_of(&document, "id");
    for case in array(&document, "pageRequest") {
        let limit = case["limit"].as_i64();
        let cursor = case["cursor"].as_str();
        let scope = text(case, "scope");
        let identity = text(case, "filterIdentity");
        let recorded = &case["recorded"];
        match codec.page_request(limit, cursor, scope, identity) {
            Ok(request) => {
                assert_eq!(recorded["ok"], Value::Bool(true), "{scope:?}/{identity:?}");
                assert_eq!(
                    request.limit() as i64,
                    recorded["limit"].as_i64().unwrap_or_default()
                );
                assert_eq!(request.scope(), text(recorded, "scope"));
                assert_eq!(request.fingerprint(), text(recorded, "fingerprint"));
                let decoded = request.cursor().map(cursor_json).unwrap_or(Value::Null);
                assert_eq!(decoded, recorded["cursor"]);
                match request.next_cursor(snapshot, created, item) {
                    Ok(next) => assert_eq!(next, text(recorded, "nextCursor")),
                    Err(error) => panic!("next cursor failed: {error}"),
                }
            }
            Err(error) => {
                assert_eq!(
                    recorded["ok"],
                    Value::Bool(false),
                    "{scope:?}/{identity:?}: Rust refuses"
                );
                assert_eq!(
                    envelope(&error),
                    recorded["error"],
                    "{scope:?}/{identity:?}"
                );
            }
        }
    }
}

#[derive(Debug)]
struct Row {
    label: String,
    created: OffsetDateTime,
    id: Uuid,
}

#[test]
fn keyset_pages_match_the_golden_vectors() {
    let document = load();
    let codec = fixture_codec(&document);
    let snapshot = instant(&document, "snapshotAt");
    for case in array(&document, "keysetPage") {
        let limit = case["limit"].as_i64();
        let scope = text(case, "scope");
        let identity = text(case, "filterIdentity");
        let request = match codec.page_request(limit, None, scope, identity) {
            Ok(request) => request,
            Err(error) => panic!("keyset request refused: {error}"),
        };
        let rows: Vec<Row> = array(case, "rows")
            .iter()
            .map(|row| Row {
                label: text(row, "label").to_owned(),
                created: instant(row, "createdAt"),
                id: uuid_of(row, "id"),
            })
            .collect();
        let page = match keyset_page(&request, rows, snapshot, |row| row.created, |row| row.id) {
            Ok(page) => page,
            Err(error) => panic!("keyset page failed: {error}"),
        };
        let labels: Vec<&str> = page.items.iter().map(|row| row.label.as_str()).collect();
        let expected: Vec<&str> = array(case, "items")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(labels, expected, "limit {limit:?}");
        assert_eq!(
            page.next_cursor.as_deref(),
            case["nextCursor"].as_str(),
            "limit {limit:?}"
        );
    }
}

#[test]
fn search_candidate_invariants_match_the_golden_vectors() {
    let document = load();
    for case in array(&document, "searchCandidates") {
        let matched = match case["matchedCount"].as_i64() {
            Some(value) => value,
            None => panic!("missing matchedCount"),
        };
        let has_more = case["hasMore"] == Value::Bool(true);
        let count = case["itemCount"].as_u64().unwrap_or_default();
        let items: Vec<u64> = (0..count).collect();
        let recorded = &case["recorded"];
        match SearchCandidates::new(matched, has_more, items) {
            Ok(_) => assert_eq!(
                recorded["ok"],
                Value::Bool(true),
                "matched {matched} items {count}"
            ),
            Err(error) => {
                assert_eq!(
                    recorded["ok"],
                    Value::Bool(false),
                    "matched {matched} items {count}"
                );
                assert_eq!(
                    envelope(&error),
                    recorded["error"],
                    "matched {matched} items {count}"
                );
            }
        }
    }
}
