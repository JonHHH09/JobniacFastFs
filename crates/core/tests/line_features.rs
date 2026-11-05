//! Feature vectors of hand-built lines against exact expected values, and the pinned feature names.

use jobniac_core::profiles::extract::lines::{
    FEATURE_COUNT, FEATURE_NAMES, LineDocument, LineGeometry, TextLine, feature_index,
    line_features,
};

const NAMES_GOLDEN: &str = include_str!("golden/line-feature-names.txt");

fn value(row: &[f64], name: &str) -> f64 {
    let index = feature_index(name).unwrap_or_else(|| panic!("unknown feature {name}"));
    row[index]
}

fn assert_close(actual: f64, expected: f64, label: &str) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "{label}: {actual} vs {expected}"
    );
}

#[test]
fn feature_names_match_the_golden_file() {
    assert_eq!(FEATURE_NAMES.len(), FEATURE_COUNT);
    assert_eq!(FEATURE_NAMES.join("\n") + "\n", NAMES_GOLDEN);
    for name in FEATURE_NAMES {
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)),
            "{name}"
        );
    }
}

fn geometry(font: i32, bold: bool, x0: i32, y0: i32, x1: i32) -> LineGeometry {
    LineGeometry {
        x0,
        y0,
        x1,
        y1: y0 + font * 12 / 10,
        font_size_centipoints: font,
        bold,
        italic: false,
        page_width_centipoints: 61_200,
        page_height_centipoints: 79_200,
    }
}

/// Four lines of a US resume header with exact geometry.
///
/// Median body font: characters weigh 11 at 2000, 15 at 1200 and 58 at 1000, so the median is 1000.
fn header_document() -> LineDocument {
    let lines = vec![
        TextLine::new(
            0,
            0,
            "ALEX SAMPLE",
            Some(geometry(2_000, true, 5_400, 5_400, 16_400)),
        ),
        TextLine::new(
            0,
            1,
            "alex.sample@example.com | (202) 555-0147",
            Some(geometry(1_000, false, 5_400, 8_200, 25_400)),
        ),
        TextLine::new(
            0,
            2,
            "Work Experience",
            Some(geometry(1_200, true, 5_400, 10_000, 12_900)),
        ),
        TextLine::new(
            0,
            3,
            "Jan 2020 – Present",
            Some(geometry(1_000, false, 5_400, 11_700, 14_400)),
        ),
    ];
    LineDocument::from_lines(lines)
}

