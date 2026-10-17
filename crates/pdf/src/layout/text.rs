//! Row preparation: sanitizing, line wrapping and row classification.
//!
//! Sanitized text is one code point per row character, so every width here counts `char`s.

use jobniac_text::java::{java_is_blank, java_is_whitespace, java_strip};
use jobniac_text::str_rules::is_text_alpha;

use super::fonts::text_width;
use super::{MARGIN, PAGE_WIDTH};

/// Wrap width of body text in characters.
pub const BODY_WRAP_CHARACTERS: usize = 104;
/// Body text size in points.
pub const BODY_SIZE: f64 = 9.5;
/// Title size in points when it fits on one row.
pub const TITLE_SIZE: f64 = 20.0;
/// Smallest title size: a title that does not fit even at this size wraps onto several rows.
pub const TITLE_MIN_SIZE: f64 = 10.0;
/// Left edge of wrapped continuation rows and bullet text.
pub const CONTINUATION_X: f64 = 78.0;
/// Right edge of every row: the page width less the margin.
const RIGHT_EDGE: f64 = PAGE_WIDTH - MARGIN;

/// Removes leading Java whitespace.
pub fn strip_leading(text: &str) -> &str {
    text.trim_start_matches(java_is_whitespace)
}

/// Removes trailing Java whitespace.
pub fn strip_trailing(text: &str) -> &str {
    text.trim_end_matches(java_is_whitespace)
}

/// Maps text to the WinAnsi-safe row alphabet.
///
/// Tab becomes a space, code points 32 to 255 are kept, other Java whitespace becomes a space,
/// and every other code point becomes `?` (or `??` above U+FFFF, as the Java char-unit rule did).
pub fn sanitize_pdf_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for character in text.chars() {
        let unit = u32::from(character);
        if character == '\t' {
            result.push(' ');
        } else if (32..=255).contains(&unit) {
            result.push(character);
        } else if java_is_whitespace(character) {
            result.push(' ');
        } else if unit > 0xFFFF {
            result.push_str("??");
        } else {
            result.push('?');
        }
    }
    result
}

/// Short uppercase text without a colon, containing a letter.
pub fn is_section_heading(line: &str) -> bool {
    if java_is_blank(line) || line.chars().count() > 48 || line.contains(':') {
        return false;
    }
    let stripped = java_strip(line);
    stripped.chars().any(is_text_alpha) && stripped == stripped.to_uppercase()
}

/// The first non-whitespace character is `-`.
pub fn is_bullet_line(line: &str) -> bool {
    strip_leading(line).starts_with('-')
}

/// Two leading spaces and some other text.
pub fn is_bullet_continuation_line(line: &str) -> bool {
    line.starts_with("  ") && !java_is_blank(line)
}

/// A colon at character index 1 to 32.
pub fn is_labeled_line(line: &str) -> bool {
    if java_is_blank(line) {
        return false;
    }
    match line.chars().position(|character| character == ':') {
        Some(index) => (1..=32).contains(&index),
        None => false,
    }
}

/// Removes the bullet marker and the spaces after it.
pub fn strip_bullet_prefix(line: &str) -> &str {
    let stripped = strip_leading(line);
    match stripped.strip_prefix('-') {
        Some(rest) => strip_leading(rest),
        None => stripped,
    }
}

/// Greedy wrapping at ASCII spaces, hard-breaking long words.
///
/// Linear in the text length: a start index advances over the characters instead of copying the
/// remainder for every row.
///
/// # Panics
///
/// Panics when `max_characters` is 0.
pub fn wrap_text(text: &str, max_characters: usize) -> Vec<String> {
    assert!(
        max_characters >= 1,
        "wrap_text requires max_characters >= 1"
    );
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0;
    let mut lines = Vec::new();
    while chars.len() - start > max_characters {
        // The last space at an index no greater than `max`.
        let found = chars[start..=start + max_characters]
            .iter()
            .rposition(|character| *character == ' ');
        let break_index = match found {
            Some(index) if index > 0 => index,
            _ => max_characters,
        };
        lines.push(chars_to_string(strip_trailing_chars(
            &chars[start..start + break_index],
        )));
        let rest = &chars[start + break_index..];
        start = chars.len() - strip_leading_chars(rest).len();
    }
    lines.push(chars_to_string(&chars[start..]));
    lines
}

/// Width of a row in points; text the font cannot draw counts as zero (the renderer reports it).
fn row_width(text: &str, bold: bool, size: f64) -> f64 {
    text_width(text, bold, size, false).unwrap_or(0.0)
}

fn char_width(character: char, bold: bool, size: f64) -> f64 {
    row_width(character.encode_utf8(&mut [0; 4]), bold, size)
}

