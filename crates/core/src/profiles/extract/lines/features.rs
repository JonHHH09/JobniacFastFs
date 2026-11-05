//! The numeric feature vector of a line.
//!
//! Every line of a [`LineDocument`] gets the same fixed vector of [`FEATURE_COUNT`] `f64` values,
//! in the order of [`FEATURE_NAMES`]. Flags are 0.0 or 1.0, ratios are in 0..=1, counts and
//! lengths are raw. The names use only `[A-Za-z0-9_.-]` and are pinned by a golden file.
//!
//! # Missing geometry
//!
//! Without geometry every geometry feature is neutral: `font_rel` 1.0, everything else 0.0, and
//! `has_geometry` is 0.0 so a model can tell the difference. With geometry but no comparable
//! neighbour (the first line of a column) the gaps are 0.0.
//!
//! # Features
//!
//! | Feature | Meaning |
//! |---|---|
//! | `len_chars` | characters of the text |
//! | `word_count` | whitespace-separated words |
//! | `upper_ratio` | upper-case letters over letters |
//! | `title_word_ratio` | words that start upper-case over words |
//! | `digit_ratio` | ASCII digits over non-space characters |
//! | `punct_ratio` | punctuation over non-space characters |
//! | `bullet_start` | starts with a bullet glyph or `-`/`*` and a space |
//! | `ends_colon`, `ends_period` | last character is `:` or `.` |
//! | `comma_count` | commas |
//! | `has_email`, `has_phone`, `has_url` | contains an email, a phone (NANP, Europass, international), a URL |
//! | `has_date` | contains a date with a month or a numeric date (not a bare year) |
//! | `has_date_range` | contains a date range, including an ambiguous numeric one |
//! | `has_ongoing_marker` | an ongoing marker next to a year, or alone |
//! | `has_year` | contains 19xx or 20xx |
//! | `has_postal_code`, `has_region` | contains a postal code, a US state or Canadian province |
//! | `has_cefr_level` | contains a CEFR level token (`B2`) |
//! | `heading_<kind>` | the whole line is a heading of that section kind (12 kinds) |
//! | `heading_any` | the line is a heading of any kind |
//! | `has_degree`, `has_company_suffix`, `has_title_keyword` | lexicon token present |
//! | `is_salutation`, `is_closing`, `is_subject`, `is_recipient` | letter marker at the line start |
//! | `rel_position` | index over the last index (0.0 for one line) |
//! | `page_index` | zero-based page |
//! | `first_on_page`, `last_on_page` | no previous or next line on the page |
//! | `blank_before` | a blank line preceded the line (plain text), or a vertical gap above 0.4 of the font size to the previous line of the same column (geometry); set by `read_lines` |
//! | `has_geometry` | the line has geometry |
//! | `font_rel` | font size over the document's weighted median body font size |
//! | `bold`, `italic` | typography flags |
//! | `x0_rel`, `width_rel` | left edge and width over the page width |
//! | `column` | 0 single or spanning, 1 left, 2 right |
//! | `gap_before_rel`, `gap_after_rel` | vertical gap to the previous or next line of the same page and column, over the font size (negative on the same row) |
//! | `prev_*`, `next_*` | `len_chars`, `upper_ratio`, `bullet_start`, `has_date`, `has_date_range`, `heading_any`, `font_rel`, `bold` of the neighbour (zeros and `font_rel` 1.0 when absent) |
//! | `prev_exists`, `next_exists` | the neighbour exists |

use super::dates::{contains_date, contains_ongoing_marker, contains_year, find_date_range};
use super::document::LineDocument;
use super::lexicon::{
    SectionKind, has_company_suffix, has_degree, has_title_keyword, heading_kind, is_closing,
    is_recipient, is_salutation, is_subject,
};
use super::locale::{LocaleHint, find_phones, find_postal_codes, find_region, has_cefr_level};
use super::model::TextLine;
use crate::profiles::extract::pattern::{EMAIL, GITHUB, LINKEDIN, URL};

/// The number of features of a line.
pub const FEATURE_COUNT: usize = 72;

