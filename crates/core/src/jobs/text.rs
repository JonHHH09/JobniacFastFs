//! Text rules of the job aggregate: canonical whitespace, the text hash,
//! the canonical fingerprint, normalized keys, skill lists, experience and title extraction.
//!
//! Java `String.strip()` comes from `jobniac-text::java`; the Unicode lower-casing, NFKD and the
//! search-text fold come from `jobniac-text::search`. The skills-line and experience-line patterns
//! are implemented by hand: each matcher tries the
//! alternatives in the same greedy order as a regular-expression engine, so the first match and its
//! group are the same. Matching is case-insensitive in the Unicode sense: `ı`, `İ`, `ſ` and the
//! Kelvin sign fold to the ASCII letter that a case-insensitive regular expression matches.
//!
//! The vectors in `tests/golden/jobs-text.json` pin these rules.

use std::collections::HashSet;

use jobniac_text::java::{java_strip, utf16_length};
use jobniac_text::search::{lower_text, normalize};
use sha2::{Digest, Sha256};

/// Longest derived title, in Java UTF-16 code units.
pub const TITLE_MAX_UTF16_UNITS: usize = 160;

/// The title used when no line of the text is short enough.
pub const UNTITLED_JOB: &str = "Untitled Job";

/// Whitespace of the regular-expression class `[ \t\n\x0b\f\r]` (not Unicode whitespace).
fn is_pattern_whitespace(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\u{0b}' | '\u{0c}' | '\r')
}

/// Every run of pattern whitespace becomes one space.
pub fn collapse_whitespace(value: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    let mut in_run = false;
    for character in value.chars() {
        if is_pattern_whitespace(character) {
            if !in_run {
                collapsed.push(' ');
            }
            in_run = true;
        } else {
            collapsed.push(character);
            in_run = false;
        }
    }
    collapsed
}

/// Java-stripped, pattern whitespace collapsed, lower-cased with the Unicode 16.0.0 case mapping,
/// so the code points that Unicode 17.0 added a lowercase mapping for stay unchanged.
pub fn canonical_whitespace(value: &str) -> String {
    lower_text(&collapse_whitespace(java_strip(value)))
}

/// The SHA-256 of the canonical whitespace form: the hash that identifies pasted text.
///
/// The argument is the raw pasted text; the Java strip happens inside the canonicalization, so the
/// stripped text is hashed.
pub fn input_text_hash(input_text: &str) -> String {
    sha256_hex(&canonical_whitespace(input_text))
}

/// The search fold of the value, with pattern whitespace collapsed.
///
/// `None` and blank text give an empty key. Only Java whitespace is stripped first, so a leading
/// U+00A0 survives and folds to a space.
pub fn normalized_key(value: Option<&str>) -> String {
    collapse_whitespace(&normalized_search_text(value))
}

/// Java strip, lower-case, NFKD, marks removed.
///
/// `jobniac-text::search::normalize` applies a Unicode `strip` before folding, which would
/// remove characters the Java strip keeps. A combining acute accent is wrapped around the text
/// instead: it is a mark (removed by the fold), is not whitespace (so nothing is stripped), and is
/// case-ignorable (so final-sigma context is the same as for the bare text).
fn normalized_search_text(value: Option<&str>) -> String {
    const SENTINEL: char = '\u{0301}';
    let Some(value) = value else {
        return String::new();
    };
    let stripped = java_strip(value);
    if stripped.is_empty() {
        return String::new();
    }
    let wrapped: String = std::iter::once(SENTINEL)
        .chain(stripped.chars())
        .chain(std::iter::once(SENTINEL))
        .collect();
    normalize(Some(&wrapped))
}

/// SHA-256 hex of the four normalized fields, joined by newlines. The
/// canonical source URL, when present, is the first field.
pub fn fingerprint(
    title: &str,
    company: Option<&str>,
    location: Option<&str>,
    description: &str,
    canonical_source_url: Option<&str>,
) -> String {
    let mut fields = vec![
        normalized_key(Some(title)),
        normalized_key(company),
        normalized_key(location),
        canonical_whitespace(description),
    ];
    if let Some(url) = canonical_source_url {
        fields.insert(0, normalized_key(Some(url)));
    }
    sha256_hex(&fields.join("\n"))
}

