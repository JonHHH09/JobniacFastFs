//! Locale-aware dates and date ranges in English, French, German and Albanian.
//!
//! Dates are parsed from the folded text (see [`fold`]): lower-case, accents dropped. The parsers
//! never guess: a numeric `a/b/YYYY` whose day/month order is not decided by the digits or the
//! [`LocaleHint`] is [`DateOutcome::Ambiguous`].
//!
//! | Form | Reading |
//! |---|---|
//! | `YYYY` | year only |
//! | `MM/YYYY`, `M-YYYY`, `MM.YYYY` | month, year |
//! | `YYYY-MM`, `YYYY/MM`, `YYYY.MM` | year, month |
//! | `YYYY-MM-DD` | ISO |
//! | `DD.MM.YYYY` | day first, always (German) |
//! | `a/b/YYYY`, `a-b-YYYY` | a day when it is above 12, b a day when it is above 12; both at most 12 and different: `Eu` day first, `Us` month first, `Ca` and `Unknown` ambiguous |
//! | `Jan 2020`, `janvier 2020`, `Januar 2020`, `Janar 2020`, `15 January 2020`, `January 15, 2020` | named month |

use std::sync::LazyLock;

use regex::Regex;

use super::lexicon::{
    END_INTRO_WORDS, MONTHS, ONGOING_MARKERS, RANGE_INTRO_WORDS, RANGE_WORDS, fold, month_number,
    ongoing_markers_longest_first,
};
use super::locale::LocaleHint;
use crate::profiles::extract::pattern::compile;

/// The lowest year read as a year.
const YEAR_MIN: i32 = 1900;
/// The highest year read as a year.
const YEAR_MAX: i32 = 2100;

/// A date that may lack its month or day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartialDate {
    /// The year, 1900 to 2100.
    pub year: i32,
    /// The month, 1 to 12, when written.
    pub month: Option<u8>,
    /// The day of the month, when written.
    pub day: Option<u8>,
}

impl PartialDate {
    /// The date as `YYYY-MM-DD`, with a missing month or day as `01` (the convention of the
    /// extractor's `parse_date`).
    #[must_use]
    pub fn iso(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}",
            self.year,
            self.month.unwrap_or(1),
            self.day.unwrap_or(1)
        )
    }

    fn sort_key(&self) -> (i32, u8, u8) {
        (self.year, self.month.unwrap_or(1), self.day.unwrap_or(1))
    }
}

/// The result of reading one date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateOutcome {
    /// A date.
    Parsed(PartialDate),
    /// A numeric date whose day/month order cannot be decided.
    Ambiguous,
    /// Not a date.
    NotADate,
}

/// The end of a date range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeEnd {
    /// An end date.
    Date(PartialDate),
    /// An ongoing marker (`Present`, `heute`, `sot`).
    Ongoing,
}

/// A date range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateRange {
    /// The start.
    pub start: PartialDate,
    /// The end.
    pub end: RangeEnd,
}

/// The result of reading one date range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeOutcome {
    /// A range.
    Range(DateRange),
    /// Only an end date (`until 2020`, `jusqu'à 2020`).
    Until(PartialDate),
    /// A side of the range is an ambiguous numeric date.
    Ambiguous,
    /// Not a range.
    NotARange,
}

/// The month alternation of the named-month patterns, longest names first.
fn month_alternation() -> String {
    let mut names: Vec<&str> = MONTHS.iter().map(|(_, name, _)| *name).collect();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    names.dedup();
    names.join("|")
}

/// The alternation of the ongoing markers, longest first, regex-escaped.
fn ongoing_alternation() -> String {
    ongoing_markers_longest_first()
        .iter()
        .map(|marker| regex::escape(marker))
        .collect::<Vec<_>>()
        .join("|")
}

/// The alternation of the range words (`to`, `au`, `bis`, `deri`).
fn range_word_alternation() -> String {
    RANGE_WORDS
        .iter()
        .map(|(_, word)| regex::escape(&fold(word)))
        .collect::<Vec<_>>()
        .join("|")
}

