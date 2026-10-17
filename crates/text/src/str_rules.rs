//! Unicode text rules: whitespace, line boundaries and character classes.
//!
//! The character classes follow Unicode 16.0.0. The general categories come from
//! `unicode-general-category` 1.1.0, which is generated from the same version. Golden vectors
//! over every code point (U+0000 to U+10FFFF except surrogates) pin each predicate, so a Unicode
//! version change shows up in a test.
//!
//! Whitespace is the set of characters with the bidirectional class WS, B or S, plus the
//! general categories Zs, Zl and Zp. It includes the information separators U+001C to U+001F,
//! which `char::is_whitespace` rejects. Rust's `str::trim`, `char::is_whitespace` (the
//! `White_Space` property), `str::lines`, `char::is_alphabetic` (the `Alphabetic` property) and
//! `char::is_alphanumeric` are different rules: `Alphabetic` also holds for combining marks
//! and letter numbers, for example. So the rules are written out here, in terms of general
//! categories.
//!
//! A digit (the Unicode Digit property) cannot be derived from the general category (U+00B2 is
//! a digit but not a decimal), so its ranges are a generated table (`digits.rs`). ASCII-only
//! checking is `str::is_ascii`, which is true for the empty string.

use std::cmp::Ordering;

use unicode_general_category::{GeneralCategory, get_general_category};

mod digits;

/// True for Unicode whitespace.
///
/// This is also the set that [`strip_text`] removes. It includes U+001C to
/// U+001F and U+0085, and every Zs, Zl and Zp character, including U+00A0, U+2007 and U+202F.
pub fn is_text_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{09}'..='\u{0D}'
            | '\u{1C}'..='\u{20}'
            | '\u{85}'
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
    )
}

/// Removes leading and trailing whitespace characters (see [`is_text_whitespace`]).
pub fn strip_text(text: &str) -> &str {
    text.trim_matches(is_text_whitespace)
}

/// True for the characters that end a line in [`split_lines`].
fn is_line_boundary(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{1C}'
            ..='\u{1E}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Splits text into lines without keeping the terminators.
///
/// The boundaries are CR, LF, CRLF, VT (U+000B), FF (U+000C), U+001C to U+001E, U+0085, U+2028
/// and U+2029. A terminator at the end adds no empty element, and empty input gives no lines.
/// U+001F is not a boundary, unlike Java and `str::lines`.
pub fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut characters = text.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if !is_line_boundary(character) {
            continue;
        }
        lines.push(&text[start..index]);
        let mut next = index + character.len_utf8();
        if character == '\r'
            && let Some(&(lf_index, '\n')) = characters.peek()
        {
            characters.next();
            next = lf_index + 1;
        }
        start = next;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// The Unicode general category of `character`.
fn category(character: char) -> GeneralCategory {
    get_general_category(character)
}

/// True for printable characters.
///
/// Non-printable are the categories Cc, Cf, Cs, Co and Cn, and Zl, Zp and Zs except U+0020.
/// Everything else is printable.
pub fn is_text_printable(character: char) -> bool {
    if character == ' ' {
        return true;
    }
    !matches!(
        category(character),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::Surrogate
            | GeneralCategory::PrivateUse
            | GeneralCategory::Unassigned
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
            | GeneralCategory::SpaceSeparator
    )
}

/// True for alphabetic characters: categories Lu, Ll, Lt, Lm and Lo.
pub fn is_text_alpha(character: char) -> bool {
    matches!(
        category(character),
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
    )
}

/// True for decimal digits: category Nd.
pub fn is_text_decimal(character: char) -> bool {
    category(character) == GeneralCategory::DecimalNumber
}

/// True for digits (the Unicode Digit property).
pub fn is_text_digit(character: char) -> bool {
    let code_point = u32::from(character);
    digits::DIGIT_RANGES
        .binary_search_by(|&(start, end)| {
            if end < code_point {
                Ordering::Less
            } else if start > code_point {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

/// True for alphanumeric characters: letters, decimals, digits and numerics.
///
/// Over Unicode 16.0.0, the numeric property holds exactly for the categories Nd, Nl and No,
/// and for the letters. So the rule is: a letter, or a number of category Nd, Nl or No.
pub fn is_text_alnum(character: char) -> bool {
    is_text_alpha(character)
        || matches!(
            category(character),
            GeneralCategory::DecimalNumber
                | GeneralCategory::LetterNumber
                | GeneralCategory::OtherNumber
        )
}

/// True when every character of `text` is printable. The empty string is printable.
pub fn str_is_printable(text: &str) -> bool {
    text.chars().all(is_text_printable)
}

/// True when `text` is non-empty and every character is alphabetic.
pub fn str_is_alpha(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_text_alpha)
}

/// True when `text` is non-empty and every character is alphanumeric.
pub fn str_is_alnum(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_text_alnum)
}

/// True when `text` is non-empty and every character is a decimal digit.
pub fn str_is_decimal(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_text_decimal)
}

/// True when `text` is non-empty and every character is whitespace.
pub fn str_is_space(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_text_whitespace)
}

/// True when `text` is non-empty and every character is a digit.
///
/// This accepts U+00B2 and other non-ASCII digits. A caller that needs ASCII digits must check
/// them separately.
pub fn str_is_digit(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_text_digit)
}

