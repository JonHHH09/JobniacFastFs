//! Replays the French translation golden vectors (`golden/generation/translation.json`).
//!
//! The vectors use a generic glossary: they compare the algorithm, not the phrases. Every case is
//! synthetic. The extra tests cover the user glossary file and the budget refusals.

use jobniac_core::errors::ApplicationError;
use jobniac_core::generation::translation::{
    FrenchTranslator, Glossary, USER_GLOSSARY_FORMAT, UserGlossary, preserve_case, unicode_fold,
};
use serde_json::{Value, json};

const TRANSLATION: &str = include_str!("golden/generation/translation.json");

fn vectors() -> Value {
    serde_json::from_str(TRANSLATION).expect("translation vectors parse")
}

fn text(value: &Value) -> &str {
    value.as_str().expect("string")
}

/// Expands the `repeat` spec of an oversized input, or returns the plain string.
fn expand(case: &Value, plain: &str, repeat: &str) -> String {
    if let Some(spec) = case.get(repeat) {
        let unit = text(&spec["text"]);
        let count = spec["count"].as_u64().expect("count") as usize;
        return unit.repeat(count);
    }
    plain.to_owned()
}

/// The envelope of a translation budget refusal, in the wire shape.
fn budget_envelope(error: jobniac_text::java::PreparationLimitExceeded) -> Value {
    let application = ApplicationError::from(error);
    serde_json::to_value(application.to_envelope()).expect("envelope serializes")
}

fn user_glossary() -> Glossary {
    let document = text(&vectors()["user_glossary_text"]).to_owned();
    let user = UserGlossary::parse(&document).expect("synthetic user glossary parses");
    Glossary::builtin().merged_with(&user)
}

fn translator_for(glossary: &str) -> FrenchTranslator {
    match glossary {
        "generic" => FrenchTranslator::builtin(),
        "user" => FrenchTranslator::new(&user_glossary()),
        other => panic!("unknown glossary {other}"),
    }
}

#[test]
fn builtin_glossary_equals_the_generic_tables_of_the_vector_file() {
    let root = vectors();
    let builtin = Glossary::builtin();
    let labels: Vec<(String, String)> = root["glossary"]["labels"]
        .as_array()
        .expect("labels")
        .iter()
        .map(|pair| (text(&pair[0]).to_owned(), text(&pair[1]).to_owned()))
        .collect();
    let mut ported_labels: Vec<(String, String)> = builtin
        .labels
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    ported_labels.sort();
    assert_eq!(ported_labels, labels);
    let phrases: Vec<(String, String)> = root["glossary"]["phrases"]
        .as_array()
        .expect("phrases")
        .iter()
        .map(|pair| (text(&pair[0]).to_owned(), text(&pair[1]).to_owned()))
        .collect();
    assert_eq!(builtin.phrases, phrases);
    let words: Vec<(String, String)> = root["glossary"]["words"]
        .as_array()
        .expect("words")
        .iter()
        .map(|pair| (text(&pair[0]).to_owned(), text(&pair[1]).to_owned()))
        .collect();
    let mut ported_words: Vec<(String, String)> = builtin
        .words
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    ported_words.sort();
    assert_eq!(ported_words, words);
}

/// The fold of every code point must equal the recorded fold. The recorded run uses Unicode
/// 16.0 (recorded in the vector); the Rust standard library may use a newer Unicode version. A
/// difference is accepted only for a code point that the recorded run leaves unassigned.
#[test]
fn unicode_fold_matches_the_golden_vectors_for_every_code_point() {
    let root = vectors();
    let mut expected = std::collections::HashMap::new();
    for pair in root["fold"]["folds"].as_array().expect("folds") {
        let code = pair[0].as_u64().expect("code") as u32;
        let folded = pair[1].as_u64().expect("folded") as u32;
        expected.insert(code, folded);
    }
    let unassigned: Vec<(u32, u32)> = root["fold"]["unassigned"]
        .as_array()
        .expect("unassigned")
        .iter()
        .map(|pair| {
            (
                pair[0].as_u64().expect("start") as u32,
                pair[1].as_u64().expect("end") as u32,
            )
        })
        .collect();
    let is_unassigned = |code: u32| {
        unassigned
            .iter()
            .any(|&(start, end)| start <= code && code <= end)
    };
    let mut checked = 0_u64;
    let mut non_identity_checked = 0_u64;
    let mut mismatches = Vec::new();
    let mut version_differences = 0_u32;
    for code in 0..=0x10FFFF_u32 {
        let Some(character) = char::from_u32(code) else {
            continue;
        };
        checked += 1;
        // A code point missing from the vector is identity here; the count check below
        // proves that no non-identity fold was silently treated as identity.
        let want = expected.get(&code).copied().unwrap_or(code);
        if want != code {
            non_identity_checked += 1;
        }
        let got = u32::from(unicode_fold(character));
        if got != want {
            if is_unassigned(code) {
                version_differences += 1;
            } else {
                mismatches.push((code, got, want));
            }
        }
    }
    assert_eq!(checked, root["fold"]["checked"].as_u64().expect("checked"));
    let vector_non_identity = expected
        .iter()
        .filter(|(code, folded)| code != folded)
        .count();
    assert_eq!(
        vector_non_identity as u64, non_identity_checked,
        "vector fold count must equal the non-identity code points checked"
    );
    assert_eq!(
        expected.len(),
        root["fold"]["folds"].as_array().expect("folds").len(),
        "duplicate code points in the fold vector"
    );
    assert!(mismatches.is_empty(), "fold mismatches: {mismatches:?}");
    assert!(
        version_differences <= 64,
        "{version_differences} differences in unassigned code points"
    );
}

