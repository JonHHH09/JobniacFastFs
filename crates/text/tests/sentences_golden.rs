//! Golden corpus test for sentence boundaries.
//!
//! The corpus holds reference offsets for synthetic texts, produced with an OpenJDK-derived
//! sentence iterator. Every text must either match the reference offsets, or be listed in
//! `tests/golden/sentences-accepted-differences.json` with a category and the exact Rust offsets.
//! The category is recomputed here by `classify_boundary` from the text, never trusted from the
//! file, so a listed category that the rules do not produce fails the test. A listed text that now
//! matches the reference is also an error, so the list cannot go stale silently.
//!
//! The reference results for inputs that a Rust `String` cannot carry (lone surrogates, explicit
//! surrogate pairs) and for the 1,000,000 UTF-16 unit preparation cap are checked against
//! `tests/golden/sentences-expected-errors.json`.
//!
//! After the corpus or the classifier changes, rewrite the accepted list with
//! `cargo test -p jobniac-text --test sentences_golden -- --ignored write_accepted_differences`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::{BTreeMap, BTreeSet};

use jobniac_text::sentences::{
    SentenceError, checked_sentence_boundaries, sentence_boundaries, sentence_spans,
};
use serde_json::{Value, json};
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_segmentation::UNICODE_VERSION;

const CORPUS: &str = include_str!("golden/sentences.json");
const ACCEPTED: &str = include_str!("golden/sentences-accepted-differences.json");
const EXPECTED_ERRORS: &str = include_str!("golden/sentences-expected-errors.json");
const ACCEPTED_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/golden/sentences-accepted-differences.json"
);

/// Classifier categories with their meaning. The first category (in this order) that applies to a
/// differing boundary names it; a text is named by its highest-priority boundary.
const CATEGORIES: [(&str, &str); 14] = [
    (
        "unicode-version",
        "A character assigned in Unicode 17.0 and unassigned in the reference data (Unicode 16.0.0) is within two code points of the boundary",
    ),
    (
        "noncharacter",
        "A noncharacter (U+FFFE, U+FFFF and their plane equivalents) is within two code points of the boundary; the reference segmentation ends or changes processing there",
    ),
    (
        "line-separator",
        "A CR, LF, NEL, U+2028 or U+2029 is within two code points of the boundary; UAX #29 breaks after each one (rule SB4), the reference segmentation breaks differently",
    ),
    (
        "bullet-or-list-marker",
        "A bullet (U+2022), a numbered marker (1. or 1)), a parenthesised marker ((a)) or a dash or asterisk item is the token next to the boundary",
    ),
    (
        "ellipsis-character",
        "U+2026 HORIZONTAL ELLIPSIS is within two code points of the boundary",
    ),
    (
        "terminator-not-in-reference",
        "A character that UAX #29 treats as a sentence terminator and the reference segmentation does not (for example U+061F, U+203D, U+3002, U+FF61)",
    ),
    (
        "terminator-before-symbol",
        "An astral character (U+1F000 and above: emoji, regional indicators, symbols) is within two code points of the boundary",
    ),
    (
        "combining-or-format-mark",
        "A nonspacing, spacing or enclosing mark, or a format character (Cf), is within two code points of the boundary",
    ),
    (
        "unassigned-or-control",
        "An unassigned, control, private-use or surrogate-class code point is within two code points of the boundary",
    ),
    (
        "abbreviation-internal-period",
        "A period, after the last non-space character before the boundary, is followed by a hyphen or a digit (for example c.-a-d. or Sec. 4.2)",
    ),
    (
        "punctuation-after-terminator",
        "Punctuation such as ; : ! ? ( ) guillemets or curly quotes is within two code points of the boundary",
    ),
    (
        "non-ascii-space-after-terminator",
        "A non-ASCII space (NBSP, U+2007, U+202F, U+3000) is within two code points of the boundary",
    ),
    (
        "period-prefixed-token",
        "A stand-alone period token precedes the boundary and the next word starts with a letter",
    ),
    (
        "other",
        "Not explained by the categories above; must stay unused (the test fails if a text needs it)",
    ),
];

struct Case {
    id: String,
    text: String,
    reference: Vec<usize>,
}

