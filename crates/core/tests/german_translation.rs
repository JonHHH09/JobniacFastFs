//! German translation tests: the six translation tests and the replay of the golden vectors in
//! `golden/german/translation_pairs.json`.
//!
//! The vector file uses a synthetic glossary: the vectors compare the algorithm, not the phrases.
//! Every text is synthetic. The structured `to_german` test belongs to the content models and is
//! covered with them.

use jobniac_core::errors::ApplicationError;
use jobniac_core::generation::translation::{
    Glossary, USER_GLOSSARY_FORMAT, USER_GLOSSARY_VERSION, UserGlossary,
};
use jobniac_core::german::translation::{
    BUILTIN_LABELS, BUILTIN_PHRASES, BUILTIN_WORDS, GermanResumeTranslator, builtin_glossary,
    preserve_case,
};
use serde_json::{Value, json};

const PAIRS: &str = include_str!("golden/german/translation_pairs.json");

fn vectors() -> Value {
    serde_json::from_str(PAIRS).expect("translation vectors parse")
}

fn text(value: &Value) -> &str {
    value.as_str().expect("string")
}

fn pairs(value: &Value) -> Vec<(String, String)> {
    value
        .as_array()
        .expect("pairs")
        .iter()
        .map(|pair| (text(&pair[0]).to_owned(), text(&pair[1]).to_owned()))
        .collect()
}

fn synthetic_glossary() -> Glossary {
    let root = vectors();
    Glossary {
        labels: pairs(&root["glossary"]["labels"]).into_iter().collect(),
        phrases: pairs(&root["glossary"]["phrases"]),
        words: pairs(&root["glossary"]["words"]).into_iter().collect(),
    }
}

fn translator() -> GermanResumeTranslator {
    GermanResumeTranslator::new(&synthetic_glossary())
}

