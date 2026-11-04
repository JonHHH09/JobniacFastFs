//! Locale rules: the document locale hint, phone forms, postal codes and US/Canadian regions.
//!
//! Dates are in [`super::dates`] and the word lexicons in [`super::lexicon`]. The existing
//! `PHONE` pattern of the extractor keeps its behaviour for `find_phone`; the phone finder here is
//! broader (international and Europass forms, every match of a text).

use std::sync::LazyLock;

use regex::Regex;

use super::lexicon::{fold, has_street_word};
use crate::profiles::extract::pattern::compile;

/// The convention a document follows, as far as it can be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LocaleHint {
    /// United States: month-first numeric dates.
    Us,
    /// Canada: numeric dates of both orders are in use, so an ambiguous one is refused.
    Ca,
    /// Europass and the EU: day-first numeric dates.
    Eu,
    /// Nothing decisive in the text.
    #[default]
    Unknown,
}

/// A US state or territory: code and name.
pub const US_STATES: &[(&str, &str)] = &[
    ("AL", "Alabama"),
    ("AK", "Alaska"),
    ("AZ", "Arizona"),
    ("AR", "Arkansas"),
    ("CA", "California"),
    ("CO", "Colorado"),
    ("CT", "Connecticut"),
    ("DE", "Delaware"),
    ("DC", "District of Columbia"),
    ("FL", "Florida"),
    ("GA", "Georgia"),
    ("HI", "Hawaii"),
    ("ID", "Idaho"),
    ("IL", "Illinois"),
    ("IN", "Indiana"),
    ("IA", "Iowa"),
    ("KS", "Kansas"),
    ("KY", "Kentucky"),
    ("LA", "Louisiana"),
    ("ME", "Maine"),
    ("MD", "Maryland"),
    ("MA", "Massachusetts"),
    ("MI", "Michigan"),
    ("MN", "Minnesota"),
    ("MS", "Mississippi"),
    ("MO", "Missouri"),
    ("MT", "Montana"),
    ("NE", "Nebraska"),
    ("NV", "Nevada"),
    ("NH", "New Hampshire"),
    ("NJ", "New Jersey"),
    ("NM", "New Mexico"),
    ("NY", "New York"),
    ("NC", "North Carolina"),
    ("ND", "North Dakota"),
    ("OH", "Ohio"),
    ("OK", "Oklahoma"),
    ("OR", "Oregon"),
    ("PA", "Pennsylvania"),
    ("RI", "Rhode Island"),
    ("SC", "South Carolina"),
    ("SD", "South Dakota"),
    ("TN", "Tennessee"),
    ("TX", "Texas"),
    ("UT", "Utah"),
    ("VT", "Vermont"),
    ("VA", "Virginia"),
    ("WA", "Washington"),
    ("WV", "West Virginia"),
    ("WI", "Wisconsin"),
    ("WY", "Wyoming"),
];

/// A Canadian province or territory: code and name.
pub const CA_PROVINCES: &[(&str, &str)] = &[
    ("AB", "Alberta"),
    ("BC", "British Columbia"),
    ("MB", "Manitoba"),
    ("NB", "New Brunswick"),
    ("NL", "Newfoundland and Labrador"),
    ("NS", "Nova Scotia"),
    ("NT", "Northwest Territories"),
    ("NU", "Nunavut"),
    ("ON", "Ontario"),
    ("PE", "Prince Edward Island"),
    ("QC", "Québec"),
    ("SK", "Saskatchewan"),
    ("YT", "Yukon"),
];