#[test]
fn translation_cases_match_the_golden_vectors() {
    let root = vectors();
    let cases = root["cases"].as_array().expect("cases");
    assert!(cases.len() >= 60, "{} cases", cases.len());
    for case in cases {
        let id = text(&case["id"]);
        match text(&case["kind"]) {
            "preserve_case" => {
                let output = preserve_case(text(&case["original"]), text(&case["replacement"]));
                assert_eq!(output, text(&case["output"]), "{id}");
            }
            "label" => {
                let translator = translator_for(text(&case["glossary"]));
                let value = text(&case["value"]);
                let output = translator.translate_label(value).expect("label budget");
                assert_eq!(output, text(&case["output"]), "{id}");
            }
            "narrative" => {
                let translator = translator_for(text(&case["glossary"]));
                let value = expand(case, case.get("value").map_or("", text), "value_repeat");
                let mut protected: Vec<String> = case["protected"]
                    .as_array()
                    .map(|items| items.iter().map(|item| text(item).to_owned()).collect())
                    .unwrap_or_default();
                if let Some(spec) = case.get("protected_repeat") {
                    protected.push(
                        text(&spec["text"]).repeat(spec["count"].as_u64().expect("count") as usize),
                    );
                }
                let refs: Vec<&str> = protected.iter().map(String::as_str).collect();
                let outcome = translator
                    .prepare_protected_terms(&refs)
                    .map_err(budget_envelope)
                    .and_then(|prepared| {
                        translator
                            .translate_narrative(&value, &prepared)
                            .map_err(budget_envelope)
                    });
                match outcome {
                    Ok(output) => {
                        assert_eq!(case["ok"], json!(true), "{id}: expected an error");
                        assert_eq!(output, text(&case["output"]), "{id}");
                    }
                    Err(envelope) => {
                        assert_eq!(
                            case["ok"],
                            json!(false),
                            "{id}: unexpected error {envelope}"
                        );
                        assert_eq!(envelope, case["error"], "{id}");
                    }
                }
            }
            other => panic!("unknown kind {other}"),
        }
    }
}

#[test]
fn user_glossary_document_is_refused_when_it_breaks_the_format() {
    let wrong_version = json!({"format": USER_GLOSSARY_FORMAT, "version": 2}).to_string();
    assert!(UserGlossary::parse(&wrong_version).is_err());
    let not_json = "{";
    assert!(UserGlossary::parse(not_json).is_err());
}

#[test]
fn user_glossary_reads_phrases_in_document_order() {
    let document = json!({
        "format": USER_GLOSSARY_FORMAT,
        "version": 1,
        "phrases": [
            {"source": "Tool Alpha", "target": "Outil Alpha"},
            {"source": "Tool Beta", "target": "Outil Bêta"}
        ]
    })
    .to_string();
    let user = UserGlossary::parse(&document).expect("parse");
    assert_eq!(user.phrases[0].0, "Tool Alpha");
    let translator = FrenchTranslator::new(&Glossary::builtin().merged_with(&user));
    let output = translator
        .translate_narrative_unprotected("tool beta and Tool Alpha")
        .expect("translate");
    assert_eq!(output, "Outil Bêta and Outil Alpha");
}

#[test]
fn oversized_protected_term_list_is_refused_before_any_translation() {
    let translator = FrenchTranslator::builtin();
    let terms: Vec<&str> = vec!["x"; 10_001];
    let error = translator
        .prepare_protected_terms(&terms)
        .expect_err("entry limit");
    let envelope = budget_envelope(error);
    assert_eq!(envelope["code"], json!("validation_error"));
    assert_eq!(envelope["details"]["field"], json!("text"));
    assert_eq!(
        envelope["details"]["reason"],
        json!("exceeds preparation safety budget")
    );
}
