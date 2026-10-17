//! End-to-end PDF worker operations: the PDFium children of the built CLI binary against the
//! synthetic corpus (qualification against the recorded extraction output) and the hostile set.
//!
//! The tests need PDFium: set `JOBNIAC_PDFIUM_DIR` to the directory that holds the library fetched
//! by `scripts/fetch-pdfium.*`. Without it each test prints a skip note and passes locally; in CI
//! (`CI` set) a missing directory fails the test instead. The fixtures and the recorded extraction are
//! committed under `crates/infra/tests`.
//!
//! Qualification rule: the text of every page, after the
//! page-text normalization (whitespace collapsed inside each line, lines
//! joined with `\n`, the page stripped), must equal the recorded page text exactly. Pages
//! are compared one by one, so page boundaries and line boundaries are part of the comparison.
//! No Unicode normalization (NFKC) is applied to either side.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jobniac_infra::workers::WorkerSupervisor;
use jobniac_infra::workers::pdf::{
    EXTRACT_FAILURE_MESSAGE, EXTRACTION_VALIDATION_MESSAGE, ExtractionOptions, MAX_FILE_BYTES,
    PdfError, PdfExtraction, PdfOperation, RENDER_FAILURE_MESSAGE, extract_text, render_page,
};
use serde_json::Value;

type TestResult = Result<(), Box<dyn Error>>;

/// The wall-clock bound of one hostile file. The worker's own limit is 15 seconds.
const HOSTILE_BOUND: Duration = Duration::from_secs(60);

/// The PDFium directory, or `None` when the tests must skip. In CI a missing directory is a
/// failure, so the PDFium tests cannot pass without running.
fn pdfium_dir() -> Option<PathBuf> {
    match std::env::var_os("JOBNIAC_PDFIUM_DIR") {
        Some(directory) => Some(PathBuf::from(directory)),
        None => {
            assert!(
                std::env::var_os("CI").is_none(),
                "JOBNIAC_PDFIUM_DIR must be set in CI so that the PDFium tests run"
            );
            eprintln!(
                "skipped: JOBNIAC_PDFIUM_DIR is not set; PDFium tests need the pinned library"
            );
            None
        }
    }
}

/// The fixture root shared with the parent-side tests.
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../infra/tests/fixtures/pdf")
}

/// A supervisor for the built CLI binary, with the parent's environment as its source.
fn supervisor() -> WorkerSupervisor {
    WorkerSupervisor::new(
        PathBuf::from(env!("CARGO_BIN_EXE_jobniac-cli")),
        std::env::vars_os().collect(),
    )
}

/// The recorded extraction of every fixture.
fn recorded() -> Result<Value, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../infra/tests/golden/pdf-extract.json");
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

/// The recorded outcome of one fixture, by its name in the recorded file.
fn record_for<'a>(corpus: &'a Value, name: &str) -> Result<&'a Value, Box<dyn Error>> {
    corpus["files"]
        .as_array()
        .and_then(|files| files.iter().find(|record| record["name"] == name))
        .ok_or_else(|| format!("missing recorded extraction for {name}").into())
}

/// The fixture files of one directory, sorted by name.
fn fixtures(directory: &str) -> Result<Vec<(String, PathBuf)>, Box<dyn Error>> {
    let mut files: Vec<(String, PathBuf)> = fs::read_dir(fixture_root().join(directory))?
        .map(|entry| {
            let entry = entry?;
            Ok((
                format!("{directory}/{}", entry.file_name().to_string_lossy()),
                entry.path(),
            ))
        })
        .collect::<Result<_, Box<dyn Error>>>()?;
    files.sort();
    Ok(files)
}