/// Country and city names that mark a European (EU or Europass) document.
pub const EU_PLACES: &[&str] = &[
    "Deutschland",
    "Germany",
    "Allemagne",
    "Österreich",
    "Austria",
    "Schweiz",
    "Switzerland",
    "Suisse",
    "France",
    "Belgique",
    "Belgium",
    "Shqipëri",
    "Albania",
    "Tiranë",
    "Tirana",
    "Italia",
    "Italy",
    "España",
    "Spain",
    "Nederland",
    "Netherlands",
    "Portugal",
    "Polska",
    "Poland",
    "Ireland",
    "Berlin",
    "Wien",
    "Vienna",
    "Hamburg",
    "München",
    "Munich",
    "Köln",
    "Cologne",
    "Leipzig",
    "Frankfurt",
    "Stuttgart",
    "Zürich",
    "Genève",
    "Paris",
    "Lyon",
    "Lille",
    "Marseille",
    "Durrës",
    "Shkodër",
    "Vlorë",
    "Kosovo",
    "Kosova",
    "Prishtina",
    "Prishtinë",
    "Pristina",
    "North Macedonia",
    "Skopje",
    "Montenegro",
    "Podgorica",
];

/// The folded words of a text, for whole-word and phrase matching.
fn words_of(folded: &str) -> Vec<&str> {
    folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect()
}

/// Whether the words hold the phrase as consecutive words.
fn has_phrase(words: &[&str], phrase: &str) -> bool {
    let wanted: Vec<&str> = phrase.split_whitespace().collect();
    !wanted.is_empty()
        && words
            .windows(wanted.len())
            .any(|window| window == wanted.as_slice())
}

/// Whether the text names a place of [`EU_PLACES`] as whole words.
#[must_use]
pub fn has_eu_place(text: &str) -> bool {
    let folded = fold(text);
    let words = words_of(&folded);
    EU_PLACES
        .iter()
        .any(|place| has_phrase(&words, &fold(place)))
}

/// Phrases that mark a Europass document or its language grid.
pub const EUROPASS_MARKERS: &[&str] = &[
    "europass",
    "mother tongue",
    "spoken interaction",
    "spoken production",
    "muttersprache",
    "langue maternelle",
    "gjuha amtare",
];

/// A country of a region code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Country {
    /// United States.
    Us,
    /// Canada.
    Ca,
}

/// A US state or Canadian province found in a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    /// The country the code belongs to.
    pub country: Country,
    /// The two-letter code.
    pub code: &'static str,
}

fn lookup_code(code: &str) -> Option<Region> {
    US_STATES
        .iter()
        .find(|(known, _)| *known == code)
        .map(|(known, _)| Region {
            country: Country::Us,
            code: known,
        })
        .or_else(|| {
            CA_PROVINCES
                .iter()
                .find(|(known, _)| *known == code)
                .map(|(known, _)| Region {
                    country: Country::Ca,
                    code: known,
                })
        })
}

static COMMA_CODE: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "COMMA_REGION",
        r"(?:,\s*|\()([A-Z]{2})(?:\)|,|$|\s+(?:[0-9]{5}|[A-Za-z][0-9][A-Za-z]))",
    )
});
static CODE_BEFORE_POSTAL: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "REGION_POSTAL",
        r"\b([A-Z]{2})\s+(?:[0-9]{5}(?:-[0-9]{4})?|[A-Za-z][0-9][A-Za-z][ \x{a0}]?[0-9][A-Za-z][0-9])\b",
    )
});
static COMMA_NAME: LazyLock<Regex> = LazyLock::new(|| {
    let mut names: Vec<(String, bool)> = US_STATES
        .iter()
        .map(|(_, name)| (fold(name), false))
        .chain(CA_PROVINCES.iter().map(|(_, name)| (fold(name), true)))
        .collect();
    names.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    let alternation = names
        .iter()
        .map(|(name, _)| regex::escape(name))
        .collect::<Vec<_>>()
        .join("|");
    compile(
        "COMMA_REGION_NAME",
        &format!(r"(?:,\s*|\()({alternation})(?:\)|,|$|\s+(?:[0-9]{{5}}|[a-z][0-9][a-z]))"),
    )
});

