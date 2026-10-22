//! The environment allowlist of worker children.

use std::ffi::OsString;

use jobniac_infra::workers::WorkerKind;
use jobniac_infra::workers::env::{ALLOWED_KEYS, child_environment, is_allowed};
use jobniac_infra::workers::protocol::MARKER_VARIABLE;

fn pairs(entries: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    entries
        .iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect()
}

fn value_of(environment: &[(OsString, OsString)], name: &str) -> Option<String> {
    environment
        .iter()
        .find(|(key, _)| {
            key.to_str()
                .is_some_and(|key| key.eq_ignore_ascii_case(name))
        })
        .and_then(|(_, value)| value.to_str().map(str::to_owned))
}

#[test]
fn allowlist_matches_case_insensitively() {
    assert!(is_allowed("path"));
    assert!(is_allowed("Temp"));
    assert!(is_allowed("lc_all"));
    assert!(!is_allowed("JOBNIAC_DB_PATH"));
    assert!(!is_allowed("PYTHONPATH"));
    assert!(!is_allowed("HTTPS_PROXY"));
    assert_eq!(ALLOWED_KEYS.len(), 9);
}

#[test]
fn only_allowlisted_variables_cross_and_secrets_never_do() {
    let source = pairs(&[
        ("path", "C:\\tools"),
        ("Temp", "C:\\temp"),
        ("SystemRoot", "C:\\Windows"),
        ("JOBNIAC_DB_PATH", "secret-value"),
        ("PYTHONPATH", "C:\\evil"),
        ("HTTPS_PROXY", "http://proxy.invalid"),
        ("JOBNIAC_INTERNAL_WORKER", "forged"),
    ]);
    let environment = child_environment(&source, WorkerKind::PdfExtract);

    for (name, _) in &environment {
        let name = name.to_string_lossy().to_uppercase();
        assert!(
            ALLOWED_KEYS.contains(&name.as_str()) || name == MARKER_VARIABLE,
            "{name} must not cross into the child"
        );
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    assert_eq!(value_of(&environment, "PATH"), None);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    assert_eq!(
        value_of(&environment, "PATH").as_deref(),
        Some("/usr/bin:/bin"),
        "the parent's PATH never crosses on Linux and macOS"
    );
    assert_eq!(
        value_of(&environment, "SYSTEMROOT").as_deref(),
        Some("C:\\Windows")
    );
    let secret = environment
        .iter()
        .any(|(_, value)| value.to_string_lossy().contains("secret-value"));
    assert!(!secret);
}

#[test]
fn original_key_spelling_is_kept() {
    let environment = child_environment(&pairs(&[("Temp", "C:\\temp")]), WorkerKind::Probe);
    assert!(
        environment
            .iter()
            .any(|(name, value)| name == "Temp" && value == "C:\\temp")
    );
}

#[test]
fn marker_is_present_once_with_the_kind_value_and_cannot_be_forged() {
    let source = pairs(&[(MARKER_VARIABLE, "forged-v1")]);
    let environment = child_environment(&source, WorkerKind::PdfRender);
    let markers: Vec<&(OsString, OsString)> = environment
        .iter()
        .filter(|(name, _)| name == MARKER_VARIABLE)
        .collect();
    assert_eq!(markers.len(), 1);
    assert_eq!(
        markers[0].1,
        OsString::from(WorkerKind::PdfRender.marker_value())
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn unix_children_run_under_the_fixed_locale_and_a_search_path() {
    let environment = child_environment(&[], WorkerKind::Probe);
    assert_eq!(value_of(&environment, "LANG").as_deref(), Some("C.UTF-8"));
    assert_eq!(value_of(&environment, "LC_ALL").as_deref(), Some("C.UTF-8"));
    assert_eq!(
        value_of(&environment, "PATH").as_deref(),
        Some("/usr/bin:/bin")
    );
}

#[cfg(windows)]
#[test]
fn windows_children_get_no_locale_or_path_that_the_parent_did_not_have() {
    let environment = child_environment(&[], WorkerKind::Probe);
    assert_eq!(value_of(&environment, "LANG"), None);
    assert_eq!(value_of(&environment, "LC_ALL"), None);
    assert_eq!(value_of(&environment, "PATH"), None);
}
