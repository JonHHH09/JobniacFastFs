//! Generic layout against the golden vectors: rows, pages, start indexes and errors.
//!
//! The inputs and expected results come from `tests/golden/pdf-generic.json` (synthetic inputs).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use jobniac_pdf::PdfError;
use jobniac_pdf::layout::pagination::page_capacity;
use jobniac_pdf::layout::{
    COMPACT_LEADING, Leading, NORMAL_LEADING, PageNumberLocale, build_pdf_layout,
};
use serde_json::Value;

const GOLDEN: &str = include_str!("golden/pdf-generic.json");

/// Recorded cases whose rows run past the right margin (a 104-letter capital row is 659 pt wide
/// on a 504 pt line). The layout re-wraps those rows by measured width; every other case must
/// match the recorded rows exactly.
const WIDTH_AWARE_CASES: [&str; 2] = ["wrap_long_words", "body_at_limit"];

fn assert_width_aware_rows(id: &str, actual: &[&str], wanted: &[&str]) {
    assert!(
        wanted.iter().any(
            |row| jobniac_pdf::layout::fonts::text_width(row, false, 9.5, false).unwrap() > 504.0
        ),
        "{id}: the recorded rows no longer overflow"
    );
    let squash = |rows: &[&str]| rows.concat().replace(' ', "");
    assert_eq!(squash(actual), squash(wanted), "{id}: text content");
    for row in &actual[2..] {
        let width = jobniac_pdf::layout::fonts::text_width(row, false, 9.5, false).unwrap();
        assert!(width <= 504.0, "{id}: row too wide: {row}");
    }
}

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden JSON parses")
}

fn optional_str(value: &Value) -> Option<&str> {
    value.as_str()
}

fn locale_of(name: &str) -> PageNumberLocale {
    match name {
        "canadian_french" => PageNumberLocale::CanadianFrench,
        _ => PageNumberLocale::English,
    }
}

