//! The regular expressions of the deterministic profile extractor.
//!
//! Three differences from the `regex` crate's defaults are handled here:
//!
//! * The whitespace class `\s` is Unicode `isspace`, which includes U+001C to U+001F. Every `\s` is
//!   written as the explicit set [`WS_SET`].
//! * Case-insensitive matching includes the four non-ASCII letters U+0130, U+0131, U+017F and U+212A for
//!   `[a-z]` and for the literal letters `i`, `s` and `k`. The `regex` crate folds case differently,
//!   so every ASCII letter that is matched case-insensitively is written by [`ci`] as an explicit
//!   class with those extras.
//! * The lookbehind and lookahead guards (`(?<!\d)`, `(?!\d)`, `(?<![a-z0-9])`) are not
//!   available. They are checked by [`guarded_find`] and [`phrase_present`] on the match indices.

use std::sync::LazyLock;

use regex::Regex;

/// The class body of `\s` (Unicode `isspace`), without the brackets.
pub(super) const WS_SET: &str =
    r"\t-\r\x1c-\x20\x{85}\x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}";
/// The ASCII letters and the four non-ASCII letters that case-insensitive matching includes for `[a-z]`.
const LETTERS: &str = r"A-Za-z\x{130}\x{131}\x{17f}\x{212a}";

/// Writes every ASCII letter outside a class as a case-insensitive class (see the module notes).
/// Escapes (`\d`, `\.`) and class contents are copied unchanged.
pub(super) fn ci(source: &str) -> String {
    let mut out = String::with_capacity(source.len() * 3);
    let mut in_class = false;
    let mut chars = source.chars();
    while let Some(character) = chars.next() {
        match character {
            '\\' => {
                out.push(character);
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            '[' if !in_class => {
                in_class = true;
                out.push(character);
            }
            ']' if in_class => {
                in_class = false;
                out.push(character);
            }
            letter if !in_class && letter.is_ascii_alphabetic() => {
                out.push_str(&ci_letter(letter));
            }
            other => out.push(other),
        }
    }
    out
}

/// The case-insensitive class of one ASCII letter, with the non-ASCII extras.
fn ci_letter(letter: char) -> String {
    match letter.to_ascii_lowercase() {
        'i' => "[iIİı]".to_owned(),
        'k' => "[kK\u{212a}]".to_owned(),
        's' => "[sSſ]".to_owned(),
        lower => format!("[{lower}{}]", lower.to_ascii_uppercase()),
    }
}

/// The source of `\s*` (or `\s+`) as an explicit class.
fn ws() -> String {
    format!("[{WS_SET}]")
}

/// Compiles a fixed pattern. The patterns are constants that the unit tests compile, so a failure
/// is a programming error reported with the pattern name.
fn compile(name: &str, source: &str) -> Regex {
    Regex::new(source).unwrap_or_else(|error| panic!("profile extraction pattern {name}: {error}"))
}

/// The email pattern: `[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}` with IGNORECASE.
pub(super) static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "EMAIL",
        &format!(r"[{LETTERS}0-9._%+\-]+@[{LETTERS}0-9.\-]+\.[{LETTERS}]{{2,}}"),
    )
});

/// The body of the phone pattern, without its guards.
const PHONE_BODY_TEMPLATE: &str = r"(?:\+?1[-.@WS@]?)?\(?\d{3}\)?[-.@WS@]?\d{3}[-.@WS@]?\d{4}";

/// The phone pattern, anchored: group 1 is the phone text, and the trailing
/// alternative is the lookahead `(?!\d)` (the match is taken on the slice that starts at it).
pub(super) static PHONE: LazyLock<Regex> = LazyLock::new(|| {
    let body = PHONE_BODY_TEMPLATE.replace("@WS@", WS_SET);
    compile("PHONE", &format!(r"^({body})(?:[^\d]|$)"))
});

