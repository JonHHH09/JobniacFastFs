//! HTML preparation against the golden vectors in `tests/golden/html.json` (synthetic input),
//! which also record the entity and digit tables.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use jobniac_text::html::{HTML4_ENTITIES, bound, bound_list, html4_unescape, plain, plain_bound};
use serde_json::Value;

const VECTORS: &str = include_str!("golden/html.json");

fn vectors() -> Value {
    serde_json::from_str(VECTORS).expect("html.json is valid JSON")
}

fn optional_text(value: &Value) -> Option<&str> {
    value.as_str()
}

#[test]
fn entity_table_equals_the_golden_table_entry_for_entry() {
    let file = vectors();
    let mut expected: BTreeMap<String, u32> = BTreeMap::new();
    let mut numeric_only = Vec::new();
    for pair in file["html4_entities"].as_array().expect("entities") {
        let name = pair[0].as_str().expect("name").to_owned();
        let value = u32::try_from(pair[1].as_u64().expect("value")).expect("u32");
        if name.starts_with('#') {
            numeric_only.push((name, value));
        } else {
            assert!(
                expected.insert(name, value).is_none(),
                "duplicate name in the golden table"
            );
        }
    }
    // The golden table keeps `#39` as a numeric entry that the decoder reaches through the numeric path.
    assert_eq!(numeric_only, vec![("#39".to_owned(), 39)]);
    assert_eq!(expected.len(), 252);

    let rust: BTreeMap<String, u32> = HTML4_ENTITIES
        .iter()
        .map(|&(name, value)| (name.to_owned(), value))
        .collect();
    assert_eq!(
        HTML4_ENTITIES.len(),
        rust.len(),
        "the Rust table has no duplicate names"
    );
    assert_eq!(rust, expected);
    assert_eq!(html4_unescape(Some("&#39;")), "'");
}

#[test]
fn bmp_digit_admission_matches_the_golden_table() {
    let file = vectors();
    let mut digits: BTreeMap<u32, u32> = BTreeMap::new();
    for pair in file["bmp_hex_digits"].as_array().expect("digits") {
        let code_point = u32::try_from(pair[0].as_u64().expect("code point")).expect("u32");
        let value = u32::try_from(pair[1].as_u64().expect("digit")).expect("u32");
        digits.insert(code_point, value);
    }
    assert_eq!(digits.len(), 394);
    // A reference `&#<c>;` decodes only when `c` is a digit below the radix; otherwise it stays literal.
    for code_point in 0..=0xFFFF_u32 {
        let Some(character) = char::from_u32(code_point) else {
            continue;
        };
        let digit = digits.get(&code_point).copied();
        let decimal_input = format!("&#{character};");
        let decimal_want = match digit {
            Some(value) if value < 10 => char::from_u32(value).expect("digit").to_string(),
            _ => decimal_input.clone(),
        };
        assert_eq!(
            html4_unescape(Some(&decimal_input)),
            decimal_want,
            "decimal U+{code_point:04X}"
        );

        let hex_input = format!("&#x{character};");
        let hex_want = match digit {
            Some(value) => char::from_u32(value).expect("digit").to_string(),
            None => hex_input.clone(),
        };
        assert_eq!(
            html4_unescape(Some(&hex_input)),
            hex_want,
            "hex U+{code_point:04X}"
        );
    }
    // Astral digits are not in the BMP table.
    assert_eq!(html4_unescape(Some("&#\u{1D7D8};")), "&#\u{1D7D8};");
}

#[test]
fn unescape_matches_every_case() {
    let file = vectors();
    let cases = file["cases"].as_array().expect("cases");
    assert!(cases.len() >= 400, "expected the full synthetic corpus");
    for case in cases {
        let input = optional_text(&case["input"]);
        assert_eq!(
            html4_unescape(input),
            case["unescape"].as_str().expect("unescape"),
            "html4_unescape({input:?})"
        );
    }
}

#[test]
fn plain_matches_every_case() {
    let file = vectors();
    for case in file["cases"].as_array().expect("cases") {
        let input = optional_text(&case["input"]);
        assert_eq!(
            plain(input),
            case["plain"].as_str().expect("plain"),
            "plain({input:?})"
        );
    }
}

