//! The PDF worker through the default PDFium location.
//!
//! The console binary is copied into a folder with the library in `pdfium/` beside it, which is the
//! layout of the installed payload. The resolution then runs with an empty environment snapshot, so
//! `JOBNIAC_PDFIUM_DIR` plays no part, and a real extraction runs through the resolved directory.
//!
//! The missing-library test needs no PDFium. The extraction test needs the library: it skips
//! locally when `JOBNIAC_PDFIUM_DIR` is unset, and fails in CI, as the other PDFium tests do.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use jobniac_core::env::Environment;
use jobniac_infra::workers::WorkerSupervisor;
use jobniac_infra::workers::pdf::{ExtractionOptions, extract_text};
use jobniac_runtime::pdfium_location::{
    PDFIUM_FOLDER, PDFIUM_LIBRARY_FILE, PdfiumNotFound, cli_bundle_roots, resolve_pdfium_dir,
};
use serde_json::Value;

type TestResult = Result<(), Box<dyn Error>>;

/// The directory of the built library to copy: `vendor/pdfium/<platform>` of this repository, or
/// `JOBNIAC_PDFIUM_DIR` when it is set and holds the library. `None` when the test must skip. In CI
/// a missing library is a failure.
fn source_library_dir() -> Option<PathBuf> {
    let platform = if cfg!(windows) {
        "windows-x64"
    } else {
        "linux-x64"
    };
    let candidates = [
        std::env::var_os("JOBNIAC_PDFIUM_DIR").map(PathBuf::from),
        Some(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../vendor/pdfium")
                .join(platform),
        ),
    ];
    let found = candidates
        .into_iter()
        .flatten()
        .find(|directory| directory.join(PDFIUM_LIBRARY_FILE).is_file());
    if found.is_none() {
        assert!(
            std::env::var_os("CI").is_none(),
            "the PDFium library must be fetched into vendor/pdfium in CI so that the bundle test runs"
        );
        eprintln!("skipped: no PDFium library to copy; run scripts/fetch-pdfium first");
    }
    found
}

/// A fresh layout under the test target directory: the copied console binary and, when `library`
/// is given, `pdfium/<library>` beside it. Returns the path of the copied executable.
fn bundle_layout(name: &str, library: Option<&Path>) -> Result<PathBuf, Box<dyn Error>> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    // The previous run's layout is removed first; a missing folder is the normal case.
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(root.join(PDFIUM_FOLDER))?;
    let executable = root.join(
        Path::new(env!("CARGO_BIN_EXE_jobniac-cli"))
            .file_name()
            .ok_or("the test binary has no file name")?,
    );
    fs::copy(env!("CARGO_BIN_EXE_jobniac-cli"), &executable)?;
    if let Some(library) = library {
        fs::copy(
            library.join(PDFIUM_LIBRARY_FILE),
            root.join(PDFIUM_FOLDER).join(PDFIUM_LIBRARY_FILE),
        )?;
    }
    Ok(executable)
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../infra/tests/fixtures/pdf")
        .join(name)
}

#[test]
fn a_missing_bundled_library_is_the_typed_error_and_nothing_else_is_searched() -> TestResult {
    let executable = bundle_layout("pdf-location-missing", None)?;
    let resolved = resolve_pdfium_dir(
        &Environment::default(),
        &cli_bundle_roots(&executable),
        |path| path.is_file(),
    );
    assert_eq!(resolved, Err(PdfiumNotFound));
    Ok(())
}

#[tokio::test]
async fn the_worker_extracts_text_from_the_default_location_without_the_variable() -> TestResult {
    let Some(library_dir) = source_library_dir() else {
        return Ok(());
    };
    let executable = bundle_layout("pdf-location-bundle", Some(&library_dir))?;
    // The empty snapshot is the point of the test: nothing from the process environment is used.
    let resolved = resolve_pdfium_dir(
        &Environment::default(),
        &cli_bundle_roots(&executable),
        |path| path.is_file(),
    )?;
    assert_eq!(
        resolved,
        executable
            .parent()
            .unwrap_or(Path::new("."))
            .join(PDFIUM_FOLDER)
    );

    let recorded: Value = serde_json::from_slice(&fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../infra/tests/golden/pdf-extract.json"),
    )?)?;
    let record = recorded["files"]
        .as_array()
        .and_then(|files| {
            files
                .iter()
                .find(|record| record["name"] == "corpus/resume-simple.pdf")
        })
        .ok_or("missing recorded extraction for resume-simple.pdf")?;
    let recorded_first_page = record["result"]["pages"][0]["text"]
        .as_str()
        .ok_or("the recorded extraction has no first page text")?
        .to_owned();

    let content = fs::read(fixture("corpus/resume-simple.pdf"))?;
    let options = ExtractionOptions::new(Some(250_000), Some(true))?;
    let extraction = extract_text(
        // The child environment is an allowlist over this source (E-NET-31); the resolution above
        // never reads it, so passing the process environment here does not affect the test.
        &WorkerSupervisor::new(executable, std::env::vars_os().collect()),
        &resolved,
        &content,
        "resume-simple.pdf",
        options,
        std::future::pending(),
    )
    .await?;
    assert_eq!(
        extraction.pages.first().map(|page| page.text.as_str()),
        Some(recorded_first_page.as_str())
    );
    Ok(())
}