/// The US state or Canadian province in an address-like text.
///
/// A two-letter code counts when it follows a comma (`Springfield, IL`), sits in parentheses
/// (`Montréal (QC)`), or precedes a ZIP or postal code; a full name counts after a comma.
#[must_use]
pub fn find_region(text: &str) -> Option<Region> {
    for pattern in [&COMMA_CODE, &CODE_BEFORE_POSTAL] {
        for captures in pattern.captures_iter(text) {
            if let Some(region) = captures.get(1).and_then(|code| lookup_code(code.as_str())) {
                return Some(region);
            }
        }
    }
    let folded = fold(text);
    let name = COMMA_NAME.captures(&folded)?.get(1)?.as_str().to_owned();
    US_STATES
        .iter()
        .find(|(_, candidate)| fold(candidate) == name)
        .map(|(code, _)| Region {
            country: Country::Us,
            code,
        })
        .or_else(|| {
            CA_PROVINCES
                .iter()
                .find(|(_, candidate)| fold(candidate) == name)
                .map(|(code, _)| Region {
                    country: Country::Ca,
                    code,
                })
        })
}

/// The shape of a postal code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostalKind {
    /// Five-digit US ZIP after a state code.
    UsZip,
    /// US ZIP+4.
    UsZipPlus4,
    /// Canadian `A1A 1A1`.
    CaPostal,
    /// Four- or five-digit European code in an address context.
    Eu,
}

/// A postal code found in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostalMatch {
    /// The shape of the code.
    pub kind: PostalKind,
    /// The code as written (an EU code keeps its country prefix: `D-10115`).
    pub text: String,
    /// Byte offset of the start in the searched text.
    pub start: usize,
    /// Byte offset of the end in the searched text.
    pub end: usize,
}

static CA_POSTAL: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "POSTAL_CA",
        r"\b[ABCEGHJ-NPRSTVXYabceghj-nprstvxy][0-9][ABCEGHJ-NPRSTV-Zabceghj-nprstv-z][ \x{a0}]?[0-9][ABCEGHJ-NPRSTV-Zabceghj-nprstv-z][0-9]\b",
    )
});
static US_ZIP_PLUS4: LazyLock<Regex> =
    LazyLock::new(|| compile("POSTAL_ZIP4", r"\b[0-9]{5}-[0-9]{4}\b"));
static US_ZIP: LazyLock<Regex> =
    LazyLock::new(|| compile("POSTAL_ZIP", r"\b([A-Z]{2})\s+([0-9]{5})\b"));
static EU_POSTAL: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "POSTAL_EU",
        r"(?:\b([A-Z]{1,2})-)?\b([0-9]{4,5})\b\s+\p{Lu}[\p{L}'-]+",
    )
});