/// An input member: a string, or the expansion of a `<key>_repeat` spec; `None` for null/absent.
fn input(case: &Value, key: &str) -> Option<String> {
    if let Some(spec) = case.get(format!("{key}_repeat")) {
        let count = spec["count"].as_u64().expect("count") as usize;
        return Some(text(&spec["text"]).repeat(count));
    }
    case.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn envelope(error: &ApplicationError) -> Value {
    serde_json::to_value(error.to_envelope()).expect("envelope serializes")
}

fn translate(translator: &GermanResumeTranslator, value: &str) -> String {
    translator
        .translate_text(Some(value))
        .expect("translate")
        .expect("some")
}

fn label(translator: &GermanResumeTranslator, value: &str) -> String {
    translator
        .translate_label(Some(value))
        .expect("label")
        .expect("some")
}

#[test]
fn replays_every_golden_vector() {
    let translator = translator();
    let root = vectors();
    let cases = root["cases"].as_array().expect("cases");
    assert!(cases.len() > 200);
    let mut errors = 0;
    for case in cases {
        let id = text(&case["id"]);
        let value = input(case, "value");
        let result: Result<Option<String>, ApplicationError> = match text(&case["kind"]) {
            "text" => translator.translate_text(value.as_deref()),
            "label" => translator.translate_label(value.as_deref()),
            "personal" => translator
                .translate_personal_field_value(input(case, "label").as_deref(), value.as_deref()),
            "preserve_case" => preserve_case(
                &input(case, "original").expect("original"),
                &input(case, "replacement").expect("replacement"),
            )
            .map(Some),
            other => panic!("unknown kind {other}"),
        };
        if case["ok"].as_bool().expect("ok") {
            let got = result.unwrap_or_else(|error| panic!("{id}: unexpected error {error}"));
            if case.get("output_same_as_input").is_some() {
                assert_eq!(got, value, "{id}");
            } else {
                let expected = case["output"].as_str().map(str::to_owned);
                assert_eq!(got, expected, "{id}");
            }
        } else {
            errors += 1;
            let error = result.expect_err(id);
            assert_eq!(envelope(&error), case["error"], "{id}");
        }
    }
    assert_eq!(errors, 6);
}

// Literal source translation (synthetic table).
#[test]
fn literal_source_translation() {
    let translator = translator();
    for (source, expected) in [
        ("Widget Builder", "Widget-Bauer"),
        ("WIDGET BUILDER", "Widget-Bauer"),
        ("prefix WIDGET BUILDER suffix", "prefix Widget-Bauer suffix"),
        ("B.A. Widget Science", "B.A. Widget-Lehre"),
        ("Built the data systems", "Erstellte Daten Systeme"),
        ("BUILT DATA SYSTEMS", "ERSTELLTE DATEN SYSTEME"),
        ("Built systems.", "Erstellte systems."),
        ("a  an the", ""),
        ("Built\t\tdata", "Erstellte Daten"),
        ("Built\tdata", "Erstellte\tDaten"),
        ("tool@systems.example", "Werkzeug@systems.example"),
        ("widget@systems.example", "widget@systems.example"),
        ("built@data", "erstellte@Daten"),
        ("https://unknown/data", "https://unknown/data"),
        ("\u{672A}\u{77E5} \u{1F600}", "\u{672A}\u{77E5} \u{1F600}"),
        ("\u{17F}cience lab", "Wissenschaftslabor"),
        ("prefix \u{17F}cience lab", "prefix \u{17F}cience lab"),
        ("f\u{131}eld test", "Feldtest"),
        ("\u{A0}Built\u{A0}", "\u{A0}Erstellte\u{A0}"),
        ("   ", "   "),
    ] {
        assert_eq!(translate(&translator, source), expected, "{source:?}");
    }
    assert_eq!(translator.translate_text(None).expect("none"), None);
}

// Literal labels (synthetic table).
#[test]
fn literal_labels() {
    let translator = translator();
    for (source, expected) in [
        (" email ", "E-Mail"),
        ("NATIVE", "Muttersprache"),
        ("cloud / ops", "Cloud / Betrieb"),
        ("fluent", "flie\u{DF}end"),
        ("unknown", "unbekannt"),
        ("\t", "\t"),
    ] {
        assert_eq!(label(&translator, source), expected, "{source:?}");
    }
    assert_eq!(translator.translate_label(None).expect("none"), None);
}

// The original personal label controls value translation.
#[test]
fn original_personal_label_controls_value_translation() {
    let translator = translator();
    for label in ["address", "Location", " nationality "] {
        assert_eq!(
            translator
                .translate_personal_field_value(Some(label), Some("Alpha"))
                .expect("personal"),
            Some("beta".to_owned()),
            "{label:?}"
        );
    }
    for label in [Some("Wohnort"), Some("email"), None, Some(" ")] {
        assert_eq!(
            translator
                .translate_personal_field_value(label, Some("Alpha"))
                .expect("personal"),
            Some("Alpha".to_owned()),
            "{label:?}"
        );
    }
}

// Case preservation on the first UTF-16 character.
#[test]
fn preserve_case_utf16_first_char() {
    for (original, replacement, expected) in [
        ("BUILT", "erstellte", "ERSTELLTE"),
        ("Built", "erstellte", "Erstellte"),
        ("Built", "\u{10428}x", "\u{10428}x"),
        ("built", "erstellte", "erstellte"),
        ("A", "ersatz", "Ersatz"),
        ("\u{10400}x", "ersatz", "ersatz"),
        ("", "ersatz", "ersatz"),
    ] {
        assert_eq!(
            preserve_case(original, replacement).expect("case"),
            expected,
            "{original:?}"
        );
    }
}

// Single-value and aggregate limits are safe (the `to_german(None)` half has no
// equivalent: a Rust content value cannot be null).
#[test]
fn single_value_limit_is_safe() {
    let translator = translator();
    let at_limit = "x".repeat(1_000_000);
    assert_eq!(translate(&translator, &at_limit), at_limit);
    let error = translator
        .translate_text(Some(&"x".repeat(1_000_001)))
        .expect_err("over the limit");
    let envelope = envelope(&error);
    assert_eq!(envelope["message"], "Resume preparation limit exceeded");
    assert_eq!(envelope["details"]["field"], "text");
}

#[test]
fn uppercase_original_with_empty_replacement_is_refused() {
    let error = preserve_case("Built", "").expect_err("refused");
    assert_eq!(
        envelope(&error)["message"],
        "Invalid German translation input"
    );
    assert_eq!(preserve_case("built", "").expect("lower"), "");
}

#[test]
fn builtin_tables_are_generic_and_consistent() {
    let glossary = builtin_glossary();
    assert_eq!(glossary.labels.len(), BUILTIN_LABELS.len());
    assert_eq!(glossary.phrases.len(), BUILTIN_PHRASES.len());
    assert_eq!(glossary.words.len(), BUILTIN_WORDS.len());
    for (key, _) in BUILTIN_LABELS {
        assert_eq!(*key, key.to_lowercase());
    }
    for (key, _) in BUILTIN_WORDS {
        assert!(
            key.chars()
                .all(|c| c.is_ascii_lowercase() || "+./-".contains(c))
        );
    }
    let translator = GermanResumeTranslator::builtin();
    assert_eq!(
        translate(&translator, "Built the data systems"),
        "Erstellte Daten Systeme"
    );
    assert_eq!(label(&translator, " Email "), "E-Mail");
    assert_eq!(
        translate(&translator, "Field of study: data"),
        "Studienfach: Daten"
    );
}

#[test]
fn user_glossary_extends_the_builtin_tables() {
    let document = json!({
        "format": USER_GLOSSARY_FORMAT,
        "version": USER_GLOSSARY_VERSION,
        "labels": {"Mentor": "Mentorin"},
        "phrases": [{"source": "Example Platform", "target": "Beispiel-Plattform"}],
        "words": {"tool": "Werkzeug", "built": "baute"},
    })
    .to_string();
    let user = UserGlossary::parse(&document).expect("synthetic user glossary");
    let translator = GermanResumeTranslator::new(&builtin_glossary().merged_with(&user));
    assert_eq!(label(&translator, "mentor"), "Mentorin");
    assert_eq!(
        translate(&translator, "example platform"),
        "Beispiel-Plattform"
    );
    assert_eq!(
        translate(&translator, "Built the tool on Example Platform"),
        "Baute Werkzeug on Beispiel-Plattform"
    );
}
