//! String helpers of the deterministic extractor: whitespace splitting, title case, the
//! Unicode decimal value, and URL normalization.

use jobniac_text::search::lower_text;
use jobniac_text::str_rules::{is_text_decimal, is_text_whitespace, strip_text};

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

/// The value of a token made only of decimal digits.
pub(super) fn decimal_text(token: &str) -> Option<u32> {
    if token.is_empty() {
        return None;
    }
    token.chars().try_fold(0_u32, |value, character| {
        decimal_value(character).map(|digit| value * 10 + digit)
    })
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
/// The lower-cased text is split on whitespace and the last token is read as the year. A start
/// with no space between the month and the year (`Jan2020`) is read as the spaced form
/// (`Jan 2020`).
pub(super) fn parse_date(value: &str) -> Option<String> {
    let lower = lower_text(value);
    let mut tokens: Vec<String> = split_words(&lower).map(str::to_owned).collect();
    let last = tokens.last()?.clone();
    if last == "present" || last == "current" {
        return None;
    }
    let (year, prefix) = match decimal_text(&last) {
        Some(year) => (year, None),
        None => split_year_token(&last)?,
    };
    if let Some(prefix) = prefix {
        tokens.pop();
        tokens.push(prefix);
        tokens.push(year.to_string());
    }
    let month = if tokens.len() == 1 {
        1
    } else {
        month_number(tokens.first()?)
    };
    Some(format!("{year:04}-{month:02}-01"))
}

/// Splits a token such as `sep2020` into its letter prefix and its trailing four-digit year.
fn split_year_token(token: &str) -> Option<(u32, Option<String>)> {
    let characters: Vec<char> = token.chars().collect();
    let split = characters.len().checked_sub(4)?;
    if split == 0 {
        return None;
    }
    let year_text: String = characters[split..].iter().collect();
    let year = decimal_text(&year_text)?;
    let prefix: String = characters[..split].iter().collect();
    Some((year, Some(prefix)))
}

/// The month number of the first three letters of a token, or 1 when unknown.
fn month_number(first: &str) -> u32 {
    let key: String = first.chars().take(3).collect();
    match key.as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => 1,
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
        assert_eq!(decimal_text("2020"), Some(2020));
        assert_eq!(decimal_text("\u{0662}\u{0660}\u{0662}\u{0660}"), Some(2020));
        assert_eq!(decimal_text("\u{1d7d8}\u{1d7d9}"), Some(1));
        assert_eq!(decimal_text("\u{1d7ff}"), Some(9));
        assert_eq!(decimal_text("20x0"), None);
        assert_eq!(decimal_text(""), None);
    }

    #[test]
    fn split_year_token_keeps_the_letter_prefix() {
        assert_eq!(parse_date("Jan2020").as_deref(), Some("2020-01-01"));
        assert_eq!(parse_date("Sept2020").as_deref(), Some("2020-09-01"));
        assert_eq!(parse_date("Sept 2020").as_deref(), Some("2020-09-01"));
        assert_eq!(parse_date("2019").as_deref(), Some("2019-01-01"));
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