/// Every postal code of a text, in order: Canadian codes, ZIP+4, ZIP after a state code, and
/// European codes in an address context.
///
/// A European code needs a capitalized city word after it, no street word after it (that would
/// make it a house number: `1200 Market Street`), and one of: a country prefix (`D-10115`), a
/// known EU place on the line ([`EU_PLACES`]), or a comma before it with only the city (one to
/// three words) after it. A four-digit number from 1900 to 2100 needs the prefix or the comma, so
/// `2020 Software Engineer` is not a postal code.
///
/// The address context is the line of the code, also when `text` holds many lines (a whole
/// document): the place, the comma and the city are looked for on that line, and the street word
/// among the [`STREET_CONTEXT_WORDS`] words after the code. Each line is read once, so the time is
/// linear in the length of the text.
#[must_use]
pub fn find_postal_codes(text: &str) -> Vec<PostalMatch> {
    let mut found: Vec<PostalMatch> = Vec::new();
    // `found` holds non-overlapping matches sorted by start (so by end too) whenever this is called.
    let overlaps = |found: &[PostalMatch], start: usize, end: usize| {
        let first_ending_after = found.partition_point(|m| m.end <= start);
        found.get(first_ending_after).is_some_and(|m| m.start < end)
    };
    for m in CA_POSTAL.find_iter(text) {
        found.push(PostalMatch {
            kind: PostalKind::CaPostal,
            text: m.as_str().to_owned(),
            start: m.start(),
            end: m.end(),
        });
    }
    for m in US_ZIP_PLUS4.find_iter(text) {
        found.push(PostalMatch {
            kind: PostalKind::UsZipPlus4,
            text: m.as_str().to_owned(),
            start: m.start(),
            end: m.end(),
        });
    }
    // A Canadian code and a ZIP+4 cannot overlap (one starts with a letter, the other is digits
    // and a hyphen between word boundaries), so the two passes give non-overlapping matches.
    found.sort_by_key(|m| m.start);
    let before_zip = found.len();
    for captures in US_ZIP.captures_iter(text) {
        if let (Some(code), Some(zip)) = (captures.get(1), captures.get(2))
            && lookup_code(code.as_str()).is_some_and(|r| r.country == Country::Us)
            && !overlaps(&found[..before_zip], zip.start(), zip.end())
        {
            found.push(PostalMatch {
                kind: PostalKind::UsZip,
                text: zip.as_str().to_owned(),
                start: zip.start(),
                end: zip.end(),
            });
        }
    }
    found.sort_by_key(|m| m.start);
    let before_eu = found.len();
    let mut line: Option<LineContext> = None;
    for captures in EU_POSTAL.captures_iter(text) {
        let Some(digits) = captures.get(2) else {
            continue;
        };
        let prefix = captures.get(1);
        let start = prefix.map_or(digits.start(), |p| p.start());
        let end = digits.end();
        if overlaps(&found[..before_eu], start, end) {
            continue;
        }
        let context = LineContext::of(text, start, &mut line);
        let before = text[context.start..start].trim_end();
        let line_rest = &text[end..context.end];
        let rest = first_words(line_rest, STREET_CONTEXT_WORDS);
        let year_like = digits.as_str().len() == 4
            && digits
                .as_str()
                .parse::<i32>()
                .is_ok_and(|n| (1900..=2100).contains(&n));
        // Four words are enough to tell one to three city words from more.
        let city_words: Vec<&str> = line_rest.split_whitespace().take(4).collect();
        let comma_city_end = before.ends_with(',')
            && (1..=3).contains(&city_words.len())
            && city_words.iter().all(|word| {
                word.chars().next().is_some_and(char::is_alphabetic)
                    && !word.chars().any(|c| c.is_ascii_digit())
            });
        // A code before a street word is a house number (`1200 Market Street`).
        // A year-like code (`2001 Durrës`) also needs the line to be just the code and the city.
        let line_is_code_and_city = before.is_empty()
            && (1..=3).contains(&city_words.len())
            && city_words
                .iter()
                .all(|word| word.chars().next().is_some_and(char::is_alphabetic));
        let accepted = !has_street_word(rest)
            && (prefix.is_some()
                || (!year_like && context.has_eu_place(text))
                || (year_like && line_is_code_and_city && context.has_eu_place(text))
                || comma_city_end);
        if accepted {
            found.push(PostalMatch {
                kind: PostalKind::Eu,
                text: text[start..end].to_owned(),
                start,
                end,
            });
        }
    }
    found.sort_by_key(|m| m.start);
    found
}

/// How many words after a European postal code are searched for a street word (`1200 North
/// Market Street` has it third).
pub const STREET_CONTEXT_WORDS: usize = 6;

/// The line of a text that a postal code is on: its byte range, and whether it names a known EU
/// place (read once per line, when first asked).
#[derive(Debug, Clone, Copy)]
struct LineContext {
    start: usize,
    end: usize,
    eu_place: Option<bool>,
}

