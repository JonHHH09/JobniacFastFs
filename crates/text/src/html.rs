//! HTML preparation for fetched job pages.
//!
//! The functions decode HTML 4 character references once, remove `script` and `style` elements and
//! tags, replace control characters, no-break spaces and U+FFFD with spaces, and cap the result.
//! Decoding comes before tag removal, so an encoded tag such as `&lt;b&gt;` is removed like a real
//! one. Named references are case-sensitive and need their semicolon. A numeric reference must be
//! at most 9 UTF-16 units long between `&` and `;`; a reference that decodes to a surrogate yields
//! U+FFFD. Caps count code points.
//!
//! The entity table is generated from the W3C HTML 4.01 entity sets and the digit table from the
//! Unicode decimal digits, both by `tools/text/generate_html_tables.py`. The golden vectors live in
//! `tests/golden/html.json`.

mod digits;
mod entities;

pub use entities::HTML4_ENTITIES;

use crate::java::java_strip;
use digits::BMP_DIGITS;

/// Replacement for a numeric reference that decodes to a surrogate.
const REPLACEMENT_CHARACTER: char = '\u{FFFD}';

/// Largest Unicode scalar value.
const MAX_CODE_POINT: u32 = 0x10FFFF;

/// Most UTF-16 units a reference may span from `&` through the character before `;`.
const REFERENCE_SPAN_UNITS: usize = 10;

/// Cap of the plain description text, in code points.
const PLAIN_CAP: usize = 1000;

/// Cap of each entry of a bounded list, in code points.
const LIST_ENTRY_CAP: usize = 128;

/// Most non-blank entries a bounded list keeps.
const LIST_CAP: usize = 20;

/// Decodes HTML 4 character references once.
///
/// Named references are looked up in [`HTML4_ENTITIES`] and need the trailing semicolon. Numeric
/// references are decimal, or hexadecimal with `x` or `X`, with an optional sign. A reference that is
/// malformed, out of range or longer than the span limit stays literal. No recursive decoding happens.
/// `None` is treated as empty text.
pub fn html4_unescape(value: Option<&str>) -> String {
    unescape(value.unwrap_or(""))
}

/// Applies the source preparation for plain description text.
///
/// The steps are: decode references, remove `script` and `style` elements and tags, replace the
/// remaining `<` and `>`, the C0 and C1 controls, no-break spaces and U+FFFD with spaces, collapse
/// ASCII whitespace, strip Java whitespace and cap the result at 1,000 code points. `None` is treated
/// as empty text.
pub fn plain(value: Option<&str>) -> String {
    let decoded = unescape(value.unwrap_or(""));
    let without_tags = remove_tags(&remove_script_and_style(&decoded));
    let cleaned: String = without_tags
        .chars()
        .map(|character| match character {
            '<' | '>' | '\u{A0}' | '\u{FFFD}' => ' ',
            other if is_control(other) => ' ',
            other => other,
        })
        .collect();
    bound_normalized(&collapse_ascii_whitespace(&cleaned), PLAIN_CAP)
}

/// Normalizes control characters and ASCII whitespace and caps the text at `cap` code points,
/// without decoding references. `None` is treated as empty text.
pub fn bound(value: Option<&str>, cap: usize) -> String {
    bound_normalized(value.unwrap_or(""), cap)
}

/// Applies [`plain`] and then caps the result at `cap` code points.
pub fn plain_bound(value: Option<&str>, cap: usize) -> String {
    bound_normalized(&plain(value), cap)
}

/// Prepares a list of short texts.
///
/// Each entry is prepared like [`plain`] and capped at 128 code points. Blank entries are dropped,
/// duplicates and order are kept, and at most 20 entries are returned. A `None` list gives an empty
/// list. An entry that a caller holds as `None` is dropped by the caller, which has the same result,
/// because a blank entry is never kept.
pub fn bound_list<S: AsRef<str>>(values: Option<&[S]>) -> Vec<String> {
    values
        .unwrap_or(&[])
        .iter()
        .map(|value| bound_normalized(&plain(Some(value.as_ref())), LIST_ENTRY_CAP))
        .filter(|entry| !entry.is_empty())
        .take(LIST_CAP)
        .collect()
}