/// An accepted entry: primary category, other categories, pinned reference and Rust offsets.
type Listed = (String, Vec<String>, Vec<usize>, Vec<usize>);

fn offsets(value: &Value) -> Vec<usize> {
    value
        .as_array()
        .expect("boundaries are an array")
        .iter()
        .map(|item| usize::try_from(item.as_u64().expect("offset is an integer")).expect("fits"))
        .collect()
}

fn load_cases() -> Vec<Case> {
    let document: Value = serde_json::from_str(CORPUS).expect("corpus is valid JSON");
    document["cases"]
        .as_array()
        .expect("cases are an array")
        .iter()
        .map(|case| Case {
            id: case["id"].as_str().expect("id").to_owned(),
            text: case["text"].as_str().expect("text").to_owned(),
            reference: offsets(&case["boundaries"]),
        })
        .collect()
}

fn unicode_version_text() -> String {
    format!(
        "{}.{}.{}",
        UNICODE_VERSION.0, UNICODE_VERSION.1, UNICODE_VERSION.2
    )
}

/// Code points assigned in Unicode 17.0 that the reference data (Unicode 16.0.0) reads as unassigned.
fn is_unicode17_addition(ch: char) -> bool {
    matches!(
        u32::from(ch),
        0xA7CE | 0xA7D2 | 0xA7D4 | 0xA7F1 | 0x16EA0..=0x16EB8
    )
}

/// UAX #29 sentence terminators that the reference segmentation does not treat as sentence ends.
fn is_terminator_not_in_reference(ch: char) -> bool {
    matches!(
        u32::from(ch),
        0x37E | 0x61F | 0x203C | 0x203D | 0x2047
            ..=0x2049 | 0x2E2E | 0x3002 | 0xFF01 | 0xFF1F | 0xFF61
    )
}

/// Punctuation that the classifier treats as a neighbour of a terminator.
fn is_punctuation(ch: char) -> bool {
    matches!(
        ch,
        ';' | ':'
            | '!'
            | '?'
            | '.'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '"'
            | '\''
            | '«'
            | '»'
            | '“'
            | '”'
            | '‘'
            | '’'
            | '،'
            | '؟'
            | '！'
            | '？'
            | '：'
            | '；'
            | '。'
            | '．'
    )
}

fn is_non_ascii_space(ch: char) -> bool {
    matches!(ch, '\u{a0}' | '\u{2007}' | '\u{202f}' | '\u{3000}')
}

/// A bullet, a dash or asterisk item, a numbered marker such as `2.` or `3)`, or a `(a)` marker.
fn is_list_marker(token: &str) -> bool {
    if matches!(token, "•" | "*" | "-" | "–") {
        return true;
    }
    if let Some(inner) = token
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return !inner.is_empty() && inner.chars().all(char::is_alphanumeric);
    }
    match token.strip_suffix(['.', ')']) {
        Some(digits) => !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()),
        None => false,
    }
}

/// The whitespace-delimited token that starts at or after `at`.
fn token_after(chars: &[char], at: usize) -> String {
    chars
        .get(at..)
        .unwrap_or_default()
        .iter()
        .copied()
        .skip_while(|ch| ch.is_whitespace())
        .take_while(|ch| !ch.is_whitespace())
        .collect()
}

/// The index of the last non-whitespace character before `at`, if any.
fn last_non_space_before(chars: &[char], at: usize) -> Option<usize> {
    (0..at.min(chars.len()))
        .rev()
        .find(|&index| !chars[index].is_whitespace())
}

/// The whitespace-delimited token that ends just before `end`.
fn token_before(chars: &[char], end: usize) -> String {
    let mut end_index = end.min(chars.len());
    while end_index > 0 && chars[end_index - 1].is_whitespace() {
        end_index -= 1;
    }
    let mut start_index = end_index;
    while start_index > 0 && !chars[start_index - 1].is_whitespace() {
        start_index -= 1;
    }
    chars[start_index..end_index].iter().collect()
}

/// The first non-whitespace character at or after `at`, if any.
fn first_non_space_from(chars: &[char], at: usize) -> Option<char> {
    chars
        .get(at..)?
        .iter()
        .copied()
        .find(|ch| !ch.is_whitespace())
}

