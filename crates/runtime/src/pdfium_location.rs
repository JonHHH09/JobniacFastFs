//! Where the PDF worker finds the PDFium library (`JOBNIAC_PDFIUM_DIR`).
//!
//! The resolution order is fixed:
//!
//! 1. `JOBNIAC_PDFIUM_DIR`: a valid value is used as given (the worker then refuses a relative
//!    path), and an invalid one gives no directory. No existence check runs, so a test can set it
//!    to a directory that does not exist.
//! 2. Otherwise the bundle roots the caller supplies, in order. A root is used only when it is
//!    absolute and holds `pdfium/<library>`. The desktop app passes its Tauri resource directory;
//!    the console binary passes the locations next to its own executable.
//!
//! The search never reads `PATH`, the working directory or any other environment variable, and the
//! library is always loaded from an absolute directory. A missing library
//! is the typed [`PdfiumNotFound`] whose text names no path.

use std::path::{Path, PathBuf};

use jobniac_core::env::{EnvValue, Environment};

use crate::services::PDFIUM_DIR_VARIABLE;

/// The folder inside a bundle root that holds the PDFium library. The installer payload and the
/// Tauri `bundle.resources` mapping both use this name (`pdfium/pdfium.dll`, `pdfium/libpdfium.so`).
pub const PDFIUM_FOLDER: &str = "pdfium";

/// The library file of the current platform.
#[cfg(windows)]
pub const PDFIUM_LIBRARY_FILE: &str = "pdfium.dll";
/// The library file of the current platform.
#[cfg(not(windows))]
pub const PDFIUM_LIBRARY_FILE: &str = "libpdfium.so";

/// The product name, which is also the Tauri resource folder name on Linux
/// (`<lib>/<productName>`). It must match `productName` in `src-tauri/tauri.conf.json`.
pub const PRODUCT_RESOURCE_FOLDER: &str = "JobniacFast";

/// No usable PDFium library was found. The text names no path, a value or a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the PDF library is not available")]
pub struct PdfiumNotFound;

/// Resolves the PDFium directory for the PDF worker.
///
/// `is_file` reports whether a path is an existing regular file; the caller passes the file system
/// check, and tests pass a fake. Only `pdfium/<library>` under an absolute root is probed.
///
/// # Errors
///
/// Returns [`PdfiumNotFound`] when the variable is present but not valid Unicode, or when no bundle
/// root holds the library.
pub fn resolve_pdfium_dir<F>(
    environment: &Environment,
    bundle_roots: &[PathBuf],
    is_file: F,
) -> Result<PathBuf, PdfiumNotFound>
where
    F: Fn(&Path) -> bool,
{
    match environment.lookup(PDFIUM_DIR_VARIABLE) {
        EnvValue::Valid(value) => return Ok(PathBuf::from(value)),
        EnvValue::Invalid => return Err(PdfiumNotFound),
        EnvValue::Missing => {}
    }
    bundle_roots
        .iter()
        .filter(|root| root.is_absolute())
        .map(|root| root.join(PDFIUM_FOLDER))
        .find(|directory| is_file(&directory.join(PDFIUM_LIBRARY_FILE)))
        .ok_or(PdfiumNotFound)
}

/// Reports the outcome of [`resolve_pdfium_dir`] once at startup and turns it into the directory
/// the worker adapters take. A missing library stays the generic PDF failure for the user,
/// but it is not silent: one fixed warning says why (no path, no value). An
/// empty `JOBNIAC_PDFIUM_DIR` is used as given, which the worker refuses as a relative path, so it
/// gets its own fixed warning.
#[must_use]
pub fn report_pdfium_dir(resolved: Result<PathBuf, PdfiumNotFound>) -> Option<PathBuf> {
    match resolved {
        Ok(directory) => {
            if directory.as_os_str().is_empty() {
                tracing::warn!("the PDF library directory setting is empty");
            }
            Some(directory)
        }
        Err(error) => {
            tracing::warn!("{error}");
            None
        }
    }
}