static ISO_YMD: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "DATE_ISO",
        r"^([0-9]{4})([-/.])([0-9]{1,2})([-/.])([0-9]{1,2})$",
    )
});
static YEAR_MONTH: LazyLock<Regex> =
    LazyLock::new(|| compile("DATE_YM", r"^([0-9]{4})[-/.]([0-9]{1,2})$"));
static MONTH_YEAR: LazyLock<Regex> =
    LazyLock::new(|| compile("DATE_MY", r"^([0-9]{1,2})[-/.]([0-9]{4})$"));
static NUMERIC_3: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "DATE_NUMERIC",
        r"^([0-9]{1,2})([-/.])([0-9]{1,2})([-/.])([0-9]{4})$",
    )
});
static YEAR_ONLY: LazyLock<Regex> = LazyLock::new(|| compile("DATE_YEAR", r"^([0-9]{4})$"));
static NAMED_DMY: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "DATE_NAMED_DMY",
        r"^([0-9]{1,2})(?:st|nd|rd|th|er)?\.?\s*([a-z]+)\.?\s*,?\s*([0-9]{4})$",
    )
});
static NAMED_MDY: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "DATE_NAMED_MDY",
        r"^([a-z]+)\.?\s*([0-9]{1,2})(?:st|nd|rd|th)?\s*,?\s*([0-9]{4})$",
    )
});
static NAMED_MY: LazyLock<Regex> =
    LazyLock::new(|| compile("DATE_NAMED_MY", r"^([a-z]+)\.?\s*,?\s*([0-9]{4})$"));

/// One date token inside running text (folded): the alternatives are tried longest first.
static DATE_TOKEN_SOURCE: LazyLock<String> = LazyLock::new(|| {
    let months = month_alternation();
    format!(
        r"(?:[0-9]{{1,2}}[-/.][0-9]{{1,2}}[-/.][0-9]{{4}}|[0-9]{{4}}[-/.][0-9]{{1,2}}(?:[-/.][0-9]{{1,2}})?|[0-9]{{1,2}}[-/.][0-9]{{4}}|(?:[0-9]{{1,2}}(?:st|nd|rd|th|er)?\.?\s*)?(?:{months})\.?\s*,?\s*(?:[0-9]{{1,2}}(?:st|nd|rd|th)?\s*,?\s*)?[0-9]{{4}}|[0-9]{{4}})"
    )
});

static DATE_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| compile("DATE_TOKEN", &format!(r"\b{}\b", *DATE_TOKEN_SOURCE)));

static RANGE_SEPARATOR: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "RANGE_SEPARATOR",
        &format!(r"\s*[{DASHES}]\s*|\s+(?:{})\s+", range_word_alternation()),
    )
});

/// The regex class body of the range dashes: hyphen-minus, U+2010 to U+2012, en and em dash and
/// the minus sign U+2212.
const DASHES: &str = r"\-\x{2010}-\x{2012}\x{2013}\x{2014}\x{2212}";
/// The same dashes as characters.
const DASH_CHARS: [char; 7] = [
    '-', '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2212}',
];

static RANGE_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    let date = &*DATE_TOKEN_SOURCE;
    let ongoing = ongoing_alternation();
    // Multi-word markers may follow a date without a separator (`bis dato`, `à ce jour`).
    let multiword = ongoing_markers_longest_first()
        .iter()
        .filter(|marker| marker.contains(' '))
        .map(|marker| regex::escape(marker))
        .collect::<Vec<_>>()
        .join("|");
    let sep = format!(
        r"(?:\s*[{DASHES}]\s*|\s+(?:{})\s+)",
        range_word_alternation()
    );
    compile(
        "RANGE_TOKEN",
        &format!(r"\b({date})(?:{sep}({date}|(?:{ongoing}))|\s+(?:{multiword}))(?:\b|$)"),
    )
});