/// The per-page texts of a recorded result.
fn recorded_pages(result: &Value) -> Vec<String> {
    result["pages"]
        .as_array()
        .map(|pages| {
            pages
                .iter()
                .map(|page| page["text"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Compares an extraction with a recorded result page by page. Returns the differences.
fn compare_with_record(name: &str, ours: &PdfExtraction, record: &Value) -> Vec<String> {
    let mut differences = Vec::new();
    let expected_count = record["pageCount"].as_u64().unwrap_or(u64::MAX);
    if u64::from(ours.page_count) != expected_count {
        differences.push(format!(
            "{name}: page count {} (recorded {expected_count})",
            ours.page_count
        ));
    }
    let expected_pages = recorded_pages(record);
    if ours.pages.len() != expected_pages.len() {
        differences.push(format!(
            "{name}: {} page texts (recorded {})",
            ours.pages.len(),
            expected_pages.len()
        ));
    }
    for (index, (page, expected)) in ours.pages.iter().zip(&expected_pages).enumerate() {
        if page.text != *expected {
            differences.push(format!(
                "{name}: page {} differs\n  pdfium: {:?}\n  recorded: {:?}",
                index + 1,
                page.text,
                expected
            ));
        }
    }
    differences
}

#[tokio::test]
async fn corpus_text_matches_the_recorded_text_page_by_page() -> TestResult {
    let Some(directory) = pdfium_dir() else {
        return Ok(());
    };
    let corpus = recorded()?;
    let mut differences = Vec::new();
    for (name, path) in fixtures("corpus")? {
        let record = record_for(&corpus, &name)?;
        assert_eq!(
            record["ok"].as_bool(),
            Some(true),
            "{name} is a success record"
        );
        let content = fs::read(&path)?;
        // The recorded pages are capped at the maximum budget (250,000 units), so the comparison
        // requests that maximum; the default budget is checked in the budget test below.
        let options = ExtractionOptions::new(Some(250_000), Some(true))?;
        match extract_text(
            &supervisor(),
            &directory,
            &content,
            "resume.pdf",
            options,
            std::future::pending(),
        )
        .await
        {
            Ok(ours) => {
                differences.extend(compare_with_record(&name, &ours, &record["result"]));
                if ours.text != record["result"]["text"].as_str().unwrap_or_default() {
                    differences.push(format!("{name}: full text differs"));
                }
            }
            Err(error) => differences.push(format!("{name}: {error}")),
        }
    }
    assert!(
        differences.is_empty(),
        "differences:\n{}",
        differences.join("\n")
    );
    Ok(())
}

/// The one-off documented difference of the off-page text fixture: PDFium returns only the text
/// inside the page box (text outside the page box is dropped), while the recorded extraction has all sixty lines.
#[tokio::test]
async fn text_below_the_page_box_is_not_returned_by_pdfium() -> TestResult {
    let Some(directory) = pdfium_dir() else {
        return Ok(());
    };
    let content = fs::read(fixture_root().join("hostile/off-page-text.pdf"))?;
    let options = ExtractionOptions::new(None, None)?;
    let result = extract_text(
        &supervisor(),
        &directory,
        &content,
        "off-page.pdf",
        options,
        std::future::pending(),
    )
    .await?;
    assert!(result.text.contains("Off page line 001"));
    assert!(!result.text.contains("Off page line 060"));
    Ok(())
}

#[tokio::test]
async fn budgets_cut_the_text_and_flag_the_truncation() -> TestResult {
    let Some(directory) = pdfium_dir() else {
        return Ok(());
    };
    let content = fs::read(fixture_root().join("corpus/long-pages.pdf"))?;
    let options = ExtractionOptions::new(Some(50), Some(false))?;
    let view = extract_text(
        &supervisor(),
        &directory,
        &content,
        "long.pdf",
        options,
        std::future::pending(),
    )
    .await?;
    assert!(view.truncated);
    assert_eq!(view.page_count, 2);
    assert!(view.text.encode_utf16().count() <= 50);
    assert!(view.pages.is_empty());
    Ok(())
}

/// The intended outcome of one hostile file, checked against the recorded outcome. The
/// documented deviations are named in the match and asserted on their own.
fn check_hostile(
    name: &str,
    record: &Value,
    outcome: &Result<PdfExtraction, PdfError>,
) -> Result<(), String> {
    match name {
        // The recorded extraction follows the page tree and reads the one real page;
        // PDFium trusts the declared /Count of 1,000,000 and the page limit refuses the file.
        "hostile/huge-page-count.pdf" => {
            return match outcome {
                Err(PdfError::PageLimit) => Ok(()),
                other => Err(format!("expected the page limit, got {other:?}")),
            };
        }
        // Text outside the page box is not returned (see the test
        // above); every other line of the fixture is returned.
        "hostile/off-page-text.pdf" => {
            return match outcome {
                Ok(view) if view.text.contains("Off page line 001") => Ok(()),
                other => Err(format!("expected the in-page text, got {other:?}")),
            };
        }
        _ => {}
    }
    let recorded_ok = record["ok"].as_bool() == Some(true);
    if recorded_ok {
        let view = outcome
            .as_ref()
            .map_err(|error| format!("expected the recorded text, got {error:?}"))?;
        let differences = compare_with_record(name, view, &record["result"]);
        return if differences.is_empty() {
            Ok(())
        } else {
            Err(differences.join("; "))
        };
    }
    let expected_failure = match name {
        "hostile/zero-byte" | "hostile/not-a-pdf.txt" => PdfError::Validation {
            field: "path",
            reason: String::new(),
        },
        "hostile/page-limit.pdf" => PdfError::PageLimit,
        _ => PdfError::Extraction,
    };
    let expected_message = expected_failure.message();
    match outcome {
        Err(error)
            if std::mem::discriminant(error) == std::mem::discriminant(&expected_failure) =>
        {
            if error.message() == expected_message {
                Ok(())
            } else {
                Err(format!("message {:?}", error.message()))
            }
        }
        other => Err(format!(
            "expected a sanitized failure of the same kind as {expected_failure:?}, got {other:?}"
        )),
    }
}

#[tokio::test]
async fn hostile_files_match_the_recorded_outcome_and_never_hang() -> TestResult {
    let Some(directory) = pdfium_dir() else {
        return Ok(());
    };
    let corpus = recorded()?;
    let mut differences = Vec::new();
    for (name, path) in fixtures("hostile")? {
        let record = record_for(&corpus, &name)?;
        let content = fs::read(&path)?;
        let options = ExtractionOptions::new(None, None)?;
        let outcome = tokio::time::timeout(
            HOSTILE_BOUND,
            extract_text(
                &supervisor(),
                &directory,
                &content,
                "hostile.pdf",
                options,
                std::future::pending(),
            ),
        )
        .await;
        let Ok(outcome) = outcome else {
            differences.push(format!("{name}: did not finish within the bound"));
            continue;
        };
        if let Err(message) = check_hostile(&name, record, &outcome) {
            differences.push(format!("{name}: {message}"));
        }
    }
    assert!(
        differences.is_empty(),
        "hostile differences:\n{}",
        differences.join("\n")
    );
    Ok(())
}

/// The import limit is admitted end to end, byte for byte. A synthetic PDF of exactly
/// `MAX_FILE_BYTES` (a valid document with trailing padding, which PDF readers accept) extracts
/// through the real worker, because the transport limit counts the framing. One byte more is
/// refused by the child.
#[tokio::test]
async fn a_pdf_at_the_import_limit_extracts_and_one_byte_more_is_refused() -> TestResult {
    let Some(directory) = pdfium_dir() else {
        return Ok(());
    };
    let corpus = recorded()?;
    let base = fs::read(fixture_root().join("corpus/resume-simple.pdf"))?;

    // The padding is the one the corpus generator uses.
    let at_limit_name = "generated/pdf-10mib.pdf";
    let record = record_for(&corpus, at_limit_name)?;
    assert_eq!(
        record["ok"].as_bool(),
        Some(true),
        "{at_limit_name} extracts in the recorded extraction"
    );
    let mut at_limit = base.clone();
    at_limit.resize(MAX_FILE_BYTES, b'\n');
    assert_eq!(at_limit.len(), 10 * 1024 * 1024);
    let options = ExtractionOptions::new(None, None)?;
    let extracted = extract_text(
        &supervisor(),
        &directory,
        &at_limit,
        "pdf-10mib.pdf",
        options,
        std::future::pending(),
    )
    .await?;
    let differences = compare_with_record(at_limit_name, &extracted, &record["result"]);
    assert!(
        differences.is_empty(),
        "a PDF of exactly the import limit must match the recorded extraction: {differences:?}"
    );

    // One byte more is refused by the parent before any child starts, with the recorded reason.
    let over_name = "generated/pdf-10mib-plus-1.pdf";
    let record = record_for(&corpus, over_name)?;
    assert_eq!(
        record["ok"].as_bool(),
        Some(false),
        "{over_name} is refused in the recorded extraction"
    );
    let reason = record["error"]["details"]["reason"]
        .as_str()
        .ok_or("reason")?
        .to_owned();
    let mut over = base;
    over.resize(MAX_FILE_BYTES + 1, b'\n');
    let options = ExtractionOptions::new(None, None)?;
    let refused = extract_text(
        &supervisor(),
        &directory,
        &over,
        "pdf-10mib-plus-1.pdf",
        options,
        std::future::pending(),
    )
    .await;
    assert_eq!(
        refused,
        Err(PdfError::Validation {
            field: "path",
            reason,
        })
    );
    Ok(())
}

/// The UTF-16 units of `text` cut at `maximum`, as the display budget cuts: a high
/// surrogate left at the cut is dropped, so the cut never keeps half of a character.
fn truncate_utf16(text: &str, maximum: usize) -> Result<String, Box<dyn Error>> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= maximum {
        return Ok(text.to_owned());
    }
    let mut cut = maximum;
    if cut > 0 && (0xD800..=0xDBFF).contains(&units[cut - 1]) {
        cut -= 1;
    }
    Ok(String::from_utf16(&units[..cut])?)
}

/// The page texts of an extraction view for one display budget.
fn budgeted_pages(pages: &[String], maximum: usize) -> Result<Vec<String>, Box<dyn Error>> {
    let mut remaining = maximum;
    let mut out = Vec::new();
    for page in pages {
        if remaining == 0 {
            break;
        }
        let text = truncate_utf16(page, remaining)?;
        remaining -= text.encode_utf16().count();
        out.push(text);
    }
    Ok(out)
}

/// The text budgets at their boundaries exactly 100,000 and 250,000 units, one
/// unit over each, and a 250,001-unit text whose 250,000 cut falls inside an emoji. Every file is
/// compared with the recorded text: the full text page by page, and the display budgets of
/// the default (100,000) and the maximum (250,000), including the cut at a split surrogate pair.
#[tokio::test]
async fn text_budgets_match_the_recorded_boundaries() -> TestResult {
    let Some(directory) = pdfium_dir() else {
        return Ok(());
    };
    const DISPLAY_BUDGETS: [usize; 2] = [100_000, 250_000];
    let corpus = recorded()?;
    let mut differences = Vec::new();
    for name in [
        "corpus/budget-100000.pdf",
        "corpus/budget-100001.pdf",
        "corpus/budget-250000.pdf",
        "corpus/budget-250001.pdf",
        "corpus/budget-250001-split-pair.pdf",
    ] {
        let record = record_for(&corpus, name)?;
        let recorded = &record["result"];
        let content = fs::read(fixture_root().join(name))?;
        let full = extract_text(
            &supervisor(),
            &directory,
            &content,
            "budget.pdf",
            ExtractionOptions::new(Some(250_000), Some(true))?,
            std::future::pending(),
        )
        .await?;
        differences.extend(compare_with_record(name, &full, recorded));
        let recorded_text = recorded["text"].as_str().unwrap_or_default();
        if full.text != recorded_text {
            differences.push(format!("{name}: full text differs from the recorded text"));
        }
        let count = usize::try_from(recorded["characterCount"].as_u64().ok_or("count")?)?;
        if full.character_count != count {
            differences.push(format!(
                "{name}: {} units (recorded {count})",
                full.character_count
            ));
        }
        for maximum in DISPLAY_BUDGETS {
            let view = extract_text(
                &supervisor(),
                &directory,
                &content,
                "budget.pdf",
                ExtractionOptions::new(Some(maximum as i64), Some(true))?,
                std::future::pending(),
            )
            .await?;
            let expected_text = truncate_utf16(recorded_text, maximum)?;
            let expected_pages = budgeted_pages(&recorded_pages(recorded), maximum)?;
            let pages: Vec<String> = view.pages.iter().map(|page| page.text.clone()).collect();
            if view.text != expected_text {
                differences.push(format!("{name} at {maximum}: text differs"));
            }
            if pages != expected_pages {
                differences.push(format!("{name} at {maximum}: page texts differ"));
            }
            if view.truncated != (count > maximum) {
                differences.push(format!("{name} at {maximum}: truncated flag differs"));
            }
        }
    }
    assert!(
        differences.is_empty(),
        "budget differences:\n{}",
        differences.join("\n")
    );
    Ok(())
}

/// Every hostile file that the recorded extraction refuses is also refused by the render child, which
/// runs the same preflight before PDFium loads the file.
#[tokio::test]
async fn refused_hostile_files_are_not_rendered() -> TestResult {
    let Some(directory) = pdfium_dir() else {
        return Ok(());
    };
    for name in [
        "hostile/encrypted.pdf",
        "hostile/malformed-xref.pdf",
        "hostile/truncated.pdf",
    ] {
        let content = fs::read(fixture_root().join(name))?;
        let outcome = render_page(
            &supervisor(),
            &directory,
            &content,
            0,
            std::future::pending(),
        )
        .await;
        assert_eq!(outcome, Err(PdfError::Render), "{name}");
    }
    Ok(())
}

/// A cancellation that is already due kills the extraction worker and reports the extraction
/// cancellation, whose copy is the extraction message.
#[tokio::test]
async fn cancelled_extraction_reports_the_extraction_message() -> TestResult {
    let Some(directory) = pdfium_dir() else {
        return Ok(());
    };
    let content = fs::read(fixture_root().join("corpus/two-pages.pdf"))?;
    let options = ExtractionOptions::new(None, None)?;
    let outcome = extract_text(
        &supervisor(),
        &directory,
        &content,
        "cancel.pdf",
        options,
        std::future::ready(()),
    )
    .await;
    assert_eq!(
        outcome,
        Err(PdfError::Cancelled {
            operation: PdfOperation::Extract
        })
    );
    assert_eq!(
        outcome.err().map(|error| error.message()),
        Some(EXTRACT_FAILURE_MESSAGE)
    );
    Ok(())
}

#[tokio::test]
async fn rendering_produces_a_bounded_png_of_the_requested_page() -> TestResult {
    let Some(directory) = pdfium_dir() else {
        return Ok(());
    };
    let content = fs::read(fixture_root().join("corpus/two-pages.pdf"))?;
    let rendered = render_page(
        &supervisor(),
        &directory,
        &content,
        1,
        std::future::pending(),
    )
    .await?;
    assert_eq!(rendered.page_count, 2);
    assert_eq!(rendered.height, 1600, "the longer side is the page limit");
    assert!(rendered.width <= 1600 && rendered.width > 1000);
    assert!(rendered.png.starts_with(&[0x89, b'P', b'N', b'G']));
    let missing = render_page(
        &supervisor(),
        &directory,
        &content,
        5,
        std::future::pending(),
    )
    .await;
    assert_eq!(missing, Err(PdfError::Render));
    assert_eq!(RENDER_FAILURE_MESSAGE, "Unable to preview this PDF safely.");
    assert_eq!(
        EXTRACTION_VALIDATION_MESSAGE,
        "Invalid PDF text extraction request"
    );
    Ok(())
}

/// The child returns a response only when its confinement is in force (the tripwire of
/// `serve`), so a successful extraction and a successful render of a corpus file prove that
/// the real PDF path ran confined.
#[tokio::test]
async fn the_pdf_children_return_only_when_confined() -> TestResult {
    let Some(directory) = pdfium_dir() else {
        return Ok(());
    };
    let (_, path) = fixtures("corpus")?
        .into_iter()
        .next()
        .ok_or("the corpus is empty")?;
    let content = fs::read(path)?;
    let options = ExtractionOptions::new(Some(250_000), Some(true))?;
    let extraction = extract_text(
        &supervisor(),
        &directory,
        &content,
        "resume.pdf",
        options,
        std::future::pending(),
    )
    .await?;
    assert!(extraction.page_count >= 1);
    render_page(
        &supervisor(),
        &directory,
        &content,
        0,
        std::future::pending(),
    )
    .await?;
    Ok(())
}
