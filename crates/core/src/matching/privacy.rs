//! Privacy rules for skills, labels and evidence facts (the safe and sensitive patterns).
//!
//! The patterns run in ASCII mode, so word characters are `[A-Za-z0-9_]`, `\d` is `[0-9]`
//! and the case-insensitive keywords fold ASCII letters only. This crate has no regular-expression
//! dependency, so each pattern is written out as a small matcher. The safe patterns are
//! full-match forms; the sensitive patterns are search forms, matched at any position.

use crate::errors::AppError;

/// One element of a keyword pattern: literal text, or the optional separator `[ _-]?`.
#[derive(Debug, Clone, Copy)]
enum Piece {
    Text(&'static str),
    Separator,
}

use Piece::{Separator, Text};

/// Keywords of `_SENSITIVE_SKILL`, each wrapped in `\b...\b`.
const SKILL_KEYWORDS: &[&[Piece]] = &[
    &[Text("localhost")],
    &[Text("prompt")],
    &[
        Text("chain"),
        Separator,
        Text("of"),
        Separator,
        Text("thought"),
    ],
    &[Text("resume")],
    &[Text("contact")],
    &[Text("password")],
    &[Text("secret")],
    &[Text("api"), Separator, Text("key")],
    &[Text("access"), Separator, Text("token")],
    &[Text("bearer")],
];

/// Keywords of `_SENSITIVE_TEXT`, each wrapped in `\b...\b`.
const TEXT_KEYWORDS: &[&[Piece]] = &[
    &[Text("localhost")],
    &[Text("prompt")],
    &[
        Text("chain"),
        Separator,
        Text("of"),
        Separator,
        Text("thought"),
    ],
    &[Text("reasoning trace")],
    &[Text("resume")],
    &[Text("curriculum vitae")],
    &[Text("contact")],
    &[Text("address")],
    &[Text("phone")],
    &[Text("password")],
    &[Text("secret")],
    &[Text("api"), Separator, Text("key")],
    &[Text("access"), Separator, Text("token")],
    &[Text("bearer")],
];

/// Top-level domains of `_SENSITIVE_TEXT`.
const TOP_LEVEL_DOMAINS: [&str; 7] = ["com", "net", "org", "io", "local", "internal", "test"];

/// Whether `value` matches `_SAFE_SKILL` in full: `[A-Za-z0-9][A-Za-z0-9 .+#/_-]{0,79}`.
pub fn is_safe_skill(value: &str) -> bool {
    is_safe_token(value, is_skill_rest)
}

/// Whether `value` matches `_SAFE_LABEL` in full: `[A-Za-z0-9][A-Za-z0-9._ -]{0,79}`.
pub fn is_safe_label(value: &str) -> bool {
    is_safe_token(value, is_label_rest)
}

/// Whether `value` matches `_SENSITIVE_SKILL` anywhere.
pub fn has_sensitive_skill_text(value: &str) -> bool {
    let bytes = value.as_bytes();
    contains_at_symbol_or_url(bytes)
        || private_address_present(bytes)
        || keyword_present(bytes, SKILL_KEYWORDS)
}

/// Whether `value` matches `_SENSITIVE_TEXT` anywhere.
pub fn has_sensitive_text(value: &str) -> bool {
    let bytes = value.as_bytes();
    contains_at_symbol_or_url(bytes)
        || private_address_present(bytes)
        || domain_present(bytes)
        || phone_shape_present(bytes)
        || keyword_present(bytes, TEXT_KEYWORDS)
}

/// Trims every leading and trailing code point up to U+0020.
pub fn trim_control_and_space(value: &str) -> &str {
    value.trim_matches(|character: char| (character as u32) <= 0x20)
}

/// The privacy-safe checks that the scorer and the evidence validator share.
pub(crate) fn require_safe_skill(value: &str) -> Result<(), AppError> {
    if is_safe_skill(value) && !has_sensitive_skill_text(value) {
        Ok(())
    } else {
        Err(AppError::DomainValue(SKILL_MESSAGE.to_owned()))
    }
}

/// The refusal message of an unsafe skill.
pub(crate) const SKILL_MESSAGE: &str = "fact must be a normalized privacy-safe skill";

fn is_safe_token(value: &str, rest: fn(u8) -> bool) -> bool {
    let bytes = value.as_bytes();
    (1..=80).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|byte| rest(*byte))
}

