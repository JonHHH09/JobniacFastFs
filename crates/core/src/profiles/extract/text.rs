//! String helpers of the deterministic extractor: whitespace splitting, title case, the
//! Unicode decimal value, and URL normalization.

use jobniac_text::search::lower_text;
use jobniac_text::str_rules::{is_text_decimal, is_text_whitespace, strip_text};

use super::lines::dates::{DateOutcome, parse_date_text_in};
use super::lines::locale::LocaleHint;
use super::pattern::{LOCAL_SEPARATOR, URL};

/// Splits on runs of whitespace characters; they separate the words.
pub(super) fn split_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(is_text_whitespace)
        .filter(|part| !part.is_empty())
}

/// The first character upper-cased and the rest lower-cased, per word.
pub(super) fn title_case(value: &str) -> String {
    split_words(value)
        .map(|part| {
            let mut characters = part.chars();
            let first: String = characters
                .next()
                .map(|character| character.to_uppercase().collect())
                .unwrap_or_default();
            format!("{first}{}", lower_text(characters.as_str()))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The decimal value of a character, as Unicode decimal digits are read.
///
/// Unicode assigns decimal digits in runs of ten that start at zero, so the value is the offset
/// from the start of the run modulo ten.
pub(super) fn decimal_value(character: char) -> Option<u32> {
    if !is_text_decimal(character) {
        return None;
    }
    if character.is_ascii_digit() {
        return character.to_digit(10);
    }
    let mut start = character as u32;
    while start > 0 && char::from_u32(start - 1).is_some_and(is_text_decimal) {
        start -= 1;
    }
    Some((character as u32 - start) % 10)
}

/// Trailing `.,;` removed, the query and fragment dropped, trailing
/// slashes removed, and `https://` added unless the value already starts with `http://` or
/// `https://` (case-insensitively).
pub(super) fn normalize_url(value: &str) -> String {
    let trimmed = strip_text(value).trim_end_matches(['.', ',', ';']);
    let without_query = match trimmed.find(['?', '#']) {
        Some(index) => &trimmed[..index],
        None => trimmed,
    };
    let url = without_query.trim_end_matches('/');
    let lower = lower_text(url);
    if lower.starts_with("http://") || lower.starts_with("https://") {
        url.to_owned()
    } else {
        format!("https://{url}")
    }
}

/// The URL matches of `text`, in order.
pub(super) fn url_matches(text: &str) -> impl Iterator<Item = &str> {
    URL.find_iter(text).map(|found| found.as_str())
}

/// Parses a month-and-year text, where the text is the matched start or end of a date range,
/// without `present` or `current`.
///
/// This delegates to the line reader's date parser (`lines::dates`), with Unicode decimal digits
/// read as ASCII digits and any four-digit year accepted, so every date of the section split keeps
/// its output: a bare year is January of that year, a month name and year is the first of that
/// month, and `Jan2020` reads as `Jan 2020`.
pub(super) fn parse_date(value: &str) -> Option<String> {
    let ascii: String = value
        .chars()
        .map(|character| {
            decimal_value(character)
                .and_then(|digit| char::from_digit(digit, 10))
                .unwrap_or(character)
        })
        .collect();
    // The month table of the section split knows ASCII letters only: a month word with another
    // letter (`ſep`, long s) is an unknown month, which reads as January. The line reader folds
    // such letters (`ſ` to `s`), so keep only the year for them.
    let lowered = lower_text(&ascii);
    let word: String = lowered
        .chars()
        .take_while(|character| !character.is_ascii_digit())
        .collect();
    let ascii = if word.is_ascii() {
        ascii
    } else {
        lowered.chars().filter(char::is_ascii_digit).collect()
    };
    match parse_date_text_in(&ascii, LocaleHint::Unknown, (0, 9999)) {
        DateOutcome::Parsed(date) => Some(date.iso()),
        DateOutcome::Ambiguous | DateOutcome::NotADate => None,
    }
}

/// Replaces runs of `.`, `_` and `-` with a space and strips the result.
pub(super) fn local_part_words(value: &str) -> String {
    strip_text(&LOCAL_SEPARATOR.replace_all(value, " ")).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_values_follow_unicode_runs() {
        assert_eq!(decimal_value('7'), Some(7));
        assert_eq!(decimal_value('\u{0662}'), Some(2));
        assert_eq!(decimal_value('\u{0660}'), Some(0));
        assert_eq!(decimal_value('\u{1d7d9}'), Some(1));
        assert_eq!(decimal_value('\u{1d7ff}'), Some(9));
        assert_eq!(decimal_value('x'), None);
        assert_eq!(
            parse_date("\u{0662}\u{0660}\u{0662}\u{0660}").as_deref(),
            Some("2020-01-01")
        );
    }

    #[test]
    fn split_year_token_keeps_the_letter_prefix() {
        assert_eq!(parse_date("Jan2020").as_deref(), Some("2020-01-01"));
        assert_eq!(parse_date("Sept2020").as_deref(), Some("2020-09-01"));
        assert_eq!(parse_date("Sept 2020").as_deref(), Some("2020-09-01"));
        assert_eq!(parse_date("2019").as_deref(), Some("2019-01-01"));
        assert_eq!(parse_date("1850").as_deref(), Some("1850-01-01"));
        assert_eq!(parse_date("March 2021").as_deref(), Some("2021-03-01"));
        assert_eq!(parse_date("june2018").as_deref(), Some("2018-06-01"));
        // A long s is not an ASCII month letter: an unknown month, January.
        assert_eq!(parse_date("\u{17f}ep 2020").as_deref(), Some("2020-01-01"));
        assert_eq!(parse_date("present"), None);
    }

    #[test]
    fn urls_are_normalized() {
        assert_eq!(
            normalize_url("example.com/a/?x=1#y"),
            "https://example.com/a"
        );
        assert_eq!(
            normalize_url("HTTP://Example.com/x."),
            "HTTP://Example.com/x"
        );
        assert_eq!(normalize_url("https://a.example/b;"), "https://a.example/b");
    }

    #[test]
    fn title_case_uses_whitespace_words() {
        assert_eq!(title_case("ADA eXAMPLE"), "Ada Example");
        assert_eq!(title_case("  x\u{a0}y  "), "X Y");
    }
}