/// An open range: an introducing word and one date (`since March 2019`, `seit 2019`).
static OPEN_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    let words = RANGE_INTRO_WORDS
        .iter()
        .map(|(_, word)| regex::escape(&fold(word)))
        .collect::<Vec<_>>()
        .join("|");
    compile(
        "OPEN_TOKEN",
        &format!(r"\b(?:{words})\s+({})\b", *DATE_TOKEN_SOURCE),
    )
});

/// The bare ongoing-marker alternation, anchored at the end of the text.
static ONGOING_AT_END: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "ONGOING_AT_END",
        &format!(r"(?:^|[^a-z0-9])({})$", ongoing_alternation()),
    )
});

static ONGOING_ANYWHERE: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "ONGOING_ANYWHERE",
        &format!(
            r"(?:^|[^a-z0-9])(?:{})(?:[^a-z0-9]|$)",
            ongoing_alternation()
        ),
    )
});

static YEAR_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| compile("YEAR_TOKEN", r"(?:^|[^0-9])((?:19|20)[0-9]{2})(?:[^0-9]|$)"));

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn number(text: &str) -> Option<i32> {
    text.parse().ok()
}

/// A validated date, or `NotADate`.
fn make_date(years: (i32, i32), year: i32, month: Option<i32>, day: Option<i32>) -> DateOutcome {
    if !(years.0..=years.1).contains(&year) {
        return DateOutcome::NotADate;
    }
    let month = match month {
        Some(value) => match u8::try_from(value) {
            Ok(value) if (1..=12).contains(&value) => Some(value),
            _ => return DateOutcome::NotADate,
        },
        None => None,
    };
    let day = match day {
        Some(value) => match (u8::try_from(value), month) {
            (Ok(value), Some(month)) if value >= 1 && value <= days_in_month(year, month) => {
                Some(value)
            }
            _ => return DateOutcome::NotADate,
        },
        None => None,
    };
    DateOutcome::Parsed(PartialDate { year, month, day })
}

/// Resolves the day and month of `a <sep> b <sep> year` (see the module table).
fn resolve_numeric(
    years: (i32, i32),
    a: i32,
    b: i32,
    separator: &str,
    year: i32,
    hint: LocaleHint,
) -> DateOutcome {
    let (day, month) = if a > 12 && b > 12 {
        return DateOutcome::NotADate;
    } else if a > 12 {
        (a, b)
    } else if b > 12 {
        (b, a)
    } else if a == b || separator == "." {
        (a, b)
    } else {
        match hint {
            LocaleHint::Eu => (a, b),
            LocaleHint::Us => (b, a),
            LocaleHint::Ca | LocaleHint::Unknown => return DateOutcome::Ambiguous,
        }
    };
    make_date(years, year, Some(month), Some(day))
}

fn named_month(name: &str) -> Option<i32> {
    month_number(name).map(i32::from)
}

/// Reads one date. See the module table for the forms.
#[must_use]
pub fn parse_date_text(text: &str, hint: LocaleHint) -> DateOutcome {
    parse_date_text_in(text, hint, (YEAR_MIN, YEAR_MAX))
}

