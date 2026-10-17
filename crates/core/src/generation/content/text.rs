//! Text, ordering and date helpers of the resume bodies.

use jobniac_text::java::{java_is_blank, java_string_key, java_strip};
use time::Date;

/// True when the value is present and not blank.
pub(super) fn has_text(value: Option<&str>) -> bool {
    value.is_some_and(|text| !java_is_blank(text))
}

/// The stripped value when it has text, else the fallback as given.
pub(super) fn default_text(value: Option<&str>, fallback: &str) -> String {
    match value {
        Some(text) if !java_is_blank(text) => java_strip(text).to_owned(),
        _ => fallback.to_owned(),
    }
}

/// `prefix`, the stripped value and `suffix` when the value has text, else the empty string.
pub(super) fn optional_text(value: Option<&str>, prefix: &str, suffix: &str) -> String {
    match value {
        Some(text) if !java_is_blank(text) => format!("{prefix}{}{suffix}", java_strip(text)),
        _ => String::new(),
    }
}

/// `label: value` for a contact or link row, where the label defaults to the stripped type.
pub(super) fn label_value(label: Option<&str>, kind: &str, value: &str) -> String {
    format!(
        "{}: {}",
        default_text(label, java_strip(kind)),
        java_strip(value)
    )
}

/// Sorts rows by display order, then by the UTF-16 order of their text.
///
/// The sort is stable, so equal keys keep their input order.
pub(super) fn ordered_text<T>(
    rows: &[T],
    order: impl Fn(&T) -> i64,
    text: impl Fn(&T) -> &str,
) -> Vec<&T> {
    let mut keyed: Vec<(i64, Vec<u8>, &T)> = rows
        .iter()
        .map(|row| (order(row), java_string_key(text(row)), row))
        .collect();
    keyed.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
    keyed.into_iter().map(|(_, _, row)| row).collect()
}

/// Like [`ordered_text`], for a nullable text: a missing text sorts after every present one.
pub(super) fn ordered_nullable<T>(
    rows: &[T],
    order: impl Fn(&T) -> i64,
    text: impl Fn(&T) -> Option<&str>,
) -> Vec<&T> {
    let mut keyed: Vec<(i64, bool, Vec<u8>, &T)> = rows
        .iter()
        .map(|row| match text(row) {
            Some(value) => (order(row), false, java_string_key(value), row),
            None => (order(row), true, Vec::new(), row),
        })
        .collect();
    keyed.sort_by(|left, right| (left.0, left.1, &left.2).cmp(&(right.0, right.1, &right.2)));
    keyed.into_iter().map(|(_, _, _, row)| row).collect()
}

/// `YYYY-MM` of a date, the first seven characters of its ISO form.
fn year_month(date: Date) -> String {
    format!("{:04}-{:02}", date.year(), u8::from(date.month()))
}

/// The period of an entry: `YYYY-MM - YYYY-MM`, with the French or English placeholders for a
/// missing start or end, and the full placeholder when both are missing.
pub(super) fn period(start: Option<Date>, end: Option<Date>, french: bool) -> String {
    match (start, end) {
        (None, None) => {
            if french {
                "Dates non précisées".to_owned()
            } else {
                "Dates not provided".to_owned()
            }
        }
        (start, end) => {
            let start_text = match start {
                Some(date) => year_month(date),
                None => if french { "Inconnu" } else { "Unknown" }.to_owned(),
            };
            let end_text = match end {
                Some(date) => year_month(date),
                None => if french { "Présent" } else { "Present" }.to_owned(),
            };
            format!("{start_text} - {end_text}")
        }
    }
}