/// The URL pattern: `https?://[^\s)>,]+` with IGNORECASE.
pub(super) static URL: LazyLock<Regex> =
    LazyLock::new(|| compile("URL", &ci(&format!(r"https?://[^{WS_SET})>,]+"))));

/// The LinkedIn pattern: `(?:https?://)?(?:www\.)?linkedin\.com/in/[^\s)>,]+` with IGNORECASE.
pub(super) static LINKEDIN: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "LINKEDIN",
        &ci(&format!(
            r"(?:https?://)?(?:www\.)?linkedin\.com/in/[^{WS_SET})>,]+"
        )),
    )
});

/// The GitHub pattern: `(?:https?://)?(?:www\.)?github\.com/[^\s)>,]+` with IGNORECASE.
pub(super) static GITHUB: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        "GITHUB",
        &ci(&format!(
            r"(?:https?://)?(?:www\.)?github\.com/[^{WS_SET})>,]+"
        )),
    )
});

/// The raw month group: every alternative is optional as a whole, so a bare year matches.
const MONTH: &str = r"(?:jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|may|june?|july?|aug(?:ust)?|sep(?:t(?:ember)?)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?)?";

/// Group 1 is the start, group 2 the end (`present`, `current` or a date).
pub(super) static DATE_RANGE: LazyLock<Regex> = LazyLock::new(|| {
    let ws = ws();
    let source = format!(
        r"({MONTH}{ws}*\d{{4}}){ws}*(?:-|–|—|to){ws}*(present|current|{MONTH}{ws}*\d{{4}})"
    );
    compile("DATE_RANGE", &ci(&source))
});

/// Group 1 is the degree, group 2 the field.
pub(super) static DEGREE: LazyLock<Regex> = LazyLock::new(|| {
    let ws = ws();
    compile(
        "DEGREE",
        &ci(&format!(
            r"^(B\.?A\.?|B\.?S\.?|BSc|BA|M\.?S\.?|MSc|MA|PhD|Bachelor(?:'s)?|Master(?:'s)?){ws}+(?:in{ws}+)?(.+)$"
        )),
    )
});

/// The section header pattern, applied with `fullmatch` to the lower-cased, stripped line. Group 1 is
/// the section name.
pub(super) static SECTION_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    let ws = ws();
    compile(
        "SECTION_HEADER",
        &format!(
            r"^(summary|profile|professional summary|skills|technical skills|technologies|technology stack|languages|language skills|experience|work experience|projects|education){ws}*:?$"
        ),
    )
});

/// The `\s+[-–—]\s+` separator of an entry start (split once).
pub(super) static ENTRY_SEPARATOR: LazyLock<Regex> = LazyLock::new(|| {
    let ws = ws();
    compile("ENTRY_SEPARATOR", &format!(r"{ws}+[\-–—]{ws}+"))
});

/// The `\s+[–—]\s+` separator that becomes ` | ` in a header.
pub(super) static PART_SEPARATOR: LazyLock<Regex> = LazyLock::new(|| {
    let ws = ws();
    compile("PART_SEPARATOR", &format!(r"{ws}+[–—]{ws}+"))
});

/// The `[._-]+` separator of an email local part, used by the name fallback.
pub(super) static LOCAL_SEPARATOR: LazyLock<Regex> =
    LazyLock::new(|| compile("LOCAL_SEPARATOR", r"[._\-]+"));

/// The first match of `pattern` in the text that is preceded by a character that is not a
/// Unicode decimal digit, and followed by one that is not a digit (both guards are part of the
/// anchored pattern). Candidate starts are the characters where `candidate` holds.
///
/// Start positions are scanned left to right, so this returns the leftmost match.
pub(super) fn guarded_find<'t>(
    text: &'t str,
    pattern: &Regex,
    candidate: impl Fn(char) -> bool,
) -> Option<&'t str> {
    let mut previous: Option<char> = None;
    for (index, character) in text.char_indices() {
        let preceded_by_digit = previous.is_some_and(jobniac_text::str_rules::is_text_decimal);
        if !preceded_by_digit
            && candidate(character)
            && let Some(captures) = pattern.captures(&text[index..])
            && let Some(group) = captures.get(1)
        {
            return Some(group.as_str());
        }
        previous = Some(character);
    }
    None
}

