//! Lexical Windows path rules checked against the golden vectors in `golden/windows-path.json`.
//! Pure string logic: runs on every platform.

use jobniac_infra::files::windows_path::{
    PathRefusal, is_allowed_component, is_reserved_device_stem, normalize_drive_path,
};
use serde_json::Value;

const VECTORS: &str = include_str!("golden/windows-path.json");

#[test]
fn golden_vectors_match_normalize_path() {
    let records: Vec<Value> = serde_json::from_str(VECTORS).expect("vector file parses");
    assert!(records.len() > 2000, "vector file holds the full set");
    let mut accepted = 0_usize;
    let mut refused = 0_usize;
    for record in &records {
        let input = record["input"].as_str().expect("input is a string");
        let outcome = normalize_drive_path(input);
        if record["ok"].as_bool() == Some(true) {
            accepted += 1;
            let expected = record["output"].as_str().expect("output is a string");
            assert_eq!(outcome.as_deref(), Ok(expected), "accepted input {input:?}");
        } else {
            refused += 1;
            let expected = record["reason"].as_str().expect("reason is a string");
            match outcome {
                Ok(value) => panic!("refused input {input:?} was accepted as {value:?}"),
                Err(refusal) => assert_eq!(refusal.message(), expected, "refused input {input:?}"),
            }
        }
    }
    assert!(accepted > 300, "accepted vectors cover the positive rules");
    assert!(refused > 300, "refused vectors cover the negative rules");
}

#[test]
fn drive_root_and_separator_forms() {
    assert_eq!(
        normalize_drive_path("c:/Users/x/file.pdf").as_deref(),
        Ok("C:\\Users\\x\\file.pdf")
    );
    assert_eq!(normalize_drive_path("C:\\").as_deref(), Ok("C:\\"));
    assert_eq!(
        normalize_drive_path("C:relative.pdf"),
        Err(PathRefusal::NotDriveAbsolute)
    );
    assert_eq!(
        normalize_drive_path("relative\\file.pdf"),
        Err(PathRefusal::NotDriveAbsolute)
    );
    assert_eq!(
        normalize_drive_path("\\\\server\\share\\file.pdf"),
        Err(PathRefusal::NotDriveAbsolute)
    );
    assert_eq!(
        normalize_drive_path("\\\\?\\C:\\file.pdf"),
        Err(PathRefusal::NotDriveAbsolute)
    );
}

#[test]
fn traversal_empty_and_repeated_separators_are_refused() {
    for path in [
        "C:\\a\\..\\b.pdf",
        "C:\\a\\.\\b.pdf",
        "C:\\a\\\\b.pdf",
        "C:\\a\\..",
        "C:\\a\\.",
    ] {
        assert_eq!(
            normalize_drive_path(path),
            Err(PathRefusal::ComponentNotAllowed),
            "{path}"
        );
    }
}

#[test]
fn alternate_data_streams_are_refused() {
    assert_eq!(
        normalize_drive_path("C:\\a\\b.pdf:stream"),
        Err(PathRefusal::ComponentNotAllowed)
    );
    assert!(!is_allowed_component("file.pdf:stream"));
}

#[test]
fn forbidden_characters_and_trailing_dot_or_space_are_refused() {
    for name in [
        "b*.pdf",
        "b?.pdf",
        "b<.pdf",
        "b>.pdf",
        "b\".pdf",
        "b|.pdf",
        "b\u{1}.pdf",
        "trailing.",
        "trailing ",
    ] {
        assert!(!is_allowed_component(name), "{name:?}");
    }
    assert!(is_allowed_component("resume final.pdf"));
}

#[test]
fn old_style_device_names_are_refused_with_or_without_extension() {
    for name in [
        "CON",
        "con.txt",
        "PRN",
        "AUX",
        "NUL.pdf",
        "COM1",
        "lpt9.pdf",
        "CONIN$",
        "CONOUT$.txt",
    ] {
        assert!(!is_allowed_component(name), "{name}");
    }
    assert!(
        !is_allowed_component("NUL .txt"),
        "a space before the dot is stripped, not allowed"
    );
}

#[test]
fn superscript_digits_and_dotless_i_are_device_names() {
    assert!(is_reserved_device_stem("COM\u{b9}"));
    assert!(is_reserved_device_stem("COM\u{b2}"));
    assert!(is_reserved_device_stem("LPT\u{b3}"));
    assert!(is_reserved_device_stem("CON\u{131}N$"));
    assert!(is_reserved_device_stem("CON\u{130}N$"));
    assert!(!is_reserved_device_stem("COM\u{b2}x"));
    assert!(!is_reserved_device_stem("COM10"));
}

#[test]
fn similar_names_are_allowed() {
    for name in [
        "CONSOLE.pdf",
        "NULL.pdf",
        "PRNX",
        "COMx",
        "resume.pdf",
        "CONIN.pdf",
    ] {
        assert!(is_allowed_component(name), "{name}");
    }
}