#[test]
fn geometry_line_features_have_exact_values() {
    let document = header_document();
    let rows = line_features(&document);
    assert_eq!(rows.len(), 4);
    let name = &rows[0];
    let contact = &rows[1];
    let heading = &rows[2];
    let range = &rows[3];

    let expected: &[(&[f64], &str, f64)] = &[
        (name, "len_chars", 11.0),
        (name, "word_count", 2.0),
        (name, "upper_ratio", 1.0),
        (name, "title_word_ratio", 1.0),
        (name, "digit_ratio", 0.0),
        (name, "has_email", 0.0),
        (name, "has_geometry", 1.0),
        (name, "font_rel", 2.0),
        (name, "bold", 1.0),
        (name, "column", 0.0),
        (name, "rel_position", 0.0),
        (name, "first_on_page", 1.0),
        (name, "last_on_page", 0.0),
        (name, "prev_exists", 0.0),
        (name, "next_exists", 1.0),
        (name, "prev_font_rel", 1.0),
        (name, "next_len_chars", 40.0),
        (contact, "len_chars", 40.0),
        (contact, "word_count", 4.0),
        (contact, "has_email", 1.0),
        (contact, "has_phone", 1.0),
        (contact, "has_url", 0.0),
        (contact, "comma_count", 0.0),
        (contact, "font_rel", 1.0),
        (contact, "bold", 0.0),
        (contact, "rel_position", 1.0 / 3.0),
        (contact, "prev_len_chars", 11.0),
        (contact, "prev_upper_ratio", 1.0),
        (contact, "next_heading_any", 1.0),
        (contact, "next_font_rel", 1.2),
        (contact, "next_bold", 1.0),
        (heading, "heading_experience", 1.0),
        (heading, "heading_any", 1.0),
        (heading, "heading_education", 0.0),
        (heading, "title_word_ratio", 1.0),
        (heading, "font_rel", 1.2),
        (heading, "prev_has_date", 0.0),
        (heading, "next_has_date", 1.0),
        (heading, "next_has_date_range", 1.0),
        (range, "has_date", 1.0),
        (range, "has_date_range", 1.0),
        (range, "has_ongoing_marker", 1.0),
        (range, "has_year", 1.0),
        (range, "last_on_page", 1.0),
        (range, "next_exists", 0.0),
        (range, "next_font_rel", 1.0),
        (range, "rel_position", 1.0),
    ];
    for (row, feature, want) in expected {
        assert_close(value(row, feature), *want, feature);
    }
    // Geometry ratios: x0 5400 and width 11000 on a 61200-wide page.
    assert_close(value(name, "x0_rel"), 5_400.0 / 61_200.0, "x0_rel");
    assert_close(value(name, "width_rel"), 11_000.0 / 61_200.0, "width_rel");
    // Gaps: line 0 ends at 5400 + 2400 = 7800 and line 1 starts at 8200, in units of the font size.
    assert_close(value(name, "gap_before_rel"), 0.0, "gap_before_rel first");
    assert_close(
        value(name, "gap_after_rel"),
        400.0 / 2_000.0,
        "gap_after_rel",
    );
    assert_close(
        value(contact, "gap_before_rel"),
        400.0 / 1_000.0,
        "gap_before_rel",
    );
    assert_close(
        value(contact, "gap_after_rel"),
        (10_000.0 - 9_400.0) / 1_000.0,
        "gap_after_rel",
    );
}

#[test]
fn plain_text_features_use_neutral_geometry() {
    let document = LineDocument::from_text(
        "Alex Sample\nSpringfield, IL 62701\n\nSkills\n- Rust, Go\nDear Ms. Example,",
    );
    let rows = line_features(&document);
    assert_eq!(rows.len(), 5);
    for row in &rows {
        assert_eq!(row.len(), FEATURE_COUNT);
        assert_close(value(row, "has_geometry"), 0.0, "has_geometry");
        assert_close(value(row, "font_rel"), 1.0, "font_rel");
        for neutral in [
            "bold",
            "italic",
            "x0_rel",
            "width_rel",
            "column",
            "gap_before_rel",
            "gap_after_rel",
        ] {
            assert_close(value(row, neutral), 0.0, neutral);
        }
    }
    let [sample, address, skills, bullet, salutation] =
        [&rows[0], &rows[1], &rows[2], &rows[3], &rows[4]];
    assert_close(value(sample, "first_on_page"), 1.0, "first");
    assert_close(value(address, "has_postal_code"), 1.0, "postal");
    assert_close(value(address, "has_region"), 1.0, "region");
    assert_close(value(address, "comma_count"), 1.0, "commas");
    assert_close(value(skills, "blank_before"), 1.0, "blank_before");
    assert_close(value(skills, "heading_skills"), 1.0, "heading_skills");
    assert_close(value(skills, "prev_exists"), 1.0, "prev_exists");
    assert_close(value(bullet, "bullet_start"), 1.0, "bullet_start");
    assert_close(value(bullet, "comma_count"), 1.0, "bullet commas");
    assert_close(value(bullet, "prev_heading_any"), 1.0, "prev_heading_any");
    assert_close(value(salutation, "is_salutation"), 1.0, "salutation");
    assert_close(value(salutation, "ends_colon"), 0.0, "ends_colon");
    assert_close(value(salutation, "last_on_page"), 1.0, "last");
    assert_close(value(salutation, "rel_position"), 1.0, "rel_position");
    assert_close(value(salutation, "next_exists"), 0.0, "next_exists");
}

