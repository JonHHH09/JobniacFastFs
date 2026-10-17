//! Lexical form of drive-absolute Windows paths.
//!
//! Pure string logic that never touches the filesystem. It is compiled on every platform so the
//! golden vectors (`tests/golden/windows-path.json`) run everywhere.

use std::fmt;

/// Why a path was refused by [`normalize_drive_path`]. The variants map to the two fixed
/// messages; they never carry the path itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRefusal {
    /// The path does not start with `X:\` or `X:/` (ASCII drive letter).
    NotDriveAbsolute,
    /// A component is empty, `.` or `..`, contains a forbidden character or a control character,
    /// ends in a space or a dot, or is a legacy device name (also with an extension).
    ComponentNotAllowed,
}

impl PathRefusal {
    /// The fixed refusal message.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NotDriveAbsolute => "path must be drive-absolute",
            Self::ComponentNotAllowed => "path component is not allowed",
        }
    }
}

impl fmt::Display for PathRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for PathRefusal {}

/// Normalizes a drive-absolute path to `X:\segments` with an upper-case drive letter.
///
/// Forward slashes become backslashes, trailing backslashes are dropped (the drive root stays
/// `X:\`), and every component is checked. Empty components (including `//`) are refused.
///
/// # Errors
///
/// Returns [`PathRefusal::NotDriveAbsolute`] when the path does not start with an ASCII drive
/// letter, a colon and a separator, and [`PathRefusal::ComponentNotAllowed`] for a bad component.
pub fn normalize_drive_path(path: &str) -> Result<String, PathRefusal> {
    let bytes = path.as_bytes();
    let drive_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/');
    if !drive_absolute {
        return Err(PathRefusal::NotDriveAbsolute);
    }
    let drive = char::from(bytes[0].to_ascii_uppercase());
    let body = path[3..].replace('/', "\\");
    let body = body.trim_end_matches('\\');
    let mut normalized = format!("{drive}:\\");
    if !body.is_empty() {
        for component in body.split('\\') {
            if !is_allowed_component(component) {
                return Err(PathRefusal::ComponentNotAllowed);
            }
        }
        normalized.push_str(body);
    }
    Ok(normalized)
}

/// Whether one path component passes the component rules.
#[must_use]
pub fn is_allowed_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component.chars().any(is_forbidden_character)
        && !component.ends_with(' ')
        && !component.ends_with('.')
        && !is_reserved_device_stem(
            component
                .split('.')
                .next()
                .unwrap_or_default()
                .trim_end_matches(' '),
        )
}

/// Characters refused in a component: `<>:"|?*` and U+0000 to U+001F.
const fn is_forbidden_character(character: char) -> bool {
    matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (character as u32) < 0x20
}

/// Whether a stem (the text before the first dot, without trailing spaces) is a reserved device
/// name.
///
/// Matches `CON|PRN|AUX|NUL|CONIN$|CONOUT$|COM[0-9²³¹]|LPT[0-9²³¹]` case-insensitively. U+0130 and
/// U+0131 fold to `I` under that case-insensitive match.
#[must_use]
pub fn is_reserved_device_stem(stem: &str) -> bool {
    let folded: String = stem.chars().map(fold_device_character).collect();
    match folded.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        other => {
            let rest = other
                .strip_prefix("COM")
                .or_else(|| other.strip_prefix("LPT"));
            rest.is_some_and(is_device_number)
        }
    }
}

/// The digit set of `COM[0-9²³¹]` and `LPT[0-9²³¹]`: ASCII digits and the superscript one, two
/// and three.
fn is_device_number(rest: &str) -> bool {
    let mut characters = rest.chars();
    matches!(
        (characters.next(), characters.next()),
        (Some('0'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None)
    )
}

/// ASCII upper-casing plus the two Unicode characters that case-insensitive device-name
/// matching folds into an ASCII `I`.
const fn fold_device_character(character: char) -> char {
    match character {
        '\u{130}' | '\u{131}' => 'I',
        other => other.to_ascii_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_root_and_separators_normalize() {
        assert_eq!(
            normalize_drive_path("c:/Users/x/file.pdf").as_deref(),
            Ok("C:\\Users\\x\\file.pdf")
        );
        assert_eq!(normalize_drive_path("C:\\").as_deref(), Ok("C:\\"));
        assert_eq!(normalize_drive_path("d:\\a\\\\").as_deref(), Ok("D:\\a"));
    }

    #[test]
    fn device_stems_match_reserved_names() {
        assert!(is_reserved_device_stem("con"));
        assert!(is_reserved_device_stem("COM\u{b9}"));
        assert!(is_reserved_device_stem("LPT\u{b2}"));
        assert!(is_reserved_device_stem("CONIN$"));
        assert!(is_reserved_device_stem("CON\u{131}N$"));
        assert!(!is_reserved_device_stem("COM10"));
        assert!(!is_reserved_device_stem("CONSOLE"));
        assert!(!is_reserved_device_stem("COM\u{b2}x"));
    }
}