/// The feature names, in vector order.
pub const FEATURE_NAMES: [&str; FEATURE_COUNT] = [
    "len_chars",
    "word_count",
    "upper_ratio",
    "title_word_ratio",
    "digit_ratio",
    "punct_ratio",
    "bullet_start",
    "ends_colon",
    "ends_period",
    "comma_count",
    "has_email",
    "has_phone",
    "has_url",
    "has_date",
    "has_date_range",
    "has_ongoing_marker",
    "has_year",
    "has_postal_code",
    "has_region",
    "has_cefr_level",
    "heading_summary",
    "heading_experience",
    "heading_education",
    "heading_skills",
    "heading_languages",
    "heading_projects",
    "heading_certifications",
    "heading_volunteering",
    "heading_publications",
    "heading_references",
    "heading_interests",
    "heading_contact",
    "heading_any",
    "has_degree",
    "has_company_suffix",
    "has_title_keyword",
    "is_salutation",
    "is_closing",
    "is_subject",
    "is_recipient",
    "rel_position",
    "page_index",
    "first_on_page",
    "last_on_page",
    "blank_before",
    "has_geometry",
    "font_rel",
    "bold",
    "italic",
    "x0_rel",
    "width_rel",
    "column",
    "gap_before_rel",
    "gap_after_rel",
    "prev_len_chars",
    "prev_upper_ratio",
    "prev_bullet_start",
    "prev_has_date",
    "prev_has_date_range",
    "prev_heading_any",
    "prev_font_rel",
    "prev_bold",
    "next_len_chars",
    "next_upper_ratio",
    "next_bullet_start",
    "next_has_date",
    "next_has_date_range",
    "next_heading_any",
    "next_font_rel",
    "next_bold",
    "prev_exists",
    "next_exists",
];

/// The names of the own features that the neighbours repeat, in the order of the `prev_*` and
/// `next_*` groups.
const CONTEXT_SOURCES: [&str; 8] = [
    "len_chars",
    "upper_ratio",
    "bullet_start",
    "has_date",
    "has_date_range",
    "heading_any",
    "font_rel",
    "bold",
];

/// The index of a feature name in the vector.
#[must_use]
pub fn feature_index(name: &str) -> Option<usize> {
    FEATURE_NAMES
        .iter()
        .position(|candidate| *candidate == name)
}

/// The number of own features (those before the `prev_*` group).
const OWN_COUNT: usize = 54;

fn flag(value: bool) -> f64 {
    if value { 1.0 } else { 0.0 }
}

fn ratio(count: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        count as f64 / total as f64
    }
}

const BULLETS: &str = "•●▪◦‣∙·○■□–—";

fn is_bullet_start(text: &str) -> bool {
    let mut characters = text.chars();
    match (characters.next(), characters.next()) {
        (Some(first), None) => BULLETS.contains(first),
        (Some(first), Some(second)) => {
            (BULLETS.contains(first) || first == '-' || first == '*') && second.is_whitespace()
        }
        _ => false,
    }
}

fn is_punctuation(character: char) -> bool {
    character.is_ascii_punctuation() || "–—•·“”‘’«»…".contains(character)
}

/// The font size at which half of the characters of the geometry lines are set at or below.
fn median_body_font(lines: &[TextLine]) -> f64 {
    let mut weighted: Vec<(i32, usize)> = lines
        .iter()
        .filter_map(|line| {
            line.geometry
                .map(|g| (g.font_size_centipoints, line.text.chars().count()))
        })
        .filter(|(font, _)| *font > 0)
        .collect();
    weighted.sort_unstable();
    let total: usize = weighted.iter().map(|(_, weight)| weight).sum();
    let mut seen = 0;
    for (font, weight) in &weighted {
        seen += weight;
        if seen * 2 >= total {
            return f64::from(*font);
        }
    }
    1.0
}

/// The own features of line `position`, in the order of the first `OWN_COUNT` names.
fn own_features(
    document: &LineDocument,
    position: usize,
    median_font: f64,
    hint: LocaleHint,
) -> Vec<f64> {
    let lines = &document.lines;
    let line = &lines[position];
    let text = line.text.as_str();
    let characters: Vec<char> = text.chars().collect();
    let letters = characters.iter().filter(|c| c.is_alphabetic()).count();
    let upper = characters.iter().filter(|c| c.is_uppercase()).count();
    let non_space = characters.iter().filter(|c| !c.is_whitespace()).count();
    let digits = characters.iter().filter(|c| c.is_ascii_digit()).count();
    let punctuation = characters.iter().filter(|c| is_punctuation(**c)).count();
    let words: Vec<&str> = text.split_whitespace().collect();
    let title_words = words
        .iter()
        .filter(|word| word.chars().next().is_some_and(char::is_uppercase))
        .count();
    let heading = heading_kind(text);
    let previous = position.checked_sub(1).map(|p| &lines[p]);
    let next = lines.get(position + 1);
    let on_same_page = |other: Option<&TextLine>| other.is_some_and(|o| o.page == line.page);
    let mut values = vec![
        characters.len() as f64,
        words.len() as f64,
        ratio(upper, letters),
        ratio(title_words, words.len()),
        ratio(digits, non_space),
        ratio(punctuation, non_space),
        flag(is_bullet_start(text)),
        flag(text.ends_with(':')),
        flag(text.ends_with('.')),
        characters.iter().filter(|c| **c == ',').count() as f64,
        flag(EMAIL.is_match(text)),
        flag(!find_phones(text).is_empty()),
        flag(
            URL.is_match(text)
                || LINKEDIN.is_match(text)
                || GITHUB.is_match(text)
                || text.to_ascii_lowercase().contains("www."),
        ),
        flag(contains_date(text)),
        flag(find_date_range(text, hint).is_some()),
        flag(contains_ongoing_marker(text)),
        flag(contains_year(text)),
        flag(!find_postal_codes(text).is_empty()),
        flag(find_region(text).is_some()),
        flag(has_cefr_level(text)),
    ];
    for kind in SectionKind::ALL {
        values.push(flag(heading == Some(kind)));
    }
    values.extend([
        flag(heading.is_some()),
        flag(has_degree(text)),
        flag(has_company_suffix(text)),
        flag(has_title_keyword(text)),
        flag(is_salutation(text)),
        flag(is_closing(text)),
        flag(is_subject(text)),
        flag(is_recipient(text)),
        if lines.len() > 1 {
            position as f64 / (lines.len() - 1) as f64
        } else {
            0.0
        },
        f64::from(line.page),
        flag(!on_same_page(previous)),
        flag(!on_same_page(next)),
        flag(line.blank_before),
    ]);
    values.extend(geometry_features(document, position, median_font));
    values
}

