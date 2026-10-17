//! The environment of a worker child.
//!
//! The child starts from an empty environment. Only the variables of [`ALLOWED_KEYS`] cross from
//! the parent, matched case-insensitively with their original spelling kept. Interpreter and path
//! overrides, proxy settings, database settings and application secrets never cross. The internal
//! marker is added last, so no parent value can replace it.

use std::ffi::OsString;

use crate::workers::kinds::WorkerKind;
use crate::workers::protocol::MARKER_VARIABLE;

/// The variables that may cross from the parent into a worker child, in upper case.
pub const ALLOWED_KEYS: [&str; 9] = [
    "PATH",
    "SYSTEMROOT",
    "WINDIR",
    "TMP",
    "TEMP",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
];

/// The locale that every Linux child runs under.
#[cfg(target_os = "linux")]
const LINUX_LOCALE: &str = "C.UTF-8";

/// The search path of every Linux child (a fixed default, never the parent's value).
#[cfg(target_os = "linux")]
const LINUX_DEFAULT_PATH: &str = "/usr/bin:/bin";

/// Whether a variable name may cross into a worker child.
#[must_use]
pub fn is_allowed(name: &str) -> bool {
    let upper = name.to_uppercase();
    ALLOWED_KEYS.contains(&upper.as_str())
}

/// The complete environment of a child of `kind`, built from the parent's `source` variables.
///
/// On Linux `LANG` and `LC_ALL` are always `C.UTF-8` and `PATH` is always the fixed system search
/// path, so no parent value crosses for them. Windows children receive only the copied variables
/// and the marker.
#[must_use]
pub fn child_environment(
    source: &[(OsString, OsString)],
    kind: WorkerKind,
) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = source
        .iter()
        .filter(|(name, _)| name.to_str().is_some_and(is_allowed))
        .filter(|(name, _)| !is_fixed_by_platform(name))
        .cloned()
        .collect();
    add_platform_fixed(&mut environment);
    environment.push((
        OsString::from(MARKER_VARIABLE),
        OsString::from(kind.marker_value()),
    ));
    environment
}

/// Variables that the platform sets itself, so the parent's values never cross: `PATH`, `LANG` and
/// `LC_ALL` on Linux, and `PATH` on Windows (the PDFium library is loaded by absolute path).
#[cfg(target_os = "linux")]
fn is_fixed_by_platform(name: &OsString) -> bool {
    name.to_str()
        .is_some_and(|name| matches!(name.to_uppercase().as_str(), "PATH" | "LANG" | "LC_ALL"))
}

/// Variables that the platform sets itself, so the parent's values never cross (Windows: `PATH`).
#[cfg(not(target_os = "linux"))]
fn is_fixed_by_platform(name: &OsString) -> bool {
    name.to_str()
        .is_some_and(|name| name.eq_ignore_ascii_case("PATH"))
}

#[cfg(target_os = "linux")]
fn add_platform_fixed(environment: &mut Vec<(OsString, OsString)>) {
    environment.push((OsString::from("PATH"), LINUX_DEFAULT_PATH.into()));
    environment.push((OsString::from("LANG"), LINUX_LOCALE.into()));
    environment.push((OsString::from("LC_ALL"), LINUX_LOCALE.into()));
}

#[cfg(not(target_os = "linux"))]
fn add_platform_fixed(_environment: &mut Vec<(OsString, OsString)>) {}