impl LineContext {
    /// The line around byte offset `at`, reusing `current` when `at` is on it. Called with
    /// increasing offsets, every byte of the text is scanned at most twice in total.
    fn of<'a>(text: &str, at: usize, current: &'a mut Option<Self>) -> &'a mut Self {
        let line = match *current {
            Some(line) if line.start <= at && at <= line.end => line,
            _ => Self {
                start: text[..at].rfind('\n').map_or(0, |index| index + 1),
                end: text[at..].find('\n').map_or(text.len(), |index| at + index),
                eu_place: None,
            },
        };
        current.insert(line)
    }

    /// Whether the line names a known EU place ([`has_eu_place`]).
    fn has_eu_place(&mut self, text: &str) -> bool {
        let (start, end) = (self.start, self.end);
        *self
            .eu_place
            .get_or_insert_with(|| has_eu_place(&text[start..end]))
    }
}

/// The start of `text` up to the end of its `count`-th whitespace-separated word.
fn first_words(text: &str, count: usize) -> &str {
    let end = text
        .split_whitespace()
        .take(count)
        .last()
        .map_or(0, |word| {
            word.as_ptr() as usize - text.as_ptr() as usize + word.len()
        });
    &text[..end]
}

/// The shape of a phone number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneKind {
    /// North American: `(202) 555-0147`, `202-555-0147`, `+1 202 555 0147`, `202.555.0147`.
    Nanp,
    /// Europass: the country code in parentheses, `(+1) 2025550147`, `(+49) 30 1234567`.
    Europass,
    /// International `+CC ...`: `+49 30 1234567`, `+355 69 123 4567`.
    International,
}

/// A phone number found in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneMatch {
    /// The shape of the number.
    pub kind: PhoneKind,
    /// The number as written.
    pub text: String,
    /// Byte offset of the start in the searched text.
    pub start: usize,
    /// Byte offset of the end in the searched text.
    pub end: usize,
}

static PHONE_EUROPASS: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "PHONE_EUROPASS",
        r"\(\+[0-9]{1,3}\)[ .-]?[0-9][0-9 .-]{5,13}[0-9]",
    )
});
static PHONE_INTERNATIONAL: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "PHONE_INTERNATIONAL",
        r"\+[0-9]{1,3}[ .-]?(?:\([0-9]{1,4}\)[ .-]?)?[0-9][0-9 .-]{5,14}[0-9]",
    )
});
static PHONE_NANP: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "PHONE_NANP",
        r"(?:\+?1[-. ]?)?(?:\([2-9][0-9]{2}\)|[2-9][0-9]{2})[-. ]?[2-9][0-9]{2}[-. ]?[0-9]{4}",
    )
});

fn digit_count(text: &str) -> usize {
    text.chars().filter(char::is_ascii_digit).count()
}

/// Whether the match is not glued to digits.
fn bounded(text: &str, start: usize, end: usize) -> bool {
    !text[..start]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_digit())
        && !text[end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
}

/// The length of the number without a trailing year group (`+49 30 1234567 2020`), when at least
/// nine digits remain.
fn without_year_tail(number: &str) -> usize {
    if let Some(index) = number.rfind([' ', '.', '-']) {
        let tail = &number[index + 1..];
        let year = tail.len() == 4
            && tail.chars().all(|c| c.is_ascii_digit())
            && (tail.starts_with("19") || tail.starts_with("20"));
        if year && digit_count(&number[..index]) >= 9 {
            return index;
        }
    }
    number.len()
}