/// Splits on `,`, `;`, `|` and `/`, Java-strip each part, keep the
/// non-empty parts.
pub fn split_skill_list(value: &str) -> Vec<String> {
    value
        .split([',', ';', '|', '/'])
        .map(java_strip)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The explicit skills, then the extracted ones, de-duplicated on the
/// normalized key (first occurrence wins), each Java-stripped.
pub fn merge_skills(explicit: &[String], extracted: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for skill in explicit.iter().chain(extracted) {
        if seen.insert(normalized_key(Some(skill))) {
            unique.push(java_strip(skill).to_owned());
        }
    }
    unique
}

/// The split group of the first `skills:` (or `required skills:`,
/// `technical skills:`) label of the text, or no skills when there is no such label.
pub fn extract_skills(text: &str) -> Vec<String> {
    let characters: Vec<char> = text.chars().collect();
    match skills_group(&characters) {
        Some(group) => split_skill_list(&group),
        None => Vec::new(),
    }
}

/// The Java-stripped rest of the first line that starts with
/// `experience`, `requirement(s)` or `qualification(s)` followed by `:` or `-`. Blank gives `None`.
pub fn extract_experience(text: &str) -> Option<String> {
    let characters: Vec<char> = text.chars().collect();
    let group = experience_group(&characters)?;
    let stripped = java_strip(&group);
    (!stripped.is_empty()).then(|| stripped.to_owned())
}

/// The first line (CR LF, LF or CR separated) that is non-blank after the
/// Java strip and at most 160 UTF-16 units long; otherwise `Untitled Job`.
pub fn derive_title(text: &str) -> String {
    for line in split_lines(text) {
        let title = java_strip(line);
        if !title.is_empty() && utf16_length(title) <= TITLE_MAX_UTF16_UNITS {
            return title.to_owned();
        }
    }
    UNTITLED_JOB.to_owned()
}

/// Splits on `\r\n`, `\n` and `\r`, as `re.split(r'\r\n|\n|\r', text)` does. U+2028 and the other
/// Unicode separators are not line breaks here.
fn split_lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                lines.push(&text[start..index]);
                index += 1;
                start = index;
            }
            b'\r' => {
                lines.push(&text[start..index]);
                index += if bytes.get(index + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = index;
            }
            _ => index += 1,
        }
    }
    lines.push(&text[start..]);
    lines
}

/// The case-insensitive regex fold for the letters of the patterns.
/// `ı` (U+0131) and `İ` (U+0130) fold to `i`, `ſ` (U+017F) to `s`, and the Kelvin sign to `k`.
fn fold(character: char) -> char {
    match character {
        '\u{0131}' | '\u{0130}' => 'i',
        '\u{017F}' => 's',
        '\u{212A}' => 'k',
        other => other.to_ascii_lowercase(),
    }
}

/// The index after `word` when the characters at `at` fold to it, or `None`.
fn word_end(characters: &[char], at: usize, word: &str) -> Option<usize> {
    let mut index = at;
    for expected in word.chars() {
        if characters
            .get(index)
            .is_some_and(|actual| fold(*actual) == expected)
        {
            index += 1;
        } else {
            return None;
        }
    }
    Some(index)
}

/// The index of the first character at or after `at` that is not pattern whitespace.
fn whitespace_end(characters: &[char], at: usize) -> usize {
    let mut index = at;
    while characters
        .get(index)
        .is_some_and(|character| is_pattern_whitespace(*character))
    {
        index += 1;
    }
    index
}

/// An optional `s` (the `s?` of the patterns), folded.
fn optional_s(characters: &[char], at: usize) -> usize {
    if characters
        .get(at)
        .is_some_and(|character| fold(*character) == 's')
    {
        at + 1
    } else {
        at
    }
}

/// The characters from `start` up to, not including, the first character in `stops` or the end.
fn take_until(characters: &[char], start: usize, stops: &[char]) -> String {
    characters[start..]
        .iter()
        .take_while(|character| !stops.contains(*character))
        .collect()
}

/// The group of `(?i)(?:required[WS]+)?(?:technical[WS]+)?skills?[WS]*[:-][WS]*([^\n.]+)`, searched
/// leftmost. The optional prefixes are taken when they match, which is the only path that can
/// succeed; the trailing whitespace is given back one character at a time until the group starts at
/// a character that is neither a line feed nor a full stop.
fn skills_group(characters: &[char]) -> Option<String> {
    (0..characters.len()).find_map(|start| skills_match_at(characters, start))
}

fn skills_match_at(characters: &[char], start: usize) -> Option<String> {
    let mut at = start;
    for prefix in ["required", "technical"] {
        if let Some(end) = word_end(characters, at, prefix) {
            let after = whitespace_end(characters, end);
            if after > end {
                at = after;
            }
        }
    }
    let skill_end = word_end(characters, at, "skill")?;
    let separator = whitespace_end(characters, optional_s(characters, skill_end));
    if !matches!(characters.get(separator), Some(':' | '-')) {
        return None;
    }
    let first = separator + 1;
    let last = whitespace_end(characters, first);
    (first..=last)
        .rev()
        .find_map(|position| match characters.get(position) {
            Some(character) if *character != '\n' && *character != '.' => {
                Some(take_until(characters, position, &['\n', '.']))
            }
            _ => None,
        })
}

/// The group of `(?im)^(?:experience|requirements?|qualifications?)[WS]*[:-][WS]*(.+)$`, searched
/// leftmost, where `^` matches at the start of the text and after each line feed.
fn experience_group(characters: &[char]) -> Option<String> {
    (0..=characters.len())
        .filter(|start| *start == 0 || characters[start - 1] == '\n')
        .find_map(|start| experience_match_at(characters, start))
}

fn experience_match_at(characters: &[char], start: usize) -> Option<String> {
    let keyword_end = match word_end(characters, start, "experience") {
        Some(end) => end,
        None => {
            let stem = word_end(characters, start, "requirement")
                .or_else(|| word_end(characters, start, "qualification"))?;
            optional_s(characters, stem)
        }
    };
    let separator = whitespace_end(characters, keyword_end);
    if !matches!(characters.get(separator), Some(':' | '-')) {
        return None;
    }
    let first = separator + 1;
    let last = whitespace_end(characters, first);
    (first..=last)
        .rev()
        .find_map(|position| match characters.get(position) {
            Some(character) if *character != '\n' => {
                Some(take_until(characters, position, &['\n']))
            }
            _ => None,
        })
}

fn sha256_hex(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