/// The nine geometry features of line `position`.
fn geometry_features(document: &LineDocument, position: usize, median_font: f64) -> [f64; 9] {
    let lines = &document.lines;
    let line = &lines[position];
    let Some(geometry) = line.geometry else {
        return [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    };
    let width = f64::from(geometry.page_width_centipoints.max(1));
    let font = f64::from(geometry.font_size_centipoints.max(1));
    let column = document.columns.get(position).copied().unwrap_or(0);
    let comparable = |other: &TextLine, other_position: usize| {
        other.page == line.page
            && document.columns.get(other_position).copied().unwrap_or(0) == column
    };
    let gap_before = position
        .checked_sub(1)
        .map(|p| (&lines[p], p))
        .filter(|(other, p)| comparable(other, *p))
        .and_then(|(other, _)| other.geometry)
        .map_or(0.0, |other| {
            f64::from(geometry.y0.saturating_sub(other.y1)) / font
        });
    let gap_after = lines
        .get(position + 1)
        .map(|other| (other, position + 1))
        .filter(|(other, p)| comparable(other, *p))
        .and_then(|(other, _)| other.geometry)
        .map_or(0.0, |other| {
            f64::from(other.y0.saturating_sub(geometry.y1)) / font
        });
    [
        1.0,
        if median_font > 0.0 {
            font / median_font
        } else {
            1.0
        },
        flag(geometry.bold),
        flag(geometry.italic),
        f64::from(geometry.x0) / width,
        f64::from(geometry.width()) / width,
        f64::from(column),
        gap_before,
        gap_after,
    ]
}

/// Whether two lines (by position) are on the same page and in the same column.
fn same_flow(document: &LineDocument, first: usize, second: usize) -> bool {
    match (document.lines.get(first), document.lines.get(second)) {
        (Some(a), Some(b)) => {
            a.page == b.page && document.columns.get(first) == document.columns.get(second)
        }
        _ => false,
    }
}

/// The feature vectors of every line of the document, in reading order. Each vector has
/// [`FEATURE_COUNT`] values.
#[must_use]
pub fn line_features(document: &LineDocument) -> Vec<Vec<f64>> {
    let median_font = median_body_font(&document.lines);
    let own: Vec<Vec<f64>> = (0..document.lines.len())
        .map(|position| own_features(document, position, median_font, document.locale))
        .collect();
    let context = |neighbour: Option<&Vec<f64>>| -> Vec<f64> {
        CONTEXT_SOURCES
            .iter()
            .map(|name| match neighbour {
                Some(values) => feature_index(name).map_or(0.0, |index| values[index]),
                None if *name == "font_rel" => 1.0,
                None => 0.0,
            })
            .collect()
    };
    (0..own.len())
        .map(|position| {
            // Context never crosses a page or a column break (as the gaps do not).
            let previous = position
                .checked_sub(1)
                .filter(|p| same_flow(document, *p, position))
                .and_then(|p| own.get(p));
            let next = Some(position + 1)
                .filter(|n| same_flow(document, position, *n))
                .and_then(|n| own.get(n));
            let mut row = own[position].clone();
            debug_assert_eq!(row.len(), OWN_COUNT);
            row.extend(context(previous));
            row.extend(context(next));
            row.push(flag(previous.is_some()));
            row.push(flag(next.is_some()));
            row
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_safe() {
        for (position, name) in FEATURE_NAMES.iter().enumerate() {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)),
                "{name}"
            );
            assert_eq!(FEATURE_NAMES.iter().position(|n| n == name), Some(position));
        }
        assert_eq!(FEATURE_NAMES[OWN_COUNT - 1], "gap_after_rel");
    }

    #[test]
    fn rows_have_the_declared_width() {
        let document = LineDocument::from_text("Alex Sample\nalex@example.com\n\nSkills\nRust");
        for row in line_features(&document) {
            assert_eq!(row.len(), FEATURE_COUNT);
        }
    }
}
