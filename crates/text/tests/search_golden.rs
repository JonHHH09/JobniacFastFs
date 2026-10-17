//! Search text normalization, tokens and prefixes against the golden vectors in
//! `tests/golden/search.json` (synthetic input).
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::{BTreeSet, HashMap};

use jobniac_text::search::{
    MAX_QUERY_CHARACTERS, MAX_QUERY_TOKENS, QueryPrefixes, SearchTerm, normalize, prefixes, tokens,
};
use serde_json::Value;
use uuid::Uuid;

const VECTORS: &str = include_str!("golden/search.json");

fn vectors() -> Value {
    serde_json::from_str(VECTORS).expect("search.json is valid JSON")
}

fn optional_text(value: &Value) -> Option<&str> {
    value.as_str()
}

fn text_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("array of strings")
        .iter()
        .map(|item| item.as_str().expect("string").to_owned())
        .collect()
}

#[test]
fn vector_file_declares_the_query_constants() {
    let file = vectors();
    let constants = &file["metadata"]["constants"];
    assert_eq!(constants["MAX_QUERY_CHARACTERS"], MAX_QUERY_CHARACTERS);
    assert_eq!(constants["MAX_QUERY_TOKENS"], MAX_QUERY_TOKENS);
    assert_eq!(file["metadata"]["excluded_lone_surrogate_cases"], 0);
}

#[test]
fn normalize_and_tokens_match_every_case() {
    let file = vectors();
    let cases = file["cases"].as_array().expect("cases");
    assert!(cases.len() >= 400, "expected the full synthetic corpus");
    for case in cases {
        let input = optional_text(&case["input"]);
        assert_eq!(
            normalize(input),
            case["normalize"].as_str().expect("normalize output"),
            "normalize({input:?})"
        );
        assert_eq!(
            tokens(input),
            text_list(&case["tokens"]),
            "tokens({input:?})"
        );
    }
}

#[test]
fn every_scalar_value_normalizes_like_the_vectors() {
    let file = vectors();
    let mut expected: HashMap<u32, String> = HashMap::new();
    for entry in file["normalize_codepoints"]
        .as_array()
        .expect("code points")
    {
        let code_point =
            u32::try_from(entry["codepoint"].as_u64().expect("code point")).expect("u32");
        let output = entry["normalize"].as_str().expect("output").to_owned();
        assert!(
            expected.insert(code_point, output).is_none(),
            "duplicate U+{code_point:04X}"
        );
    }
    let mut checked = 0_usize;
    let mut mismatches = Vec::new();
    let mut mismatched_code_points = BTreeSet::new();
    for code_point in 0..=0x10FFFF_u32 {
        let Some(character) = char::from_u32(code_point) else {
            continue;
        };
        let input = character.to_string();
        let want = expected
            .get(&code_point)
            .cloned()
            .unwrap_or_else(|| input.clone());
        let actual = normalize(Some(&input));
        if actual != want {
            mismatched_code_points.insert(code_point);
            mismatches.push(format!("U+{code_point:04X}: got {actual:?}, want {want:?}"));
        }
        checked += 1;
    }
    assert_eq!(checked, 0x110000 - 0x800);
    assert!(
        mismatched_code_points.is_empty(),
        "normalize differs from the vectors:\n{}",
        mismatches.join("\n")
    );
}

#[test]
fn normalize_keeps_unicode_17_only_case_pairs_unchanged() {
    // Rust's Unicode 17.0 tables would lower-case these; the vectors (Unicode 16.0.0) keep them.
    for code_point in [0xA7CE_u32, 0xA7D2, 0x16EA0, 0x16EB8] {
        let character = char::from_u32(code_point).expect("scalar value");
        assert_eq!(
            normalize(Some(&character.to_string())),
            character.to_string()
        );
    }
    // Context: a segment break does not change the sigma rule for the rest of the text.
    assert_eq!(normalize(Some("ΑΣ\u{A7CE}ΣΑ")), "ας\u{A7CE}σα");
}

#[test]
fn none_and_blank_text_give_no_tokens() {
    assert_eq!(normalize(None), "");
    assert!(tokens(None).is_empty());
    assert_eq!(normalize(Some("\u{a0}\u{2007}\u{202f}\u{85}\u{1c}")), "");
    assert!(tokens(Some("\u{3000} \t")).is_empty());
}

#[test]
fn prefixes_match_every_case() {
    let file = vectors();
    for case in file["prefixes"].as_array().expect("prefix cases") {
        let query_tokens = text_list(&case["tokens"]);
        let built = prefixes(&query_tokens).expect("vector queries are within the limits");
        assert_eq!(
            built.values(),
            text_list(&case["values"]).as_slice(),
            "values of {query_tokens:?}"
        );
        let owners: Vec<String> = built
            .owners()
            .iter()
            .map(|owner| owner.to_string())
            .collect();
        assert_eq!(
            owners,
            text_list(&case["owners"]),
            "owners of {query_tokens:?}"
        );
    }
}

#[test]
fn prefixes_refuse_only_too_many_tokens() {
    let longest = "a".repeat(MAX_QUERY_CHARACTERS);
    assert!(prefixes(std::slice::from_ref(&longest)).is_some());
    // A normalized token may exceed the raw query bound (`prefixes` has no token bound).
    let expanded = "ffi".repeat(MAX_QUERY_CHARACTERS);
    let built = prefixes(std::slice::from_ref(&expanded)).expect("no per-token bound");
    assert_eq!(built.values().len(), expanded.len());
    let many: Vec<String> = (0..MAX_QUERY_TOKENS + 1)
        .map(|index| index.to_string())
        .collect();
    assert!(prefixes(&many).is_none());
    let owners = prefixes(std::slice::from_ref(&longest)).expect("within limits");
    assert_eq!(owners.owners().len(), MAX_QUERY_CHARACTERS);
    assert!(
        owners
            .owners()
            .windows(2)
            .all(|pair| std::ptr::eq(pair[0].as_ptr(), pair[1].as_ptr()))
    );
}

#[test]
fn query_prefixes_reject_mismatched_lengths() {
    assert!(QueryPrefixes::new(vec!["a".to_owned()], Vec::new()).is_none());
    let built = QueryPrefixes::new(vec!["a".to_owned()], vec![std::sync::Arc::from("a")])
        .expect("equal lengths");
    assert_eq!(built.values(), ["a".to_owned()].as_slice());
}

#[test]
fn search_terms_match_every_case() {
    let file = vectors();
    for case in file["search_terms"].as_array().expect("search term cases") {
        let entity_id =
            Uuid::parse_str(case["entity_id"].as_str().expect("entity id")).expect("uuid");
        let weight = case["weight"].as_i64().expect("weight");
        let field_key = case["field_key"].as_str().expect("field key");
        let text = optional_text(&case["text"]);
        let built = SearchTerm::from_text(entity_id, field_key, text, weight);
        let terms: Vec<(String, i64)> = built
            .iter()
            .map(|term| (term.term.clone(), term.weight))
            .collect();
        let expected: Vec<(String, i64)> = case["terms"]
            .as_array()
            .expect("terms")
            .iter()
            .map(|pair| {
                (
                    pair[0].as_str().expect("term").to_owned(),
                    pair[1].as_i64().expect("weight"),
                )
            })
            .collect();
        assert_eq!(terms, expected, "terms of {text:?}");
        assert!(
            built
                .iter()
                .all(|term| term.entity_id == entity_id && term.field_key == field_key)
        );
    }
}
