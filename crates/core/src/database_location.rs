//! Pure resolution of the local database file (`jobniac.sqlite3`).
//!
//! Like the owned generated-artifact root, the location is derived from an injected
//! [`Environment`] snapshot only, and nothing here touches the filesystem. Creating the owner-only
//! parent directory lives in `jobniac-infra`.
//!
//! The default is per-user and outside the install directory, so an update or a reinstall never
//! touches it. It sits next to the generated root, under the same application folder:
//!
//! - Windows: `%LOCALAPPDATA%\Jobniac\data\jobniac.sqlite3` (drive-absolute base only).
//! - macOS: `$HOME/Library/Application Support/Jobniac/data/jobniac.sqlite3`.
//! - Other: `$XDG_DATA_HOME/jobniac/data/jobniac.sqlite3` when the value is absolute and usable;
//!   an empty or relative value is treated as unset and falls back to
//!   `$HOME/.local/share/jobniac/data/jobniac.sqlite3`. An absolute but malformed value fails closed.
//!
//! `JOBNIAC_DB_PATH` overrides the default. It must be a clean absolute path to the database file
//! (same rules as `JOBNIAC_GENERATED_ROOT`: no `.` or `..` component, no surrounding whitespace or
//! control characters, drive-absolute on Windows). A value that is present but unusable fails
//! closed and is never replaced by the default. The path never crosses IPC.

use crate::env::{EnvValue, Environment};
use crate::generated_root::{
    GENERATED_DIR, RootPlatform, admit_root_path, resolve_default_generated_root,
};

/// Name of the variable that overrides the database file location.
pub const DB_PATH_VARIABLE: &str = "JOBNIAC_DB_PATH";
/// Directory of the database under the application folder, on every platform.
pub const DATA_DIR: &str = "data";
/// File name of the database.
pub const DATABASE_FILE: &str = "jobniac.sqlite3";

/// Why no database location could be resolved. The text names no path, value or variable content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DatabaseLocationError {
    /// `JOBNIAC_DB_PATH` is set but is not a clean absolute path.
    #[error("the database path setting is not a clean absolute path")]
    InvalidOverride,
    /// No per-user data directory could be derived from the environment.
    #[error("the local data directory could not be determined")]
    NoDataDirectory,
}

/// Resolves the database file path for `platform` from the injected environment.
///
/// `JOBNIAC_DB_PATH` wins when it is present; otherwise the per-user default applies. The returned
/// string uses the platform separator (`\` on Windows), as the generated root does.
///
/// # Errors
///
/// - [`DatabaseLocationError::InvalidOverride`] when the variable is present but is not valid
///   Unicode or not a clean absolute path (including the empty string).
/// - [`DatabaseLocationError::NoDataDirectory`] when there is no override and the default base is
///   missing or unusable.
pub fn resolve_database_path(
    environment: &Environment,
    platform: RootPlatform,
) -> Result<String, DatabaseLocationError> {
    match environment.lookup(DB_PATH_VARIABLE) {
        EnvValue::Valid(value) => {
            return admit_root_path(value, platform).ok_or(DatabaseLocationError::InvalidOverride);
        }
        EnvValue::Invalid => return Err(DatabaseLocationError::InvalidOverride),
        EnvValue::Missing => {}
    }
    default_database_path(environment, platform).ok_or(DatabaseLocationError::NoDataDirectory)
}