/// Decodes references in one pass over `value`, leaving malformed ones literal.
fn unescape(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut position = 0;
    // The first `;` after the most recent `&` that was searched. A cached value above the current
    // `&` is the first `;` after it as well, so the search is not repeated for each `&`.
    let mut cached_semicolon: Option<usize> = None;
    while let Some(offset) = value[position..].find('&') {
        let ampersand = position + offset;
        result.push_str(&value[position..ampersand]);
        let semicolon = match cached_semicolon {
            Some(index) if index > ampersand => index,
            _ => match value[ampersand + 1..].find(';') {
                Some(relative) => ampersand + 1 + relative,
                None => {
                    result.push_str(&value[ampersand..]);
                    return result;
                }
            },
        };
        cached_semicolon = Some(semicolon);
        let span_units = value[ampersand..semicolon]
            .encode_utf16()
            .take(REFERENCE_SPAN_UNITS)
            .count();
        let replacement = if span_units < REFERENCE_SPAN_UNITS {
            reference(&value[ampersand + 1..semicolon])
        } else {
            None
        };
        match replacement {
            Some(character) => {
                result.push(character);
                position = semicolon + 1;
            }
            None => {
                result.push('&');
                position = ampersand + 1;
            }
        }
    }
    result.push_str(&value[position..]);
    result
}

/// Resolves the text between `&` and `;`.
fn reference(name: &str) -> Option<char> {
    let Some(numeric) = name.strip_prefix('#') else {
        return HTML4_ENTITIES
            .binary_search_by_key(&name, |&(candidate, _)| candidate)
            .ok()
            .and_then(|index| char::from_u32(HTML4_ENTITIES[index].1));
    };
    let (radix, digits) = match numeric.strip_prefix(['x', 'X']) {
        Some(rest) => (16, rest),
        None => (10, numeric),
    };
    let negative = digits.starts_with('-');
    // One sign character is dropped after the optional `-` check; a second sign is then invalid.
    let digits = digits.strip_prefix(['+', '-']).unwrap_or(digits);
    if digits.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    for character in digits.chars() {
        let digit = digit_value(character)?;
        if digit >= radix {
            return None;
        }
        value = value.checked_mul(radix)?.checked_add(digit)?;
    }
    // Only a negative zero is in range.
    if negative && value != 0 {
        return None;
    }
    if value > MAX_CODE_POINT {
        return None;
    }
    if (0xD800..=0xDFFF).contains(&value) {
        return Some(REPLACEMENT_CHARACTER);
    }
    char::from_u32(value)
}

/// The digit value of a BMP character in the pinned table, or `None` when it is not a digit.
fn digit_value(character: char) -> Option<u32> {
    let code_point = u16::try_from(u32::from(character)).ok()?;
    let index = BMP_DIGITS
        .binary_search_by_key(&code_point, |&(candidate, _)| candidate)
        .ok()?;
    Some(u32::from(BMP_DIGITS[index].1))
}

/// Removes every `script` or `style` element whose closing tag is the exact `</script>` or
/// `</style>` (ASCII case-insensitive). The regex it mirrors is `<(script|style)[^>]*>.*?</\1>`.
fn remove_script_and_style(text: &str) -> String {
    let mut scanner = ElementScanner::new(text.as_bytes());
    let mut result = String::with_capacity(text.len());
    let mut copied = 0;
    let mut position = 0;
    while let Some(offset) = text[position..].find('<') {
        let start = position + offset;
        match scanner.end(start) {
            Some(end) => {
                result.push_str(&text[copied..start]);
                result.push(' ');
                copied = end;
                position = end;
            }
            None => position = start + 1,
        }
    }
    result.push_str(&text[copied..]);
    result
}

/// Finds the element ends of `script` and `style` openers in one left-to-right pass.
///
/// Every query point is at least the previous one, so each search reuses the answer of the
/// previous search instead of rescanning the text. A failed search is remembered as well, which
/// keeps adversarial inputs such as thousands of unclosed openers linear.
struct ElementScanner<'a> {
    bytes: &'a [u8],
    tag_end: ForwardFind,
    script_close: ForwardFind,
    style_close: ForwardFind,
}