/// Every phone number of a text, in order. Europass and international forms need 8 to 15 digits;
/// a `+1` number with 11 digits is [`PhoneKind::Nanp`].
#[must_use]
pub fn find_phones(text: &str) -> Vec<PhoneMatch> {
    let mut found: Vec<PhoneMatch> = Vec::new();
    for (pattern, kind) in [
        (&PHONE_EUROPASS, PhoneKind::Europass),
        (&PHONE_INTERNATIONAL, PhoneKind::International),
        (&PHONE_NANP, PhoneKind::Nanp),
    ] {
        for m in pattern.find_iter(text) {
            let length = if kind == PhoneKind::Nanp {
                m.as_str().len()
            } else {
                without_year_tail(m.as_str())
            };
            let matched = &m.as_str()[..length];
            let end = m.start() + length;
            let digits = digit_count(matched);
            if !bounded(text, m.start(), end)
                || found.iter().any(|f| m.start() < f.end && f.start < end)
                || (kind != PhoneKind::Nanp && !(8..=15).contains(&digits))
            {
                continue;
            }
            let kind =
                if kind == PhoneKind::International && matched.starts_with("+1") && digits == 11 {
                    PhoneKind::Nanp
                } else {
                    kind
                };
            found.push(PhoneMatch {
                kind,
                text: matched.to_owned(),
                start: m.start(),
                end,
            });
        }
    }
    found.sort_by_key(|m| m.start);
    found
}

static CEFR_LEVEL: LazyLock<Regex> = LazyLock::new(|| compile("CEFR_LEVEL", r"\b[ABCabc][12]\b"));

/// Whether the text has three or more CEFR level tokens (`B2`, `C1`), as a Europass language grid.
#[must_use]
pub fn has_cefr_grid(text: &str) -> bool {
    CEFR_LEVEL.find_iter(text).count() >= 3
}

/// Whether the text has a single CEFR level token.
#[must_use]
pub fn has_cefr_level(text: &str) -> bool {
    CEFR_LEVEL.is_match(text)
}

/// Reads the document locale from its whole text.
///
/// Evidence is weighted: a US state with a ZIP, or a Canadian province with a postal code, counts
/// 3; Europass markers, a CEFR grid and non-NANP phone numbers count 2 or 3; a country name 2; a
/// NANP phone number alone 1 (for the US). The highest score wins; a tie, or no evidence, is
/// [`LocaleHint::Unknown`].
#[must_use]
pub fn detect_locale(text: &str) -> LocaleHint {
    let folded = fold(text);
    let (mut us, mut ca, mut eu) = (0_u32, 0_u32, 0_u32);
    for m in find_postal_codes(text) {
        match m.kind {
            PostalKind::CaPostal => ca += 3,
            PostalKind::UsZip | PostalKind::UsZipPlus4 => us += 3,
            PostalKind::Eu => eu += 1,
        }
    }
    if let Some(region) = find_region(text) {
        match region.country {
            Country::Us => us += 1,
            Country::Ca => ca += 1,
        }
    }
    let mut nanp = false;
    for phone in find_phones(text) {
        match phone.kind {
            PhoneKind::Nanp => nanp = true,
            PhoneKind::Europass | PhoneKind::International => eu += 2,
        }
    }
    if nanp {
        us += 1;
    }
    if EUROPASS_MARKERS
        .iter()
        .any(|marker| folded.contains(marker))
    {
        eu += 3;
    }
    if has_cefr_grid(text) {
        eu += 2;
    }
    let words = words_of(&folded);
    if has_phrase(&words, "united states") || has_phrase(&words, "usa") {
        us += 2;
    }
    if has_phrase(&words, "canada") {
        ca += 2;
    }
    for place in EU_PLACES {
        if has_phrase(&words, &fold(place)) {
            eu += 2;
        }
    }
    let best = us.max(ca).max(eu);
    let winners = [us, ca, eu].iter().filter(|&&score| score == best).count();
    if best == 0 || winners != 1 {
        LocaleHint::Unknown
    } else if us == best {
        LocaleHint::Us
    } else if ca == best {
        LocaleHint::Ca
    } else {
        LocaleHint::Eu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_compile() {
        assert!(COMMA_NAME.is_match(", ontario"));
        assert!(CODE_BEFORE_POSTAL.is_match("IL 62701"));
        assert!(EU_POSTAL.is_match("10115 Berlin"));
        assert!(!find_phones("(202) 555-0147").is_empty());
    }
}
