//! Java ROOT-locale lowercasing (clean-room).
//!
//! The pinned OpenJDK 25 `toLowerCase(Locale.ROOT)` behaviour is reproduced from three ingredients,
//! none of them copied from OpenJDK:
//!
//! - the single-character mappings: Rust `char::to_lowercase` (Unicode simple and special casing),
//!   except the code points in [`UNMAPPED_BY_JDK_25`];
//! - Greek capital sigma (U+03A3) becomes the final form U+03C2 when a cased letter comes before it in
//!   its word and no cased letter comes after it in the same word (see [`is_final_sigma`]);
//! - every other character maps to itself.
//!
//! The word rules are a small model written from recorded JDK observations (see [`class_of`] and
//! [`splits`]). The vectors in `tests/golden` pin it: every scalar value is checked through nine probes
//! (`root-lower-sigma-classes.json`), every string of up to three representatives and up to four core
//! representatives with one sigma (`root-lower-sigma-pairs.json`), the sigma contexts, the recorded JDK
//! observations and seeded random strings. The model is not the JDK's state machine. Recorded
//! differences and open cases:
//!
//! - Unicode versions: the code points in [`UNMAPPED_BY_JDK_25`] have case mappings or categories in the
//!   Unicode data of this build that OpenJDK 25 does not know; OpenJDK 25 leaves them unchanged and
//!   treats them as breaks.
//! - Cased letters: OpenJDK 25 counts some `Other_Lowercase` characters that are not `Ll` as cased (for
//!   example U+02B0 and the Roman numerals U+2160..U+217F) and does not count the feminine and masculine
//!   ordinal indicators (U+00AA, U+00BA). [`OTHER_LOWERCASE_CASED`] lists the observed set. The Roman
//!   numerals count as digits for the word splits, as the recorded observations show.
//! - U+0345 COMBINING GREEK YPOGEGRAMMENI counts as cased in a forward scan, and in a backward scan only
//!   when it continues a letter or digit to its left; a mark run that starts a word does not count. See
//!   [`class_of`].
//! - Format characters: the code points in [`IGNORED_FORMAT`] are skipped by both scans. Other format
//!   characters are word breaks. The tag characters U+E0020..U+E007F are breaks here, but OpenJDK
//!   does not always agree when one stands between a letter and U+0345 (a few random contexts with a tag
//!   character directly before U+0345 differ); those contexts are not pinned.
//! - Word breaks: colons and most other punctuation break a word; apostrophes and full stops join two
//!   digits; the other splits are listed in [`splits`]. Supplementary characters break a word, except a
//!   cased letter, which counts only at the start of the text in a backward scan.
//! - Lone surrogates: not representable in `str`, so they are not handled. The golden
//!   vectors replace each lone surrogate with U+E000 (a break in both implementations).

use unicode_general_category::{GeneralCategory, get_general_category};

const CAPITAL_SIGMA: char = '\u{3a3}';
const SMALL_SIGMA: char = '\u{3c3}';
const FINAL_SIGMA: char = '\u{3c2}';

/// Inclusive code point ranges that OpenJDK 25 leaves unchanged under ROOT lowercasing and treats as
/// unassigned, although the Unicode data of this build assigns them case mappings or categories.
const UNMAPPED_BY_JDK_25: &[(u32, u32)] = &[
    (0xA7CE, 0xA7CE),
    (0xA7D2, 0xA7D2),
    (0xA7D4, 0xA7D4),
    (0x16EA0, 0x16EB8),
];

/// Inclusive ranges of `Other_Lowercase` characters that OpenJDK 25 counts as cased letters although
/// their general category is not `Ll` (Lm and Nl characters in the observed set). U+0345 is handled
/// separately by [`class_of`].
const OTHER_LOWERCASE_CASED: &[(u32, u32)] = &[
    (0x02B0, 0x02B8),
    (0x02C0, 0x02C1),
    (0x02E0, 0x02E4),
    (0x037A, 0x037A),
    (0x1D2C, 0x1D61),
    (0x2160, 0x217F),
];

/// Format characters that OpenJDK 25 skips in the word scans (the sixth-category `Cf` code points that
/// OpenJDK ignores; the soft hyphen U+00AD is a joining punctuation mark instead).
const IGNORED_FORMAT: &[(u32, u32)] = &[
    (0x0600, 0x0605),
    (0x061C, 0x061C),
    (0x06DD, 0x06DD),
    (0x070F, 0x070F),
    (0x0890, 0x0891),
    (0x08E2, 0x08E2),
    (0x180E, 0x180E),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x2064),
    (0x2066, 0x206F),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
];