impl<'a> ElementScanner<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            tag_end: ForwardFind::default(),
            script_close: ForwardFind::default(),
            style_close: ForwardFind::default(),
        }
    }

    /// The index just after the closing tag of a `script` or `style` element that opens at `start`.
    fn end(&mut self, start: usize) -> Option<usize> {
        let bytes = self.bytes;
        let rest = &bytes[start + 1..];
        let is_script = if rest
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"script"))
        {
            true
        } else if rest
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"style"))
        {
            false
        } else {
            return None;
        };
        let (name, close): (&[u8], &[u8]) = if is_script {
            (b"script", b"</script>")
        } else {
            (b"style", b"</style>")
        };
        let after_name = start + 1 + name.len();
        // The opening tag runs to the first `>`; `[^>]*` cannot pass it.
        let open_end = self
            .tag_end
            .find(after_name, |from| find_byte(bytes, from, b'>'))?;
        let close_finder = if is_script {
            &mut self.script_close
        } else {
            &mut self.style_close
        };
        let found = close_finder.find(open_end + 1, |from| find_ascii_case(bytes, from, close))?;
        Some(found + close.len())
    }
}

/// Memoizes the first match at or after the most recent query point.
///
/// A query at or after the remembered point, and at or before the remembered match (or any query
/// when the search found nothing), has the same answer, so it is returned without searching.
#[derive(Debug, Default)]
struct ForwardFind {
    /// The query point of the last search and its first match at or after that point.
    last: Option<(usize, Option<usize>)>,
}

impl ForwardFind {
    fn find(&mut self, from: usize, search: impl FnOnce(usize) -> Option<usize>) -> Option<usize> {
        let reusable = match self.last {
            Some((known_from, known)) if from >= known_from => match known {
                None => Some(None),
                Some(found) if from <= found => Some(Some(found)),
                Some(_) => None,
            },
            _ => None,
        };
        if let Some(answer) = reusable {
            return answer;
        }
        let found = search(from);
        self.last = Some((from, found));
        found
    }
}

/// The index of the first `byte` at or after `from`.
fn find_byte(bytes: &[u8], from: usize, byte: u8) -> Option<usize> {
    bytes
        .get(from..)?
        .iter()
        .position(|&candidate| candidate == byte)
        .map(|index| from + index)
}

/// The index of the first ASCII case-insensitive occurrence of `needle` at or after `from`.
fn find_ascii_case(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
        .map(|index| from + index)
}

/// Replaces every tag `<[^>]+>` with a space, then the remaining `<` and `>` are
/// handled by the caller. A `<` directly followed by `>` is not a tag.
fn remove_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut copied = 0;
    let mut position = 0;
    while let Some(offset) = text[position..].find('<') {
        let start = position + offset;
        let Some(relative) = text[start + 1..].find('>') else {
            // No `>` follows, so no later `<` can start a tag either.
            break;
        };
        if relative == 0 {
            position = start + 1;
            continue;
        }
        let end = start + 1 + relative;
        result.push_str(&text[copied..start]);
        result.push(' ');
        copied = end + 1;
        position = end + 1;
    }
    result.push_str(&text[copied..]);
    result
}

/// Replaces controls with spaces, collapses ASCII whitespace, strips Java whitespace and caps the
/// text at `cap` code points.
fn bound_normalized(value: &str, cap: usize) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            if is_control(character) {
                ' '
            } else {
                character
            }
        })
        .collect();
    let collapsed = collapse_ascii_whitespace(&cleaned);
    let normalized = java_strip(&collapsed);
    let prefix: String = normalized.chars().take(cap).collect();
    java_strip(&prefix).to_owned()
}

/// True for the C0 and C1 controls, U+0000 to U+001F and U+007F to U+009F.
fn is_control(character: char) -> bool {
    matches!(u32::from(character), 0x00..=0x1F | 0x7F..=0x9F)
}

/// Replaces each run of the six ASCII whitespace characters with one space.
fn collapse_ascii_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_run = false;
    for character in text.chars() {
        if matches!(character, ' ' | '\t' | '\n' | '\r' | '\u{0B}' | '\u{0C}') {
            if !in_run {
                result.push(' ');
                in_run = true;
            }
        } else {
            result.push(character);
            in_run = false;
        }
    }
    result
}