/// Splits one row so every piece fits its width budget (`first` for the first piece, `rest` for
/// the others): at the last space that fits, else inside the word. Every piece holds at least one
/// character, so the walk always ends.
fn split_to_width(row: &str, first: f64, rest: f64, bold: bool, size: f64) -> Vec<String> {
    let chars: Vec<char> = row.chars().collect();
    let mut pieces = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let budget = if pieces.is_empty() { first } else { rest };
        let mut end = start;
        let mut width = 0.0;
        while end < chars.len() {
            let next = char_width(chars[end], bold, size);
            if width + next > budget && end > start {
                break;
            }
            width += next;
            end += 1;
        }
        if end == chars.len() {
            pieces.push(chars_to_string(&chars[start..]));
            break;
        }
        // Like `wrap_text`, a space right after the last fitting character may serve as the break.
        let break_index = match chars[start + 1..=end]
            .iter()
            .rposition(|character| *character == ' ')
        {
            Some(offset) => start + 1 + offset,
            None => end,
        };
        pieces.push(chars_to_string(strip_trailing_chars(
            &chars[start..break_index],
        )));
        start = chars.len() - strip_leading_chars(&chars[break_index..]).len();
    }
    pieces
}

/// Wraps a paragraph by character count, then, only when some row would run past its
/// width budget, wraps the whole paragraph again by measured width (`first` for the first row,
/// `rest` for the others). Paragraphs whose rows all fit keep their character-count row breaks; re-wrapping
/// the whole paragraph (not just the wide rows) avoids ragged one-word tail rows.
fn wrap_fitted(
    text: &str,
    max_characters: usize,
    first: f64,
    rest: f64,
    bold: bool,
    size: f64,
) -> Vec<String> {
    let rows = wrap_text(text, max_characters);
    let fits = rows
        .iter()
        .enumerate()
        .all(|(index, row)| row_width(row, bold, size) <= if index == 0 { first } else { rest });
    if fits {
        rows
    } else {
        split_to_width(text, first, rest, bold, size)
    }
}

/// Width budget in points of body rows drawn from the margin.
const BODY_WIDTH: f64 = RIGHT_EDGE - MARGIN;
/// Width budget in points of rows drawn from [`CONTINUATION_X`].
const INDENTED_WIDTH: f64 = RIGHT_EDGE - CONTINUATION_X;

/// Rows of the title and the size it is drawn at.
///
/// A title that fits in one row at 20 pt keeps it. A longer one shrinks to fit one row down to
/// 10 pt, and beyond that wraps by measured width at 10 pt, so it never
/// runs off the page.
pub fn title_rows(title: &str) -> (Vec<String>, f64) {
    let full = row_width(title, true, TITLE_SIZE);
    if full <= BODY_WIDTH {
        return (vec![title.to_owned()], TITLE_SIZE);
    }
    let shrunk = ((TITLE_SIZE * BODY_WIDTH / full) * 10.0).floor() / 10.0;
    if shrunk >= TITLE_MIN_SIZE {
        return (vec![title.to_owned()], shrunk);
    }
    let rows = split_to_width(title, BODY_WIDTH, BODY_WIDTH, true, TITLE_MIN_SIZE);
    (rows, TITLE_MIN_SIZE)
}

fn strip_leading_chars(chars: &[char]) -> &[char] {
    let start = chars
        .iter()
        .position(|character| !java_is_whitespace(*character))
        .unwrap_or(chars.len());
    &chars[start..]
}

fn strip_trailing_chars(chars: &[char]) -> &[char] {
    let end = chars
        .iter()
        .rposition(|character| !java_is_whitespace(*character))
        .map_or(0, |index| index + 1);
    &chars[..end]
}

fn chars_to_string(chars: &[char]) -> String {
    chars.iter().collect()
}

/// The first row carries `- `, continuation rows carry two spaces.
fn wrap_bullet(text: &str) -> Vec<String> {
    let payload = wrap_fitted(
        strip_bullet_prefix(text),
        BODY_WRAP_CHARACTERS.saturating_sub(4).max(1),
        INDENTED_WIDTH,
        INDENTED_WIDTH,
        false,
        BODY_SIZE,
    );
    payload
        .into_iter()
        .enumerate()
        .map(|(index, row)| format!("{}{row}", if index == 0 { "- " } else { "  " }))
        .collect()
}

/// The label stays on the first row, values wrap under it.
fn wrap_label(text: &str) -> Vec<String> {
    let colon = text.find(':').unwrap_or(0);
    let label = &text[..=colon];
    let value = strip_leading(&text[colon + 1..]);
    let label_width = label.chars().count();
    let width = BODY_WRAP_CHARACTERS
        .saturating_sub(label_width)
        .saturating_sub(1)
        .max(1);
    // The renderer draws the value after the bold label, and continuation rows from 78 pt.
    let value_x = MARGIN + row_width(&format!("{label} "), true, BODY_SIZE);
    let payload = wrap_fitted(
        value,
        width,
        (RIGHT_EDGE - value_x).max(0.0),
        INDENTED_WIDTH,
        false,
        BODY_SIZE,
    );
    payload
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            if index == 0 {
                format!("{label} {row}")
            } else {
                format!("  {row}")
            }
        })
        .collect()
}