#[test]
fn lexicon_and_letter_features() {
    let document = LineDocument::from_text(
        "Subject: Application\nMit freundlichen Grüßen\nBachelor of Science, Example University\nSenior Developer, Example Systems GmbH",
    );
    let rows = line_features(&document);
    assert_close(value(&rows[0], "is_subject"), 1.0, "subject");
    assert_close(value(&rows[1], "is_closing"), 1.0, "closing");
    assert_close(value(&rows[2], "has_degree"), 1.0, "degree");
    assert_close(value(&rows[3], "has_title_keyword"), 1.0, "title");
    assert_close(value(&rows[3], "has_company_suffix"), 1.0, "suffix");
    assert_close(value(&rows[2], "has_company_suffix"), 0.0, "no suffix");
}

#[test]
fn geometry_derives_blank_before_from_the_vertical_gap() {
    let document = header_document();
    let rows = line_features(&document);
    // Gaps of 0.0, 0.4, 0.6 and 0.5 of the font size; a blank line is a gap above 0.4.
    let flags: Vec<f64> = rows.iter().map(|row| value(row, "blank_before")).collect();
    assert_eq!(flags, vec![0.0, 0.0, 1.0, 0.0]);
}

#[test]
fn context_does_not_cross_a_column_break() {
    let line = |text: &str, x0: i32, y0: i32, width: i32| {
        TextLine::new(0, 0, text, Some(geometry_with_width(x0, y0, width)))
    };
    let mut lines = Vec::new();
    for row in 0..6 {
        let y = 5_000 + row * 1_400;
        lines.push(line(&format!("side {row}"), 2_800, y, 4_000 + row * 300));
        lines.push(line(
            &format!("main {row} with some more words"),
            21_000,
            y,
            9_000 + row * 1_700,
        ));
    }
    let document = LineDocument::from_lines(lines);
    assert_eq!(document.columns[5], 1);
    assert_eq!(document.columns[6], 2);
    let rows = line_features(&document);
    // The last sidebar line has no next line, the first main line has no previous line.
    assert_close(value(&rows[5], "next_exists"), 0.0, "next_exists");
    assert_close(value(&rows[5], "next_len_chars"), 0.0, "next_len_chars");
    assert_close(value(&rows[6], "prev_exists"), 0.0, "prev_exists");
    assert_close(value(&rows[6], "prev_len_chars"), 0.0, "prev_len_chars");
    assert_close(value(&rows[4], "next_exists"), 1.0, "inside the column");
    assert_close(value(&rows[6], "next_exists"), 1.0, "inside the column");
}

#[test]
fn context_does_not_cross_a_page_break() {
    let page = |number: u32, text: &str, y0: i32| {
        TextLine::new(number, 0, text, Some(geometry_with_width(5_400, y0, 5_000)))
    };
    let document = LineDocument::from_lines(vec![
        page(0, "last on page one", 70_000),
        page(1, "first on page two", 5_400),
    ]);
    let rows = line_features(&document);
    assert_close(value(&rows[0], "next_exists"), 0.0, "next_exists");
    assert_close(value(&rows[1], "prev_exists"), 0.0, "prev_exists");
    assert_close(value(&rows[0], "last_on_page"), 1.0, "last_on_page");
    assert_close(value(&rows[1], "first_on_page"), 1.0, "first_on_page");
}

fn geometry_with_width(x0: i32, y0: i32, width: i32) -> LineGeometry {
    LineGeometry {
        x0,
        y0,
        x1: x0 + width,
        y1: y0 + 1_200,
        font_size_centipoints: 1_000,
        bold: false,
        italic: false,
        page_width_centipoints: 59_500,
        page_height_centipoints: 84_200,
    }
}