fn is_skill_rest(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'.' | b'+' | b'#' | b'/' | b'_' | b'-')
}

fn is_label_rest(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'.' | b'_' | b'-')
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The `\b` assertion at `index` (ASCII word boundary; `index` may be the end of the text).
fn boundary_at(bytes: &[u8], index: usize) -> bool {
    let before = index > 0 && bytes.get(index - 1).is_some_and(|byte| is_word(*byte));
    let after = bytes.get(index).is_some_and(|byte| is_word(*byte));
    before != after
}

fn contains_at_symbol_or_url(bytes: &[u8]) -> bool {
    bytes.contains(&b'@') || bytes.windows(3).any(|window| window == b"://")
}

/// `\b(?:127|10)\.`, `\b192\.168\.` and `\b172\.(1[6-9]|2[0-9]|3[01])\.`.
fn private_address_present(bytes: &[u8]) -> bool {
    (0..bytes.len()).any(|index| {
        boundary_at(bytes, index)
            && (starts_with(bytes, index, b"127.")
                || starts_with(bytes, index, b"10.")
                || starts_with(bytes, index, b"192.168.")
                || private_172_at(bytes, index))
    })
}

fn private_172_at(bytes: &[u8], index: usize) -> bool {
    if !starts_with(bytes, index, b"172.") {
        return false;
    }
    let first = bytes.get(index + 4).copied();
    let second = bytes.get(index + 5).copied();
    let octet_ok = match (first, second) {
        (Some(b'1'), Some(digit)) => (b'6'..=b'9').contains(&digit),
        (Some(b'2'), Some(digit)) => digit.is_ascii_digit(),
        (Some(b'3'), Some(digit)) => matches!(digit, b'0' | b'1'),
        _ => false,
    };
    octet_ok && bytes.get(index + 6) == Some(&b'.')
}

fn starts_with(bytes: &[u8], index: usize, prefix: &[u8]) -> bool {
    bytes.get(index..index + prefix.len()) == Some(prefix)
}

/// Matches a keyword pattern at `start`, returning the index just after it. The separator is
/// optional and is always followed by a letter, so consuming it when present is the only match.
fn match_pieces(bytes: &[u8], start: usize, pieces: &[Piece]) -> Option<usize> {
    let mut position = start;
    for piece in pieces {
        match piece {
            Text(text) => {
                let end = position + text.len();
                if bytes
                    .get(position..end)?
                    .eq_ignore_ascii_case(text.as_bytes())
                {
                    position = end;
                } else {
                    return None;
                }
            }
            Separator => {
                if bytes
                    .get(position)
                    .is_some_and(|byte| matches!(byte, b' ' | b'_' | b'-'))
                {
                    position += 1;
                }
            }
        }
    }
    Some(position)
}

/// `\b(alt1|alt2|...)\b` over the keyword patterns, ASCII case-insensitive.
fn keyword_present(bytes: &[u8], keywords: &[&[Piece]]) -> bool {
    keywords.iter().any(|pieces| {
        (0..bytes.len()).any(|start| {
            boundary_at(bytes, start)
                && match_pieces(bytes, start, pieces).is_some_and(|end| boundary_at(bytes, end))
        })
    })
}

/// `\b(?:[a-z0-9-]+\.)+(com|net|org|io|local|internal|test)\b`, ASCII case-insensitive.
fn domain_present(bytes: &[u8]) -> bool {
    (0..bytes.len()).any(|start| boundary_at(bytes, start) && domain_from(bytes, start))
}