/// [`parse_date_text`] with an explicit inclusive year range (the extractor's legacy date reader
/// accepts any four-digit year).
pub(in crate::profiles::extract) fn parse_date_text_in(
    text: &str,
    hint: LocaleHint,
    years: (i32, i32),
) -> DateOutcome {
    let folded = fold(text);
    let value = folded
        .trim_matches(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .trim_end_matches('.');
    if value.is_empty() {
        return DateOutcome::NotADate;
    }
    if let Some(c) = ISO_YMD.captures(value) {
        let (year, month, day) = (number(&c[1]), number(&c[3]), number(&c[5]));
        return match (year, month, day) {
            (Some(year), Some(month), Some(day)) => make_date(years, year, Some(month), Some(day)),
            _ => DateOutcome::NotADate,
        };
    }
    if let Some(c) = YEAR_MONTH.captures(value) {
        return match (number(&c[1]), number(&c[2])) {
            (Some(year), Some(month)) => make_date(years, year, Some(month), None),
            _ => DateOutcome::NotADate,
        };
    }
    if let Some(c) = MONTH_YEAR.captures(value) {
        return match (number(&c[1]), number(&c[2])) {
            (Some(month), Some(year)) => make_date(years, year, Some(month), None),
            _ => DateOutcome::NotADate,
        };
    }
    if let Some(c) = NUMERIC_3.captures(value) {
        return match (number(&c[1]), number(&c[3]), number(&c[5])) {
            (Some(a), Some(b), Some(year)) => resolve_numeric(years, a, b, &c[2], year, hint),
            _ => DateOutcome::NotADate,
        };
    }
    if let Some(c) = YEAR_ONLY.captures(value) {
        return number(&c[1]).map_or(DateOutcome::NotADate, |year| {
            make_date(years, year, None, None)
        });
    }
    if let Some(c) = NAMED_DMY.captures(value) {
        return match (number(&c[1]), named_month(&c[2]), number(&c[3])) {
            (Some(day), Some(month), Some(year)) => make_date(years, year, Some(month), Some(day)),
            _ => DateOutcome::NotADate,
        };
    }
    if let Some(c) = NAMED_MDY.captures(value)
        && let (Some(month), Some(day), Some(year)) =
            (named_month(&c[1]), number(&c[2]), number(&c[3]))
    {
        return make_date(years, year, Some(month), Some(day));
    }
    if let Some(c) = NAMED_MY.captures(value)
        && let (Some(month), Some(year)) = (named_month(&c[1]), number(&c[2]))
    {
        return make_date(years, year, Some(month), None);
    }
    DateOutcome::NotADate
}

/// Removes a leading range-introducing word (`from`, `depuis`, `von`, `nga`).
fn strip_intro(value: &str) -> &str {
    for (_, word) in RANGE_INTRO_WORDS {
        let word = fold(word);
        if let Some(rest) = value.strip_prefix(word.as_str())
            && rest.starts_with(' ')
        {
            return rest.trim_start();
        }
    }
    value
}

/// Removes a trailing range separator (a dash or a separator word) from the head of a range.
fn strip_trailing_separator(head: &str) -> &str {
    let mut head = head.trim_end();
    'again: loop {
        if let Some(stripped) = head.strip_suffix(DASH_CHARS) {
            head = stripped.trim_end();
            continue;
        }
        for (_, word) in RANGE_WORDS {
            let word = fold(word);
            if let Some(stripped) = head.strip_suffix(word.as_str())
                && stripped.ends_with(' ')
            {
                head = stripped.trim_end();
                continue 'again;
            }
        }
        return head;
    }
}

/// Removes a leading end-introducing word (`until`, `jusqu'à`, `bis`, `deri`).
fn strip_end_intro(value: &str) -> Option<&str> {
    for (_, word) in END_INTRO_WORDS {
        let word = fold(word);
        if let Some(rest) = value.strip_prefix(word.as_str())
            && rest.starts_with(' ')
        {
            return Some(rest.trim_start());
        }
    }
    None
}