/// The word class of the phrase boundaries: `[a-z0-9]` matched case-insensitively.
fn is_phrase_word(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, 'İ' | 'ı' | 'ſ' | '\u{212a}')
}

/// Searches `(?<![a-z0-9])<phrase>(?![a-z0-9])` case-insensitively.
///
/// Every start position where the phrase occurs is checked, so an earlier occurrence that fails a
/// boundary does not hide a later valid one.
pub(super) fn phrase_present(text: &str, phrase: &Regex) -> bool {
    let mut position = 0;
    while position <= text.len() {
        let Some(found) = phrase.find_at(text, position) else {
            return false;
        };
        let before = text[..found.start()].chars().next_back();
        let after = text[found.end()..].chars().next();
        let bounded = !before.is_some_and(is_phrase_word) && !after.is_some_and(is_phrase_word);
        if bounded {
            return true;
        }
        match text[found.start()..].chars().next() {
            Some(character) => position = found.start() + character.len_utf8(),
            None => return false,
        }
    }
    false
}

/// Compiles the case-insensitive literal of one known skill or language phrase.
pub(super) fn phrase_pattern(phrase: &str) -> Regex {
    compile("phrase", &ci(&regex::escape(phrase)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixed_pattern_compiles() {
        assert!(EMAIL.is_match("a@example.com"));
        assert!(PHONE.is_match("5550100199 "));
        assert!(URL.is_match("https://example.com"));
        assert!(LINKEDIN.is_match("linkedin.com/in/ada"));
        assert!(GITHUB.is_match("github.com/ada"));
        assert!(DATE_RANGE.is_match("Jan 2020 - Present"));
        assert!(DEGREE.is_match("BSc in Computer Science"));
        assert!(SECTION_HEADER.is_match("summary:"));
        assert!(ENTRY_SEPARATOR.is_match("a - b"));
        assert!(PART_SEPARATOR.is_match("a — b"));
        assert!(LOCAL_SEPARATOR.is_match("._"));
    }

    #[test]
    fn ci_adds_non_ascii_extras_to_letters() {
        let pattern = Regex::new(&format!("^{}$", ci("is"))).ok();
        assert!(pattern.as_ref().is_some_and(|re| re.is_match("İſ")));
        assert!(pattern.as_ref().is_some_and(|re| re.is_match("ıS")));
        let kelvin = Regex::new(&format!("^{}$", ci("k"))).ok();
        assert!(kelvin.as_ref().is_some_and(|re| re.is_match("\u{212a}")));
    }

    #[test]
    fn guarded_phone_respects_digit_boundaries() {
        assert_eq!(
            guarded_find("x5550100199", &PHONE, |c| c.is_ascii_digit()),
            Some("5550100199")
        );
        assert_eq!(
            guarded_find("95550100199", &PHONE, |c| c.is_ascii_digit()),
            None
        );
        assert_eq!(
            guarded_find("call 555 010 0199 now", &PHONE, |c| c.is_ascii_digit()),
            Some("555 010 0199")
        );
    }

    #[test]
    fn phrase_boundaries_use_the_word_class() {
        let spring = phrase_pattern("spring ai");
        assert!(phrase_present("use Spring AI, ok", &spring));
        assert!(!phrase_present("Spring AIx", &spring));
        assert!(!phrase_present("xSpring AI", &spring));
        let skill = phrase_pattern("python");
        assert!(phrase_present("python and", &skill));
        assert!(!phrase_present("pythonista python3 pythonİ", &skill));
    }
}