/// Names the reason for one differing boundary at code-point offset `at` (the first category in
/// [`CATEGORIES`] order that applies to the characters within two code points of it).
fn classify_boundary(chars: &[char], at: usize) -> &'static str {
    let window = &chars[at.saturating_sub(2)..(at + 2).min(chars.len())];
    let any = |predicate: fn(char) -> bool| window.iter().copied().any(predicate);
    let any_category = |categories: &[GeneralCategory]| {
        window
            .iter()
            .any(|&ch| categories.contains(&get_general_category(ch)))
    };
    if any(is_unicode17_addition) {
        return "unicode-version";
    }
    if any(|ch| (u32::from(ch) & 0xFFFE) == 0xFFFE) {
        return "noncharacter";
    }
    if any(|ch| matches!(ch, '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}')) {
        return "line-separator";
    }
    if any(|ch| ch == '•')
        || is_list_marker(&token_after(chars, at))
        || is_list_marker(&token_before(chars, at))
    {
        return "bullet-or-list-marker";
    }
    if any(|ch| ch == '\u{2026}') {
        return "ellipsis-character";
    }
    if any(is_terminator_not_in_reference) {
        return "terminator-not-in-reference";
    }
    if any(|ch| u32::from(ch) >= 0x1F000) {
        return "terminator-before-symbol";
    }
    if any_category(&[
        GeneralCategory::NonspacingMark,
        GeneralCategory::SpacingMark,
        GeneralCategory::EnclosingMark,
        GeneralCategory::Format,
    ]) {
        return "combining-or-format-mark";
    }
    if any_category(&[
        GeneralCategory::Unassigned,
        GeneralCategory::Control,
        GeneralCategory::PrivateUse,
        GeneralCategory::Surrogate,
    ]) {
        return "unassigned-or-control";
    }
    let before = last_non_space_before(chars, at).map(|index| chars[index]);
    let after = first_non_space_from(chars, at);
    if before == Some('.') && after.is_some_and(|ch| ch == '-' || ch.is_ascii_digit()) {
        return "abbreviation-internal-period";
    }
    if any(is_punctuation) {
        return "punctuation-after-terminator";
    }
    if any(is_non_ascii_space) {
        return "non-ascii-space-after-terminator";
    }
    if last_non_space_before(chars, at)
        .is_some_and(|index| chars[index] == '.' && token_before(chars, index + 1) == ".")
    {
        return "period-prefixed-token";
    }
    "other"
}

/// Returns the (primary, also) categories of a text, or `None` when its boundaries match the reference.
fn classify_text(text: &str, reference: &[usize], rust: &[usize]) -> Option<(String, Vec<String>)> {
    if reference == rust {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let reference_set: BTreeSet<usize> = reference.iter().copied().collect();
    let rust_set: BTreeSet<usize> = rust.iter().copied().collect();
    let differing: BTreeSet<usize> = reference_set
        .symmetric_difference(&rust_set)
        .copied()
        .collect();
    let found: BTreeSet<&str> = differing
        .iter()
        .map(|&at| classify_boundary(&chars, at))
        .collect();
    let ordered: Vec<String> = CATEGORIES
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| found.contains(name))
        .map(str::to_owned)
        .collect();
    let (primary, also) = ordered.split_first()?;
    Some((primary.clone(), also.to_vec()))
}

#[test]
fn corpus_has_the_expected_shape() {
    let document: Value = serde_json::from_str(CORPUS).expect("corpus is valid JSON");
    assert_eq!(document["format"], "jobniac-sentences");
    assert_eq!(document["unit"], "code points");
    let cases = load_cases();
    assert!(cases.len() >= 300, "corpus has {} texts", cases.len());
    assert_eq!(
        document["count"].as_u64(),
        Some(u64::try_from(cases.len()).expect("fits"))
    );
    let ids: BTreeSet<&str> = cases.iter().map(|case| case.id.as_str()).collect();
    assert_eq!(ids.len(), cases.len(), "ids are unique");
}

#[test]
fn reference_offsets_satisfy_the_contract() {
    for case in load_cases() {
        let length = case.text.chars().count();
        assert_eq!(case.reference[0], 0, "{}", case.id);
        assert_eq!(
            *case.reference.last().expect("non-empty"),
            length,
            "{}",
            case.id
        );
        assert!(
            case.reference.windows(2).all(|pair| pair[0] < pair[1]),
            "{}: strictly increasing",
            case.id
        );
    }
}