/// The per-user default, derived from the default generated root so both share the same base
/// rules and application folder by construction: `<base>/<app>/generated` becomes
/// `<base>/<app>/data/jobniac.sqlite3`.
fn default_database_path(environment: &Environment, platform: RootPlatform) -> Option<String> {
    let separator = if platform == RootPlatform::Windows {
        '\\'
    } else {
        '/'
    };
    let root = resolve_default_generated_root(environment, platform)?;
    let app_directory = root.strip_suffix(GENERATED_DIR)?.strip_suffix(separator)?;
    Some(format!(
        "{app_directory}{separator}{DATA_DIR}{separator}{DATABASE_FILE}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(
        pairs: &[(&str, &str)],
        platform: RootPlatform,
    ) -> Result<String, DatabaseLocationError> {
        let environment = Environment::from_pairs(pairs.iter().copied());
        resolve_database_path(&environment, platform)
    }

    #[test]
    fn windows_default_is_under_local_app_data() {
        assert_eq!(
            resolve(
                &[("LOCALAPPDATA", r"C:\Users\someone\AppData\Local")],
                RootPlatform::Windows
            ),
            Ok(r"C:\Users\someone\AppData\Local\Jobniac\data\jobniac.sqlite3".to_owned())
        );
        // A trailing separator on the base does not double up.
        assert_eq!(
            resolve(&[("LOCALAPPDATA", r"D:\Local\")], RootPlatform::Windows),
            Ok(r"D:\Local\Jobniac\data\jobniac.sqlite3".to_owned())
        );
    }

    #[test]
    fn windows_refuses_unusable_bases() {
        for base in [
            r"\\server\share",
            r"\\?\C:\Local",
            r"C:Local",
            r"Local",
            r"C:\Local\..\Other",
            "",
        ] {
            assert_eq!(
                resolve(&[("LOCALAPPDATA", base)], RootPlatform::Windows),
                Err(DatabaseLocationError::NoDataDirectory),
                "{base:?}"
            );
        }
        assert_eq!(
            resolve(&[], RootPlatform::Windows),
            Err(DatabaseLocationError::NoDataDirectory)
        );
    }

    #[test]
    fn linux_prefers_an_absolute_xdg_data_home() {
        assert_eq!(
            resolve(
                &[("XDG_DATA_HOME", "/data/xdg"), ("HOME", "/home/someone")],
                RootPlatform::Other
            ),
            Ok("/data/xdg/jobniac/data/jobniac.sqlite3".to_owned())
        );
    }

    #[test]
    fn linux_falls_back_to_home_for_a_missing_empty_or_relative_xdg() {
        let expected = Ok("/home/someone/.local/share/jobniac/data/jobniac.sqlite3".to_owned());
        for xdg in [None, Some(""), Some("relative/data")] {
            let mut pairs = vec![("HOME", "/home/someone")];
            if let Some(value) = xdg {
                pairs.push(("XDG_DATA_HOME", value));
            }
            assert_eq!(resolve(&pairs, RootPlatform::Other), expected, "{xdg:?}");
        }
    }

    #[test]
    fn linux_fails_closed_on_a_malformed_absolute_xdg_or_a_missing_home() {
        // An absolute but malformed XDG value never falls back to HOME.
        assert_eq!(
            resolve(
                &[("XDG_DATA_HOME", "/data/../etc"), ("HOME", "/home/someone")],
                RootPlatform::Other
            ),
            Err(DatabaseLocationError::NoDataDirectory)
        );
        let invalid =
            Environment::from_pairs([("HOME", "/home/someone")]).with_invalid("XDG_DATA_HOME");
        assert_eq!(
            resolve_database_path(&invalid, RootPlatform::Other),
            Err(DatabaseLocationError::NoDataDirectory)
        );
        assert_eq!(
            resolve(&[], RootPlatform::Other),
            Err(DatabaseLocationError::NoDataDirectory)
        );
        assert_eq!(
            resolve(&[("HOME", "home/relative")], RootPlatform::Other),
            Err(DatabaseLocationError::NoDataDirectory)
        );
    }

    #[test]
    fn macos_default_is_under_application_support() {
        assert_eq!(
            resolve(&[("HOME", "/Users/someone")], RootPlatform::MacOs),
            Ok(
                "/Users/someone/Library/Application Support/Jobniac/data/jobniac.sqlite3"
                    .to_owned()
            )
        );
    }

    #[test]
    fn the_override_wins_and_is_used_as_given() {
        assert_eq!(
            resolve(
                &[
                    (DB_PATH_VARIABLE, "/srv/jobniac/db.sqlite3"),
                    ("HOME", "/home/someone")
                ],
                RootPlatform::Other
            ),
            Ok("/srv/jobniac/db.sqlite3".to_owned())
        );
        assert_eq!(
            resolve(
                &[(DB_PATH_VARIABLE, r"E:\Data\jobniac.sqlite3")],
                RootPlatform::Windows
            ),
            Ok(r"E:\Data\jobniac.sqlite3".to_owned()),
            "the override needs no default base"
        );
    }

    #[test]
    fn an_unusable_override_fails_closed_and_never_falls_back() {
        for value in [
            "",
            " ",
            "relative.sqlite3",
            "./jobniac.sqlite3",
            "/srv/../etc/jobniac.sqlite3",
            " /srv/jobniac.sqlite3",
            "/srv/jobniac\n.sqlite3",
        ] {
            assert_eq!(
                resolve(
                    &[(DB_PATH_VARIABLE, value), ("HOME", "/home/someone")],
                    RootPlatform::Other
                ),
                Err(DatabaseLocationError::InvalidOverride),
                "{value:?}"
            );
        }
        for value in [
            r"\\server\share\db.sqlite3",
            r"relative\db.sqlite3",
            "/unix/style",
        ] {
            assert_eq!(
                resolve(
                    &[(DB_PATH_VARIABLE, value), ("LOCALAPPDATA", r"C:\Local")],
                    RootPlatform::Windows
                ),
                Err(DatabaseLocationError::InvalidOverride),
                "{value:?}"
            );
        }
        let invalid = Environment::default().with_invalid(DB_PATH_VARIABLE);
        assert_eq!(
            resolve_database_path(&invalid, RootPlatform::Other),
            Err(DatabaseLocationError::InvalidOverride)
        );
    }

    #[test]
    fn errors_name_no_value_or_path() {
        for error in [
            DatabaseLocationError::InvalidOverride,
            DatabaseLocationError::NoDataDirectory,
        ] {
            let text = error.to_string();
            assert!(!text.contains('/') && !text.contains('\\'), "{text}");
        }
    }

    #[test]
    fn the_default_sits_next_to_the_generated_root() {
        let environment = Environment::from_pairs([("HOME", "/home/someone")]);
        let root = resolve_default_generated_root(&environment, RootPlatform::Other);
        let database = resolve_database_path(&environment, RootPlatform::Other);
        assert_eq!(
            root,
            Some("/home/someone/.local/share/jobniac/generated".to_owned())
        );
        assert_eq!(
            database,
            Ok("/home/someone/.local/share/jobniac/data/jobniac.sqlite3".to_owned())
        );
    }
}