/// Lowercases `text` with the pinned OpenJDK 25 ROOT rules.
///
/// The result does not depend on the locale. Greek capital sigma becomes its final form when the word
/// context rule of the module documentation says so. Lone surrogates cannot occur in `text`.
pub fn root_lower(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, &character) in characters.iter().enumerate() {
        if character == CAPITAL_SIGMA {
            out.push(if is_final_sigma(&characters, index) {
                FINAL_SIGMA
            } else {
                SMALL_SIGMA
            });
        } else if in_ranges(character, UNMAPPED_BY_JDK_25) {
            out.push(character);
        } else {
            out.extend(character.to_lowercase());
        }
    }
    out
}

/// The joint of a character in the word rules: the kind of neighbour it makes a split with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Joint {
    /// Connector and dash punctuation, quotation marks, the apostrophe, the full stop, U+2027 and the soft
    /// hyphen.
    Punctuation,
    /// Decimal, letter and other numbers, and the Roman numerals.
    Digit,
    /// Non-spacing and enclosing marks.
    Mark,
    /// Other letters and spacing marks.
    Other,
    /// The Devanagari danda U+0964 and double danda U+0965. A backward scan stops at them (they are word
    /// breaks there). A forward scan passes over them, and they split from a following punctuation mark
    /// but not from a following digit.
    Terminal,
}

/// How the final sigma scans treat one character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// A word break: both scans stop at it.
    Break,
    /// A format character that both scans skip.
    Ignored,
    /// A cased BMP letter: a scan that reaches it finds a cased letter.
    Cased(Joint),
    /// U+0345: cased for a forward scan; cased for a backward scan only when it continues a letter or a
    /// digit to its left.
    CasedMark,
    /// A cased supplementary letter: a backward scan finds it only at the start of the text; a forward
    /// scan counts it as cased.
    CasedSupplementary,
    /// An uncased character inside a word: both scans pass over it, subject to [`splits`].
    Transparent(Joint),
}

/// Classifies `character` for the final sigma scans (see the module documentation).
fn class_of(character: char) -> Class {
    if in_ranges(character, UNMAPPED_BY_JDK_25) {
        return Class::Break;
    }
    if character == '\u{345}' {
        return Class::CasedMark;
    }
    let category = get_general_category(character);
    if in_ranges(character, OTHER_LOWERCASE_CASED) {
        return Class::Cased(if category == GeneralCategory::LetterNumber {
            Joint::Digit
        } else {
            Joint::Other
        });
    }
    match category {
        GeneralCategory::UppercaseLetter
        | GeneralCategory::LowercaseLetter
        | GeneralCategory::TitlecaseLetter => {
            return if u32::from(character) > 0xFFFF {
                Class::CasedSupplementary
            } else {
                Class::Cased(Joint::Other)
            };
        }
        _ => {}
    }
    if u32::from(character) > 0xFFFF || is_ideographic_break(character) {
        return Class::Break;
    }
    if matches!(character, '"' | '\'' | '.' | '\u{2027}' | '\u{ad}') {
        return Class::Transparent(Joint::Punctuation);
    }
    if matches!(character, '\u{964}' | '\u{965}') {
        return Class::Transparent(Joint::Terminal);
    }
    if in_ranges(character, IGNORED_FORMAT) {
        return Class::Ignored;
    }
    match category {
        GeneralCategory::ConnectorPunctuation | GeneralCategory::DashPunctuation => {
            Class::Transparent(Joint::Punctuation)
        }
        GeneralCategory::NonspacingMark | GeneralCategory::EnclosingMark => {
            Class::Transparent(Joint::Mark)
        }
        GeneralCategory::SpacingMark => Class::Transparent(Joint::Other),
        GeneralCategory::DecimalNumber
        | GeneralCategory::LetterNumber
        | GeneralCategory::OtherNumber => Class::Transparent(Joint::Digit),
        GeneralCategory::ModifierLetter | GeneralCategory::OtherLetter
            if u32::from(character) <= 0xFFFF =>
        {
            Class::Transparent(Joint::Other)
        }
        _ => Class::Break,
    }
}

/// Whether `class` is a mark in the word rules: it takes the joint of a letter or digit to its left.
fn is_mark_like(class: Class) -> bool {
    matches!(class, Class::CasedMark | Class::Transparent(Joint::Mark))
}

/// The index of the nearest character before `index` that is not ignored.
fn previous_index(characters: &[char], index: usize) -> Option<usize> {
    (0..index)
        .rev()
        .find(|&position| class_of(characters[position]) != Class::Ignored)
}

/// The index of the nearest character after `index` that is not ignored.
fn next_index(characters: &[char], index: usize) -> Option<usize> {
    (index + 1..characters.len()).find(|&position| class_of(characters[position]) != Class::Ignored)
}