#[test]
fn rust_offsets_satisfy_the_contract() {
    for case in load_cases() {
        let rust = sentence_boundaries(&case.text);
        let length = case.text.chars().count();
        assert_eq!(rust[0], 0, "{}", case.id);
        assert_eq!(*rust.last().expect("non-empty"), length, "{}", case.id);
        assert!(
            rust.windows(2).all(|pair| pair[0] < pair[1]),
            "{}: strictly increasing",
            case.id
        );
    }
}

#[test]
fn spans_partition_every_text() {
    for case in load_cases() {
        let boundaries = sentence_boundaries(&case.text);
        let spans = sentence_spans(&case.text);
        assert_eq!(spans.len() + 1, boundaries.len(), "{}", case.id);
        assert_eq!(spans.concat(), case.text, "{}", case.id);
    }
}

#[test]
fn reference_and_candidate_versions_are_pinned() {
    let corpus: Value = serde_json::from_str(CORPUS).expect("corpus is valid JSON");
    let accepted: Value = serde_json::from_str(ACCEPTED).expect("accepted list is valid JSON");
    assert_eq!(
        UNICODE_VERSION,
        (17, 0, 0),
        "unicode-segmentation changed its Unicode version: rerun the candidate comparison and \
         rewrite the accepted list"
    );
    assert_eq!(corpus["reference_unicode_version"], "16.0.0");
    assert_eq!(accepted["reference"]["unicode_version"], "16.0.0");
    assert_eq!(
        accepted["candidate"]["unicode_version"],
        unicode_version_text()
    );
    assert_eq!(accepted["candidate"]["crate"], "unicode-segmentation");
}

#[test]
fn every_difference_from_reference_is_accepted_and_pinned() {
    let accepted: Value = serde_json::from_str(ACCEPTED).expect("accepted list is valid JSON");
    assert_eq!(accepted["format"], "jobniac-sentence-accepted-differences");
    assert_eq!(accepted["version"], 2);

    let declared: BTreeMap<&str, &str> = accepted["categories"]
        .as_object()
        .expect("categories object")
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str().expect("description")))
        .collect();
    let expected: BTreeMap<&str, &str> = CATEGORIES.into_iter().collect();
    assert_eq!(declared, expected, "categories match the classifier");

    let mut entries: BTreeMap<String, Listed> = BTreeMap::new();
    for entry in accepted["entries"].as_array().expect("entries array") {
        let id = entry["id"].as_str().expect("entry id").to_owned();
        let category = entry["category"]
            .as_str()
            .expect("entry category")
            .to_owned();
        assert!(
            expected.contains_key(category.as_str()),
            "{id}: unknown category {category}"
        );
        assert_ne!(
            category, "other",
            "{id}: the classifier must name every difference"
        );
        let also: Vec<String> = entry["also"]
            .as_array()
            .expect("also array")
            .iter()
            .map(|item| item.as_str().expect("also item").to_owned())
            .collect();
        for name in &also {
            assert!(
                expected.contains_key(name.as_str()),
                "{id}: unknown category {name}"
            );
        }
        let reference = offsets(&entry["reference"]);
        let rust = offsets(&entry["rust"]);
        assert!(
            entries
                .insert(id.clone(), (category, also, reference, rust))
                .is_none(),
            "{id}: listed twice"
        );
    }

    let cases = load_cases();
    let corpus_ids: BTreeSet<&str> = cases.iter().map(|case| case.id.as_str()).collect();
    let unknown: Vec<&String> = entries
        .keys()
        .filter(|id| !corpus_ids.contains(id.as_str()))
        .collect();
    assert!(unknown.is_empty(), "entries not in the corpus: {unknown:?}");

    let mut unlisted = Vec::new();
    let mut stale = Vec::new();
    let mut mismatched = Vec::new();
    let mut differing = 0_usize;
    for case in &cases {
        let rust = sentence_boundaries(&case.text);
        let computed = classify_text(&case.text, &case.reference, &rust);
        match (computed, entries.get(&case.id)) {
            (None, None) => {}
            (None, Some(_)) => stale.push(case.id.clone()),
            (Some(_), None) => {
                differing += 1;
                unlisted.push(case.id.clone());
            }
            (Some((primary, also)), Some((category, listed_also, reference, pinned_rust))) => {
                differing += 1;
                if reference != &case.reference || pinned_rust != &rust {
                    mismatched.push(format!("{}: offsets changed", case.id));
                } else if &primary != category || &also != listed_also {
                    mismatched.push(format!(
                        "{}: listed {category} {listed_also:?}, classifier {primary} {also:?}",
                        case.id
                    ));
                }
            }
        }
    }

    assert!(
        unlisted.is_empty(),
        "unlisted differences from the reference: {unlisted:?}"
    );
    assert!(
        stale.is_empty(),
        "listed texts that now match the reference: {stale:?}"
    );
    assert!(
        mismatched.is_empty(),
        "accepted entries out of date: {mismatched:?}"
    );
    assert_eq!(
        differing,
        entries.len(),
        "every listed entry is a real difference"
    );
}