/// Reads one date range, in any of the four languages.
///
/// The end is either a date or an ongoing marker. A range whose end is before its start is
/// [`RangeOutcome::NotARange`]. A numeric date with an undecidable day/month order makes the
/// range [`RangeOutcome::Ambiguous`].
#[must_use]
pub fn parse_date_range(text: &str, hint: LocaleHint) -> RangeOutcome {
    let folded = fold(text);
    let trimmed = folded
        .trim_matches(|c: char| c == ',' || c == ';' || c == ':' || c == '.' || c.is_whitespace());
    let value = strip_intro(trimmed);
    if value.is_empty() {
        return RangeOutcome::NotARange;
    }
    if let Some(rest) = strip_end_intro(trimmed) {
        return match parse_date_text(rest, hint) {
            DateOutcome::Parsed(end) => RangeOutcome::Until(end),
            DateOutcome::Ambiguous => RangeOutcome::Ambiguous,
            DateOutcome::NotADate => RangeOutcome::NotARange,
        };
    }
    if let Some(found) = ONGOING_AT_END.captures(value).and_then(|c| c.get(1)) {
        let head = strip_trailing_separator(&value[..found.start()]);
        return match parse_date_text(head, hint) {
            DateOutcome::Parsed(start) => RangeOutcome::Range(DateRange {
                start,
                end: RangeEnd::Ongoing,
            }),
            DateOutcome::Ambiguous => RangeOutcome::Ambiguous,
            DateOutcome::NotADate => RangeOutcome::NotARange,
        };
    }
    let mut ambiguous = false;
    for separator in RANGE_SEPARATOR.find_iter(value) {
        let left = parse_date_text(&value[..separator.start()], hint);
        let right = parse_date_text(&value[separator.end()..], hint);
        match (left, right) {
            (DateOutcome::Parsed(start), DateOutcome::Parsed(end)) => {
                if end.sort_key() < start.sort_key() {
                    return RangeOutcome::NotARange;
                }
                return RangeOutcome::Range(DateRange {
                    start,
                    end: RangeEnd::Date(end),
                });
            }
            (DateOutcome::Ambiguous, DateOutcome::Parsed(_) | DateOutcome::Ambiguous)
            | (DateOutcome::Parsed(_), DateOutcome::Ambiguous) => ambiguous = true,
            _ => {}
        }
    }
    if value.len() != trimmed.len() && !ambiguous {
        // An introducing word and one date is an open range: `since 2019`, `depuis janvier 2019`.
        match parse_date_text(value, hint) {
            DateOutcome::Parsed(start) => {
                return RangeOutcome::Range(DateRange {
                    start,
                    end: RangeEnd::Ongoing,
                });
            }
            DateOutcome::Ambiguous => return RangeOutcome::Ambiguous,
            DateOutcome::NotADate => {}
        }
    }
    if ambiguous {
        RangeOutcome::Ambiguous
    } else {
        RangeOutcome::NotARange
    }
}

/// The first date range inside running text, read with [`parse_date_range`].
#[must_use]
pub fn find_date_range(text: &str, hint: LocaleHint) -> Option<RangeOutcome> {
    let folded = fold(text);
    RANGE_TOKEN
        .find_iter(&folded)
        .chain(OPEN_TOKEN.find_iter(&folded))
        .map(|found| parse_date_range(found.as_str(), hint))
        .find(|outcome| *outcome != RangeOutcome::NotARange)
}

/// Whether the text holds a date that has more than a bare year (a month or a numeric date).
#[must_use]
pub fn contains_date(text: &str) -> bool {
    let folded = fold(text);
    DATE_TOKEN.find_iter(&folded).any(|found| {
        !YEAR_ONLY.is_match(found.as_str())
            && !matches!(
                parse_date_text(found.as_str(), LocaleHint::Unknown),
                DateOutcome::NotADate
            )
    })
}

/// Whether the text holds a plausible year (1900 to 2099).
#[must_use]
pub fn contains_year(text: &str) -> bool {
    YEAR_TOKEN.is_match(&fold(text))
}

/// Whether the text holds an open-ended date range (`2019 - present`, `seit 2019`), or is an
/// ongoing marker alone on its line. An ongoing word elsewhere (`the current migration in 2019`)
/// does not count.
#[must_use]
pub fn contains_ongoing_marker(text: &str) -> bool {
    let folded = fold(text);
    if !ONGOING_ANYWHERE.is_match(&folded) && !OPEN_TOKEN.is_match(&folded) {
        return false;
    }
    let alone = ONGOING_MARKERS
        .iter()
        .any(|(_, marker)| fold(marker) == folded.trim_matches(|c: char| !c.is_alphanumeric()));
    alone
        || find_date_range(text, LocaleHint::Unknown).is_some_and(|outcome| {
            matches!(
                outcome,
                RangeOutcome::Range(DateRange {
                    end: RangeEnd::Ongoing,
                    ..
                })
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_compile() {
        assert!(DATE_TOKEN.is_match("jan 2020"));
        assert!(RANGE_TOKEN.is_match("jan 2020 - present"));
        assert!(ONGOING_AT_END.is_match("x present"));
    }
}