fn domain_from(bytes: &[u8], start: usize) -> bool {
    let mut position = start;
    loop {
        // A label is a maximal run of label bytes: the next byte after it must be the dot.
        let end = run_end(bytes, position, |byte| {
            byte.is_ascii_alphanumeric() || byte == b'-'
        });
        if end == position || bytes.get(end) != Some(&b'.') {
            return false;
        }
        position = end + 1;
        if top_level_domain_at(bytes, position) {
            return true;
        }
    }
}

fn top_level_domain_at(bytes: &[u8], position: usize) -> bool {
    TOP_LEVEL_DOMAINS.iter().any(|domain| {
        let end = position + domain.len();
        bytes
            .get(position..end)
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(domain.as_bytes()))
            && boundary_at(bytes, end)
    })
}

/// `\+?\d[\d ()-]{7,}\d`: a digit, at least seven phone bytes, and a digit at least eight bytes on.
fn phone_shape_present(bytes: &[u8]) -> bool {
    (0..bytes.len()).any(|index| {
        bytes[index].is_ascii_digit() && {
            let end = run_end(bytes, index + 1, |byte| {
                byte.is_ascii_digit() || matches!(byte, b' ' | b'(' | b')' | b'-')
            });
            (index + 8..end).any(|position| bytes[position].is_ascii_digit())
        }
    })
}

/// The first index at or after `start` whose byte fails `in_class` (or the end of the text).
fn run_end(bytes: &[u8], start: usize, in_class: impl Fn(u8) -> bool) -> usize {
    let mut end = start;
    while bytes.get(end).is_some_and(|byte| in_class(*byte)) {
        end += 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_skill_follows_the_fullmatch_pattern() {
        assert!(is_safe_skill("c++"));
        assert!(is_safe_skill("node.js"));
        assert!(is_safe_skill(&"a".repeat(80)));
        assert!(!is_safe_skill(&"a".repeat(81)));
        assert!(!is_safe_skill(""));
        assert!(!is_safe_skill(" java"));
        assert!(!is_safe_skill("\u{a0}java"));
        assert!(!is_safe_skill("é"));
    }

    #[test]
    fn sensitive_skill_text_matches_the_search_pattern() {
        for value in [
            "email@example.test",
            "http://example.invalid",
            "localhost",
            "127.0.0.1",
            "10.2.3.4",
            "192.168.2.1",
            "172.16.0.1",
            "prompt",
            "secret",
            "API key",
            "access-token",
            "bearer",
            "contact",
            "resume",
        ] {
            assert!(has_sensitive_skill_text(value), "{value}");
        }
        for value in [
            "java",
            "postgresql",
            "172.15.0.1",
            "110.0.0.1",
            "the reasoning",
            "keyring",
        ] {
            assert!(!has_sensitive_skill_text(value), "{value}");
        }
    }

    #[test]
    fn sensitive_text_covers_domains_phones_and_keywords() {
        for value in [
            "example.com",
            "build.example.org-x",
            "reasoning trace",
            "chain_of-thought",
            "curriculum vitae",
            "+355 123456789",
            "(555) 123-4567",
            "address",
        ] {
            assert!(has_sensitive_text(value), "{value}");
        }
        for value in [
            "built with rust",
            "version 1.2.3",
            "example.community",
            "12345",
            "reasoning",
        ] {
            assert!(!has_sensitive_text(value), "{value}");
        }
    }

    #[test]
    fn safe_label_follows_the_fullmatch_pattern() {
        assert!(is_safe_label("experience_dates"));
        assert!(is_safe_label("notes v2.1"));
        assert!(!is_safe_label("_leading"));
        assert!(!is_safe_label("tab\there"));
    }

    #[test]
    fn trimming_follows_the_java_trim_set() {
        assert_eq!(trim_control_and_space("\u{0}\u{20}a b\u{1f}"), "a b");
        assert_eq!(trim_control_and_space("\u{a0}a"), "\u{a0}a");
    }
}