#[test]
fn surrogate_entries_match_reference_as_code_points() {
    let manifest: Value = serde_json::from_str(EXPECTED_ERRORS).expect("manifest is valid JSON");
    assert_eq!(manifest["format"], "jobniac-sentence-expected-errors");
    let entries = manifest["surrogates"].as_array().expect("surrogates array");
    assert!(entries.len() >= 9, "surrogate entries: {}", entries.len());
    let mut pairs_seen = 0_usize;
    let mut lone_entries = 0_usize;
    for entry in entries {
        let id = entry["id"].as_str().expect("id");
        let points: Vec<u32> = entry["code_points"]
            .as_array()
            .expect("code points")
            .iter()
            .map(|value| u32::try_from(value.as_u64().expect("code point")).expect("u32"))
            .collect();
        let reference = offsets(&entry["reference"]["boundaries"]);
        assert_eq!(reference.first(), Some(&0), "{id}");
        assert_eq!(reference.last(), Some(&points.len()), "{id}");
        assert!(reference.windows(2).all(|pair| pair[0] < pair[1]), "{id}");

        let is_high = |point: u32| (0xD800..=0xDBFF).contains(&point);
        let is_low = |point: u32| (0xDC00..=0xDFFF).contains(&point);
        // Pair starts: a high surrogate followed by a low surrogate, scanned left to right.
        let mut pair_starts = BTreeSet::new();
        let mut index = 0;
        while index + 1 < points.len() {
            if is_high(points[index]) && is_low(points[index + 1]) {
                pair_starts.insert(index);
                index += 2;
            } else {
                index += 1;
            }
        }
        if entry["category"] == "explicit-pair" {
            pairs_seen += pair_starts.len();
            assert!(!pair_starts.is_empty(), "{id}: has an explicit pair");
            // Boundaries never split an explicit pair.
            for &start in &pair_starts {
                assert!(!reference.contains(&(start + 1)), "{id}: splits a pair");
            }
            // The Rust equivalent holds each pair as one scalar value. Each pair before a boundary
            // removes one code point from the Rust offset. An entry that also holds a lone
            // surrogate has no Rust equivalent and is checked only against the reference.
            let mut scalars = String::new();
            let mut lone = false;
            let mut index = 0;
            while index < points.len() {
                if pair_starts.contains(&index) {
                    let combined =
                        0x10000 + ((points[index] - 0xD800) << 10) + (points[index + 1] - 0xDC00);
                    scalars.push(char::from_u32(combined).expect("scalar value"));
                    index += 2;
                } else {
                    match char::from_u32(points[index]) {
                        Some(ch) => scalars.push(ch),
                        None => {
                            lone = true;
                            break;
                        }
                    }
                    index += 1;
                }
            }
            if lone {
                lone_entries += 1;
                continue;
            }
            let mapped: Vec<usize> = reference
                .iter()
                .map(|&offset| {
                    offset
                        - pair_starts
                            .iter()
                            .filter(|&&start| start + 2 <= offset)
                            .count()
                })
                .collect();
            assert_eq!(sentence_boundaries(&scalars), mapped, "{id}");
        } else {
            assert!(
                points.iter().any(|&point| is_high(point) || is_low(point)),
                "{id}: holds a surrogate code point"
            );
        }
    }
    assert!(
        pairs_seen >= 5,
        "explicit pairs in the manifest: {pairs_seen}"
    );
    assert_eq!(
        lone_entries, 1,
        "explicit-pair entries that also hold a lone surrogate"
    );
}