/// True when every character of `text` is below U+0080. The empty string is ASCII.
pub fn str_is_ascii(text: &str) -> bool {
    text.is_ascii()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::cmp::Ordering;

    type StrPredicate = fn(&str) -> bool;

    const CHARACTER_VECTORS: &str = include_str!("../tests/golden/str-rules-char-classes.json");
    const STRING_VECTORS: &str = include_str!("../tests/golden/str-rules-str.json");

    /// A copy of the private whitespace predicate in `jobniac-core` (`errors.rs` and
    /// `clock.rs`), typed out here because core cannot depend on this crate yet. The test below
    /// checks this copy against the golden vectors, so it cannot drift from the whitespace rule
    /// unnoticed. The core functions themselves are not reachable from this crate.
    fn core_copy_is_whitespace(c: char) -> bool {
        matches!(
            c,
            '\u{09}'..='\u{0D}'
                | '\u{1C}'..='\u{20}'
                | '\u{85}'
                | '\u{A0}'
                | '\u{1680}'
                | '\u{2000}'..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
        )
    }

    fn parse(source: &str) -> Value {
        serde_json::from_str(source).expect("golden JSON parses")
    }

    fn string_field<'a>(value: &'a Value, key: &str) -> &'a str {
        value[key].as_str().expect("string field")
    }

    fn parse_ranges(items: &[Value]) -> Vec<(u32, u32)> {
        items
            .iter()
            .map(|item| {
                let text = item.as_str().expect("range string");
                let (start, end) = text.split_once('-').unwrap_or((text, text));
                (
                    u32::from_str_radix(start, 16).expect("range start"),
                    u32::from_str_radix(end, 16).expect("range end"),
                )
            })
            .collect()
    }

    fn in_ranges(ranges: &[(u32, u32)], code_point: u32) -> bool {
        ranges
            .binary_search_by(|&(start, end)| {
                if end < code_point {
                    Ordering::Less
                } else if start > code_point {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            })
            .is_ok()
    }

    /// Every scalar value (U+0000 to U+10FFFF except surrogates), with its code point.
    fn scalar_values() -> impl Iterator<Item = (u32, char)> {
        (0..=0x10FFFF_u32).filter_map(|code_point| {
            char::from_u32(code_point).map(|character| (code_point, character))
        })
    }

    /// Compares one predicate with its golden ranges over every scalar value.
    fn assert_predicate(root: &Value, name: &str, predicate: fn(char) -> bool) {
        let ranges = parse_ranges(
            root["predicates"][name]["ranges"]
                .as_array()
                .expect("ranges"),
        );
        let mismatches: Vec<u32> = scalar_values()
            .filter(|&(code_point, character)| {
                predicate(character) != in_ranges(&ranges, code_point)
            })
            .map(|(code_point, _)| code_point)
            .take(32)
            .collect();
        assert!(mismatches.is_empty(), "{name} differs at {mismatches:X?}");
    }

    #[test]
    fn character_predicates_match_golden_for_every_code_point() {
        let root = parse(CHARACTER_VECTORS);
        assert_predicate(&root, "isspace", is_text_whitespace);
        assert_predicate(&root, "isprintable", is_text_printable);
        assert_predicate(&root, "isalpha", is_text_alpha);
        assert_predicate(&root, "isalnum", is_text_alnum);
        assert_predicate(&root, "isdecimal", is_text_decimal);
        assert_predicate(&root, "isdigit", is_text_digit);
    }

    #[test]
    fn digit_table_is_sorted_and_disjoint() {
        let mut previous_end: Option<u32> = None;
        for &(start, end) in digits::DIGIT_RANGES {
            assert!(start <= end, "range {start:X}-{end:X}");
            if let Some(previous) = previous_end {
                assert!(previous < start, "ranges overlap at {start:X}");
            }
            previous_end = Some(end);
        }
    }

    #[test]
    fn every_decimal_is_a_digit() {
        for (code_point, character) in scalar_values() {
            if is_text_decimal(character) {
                assert!(is_text_digit(character), "U+{code_point:04X}");
            }
        }
        assert!(is_text_digit('\u{B2}'), "superscript two is a digit");
        assert!(!is_text_decimal('\u{B2}'), "superscript two is not decimal");
    }

    #[test]
    fn numeric_property_is_the_number_categories_plus_letters() {
        // The vectors record isnumeric (Numeric_Type). Among the characters that are not
        // alphabetic, isnumeric holds exactly for the categories Nd, Nl and No. The letters that
        // carry a numeric value are alphabetic already, so is_text_alnum needs no table for them.
        let root = parse(CHARACTER_VECTORS);
        let numeric = parse_ranges(
            root["predicates"]["isnumeric"]["ranges"]
                .as_array()
                .expect("ranges"),
        );
        for (code_point, character) in scalar_values() {
            if is_text_alpha(character) {
                continue;
            }
            let number_category = matches!(
                get_general_category(character),
                GeneralCategory::DecimalNumber
                    | GeneralCategory::LetterNumber
                    | GeneralCategory::OtherNumber
            );
            assert_eq!(
                number_category,
                in_ranges(&numeric, code_point),
                "U+{code_point:04X}"
            );
        }
    }

    #[test]
    fn core_whitespace_copy_matches_the_golden_vectors_and_this_module() {
        let root = parse(CHARACTER_VECTORS);
        let ranges = parse_ranges(
            root["predicates"]["isspace"]["ranges"]
                .as_array()
                .expect("ranges"),
        );
        for (code_point, character) in scalar_values() {
            let expected = in_ranges(&ranges, code_point);
            assert_eq!(
                core_copy_is_whitespace(character),
                expected,
                "core copy, U+{code_point:04X}"
            );
            assert_eq!(
                is_text_whitespace(character),
                core_copy_is_whitespace(character),
                "U+{code_point:04X}"
            );
        }
    }

    #[test]
    fn str_level_rules_match_golden() {
        let root = parse(STRING_VECTORS);
        let vectors = &root["vectors"];
        for case in vectors["strip"].as_array().expect("strip") {
            let input = string_field(case, "input");
            assert_eq!(strip_text(input), string_field(case, "output"), "{input:?}");
        }
        for case in vectors["splitlines"].as_array().expect("splitlines") {
            let input = string_field(case, "input");
            let expected: Vec<&str> = case["lines"]
                .as_array()
                .expect("lines")
                .iter()
                .map(|line| line.as_str().expect("line"))
                .collect();
            assert_eq!(split_lines(input), expected, "{input:?}");
        }
        let predicates: [(&str, StrPredicate); 7] = [
            ("isprintable", str_is_printable),
            ("isalpha", str_is_alpha),
            ("isalnum", str_is_alnum),
            ("isdecimal", str_is_decimal),
            ("isspace", str_is_space),
            ("isdigit", str_is_digit),
            ("isascii", str_is_ascii),
        ];
        for (name, predicate) in predicates {
            for case in vectors[name].as_array().expect(name) {
                let input = string_field(case, "input");
                assert_eq!(
                    predicate(input),
                    case["result"].as_bool().expect("bool"),
                    "{name} {input:?}"
                );
            }
        }
    }

    #[test]
    fn splitlines_boundaries_and_crlf() {
        assert_eq!(split_lines(""), Vec::<&str>::new());
        assert_eq!(split_lines("\r\n"), vec![""]);
        assert_eq!(
            split_lines("a\rb\u{2028}c\u{1F}d"),
            vec!["a", "b", "c\u{1F}d"]
        );
        assert_eq!(split_lines("a\n\nb\n"), vec!["a", "", "b"]);
    }

    #[test]
    fn strip_uses_unicode_whitespace_not_rust_trim() {
        assert_eq!(strip_text("\u{1C}x\u{1F}"), "x");
        assert_eq!(strip_text("\u{A0}x\u{A0}"), "x");
        assert_eq!(strip_text("\u{200B}x"), "\u{200B}x");
    }
}