#[test]
fn every_layout_case_matches_the_golden_vectors() {
    let golden = golden();
    let rows = golden["layouts"].as_array().expect("layouts array");
    assert_eq!(rows.len(), 20, "all recorded cases are replayed");
    for row in rows {
        let case = &row["case"];
        let id = case["id"].as_str().expect("case id");
        let title = optional_str(&case["title"]);
        let body = optional_str(&case["body"]);
        let compact = case["compact"].as_bool().expect("compact flag");
        let locale = locale_of(case["pageNumberLocale"].as_str().expect("locale"));
        let result = build_pdf_layout(title, body, compact, locale);
        let expected = &row["result"];
        if let Some(error) = expected.get("error") {
            let Err(actual) = result else {
                panic!("{id}: the vectors refuse the layout but Rust accepted it");
            };
            assert_eq!(
                actual.message(),
                error["message"].as_str().unwrap(),
                "{id}: message"
            );
            let details = &error["details"];
            match actual {
                PdfError::Validation { field, reason, .. } => {
                    assert_eq!(
                        field,
                        details.get("field").and_then(Value::as_str),
                        "{id}: field"
                    );
                    assert_eq!(
                        reason,
                        details.get("reason").and_then(Value::as_str),
                        "{id}: reason"
                    );
                }
                PdfError::Internal => panic!("{id}: unexpected internal error"),
            }
            continue;
        }
        let layout = result.unwrap_or_else(|error| panic!("{id}: unexpected {error}"));
        let leading: f64 = expected["leading"].as_str().unwrap().parse().unwrap();
        assert_eq!(layout.leading.value(), leading, "{id}: leading");
        let pages = expected["pages"].as_array().unwrap();
        if WIDTH_AWARE_CASES.contains(&id) {
            let wanted: Vec<&str> = pages
                .iter()
                .flat_map(|page| page["lines"].as_array().unwrap())
                .map(|line| line.as_str().unwrap())
                .collect();
            assert_width_aware_rows(id, &all_rows(&layout), &wanted);
            continue;
        }
        assert_eq!(layout.pages.len(), pages.len(), "{id}: page count");
        for (actual, wanted) in layout.pages.iter().zip(pages) {
            assert_eq!(
                actual.start_index as u64,
                wanted["startIndex"].as_u64().unwrap(),
                "{id}: start index"
            );
            let wanted_lines: Vec<&str> = wanted["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|line| line.as_str().unwrap())
                .collect();
            let actual_lines: Vec<&str> = actual.lines.iter().map(String::as_str).collect();
            assert_eq!(actual_lines, wanted_lines, "{id}: rows");
        }
    }
}

#[test]
fn leading_values_are_the_two_qualified_layouts() {
    assert_eq!(Leading::from_compact(false).value(), NORMAL_LEADING);
    assert_eq!(Leading::from_compact(true).value(), COMPACT_LEADING);
}

#[test]
fn capacity_follows_the_floor_formula() {
    // floor((726 - 60) / leading) + 1
    assert_eq!(page_capacity(Leading::Normal), 50);
    assert_eq!(page_capacity(Leading::Compact), 62);
}

#[test]
fn title_default_and_strip_rules() {
    let none = build_pdf_layout(None, Some("Body."), false, PageNumberLocale::English).unwrap();
    assert_eq!(none.pages[0].lines[0], "Generated PDF");
    let blank = build_pdf_layout(
        Some(" \t "),
        Some("Body."),
        false,
        PageNumberLocale::English,
    )
    .unwrap();
    assert_eq!(blank.pages[0].lines[0], "Generated PDF");
    let padded = build_pdf_layout(
        Some("  Ada  "),
        Some("Body."),
        false,
        PageNumberLocale::English,
    )
    .unwrap();
    assert_eq!(padded.pages[0].lines[0], "Ada");
}

#[test]
fn preparation_budget_counts_title_units_before_body_validation() {
    let title = "a".repeat(1_000_001);
    let error = build_pdf_layout(Some(&title), None, false, PageNumberLocale::English).unwrap_err();
    assert_eq!(
        error,
        PdfError::Validation {
            message: "Resume preparation limit exceeded",
            field: Some("text"),
            reason: Some("exceeds preparation safety budget"),
        }
    );
}

#[test]
fn blank_body_is_refused_after_the_budget() {
    let error =
        build_pdf_layout(Some("Title"), Some("   "), false, PageNumberLocale::English).unwrap_err();
    assert_eq!(
        error,
        PdfError::Validation {
            message: "Invalid PDF generation request",
            field: Some("body"),
            reason: Some("must not be blank"),
        }
    );
}

fn width(text: &str, bold: bool, size: f64) -> f64 {
    jobniac_pdf::layout::fonts::text_width(text, bold, size, false).unwrap()
}

fn layout_of(title: &str, body: &str) -> jobniac_pdf::layout::PdfLayout {
    build_pdf_layout(Some(title), Some(body), false, PageNumberLocale::English).unwrap()
}

fn all_rows(layout: &jobniac_pdf::layout::PdfLayout) -> Vec<&str> {
    layout
        .pages
        .iter()
        .flat_map(|page| page.lines.iter().map(String::as_str))
        .collect()
}

#[test]
fn short_title_keeps_one_row_at_twenty_points() {
    let layout = layout_of("Resume: Ada", "Body.");
    assert_eq!(layout.title_rows, 1);
    assert_eq!(layout.title_size, 20.0);
}

#[test]
fn long_title_shrinks_to_one_row_inside_the_margins() {
    let title = "Senior Software Engineer Application for Contoso Berlin";
    assert!(width(title, true, 20.0) > 504.0);
    let layout = layout_of(title, "Body.");
    assert_eq!(layout.title_rows, 1);
    assert!(layout.title_size < 20.0 && layout.title_size >= 10.0);
    assert!(width(title, true, layout.title_size) <= 504.0);
    assert_eq!(layout.pages[0].lines[0], title);
}

#[test]
fn very_long_title_wraps_by_measured_width_and_pages_it() {
    let title = "WWWW ".repeat(120) + &"x".repeat(300);
    let layout = layout_of(&title, "Body.");
    assert!(layout.title_rows > 1);
    assert_eq!(layout.title_size, 10.0);
    let rows = all_rows(&layout);
    for row in &rows[..layout.title_rows] {
        assert!(width(row, true, 10.0) <= 504.0, "title row too wide: {row}");
        assert!(!row.is_empty());
    }
    assert_eq!(rows[layout.title_rows], "");
    let joined: String = rows[..layout.title_rows].concat().replace(' ', "");
    assert_eq!(joined, title.replace(' ', ""));
    let streams = jobniac_pdf::render::generic::page_streams(&layout).unwrap();
    assert!(streams[0].contains("/FBold 10 Tf"));
}

#[test]
fn wide_capitals_wrap_inside_the_body_width() {
    let body = "A".repeat(104);
    let layout = layout_of("T", &body);
    let rows = all_rows(&layout);
    assert!(rows.len() > 3);
    for row in &rows[2..] {
        assert!(width(row, false, 9.5) <= 504.0, "row too wide: {row}");
    }
    assert_eq!(rows[2..].concat(), body);
}

#[test]
fn wide_capital_words_break_at_spaces() {
    let body = "WORD ".repeat(40);
    let layout = layout_of("T", body.trim());
    for row in &all_rows(&layout)[2..] {
        assert!(width(row, false, 9.5) <= 504.0, "row too wide: {row}");
        assert!(!row.starts_with(' ') && !row.ends_with(' '));
    }
}

#[test]
fn long_unbreakable_token_is_hard_broken_by_width() {
    let body = "W".repeat(2000);
    let layout = layout_of("T", &body);
    let rows = all_rows(&layout);
    for row in &rows[2..] {
        assert!(width(row, false, 9.5) <= 504.0);
    }
    assert_eq!(rows[2..].concat(), body);
}

#[test]
fn bullet_and_label_rows_respect_their_indents() {
    let layout = layout_of(
        "T",
        &format!("- {}\nSkills: {}", "W".repeat(300), "M".repeat(300)),
    );
    let rows = all_rows(&layout);
    let mut bullets = 0;
    for row in &rows[2..] {
        if let Some(text) = row.strip_prefix("- ").or_else(|| row.strip_prefix("  ")) {
            assert!(width(text, false, 9.5) <= 480.0, "indented row too wide");
            bullets += 1;
        } else {
            let colon = row.find(':').unwrap();
            let label = &row[..=colon];
            let value_x = 54.0 + width(&format!("{label} "), true, 9.5);
            let value = row[colon + 1..].trim_start();
            assert!(
                value_x + width(value, false, 9.5) <= 558.0,
                "label row too wide"
            );
        }
    }
    assert!(bullets > 2);
}

#[test]
fn rows_that_fit_keep_the_character_count_breaks() {
    let body = "lorem ipsum dolor sit amet ".repeat(30);
    let layout = layout_of("T", body.trim());
    let rows = all_rows(&layout);
    assert!(rows[2..].iter().all(|row| row.chars().count() <= 104));
    assert!(
        rows[2..rows.len() - 1]
            .iter()
            .all(|row| row.chars().count() >= 90)
    );
}