#[test]
fn bound_and_plain_bound_match_every_cap() {
    let file = vectors();
    let mut checked = 0_usize;
    for case in file["cases"].as_array().expect("cases") {
        let input = optional_text(&case["input"]);
        for entry in case["bounds"].as_array().expect("bounds") {
            let cap = usize::try_from(entry["cap"].as_u64().expect("cap")).expect("usize");
            assert_eq!(
                bound(input, cap),
                entry["bound"].as_str().expect("bound"),
                "bound({input:?}, {cap})"
            );
            assert_eq!(
                plain_bound(input, cap),
                entry["plain_bound"].as_str().expect("plain_bound"),
                "plain_bound({input:?}, {cap})"
            );
            checked += 1;
        }
    }
    assert!(checked > 5000);
}

#[test]
fn bound_list_matches_every_case() {
    let file = vectors();
    for case in file["bound_list"].as_array().expect("bound list cases") {
        let input = case["input"].as_array().map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect::<Vec<String>>()
        });
        let input_slice = input.as_deref();
        let actual = bound_list(input_slice);
        let expected: Vec<String> = case["output"]
            .as_array()
            .expect("output")
            .iter()
            .map(|item| item.as_str().expect("entry").to_owned())
            .collect();
        assert_eq!(actual, expected, "bound_list({input_slice:?})");
    }
}

#[test]
fn adversarial_unclosed_openers_stay_linear() {
    // Regression: an unclosed opener used to rescan the rest of the text, which is quadratic.
    let started = std::time::Instant::now();
    let openers = "<script>".repeat(200_000);
    assert_eq!(plain(Some(&openers)), "");
    let unterminated = "<style".repeat(200_000);
    // Without `>` there is no tag: only the `<` characters become spaces, the words stay, and the
    // result is capped at 1,000 code points.
    let words = plain(Some(&unterminated));
    assert!(words.starts_with("style style") && words.chars().count() <= 1000);
    // The cap may cut the last word, so every word is a prefix of "style".
    assert!(words.split(' ').all(|word| "style".starts_with(word)));
    let mixed = "<script><style>".repeat(50_000) + "</script>";
    assert_eq!(plain(Some(&mixed)), "");
    // Strict in an optimised run; unoptimised test builds on a slow runner get a loose bound that a
    // quadratic rescan (minutes at this size) still exceeds.
    let bound = if cfg!(debug_assertions) {
        std::time::Duration::from_secs(20)
    } else {
        std::time::Duration::from_secs(1)
    };
    assert!(
        started.elapsed() < bound,
        "unclosed openers took {:?}",
        started.elapsed()
    );
}

#[test]
fn none_is_empty_text() {
    assert_eq!(html4_unescape(None), "");
    assert_eq!(plain(None), "");
    assert_eq!(bound(None, 10), "");
    assert_eq!(plain_bound(None, 10), "");
    assert!(bound_list::<&str>(None).is_empty());
}

#[test]
fn surrogate_references_become_replacement_characters() {
    // Rust strings cannot hold a lone surrogate, so the vectors hold no output for one.
    assert_eq!(html4_unescape(Some("a&#xD800;b")), "a\u{FFFD}b");
    assert_eq!(html4_unescape(Some("&#55296;")), "\u{FFFD}");
    assert_eq!(html4_unescape(Some("&#xDFFF;")), "\u{FFFD}");
    assert_eq!(plain(Some("a&#xD800;b")), "a b");
}

#[test]
fn span_limit_counts_utf16_units_including_the_ampersand() {
    // `&#x000041;` spans 9 UTF-16 units and decodes; `&#x0000041;` spans 10 and stays literal.
    assert_eq!(html4_unescape(Some("&#x000041;")), "A");
    assert_eq!(html4_unescape(Some("&#x0000041;")), "&#x0000041;");
    // An astral character inside the span counts two units.
    assert_eq!(html4_unescape(Some("&#x\u{1F600}1;")), "&#x\u{1F600}1;");
}

#[test]
fn bound_list_keeps_twenty_non_blank_entries() {
    let mut values: Vec<String> = Vec::new();
    for index in 0..30 {
        values.push(if index % 2 == 0 {
            " ".to_owned()
        } else {
            format!("v{index}")
        });
    }
    let result = bound_list(Some(values.as_slice()));
    assert_eq!(result.len(), 15);
    let many: Vec<String> = (0..25).map(|index| format!("w{index}")).collect();
    let capped = bound_list(Some(many.as_slice()));
    assert_eq!(capped.len(), 20);
    assert_eq!(capped.last().map(String::as_str), Some("w19"));
}
