//! Character and string case rules of the French translator.
//!
//! The rules work character by character. A simple case fold is used for
//! whole-phrase and protected-term matching; it never expands a character to several characters.

use jobniac_text::java::utf16_length;

/// The one-character title-case form of a character whose full uppercase form has several
/// characters. Only the Greek letters with an iota
/// subscript have a one-character title form among those, so the table lists exactly them.
fn single_title(character: char) -> Option<char> {
    let code = u32::from(character);
    let title = match code {
        0x1F80..=0x1F87 | 0x1F90..=0x1F97 | 0x1FA0..=0x1FA7 => code + 8,
        0x1F88..=0x1F8F | 0x1F98..=0x1F9F | 0x1FA8..=0x1FAF | 0x1FBC | 0x1FCC | 0x1FFC => code,
        0x1FB3 => 0x1FBC,
        0x1FC3 => 0x1FCC,
        0x1FF3 => 0x1FFC,
        _ => return None,
    };
    char::from_u32(title)
}

/// The Java-compatible simple case fold of one character.
///
/// `i`, `I`, U+0130 and U+0131 all fold to `i`. Otherwise the character is uppercased; when that
/// is not one character, its title form is used if it is one character, else the character
/// itself. The result is lowercased and returned only when it is one character, so the fold is
/// always a single character.
#[must_use]
pub fn unicode_fold(character: char) -> char {
    if matches!(character, 'i' | 'I' | '\u{130}' | '\u{131}') {
        return 'i';
    }
    let upper: Vec<char> = character.to_uppercase().collect();
    let base = match upper.as_slice() {
        [single] => *single,
        _ => single_title(character).unwrap_or(character),
    };
    let lower: Vec<char> = base.to_lowercase().collect();
    match lower.as_slice() {
        [single] => *single,
        _ => character,
    }
}

/// True when the character's uppercase form is one ASCII capital letter.
#[must_use]
pub fn is_ascii_upper_letter(character: char) -> bool {
    let upper: Vec<char> = character.to_uppercase().collect();
    matches!(upper.as_slice(), [single] if single.is_ascii_uppercase())
}

/// Case-preserving replacement of a matched word or phrase.
///
/// An all-caps original longer than one UTF-16 unit gives the uppercase replacement. Otherwise,
/// when the original's first code point is at most U+FFFF and uppercase, the replacement's first
/// character is uppercased and the rest is kept. Anything else keeps the replacement unchanged.
#[must_use]
pub fn preserve_case(original: &str, replacement: &str) -> String {
    if utf16_length(original) > 1 && original == original.to_uppercase() {
        return replacement.to_uppercase();
    }
    if let Some(first) = original.chars().next()
        && u32::from(first) <= 0xFFFF
        && first.is_uppercase()
    {
        let mut characters = replacement.chars();
        let mut result = String::with_capacity(replacement.len());
        if let Some(head) = characters.next() {
            result.extend(head.to_uppercase());
        }
        result.push_str(characters.as_str());
        return result;
    }
    replacement.to_owned()
}

/// ASCII-only case-insensitive character equality.
#[must_use]
pub fn eq_ascii_insensitive(left: char, right: char) -> bool {
    left == right
        || (left.is_ascii_alphabetic()
            && right.is_ascii_alphabetic()
            && left.eq_ignore_ascii_case(&right))
}

/// The characters that end an opaque chunk: ASCII whitespace and `<`, `>`, `(` and `)`.
#[must_use]
pub fn is_opaque_boundary(character: char) -> bool {
    matches!(
        character,
        ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | '<' | '>' | '(' | ')'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_maps_dotted_and_dotless_i_to_i() {
        for character in ['i', 'I', '\u{130}', '\u{131}'] {
            assert_eq!(unicode_fold(character), 'i');
        }
    }

    #[test]
    fn fold_keeps_sharp_s_and_folds_long_s() {
        assert_eq!(unicode_fold('\u{DF}'), '\u{DF}');
        assert_eq!(unicode_fold('\u{17F}'), 's');
        assert_eq!(unicode_fold('\u{212A}'), 'k');
    }

    #[test]
    fn fold_uses_title_form_for_iota_subscript_capitals() {
        assert_eq!(unicode_fold('\u{1F88}'), '\u{1F80}');
        assert_eq!(unicode_fold('\u{1F80}'), '\u{1F80}');
        assert_eq!(unicode_fold('\u{1FBC}'), '\u{1FB3}');
    }

    #[test]
    fn preserve_case_follows_the_original_first_character() {
        assert_eq!(preserve_case("Built", "élaboré"), "Élaboré");
        assert_eq!(preserve_case("built", "élaboré"), "élaboré");
        assert_eq!(preserve_case("BUILT", "élaboré"), "ÉLABORÉ");
        assert_eq!(preserve_case("B", "élaboré"), "Élaboré");
    }

    #[test]
    fn preserve_case_expands_a_sharp_s_in_the_first_character() {
        assert_eq!(preserve_case("X", "\u{DF}a"), "SSa");
    }

    #[test]
    fn ascii_insensitive_equality_ignores_non_ascii_case() {
        assert!(eq_ascii_insensitive('A', 'a'));
        assert!(!eq_ascii_insensitive('\u{C9}', '\u{E9}'));
        assert!(!eq_ascii_insensitive('K', '\u{212A}'));
    }
}
