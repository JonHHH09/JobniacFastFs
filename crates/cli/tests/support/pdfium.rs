//! The PDFium prerequisite of the PDF tests, shared by every test binary that needs the library.
//!
//! The library directory comes from `JOBNIAC_PDFIUM_DIR`; `scripts/fetch-pdfium.*` downloads the pinned build there. On a
//! developer machine a missing directory prints a note and the test skips. When the `CI` environment variable is set the
//! directory must be present, so the test panics instead and a run cannot report a silent pass.
//!
//! Each test binary compiles this module on its own, so the test files declare it with `#[path]`.

use std::path::PathBuf;

const PDFIUM_DIR_KEY: &str = "JOBNIAC_PDFIUM_DIR";

/// The PDFium directory, or `None` when the test must skip. Panics on CI when the directory is missing.
pub fn pdfium_dir_or_skip() -> Option<PathBuf> {
    match std::env::var_os(PDFIUM_DIR_KEY) {
        Some(directory) => Some(PathBuf::from(directory)),
        None => {
            assert!(
                std::env::var_os("CI").is_none(),
                "{PDFIUM_DIR_KEY} must be set in CI so that the PDFium tests run"
            );
            eprintln!(
                "skipped: {PDFIUM_DIR_KEY} is not set; the PDF worker needs the pinned library"
            );
            None
        }
    }
}