/// The bundle roots of the console binary: the directory of its executable (the Windows install
/// and the Tauri sidecar layout) and the Linux resource folder `../lib/<product>` relative to it
/// (the deb and AppImage layout, where `jobniac-cli` sits in `usr/bin`). Both are probed on every
/// platform; a missing folder is simply not used.
#[must_use]
pub fn cli_bundle_roots(executable: &Path) -> Vec<PathBuf> {
    let Some(directory) = executable.parent() else {
        return Vec::new();
    };
    vec![
        directory.to_path_buf(),
        directory
            .join("..")
            .join("lib")
            .join(PRODUCT_RESOURCE_FOLDER),
    ]
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// An absolute path on the current platform, built from a Unix-style spelling.
    fn absolute(unix_style: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!("C:{}", unix_style.replace('/', "\\")))
        } else {
            PathBuf::from(unix_style)
        }
    }

    fn root() -> PathBuf {
        absolute("/opt/jobniac/bin")
    }

    fn library_in(directory: &Path) -> PathBuf {
        directory.join(PDFIUM_FOLDER).join(PDFIUM_LIBRARY_FILE)
    }

    #[test]
    fn the_explicit_variable_wins_without_any_probe() {
        let probes = RefCell::new(Vec::new());
        let environment = Environment::from_pairs([(PDFIUM_DIR_VARIABLE, "/override/pdfium")]);
        let resolved = resolve_pdfium_dir(&environment, &[root()], |path| {
            probes.borrow_mut().push(path.to_path_buf());
            true
        });
        assert_eq!(resolved, Ok(PathBuf::from("/override/pdfium")));
        assert!(probes.borrow().is_empty(), "the bundle was probed");
    }

    #[test]
    fn a_relative_variable_is_passed_through_for_the_worker_to_refuse() {
        let environment = Environment::from_pairs([(PDFIUM_DIR_VARIABLE, "relative/dir")]);
        assert_eq!(
            resolve_pdfium_dir(&environment, &[root()], |_| true),
            Ok(PathBuf::from("relative/dir"))
        );
    }

    #[test]
    fn an_empty_variable_is_passed_through_unchanged() {
        let environment = Environment::from_pairs([(PDFIUM_DIR_VARIABLE, "")]);
        assert_eq!(
            resolve_pdfium_dir(&environment, &[root()], |_| true),
            Ok(PathBuf::new())
        );
    }

    #[test]
    fn an_invalid_variable_gives_no_directory_and_no_fallback() {
        let environment = Environment::default().with_invalid(PDFIUM_DIR_VARIABLE);
        assert_eq!(
            resolve_pdfium_dir(&environment, &[root()], |_| true),
            Err(PdfiumNotFound)
        );
    }

    #[test]
    fn the_bundle_root_is_used_when_the_variable_is_missing() {
        let bundle = library_in(&root());
        let resolved =
            resolve_pdfium_dir(&Environment::default(), &[root()], |path| path == bundle);
        assert_eq!(resolved, Ok(root().join(PDFIUM_FOLDER)));
    }

    #[test]
    fn the_first_bundle_root_that_holds_the_library_wins() {
        let first = absolute("/first/root");
        let second = absolute("/second/root");
        let present = library_in(&second);
        let resolved =
            resolve_pdfium_dir(&Environment::default(), &[first, second.clone()], |path| {
                path == present
            });
        assert_eq!(resolved, Ok(second.join(PDFIUM_FOLDER)));
    }

    #[test]
    fn a_missing_library_is_the_typed_error_with_no_path_in_its_text() {
        let resolved = resolve_pdfium_dir(&Environment::default(), &[root()], |_| false);
        assert_eq!(resolved, Err(PdfiumNotFound));
        let text = PdfiumNotFound.to_string();
        assert!(
            !text.contains('/') && !text.contains('\\'),
            "path in: {text}"
        );
        assert!(!text.contains(PDFIUM_LIBRARY_FILE), "file name in: {text}");
        assert!(!text.contains("jobniac"), "root in: {text}");
    }

    #[test]
    fn relative_roots_are_never_probed() {
        let probes = RefCell::new(Vec::new());
        let resolved = resolve_pdfium_dir(
            &Environment::default(),
            &[PathBuf::from("relative").join("root")],
            |path| {
                probes.borrow_mut().push(path.to_path_buf());
                true
            },
        );
        assert_eq!(resolved, Err(PdfiumNotFound));
        assert!(probes.borrow().is_empty());
    }

    #[test]
    fn path_and_the_working_directory_are_never_consulted() {
        // A library planted under PATH or in the working directory must not be found: every probe
        // stays inside the given bundle roots.
        let planted = absolute("/planted/on/path");
        let environment = Environment::from_pairs([("PATH", planted.to_string_lossy().as_ref())]);
        let probes = RefCell::new(Vec::new());
        let resolved = resolve_pdfium_dir(&environment, &[root()], |path| {
            probes.borrow_mut().push(path.to_path_buf());
            path.starts_with(&planted) || path.starts_with(".") || path.starts_with("..")
        });
        assert_eq!(resolved, Err(PdfiumNotFound));
        let probes = probes.into_inner();
        assert!(!probes.is_empty());
        assert!(probes.iter().all(|path| path.starts_with(root())));
    }

    #[test]
    fn cli_roots_are_the_executable_folder_and_the_resource_folder_beside_it() {
        let executable = absolute("/usr/bin/jobniac-cli");
        let roots = cli_bundle_roots(&executable);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0], absolute("/usr/bin"));
        assert_eq!(
            roots[1],
            absolute("/usr/bin")
                .join("..")
                .join("lib")
                .join(PRODUCT_RESOURCE_FOLDER)
        );
    }

    #[test]
    fn cli_roots_find_the_library_in_the_deb_resource_folder() {
        let executable = absolute("/usr/bin/jobniac-cli");
        let roots = cli_bundle_roots(&executable);
        let deb_directory = absolute("/usr/bin")
            .join("..")
            .join("lib")
            .join(PRODUCT_RESOURCE_FOLDER)
            .join(PDFIUM_FOLDER);
        let deb_library = deb_directory.join(PDFIUM_LIBRARY_FILE);
        let resolved =
            resolve_pdfium_dir(&Environment::default(), &roots, |path| path == deb_library);
        assert_eq!(resolved, Ok(deb_directory));
    }

    #[test]
    fn a_root_without_a_parent_yields_no_roots() {
        assert!(cli_bundle_roots(Path::new("/")).is_empty());
    }
}