/// The joint of the character at `index` when it is the left element of a pair.
fn left_role(characters: &[char], index: usize) -> Option<Joint> {
    let class = class_of(characters[index]);
    if is_mark_like(class) {
        return Some(match previous_index(characters, index) {
            None => Joint::Mark,
            Some(previous) => match left_role(characters, previous) {
                Some(joint @ (Joint::Digit | Joint::Other)) => joint,
                _ => Joint::Mark,
            },
        });
    }
    match class {
        Class::Cased(joint) | Class::Transparent(joint) => Some(joint),
        Class::CasedSupplementary => Some(Joint::Other),
        Class::Break | Class::Ignored | Class::CasedMark => None,
    }
}

/// The joint of a class when it is the right element of a pair.
fn right_role(class: Class) -> Option<Joint> {
    if is_mark_like(class) {
        return Some(Joint::Mark);
    }
    match class {
        Class::Transparent(Joint::Terminal) => Some(Joint::Punctuation),
        Class::Cased(joint) | Class::Transparent(joint) => Some(joint),
        Class::CasedSupplementary => Some(Joint::Other),
        Class::Break | Class::Ignored | Class::CasedMark => None,
    }
}

/// Whether the pinned JDK ends a word between the non-ignored characters at `left` and `right`:
/// punctuation followed by punctuation, a digit or a mark; a digit followed by punctuation; and a mark
/// followed by punctuation. An apostrophe or full stop between two digits joins them.
fn splits(characters: &[char], left: usize, right: usize) -> bool {
    let (Some(left_joint), Some(right_joint)) = (
        left_role(characters, left),
        right_role(class_of(characters[right])),
    ) else {
        return false;
    };
    if left_joint == Joint::Punctuation
        && right_joint == Joint::Digit
        && is_midnum(characters[left])
        && previous_index(characters, left).and_then(|previous| left_role(characters, previous))
            == Some(Joint::Digit)
    {
        return false;
    }
    if left_joint == Joint::Digit
        && right_joint == Joint::Punctuation
        && is_midnum(characters[right])
        && next_index(characters, right).and_then(|next| right_role(class_of(characters[next])))
            == Some(Joint::Digit)
    {
        return false;
    }
    matches!(
        (left_joint, right_joint),
        (
            Joint::Punctuation,
            Joint::Punctuation | Joint::Digit | Joint::Mark
        ) | (Joint::Terminal, Joint::Punctuation)
            | (Joint::Digit, Joint::Punctuation)
            | (Joint::Mark, Joint::Punctuation)
    )
}

/// The apostrophe and the full stop, which join two digits.
fn is_midnum(character: char) -> bool {
    matches!(character, '\'' | '.')
}

/// Hiragana, katakana, the ideographic iteration marks and the unified and compatibility ideographs,
/// which the pinned JDK treats as one-character words.
fn is_ideographic_break(character: char) -> bool {
    matches!(
        u32::from(character),
        0x3041..=0x3094
            | 0x30A1..=0x30FA
            | 0x3005
            | 0x309D..=0x309E
            | 0x30FC..=0x30FE
            | 0x4E00..=0x9FA5
            | 0xF900..=0xFA2D
    )
}

/// Whether `character` lies in one of the inclusive `ranges`.
fn in_ranges(character: char, ranges: &[(u32, u32)]) -> bool {
    let value = u32::from(character);
    ranges
        .iter()
        .any(|&(low, high)| low <= value && value <= high)
}

/// Whether the capital sigma at `index` of `characters` is final: a cased letter comes before it in its
/// word and no cased letter comes after it in the same word.
fn is_final_sigma(characters: &[char], index: usize) -> bool {
    let mut right: Option<usize> = None;
    let mut cased_before = false;
    for position in (0..index).rev() {
        let class = class_of(characters[position]);
        match class {
            Class::Ignored => continue,
            Class::Break | Class::Transparent(Joint::Terminal) => break,
            _ => {}
        }
        if right.is_some_and(|next| splits(characters, position, next)) {
            break;
        }
        match class {
            Class::CasedMark => {
                if matches!(
                    left_role(characters, position),
                    Some(Joint::Digit | Joint::Other)
                ) {
                    cased_before = true;
                    break;
                }
            }
            Class::Cased(_) => {
                cased_before = true;
                break;
            }
            Class::CasedSupplementary => {
                cased_before = position == 0;
                break;
            }
            Class::Transparent(_) | Class::Ignored | Class::Break => {}
        }
        right = Some(position);
    }
    if !cased_before {
        return false;
    }
    let mut left: Option<usize> = None;
    for position in index + 1..characters.len() {
        let class = class_of(characters[position]);
        match class {
            Class::Ignored => continue,
            Class::Break => break,
            _ => {}
        }
        if left.is_some_and(|previous| splits(characters, previous, position)) {
            break;
        }
        match class {
            Class::Cased(_) | Class::CasedMark | Class::CasedSupplementary => return false,
            Class::Transparent(_) | Class::Ignored | Class::Break => {}
        }
        left = Some(position);
    }
    true
}