#[test]
fn budget_entries_match_the_preparation_cap() {
    let manifest: Value = serde_json::from_str(EXPECTED_ERRORS).expect("manifest is valid JSON");
    let entries = manifest["budget"].as_array().expect("budget array");
    assert_eq!(entries.len(), 4);
    for entry in entries {
        let id = entry["id"].as_str().expect("id");
        let hex = entry["unit_code_point"]
            .as_str()
            .expect("unit")
            .trim_start_matches("U+");
        let unit = char::from_u32(u32::from_str_radix(hex, 16).expect("hex")).expect("scalar");
        let repeat = usize::try_from(entry["repeat"].as_u64().expect("repeat")).expect("fits");
        let text = unit.to_string().repeat(repeat);
        assert_eq!(
            u64::try_from(text.encode_utf16().count()).expect("fits"),
            entry["utf16_units"].as_u64().expect("units"),
            "{id}: UTF-16 length"
        );
        match entry["reference"].get("error") {
            Some(error) => {
                let refusal = SentenceError::PreparationLimitExceeded;
                assert_eq!(error["message"], refusal.message(), "{id}");
                assert_eq!(
                    checked_sentence_boundaries(&text),
                    Err(refusal),
                    "{id}: refused as the reference refuses"
                );
            }
            None => {
                assert_eq!(
                    checked_sentence_boundaries(&text),
                    Ok(offsets(&entry["reference"]["boundaries"])),
                    "{id}: accepted as the reference accepts"
                );
            }
        }
    }
}

/// Rewrites the accepted list from the corpus and the classifier. Not part of the normal run.
#[test]
#[ignore = "rewrites tests/golden/sentences-accepted-differences.json from the corpus and the classifier"]
fn write_accepted_differences() {
    let corpus: Value = serde_json::from_str(CORPUS).expect("corpus is valid JSON");
    let mut entries = Vec::new();
    // Per category: (extra Rust boundaries, missing reference boundaries, texts with the category).
    let mut per_category: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    for case in load_cases() {
        let rust = sentence_boundaries(&case.text);
        if let Some((primary, also)) = classify_text(&case.text, &case.reference, &rust) {
            let chars: Vec<char> = case.text.chars().collect();
            let reference_set: BTreeSet<usize> = case.reference.iter().copied().collect();
            let rust_set: BTreeSet<usize> = rust.iter().copied().collect();
            let mut texts_here: BTreeSet<&str> = BTreeSet::new();
            for &at in reference_set.symmetric_difference(&rust_set) {
                let name = classify_boundary(&chars, at);
                let row = per_category.entry(name).or_insert((0, 0, 0));
                if rust_set.contains(&at) {
                    row.0 += 1;
                } else {
                    row.1 += 1;
                }
                texts_here.insert(name);
            }
            for name in texts_here {
                if let Some(row) = per_category.get_mut(name) {
                    row.2 += 1;
                }
            }
            entries.push(json!({
                "id": case.id,
                "category": primary,
                "also": also,
                "reference": case.reference,
                "rust": rust,
            }));
        }
    }
    let categories: BTreeMap<&str, &str> = CATEGORIES.into_iter().collect();
    let document = json!({
        "format": "jobniac-sentence-accepted-differences",
        "version": 2,
        "reference": {
            "unicode_version": corpus["reference_unicode_version"],
        },
        "candidate": {
            "crate": "unicode-segmentation",
            "version": "1.13.3",
            "unicode_version": unicode_version_text(),
        },
        "classifier": "crates/text/tests/sentences_golden.rs (classify_boundary, classify_text)",
        "categories": categories,
        "entries": entries,
    });
    let mut body = serde_json::to_string_pretty(&document).expect("serialize");
    body.push('\n');
    std::fs::write(ACCEPTED_PATH, body).expect("write accepted list");
    for (category, (extra, missing, texts)) in &per_category {
        eprintln!("{category}: extra {extra}, missing {missing}, texts {texts}");
    }
    let total: usize = entries.len();
    eprintln!("total: {total} texts");
}