/// Splits a body at the paragraph breaks: CRLF, LF, VT, FF, CR, NEL, LS and PS.
pub fn split_paragraphs(body: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut characters = body.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if !is_paragraph_break(character) {
            continue;
        }
        parts.push(&body[start..index]);
        let mut end = index + character.len_utf8();
        if character == '\r'
            && let Some(&(_, '\n')) = characters.peek()
        {
            characters.next();
            end += 1;
        }
        start = end;
    }
    parts.push(&body[start..]);
    parts
}

fn is_paragraph_break(character: char) -> bool {
    matches!(
        character,
        '\n' | '\u{0B}' | '\u{0C}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Prepares the title row and the validated body.
///
/// The title is expected to be sanitized already. Blank paragraphs become empty rows. Returns the
/// rows, the number of title rows at their start and the title size. Body rows are wrapped by
/// the character count first, then re-wrapped by measured width where they would run past
/// the right margin.
pub fn body_lines(title: &str, body: &str) -> (Vec<String>, usize, f64) {
    let (mut lines, title_size) = title_rows(title);
    let title_row_count = lines.len();
    lines.push(String::new());
    for paragraph in split_paragraphs(body) {
        if java_is_blank(paragraph) {
            lines.push(String::new());
            continue;
        }
        let sanitized = sanitize_pdf_text(java_strip(paragraph));
        if is_bullet_line(&sanitized) {
            lines.extend(wrap_bullet(&sanitized));
        } else if is_labeled_line(&sanitized) {
            lines.extend(wrap_label(&sanitized));
        } else {
            lines.extend(wrap_fitted(
                &sanitized,
                BODY_WRAP_CHARACTERS,
                BODY_WIDTH,
                BODY_WIDTH,
                false,
                BODY_SIZE,
            ));
        }
    }
    (lines, title_row_count, title_size)
}

/// A fixed date prefix or a `MM/YYYY - ` or `YYYY-MM - ` range.
pub fn is_resume_date_line(line: &str) -> bool {
    const PREFIXES: [&str; 5] = [
        "Dates not provided",
        "Dates non précisées",
        "Unknown - ",
        "Inconnu - ",
        "unbekannt - ",
    ];
    PREFIXES.iter().any(|prefix| line.starts_with(prefix)) || is_date_range(line)
}

/// True for a `MM/YYYY - ` or `YYYY-MM - ` prefix followed by a non-empty single-line rest.
fn is_date_range(line: &str) -> bool {
    let Some(head) = line.as_bytes().get(..7) else {
        return false;
    };
    let digits = |bytes: &[u8]| bytes.iter().all(u8::is_ascii_digit);
    let month_first = digits(&head[0..2]) && head[2] == b'/' && digits(&head[3..7]);
    let year_first = digits(&head[0..4]) && head[4] == b'-' && digits(&head[5..7]);
    if !(month_first || year_first) {
        return false;
    }
    match line[7..].strip_prefix(" - ") {
        Some(rest) => {
            !rest.is_empty() && !rest.contains(['\n', '\r', '\u{85}', '\u{2028}', '\u{2029}'])
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_at_last_space_within_width() {
        assert_eq!(wrap_text("aaa bbb ccc", 7), vec!["aaa bbb", "ccc"]);
    }

    #[test]
    fn wrap_hard_breaks_when_no_space_fits() {
        assert_eq!(wrap_text("abcdefgh", 3), vec!["abc", "def", "gh"]);
    }

    #[test]
    #[should_panic(expected = "max_characters >= 1")]
    fn wrap_rejects_zero_width() {
        let _ = wrap_text("abc", 0);
    }

    /// The previous copy-the-remainder implementation, kept as the reference for equivalence.
    fn wrap_text_reference(text: &str, max: usize) -> Vec<String> {
        let mut remaining: Vec<char> = text.chars().collect();
        let mut lines = Vec::new();
        while remaining.len() > max {
            let found = remaining[..=max].iter().rposition(|c| *c == ' ');
            let break_index = match found {
                Some(index) if index > 0 => index,
                _ => max,
            };
            lines.push(chars_to_string(strip_trailing_chars(
                &remaining[..break_index],
            )));
            remaining = strip_leading_chars(&remaining[break_index..]).to_vec();
        }
        lines.push(chars_to_string(&remaining));
        lines
    }

    #[test]
    fn wrap_matches_the_reference_on_long_and_awkward_text() {
        let long = "word ".repeat(10_000);
        let mixed = format!("  lead {} tail  x{}  y", "z".repeat(250), " ".repeat(30));
        for text in [long.as_str(), mixed.as_str(), "", "  ", "a b", " abc"] {
            for max in [1, 2, 7, 96, 104] {
                assert_eq!(wrap_text(text, max), wrap_text_reference(text, max));
            }
        }
    }

    #[test]
    fn date_range_requires_exact_shape() {
        assert!(is_date_range("01/2020 - 02/2021"));
        assert!(is_date_range("2020-01 - Present"));
        assert!(!is_date_range("2020-01 -"));
        assert!(!is_date_range("2020-01 - "));
    }
}
