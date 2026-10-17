//! Arbeitnow text preparation: the cases the Rust types can express and the comparison with
//! `golden/arbeitnow-text.json`. Runtime checks of non-string values and non-integer or negative
//! caps are type-enforced in Rust and have no test.

use jobniac_core::arbeitnow::text::{
    TEXT_PREPARATION_MESSAGE, bound, bound_list, html4_unescape, plain, plain_bound,
};
use serde_json::Value;

const GOLDEN: &str = include_str!("golden/arbeitnow-text.json");

fn list(values: &[&str]) -> Vec<String> {
    bound_list(Some(values))
}

#[test]
fn message_is_the_documented_constant() {
    assert_eq!(
        TEXT_PREPARATION_MESSAGE,
        "Invalid Arbeitnow text preparation"
    );
}

#[test]
fn numeric_overlap_and_one_pass_semantics() {
    let cases = [
        ("&#+65;", "A"),
        ("&#-0;", "\0"),
        ("&#x+41;", "A"),
        ("&#X-0;", "\0"),
        ("&#x\u{FF26}\u{FF26};", "\u{FF}"),
        ("&#\u{666}\u{665};", "A"),
        ("&#128;", "\u{80}"),
        ("&#1114111;", "\u{10FFFF}"),
        ("&thetasym;", "\u{3D1}"),
        ("&#0000065;", "A"),
        ("&#x000041;", "A"),
        ("&&amp;", "&&"),
        ("&bogus&amp;", "&bogus&"),
        (
            "&amp;lt;script&amp;gt;x&amp;lt;/script&amp;gt;",
            "&lt;script&gt;x&lt;/script&gt;",
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(html4_unescape(text), expected, "{text}");
    }
}

#[test]
fn malformed_unknown_overlong_and_non_bmp_digits_survive_literally() {
    let cases = [
        "&amp",
        "&AMP;",
        "&apos;",
        "&npar;",
        "&copy",
        "&thetasymX;",
        "&#00000065;",
        "&#x0000041;",
        "&#1114112;",
        "&#-1;",
        "&#x-1;",
        "&#x;",
        "&#;",
        "&#++1;",
        "&# 65;",
        "&#\u{1D7DE}\u{1D7DD};",
        "&unknown;",
        "&;",
        "&",
        "&&#x;",
    ];
    for text in cases {
        assert_eq!(html4_unescape(text), text, "{text}");
    }
}

#[test]
fn reference_span_guard_handles_long_runs() {
    let text = format!("&{}&amp;", "x".repeat(100_000));
    assert_eq!(html4_unescape(&text), format!("&{}&", "x".repeat(100_000)));
    let text = format!("{};", "&".repeat(100_000));
    assert_eq!(html4_unescape(&text), text);
}

#[test]
fn decode_precedes_script_and_tag_removal_with_ascii_case_only() {
    assert_eq!(
        plain("&lt;script&gt;hidden&lt;/script&gt;&lt;b&gt;visible&lt;/b&gt;"),
        "visible"
    );
    assert_eq!(
        plain("<ScRiPt>hidden\nmore</sCrIpT><STYLE>x</STYLE>visible"),
        "visible"
    );
    assert_eq!(plain("<\u{17F}cript>visible</\u{17F}cript>"), "visible");
    assert_eq!(
        plain("&amp;lt;b&amp;gt;visible&amp;lt;/b&amp;gt;"),
        "&lt;b&gt;visible&lt;/b&gt;"
    );
    assert_eq!(plain("<script>unclosed"), "unclosed");
}

#[test]
fn control_nbsp_replacement_and_internal_unicode_whitespace() {
    let text = "a\0b\u{85}c\u{A0}d\u{FFFD}e\u{2003}f\u{2007}g\u{202F}h";
    assert_eq!(plain(text), "a b c d e\u{2003}f\u{2007}g\u{202F}h");
    assert_eq!(
        bound(text, 100),
        "a b c\u{A0}d\u{FFFD}e\u{2003}f\u{2007}g\u{202F}h"
    );
    assert_eq!(plain("\u{2003}visible\u{2028}"), "visible");
    assert_eq!(
        bound("\u{2007}visible\u{202F}", 100),
        "\u{2007}visible\u{202F}"
    );
    assert_eq!(plain("a > b < c <>"), "a b");
    assert_eq!(plain("a > b < c"), "a b c");
    assert_eq!(bound("a > b < c <>", 100), "a > b < c <>");
}

#[test]
fn caps_count_code_points_and_a_pair_counts_once() {
    let value = "\u{1F600}";
    assert_eq!(bound(&format!("{value}tail"), 1), value);
    assert_eq!(
        plain(&format!("{}{value}tail", "a".repeat(999))),
        format!("{}{value}", "a".repeat(999))
    );
    assert_eq!(bound(value, 0), "");
    assert_eq!(bound(&format!("{value}tail"), 100), format!("{value}tail"));
    assert_eq!(bound(&"\u{1F600}".repeat(3), 2), "\u{1F600}".repeat(2));
}

#[test]
fn surrogate_references_become_replacement_characters() {
    assert_eq!(html4_unescape("&#xD800;"), "\u{FFFD}");
    assert_eq!(html4_unescape("&#56320;"), "\u{FFFD}");
    assert_eq!(html4_unescape("&#xD83D;&#xDE00;"), "\u{FFFD}\u{FFFD}");
    // plain turns U+FFFD into a space.
    assert_eq!(plain("a&#xD800;b"), "a b");
    assert_eq!(plain("&#xDFFF;"), "");
}

#[test]
fn plain_bound_keeps_the_thousand_cap_and_strips_the_truncated_prefix() {
    assert_eq!(plain_bound(&"x".repeat(1001), 2000), "x".repeat(1000));
    assert_eq!(bound(&"x".repeat(1001), 2000), "x".repeat(1001));
    assert_eq!(plain(&format!("{} tail", "x".repeat(999))), "x".repeat(999));
    assert_eq!(
        plain_bound(&format!("{} tail", "x".repeat(127)), 128),
        "x".repeat(127)
    );
}

#[test]
fn output_cap_does_not_truncate_input_before_preparation() {
    let text = format!(
        "<script>{}</script><b>visible</b>",
        "hidden ".repeat(10_000)
    );
    assert_eq!(plain(&text), "visible");
    assert_eq!(plain(&format!("{}visible", " ".repeat(1100))), "visible");
}

#[test]
fn empty_and_absent_input() {
    assert_eq!(html4_unescape(""), "");
    assert_eq!(plain(""), "");
    assert_eq!(bound("", 0), "");
    assert_eq!(plain_bound("", 1), "");
    assert!(bound_list::<&str>(None).is_empty());
    assert!(list(&[]).is_empty());
}

#[test]
fn list_keeps_order_and_duplicates_and_drops_blanks() {
    assert_eq!(
        list(&["first", "", "<b>second</b>", "first"]),
        ["first", "second", "first"]
    );
    assert!(list(&["\u{2003}", "\0", "&nbsp;", "\u{FFFD}"]).is_empty());
}

#[test]
fn list_filters_before_limit() {
    let mut values = vec!["", " ", "<script>hidden</script>"];
    values.extend(vec!["same"; 22]);
    assert_eq!(list(&values), vec!["same"; 20]);
    let mut values = vec![""; 25];
    values.extend(vec!["b"; 25]);
    assert_eq!(list(&values), vec!["b"; 20]);
}

#[test]
fn list_entries_are_capped_at_128_code_points() {
    assert_eq!(list(&[&"\u{1F600}".repeat(129)]), ["\u{1F600}".repeat(128)]);
}

fn golden() -> Value {
    match serde_json::from_str(GOLDEN) {
        Ok(value) => value,
        Err(error) => panic!("arbeitnow-text.json does not parse: {error}"),
    }
}

fn string(value: &Value) -> String {
    match value.as_str() {
        Some(found) => found.to_owned(),
        None => panic!("expected a string, got {value}"),
    }
}

fn strings(value: &Value) -> Vec<String> {
    match value.as_array() {
        Some(items) => items.iter().map(string).collect(),
        None => panic!("expected an array, got {value}"),
    }
}

#[test]
fn matches_the_golden_vectors() {
    let document = golden();
    let cases = match document["cases"].as_array() {
        Some(items) => items,
        None => panic!("missing cases"),
    };
    assert!(cases.len() > 500, "golden vectors are missing");
    let mut deviations = 0;
    for case in cases {
        let operation = string(&case["op"]);
        let expected = if case.get("deviation").is_some() {
            deviations += 1;
            &case["expected_rust"]
        } else {
            &case["expected"]
        };
        let cap = case["cap"].as_u64().map(|cap| cap as usize);
        if operation == "bound_list" {
            let values = case["values"]
                .as_array()
                .map(|items| items.iter().map(string).collect::<Vec<_>>());
            let actual = bound_list(values.as_deref());
            assert_eq!(actual, strings(expected), "{case}");
            continue;
        }
        let value = string(&case["value"]);
        let actual = match (operation.as_str(), cap) {
            ("html4_unescape", _) => html4_unescape(&value),
            ("plain", _) => plain(&value),
            ("bound", Some(cap)) => bound(&value, cap),
            ("plain_bound", Some(cap)) => plain_bound(&value, cap),
            other => panic!("unknown operation {other:?}"),
        };
        assert_eq!(actual, string(expected), "{case}");
    }
    assert!(deviations > 0, "the surrogate deviation cases are missing");
}
