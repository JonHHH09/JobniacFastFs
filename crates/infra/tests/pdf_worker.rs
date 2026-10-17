//! PDF worker parent side without PDFium: codecs, validation order, the canonical shape and
//! the recorded extractor output of the synthetic corpus.
//!
//! The end-to-end runs of the PDFium children (qualification and hostile set) are in
//! `crates/cli/tests/pdf_worker.rs`, because only the CLI crate can name the built binary.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use jobniac_infra::workers::WorkerSupervisor;
use jobniac_infra::workers::pdf::{
    EXTRACTOR_VERSION, ExtractionOptions, MAX_CHARACTERS_LIMIT, MAX_FILE_BYTES, MAX_PAGE_COUNT,
    PdfError, PdfExtraction, PdfOperation, PdfPage, RenderedPage, canonical_extraction,
    decode_extract_response, decode_request, encode_extract_response, encode_request, extract_text,
    extraction_view, render_page, validate_pdf_content,
};
use serde_json::Value;

type TestResult = Result<(), Box<dyn Error>>;

/// The committed synthetic fixtures.
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pdf")
}

/// The recorded extraction of the synthetic corpus.
fn golden() -> Result<Value, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/pdf-extract.json");
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

/// Converts one recorded `result` object of the golden record into the parent's type.
fn recorded(result: &Value) -> Result<PdfExtraction, Box<dyn Error>> {
    let pages = result["pages"]
        .as_array()
        .ok_or("pages")?
        .iter()
        .map(|page| {
            Ok(PdfPage {
                page_number: u32::try_from(page["pageNumber"].as_u64().ok_or("pageNumber")?)?,
                text: page["text"].as_str().ok_or("text")?.to_owned(),
            })
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    Ok(PdfExtraction {
        file_name: result["fileName"].as_str().ok_or("fileName")?.to_owned(),
        page_count: u32::try_from(result["pageCount"].as_u64().ok_or("pageCount")?)?,
        character_count: usize::try_from(result["characterCount"].as_u64().ok_or("count")?)?,
        truncated: result["truncated"].as_bool().ok_or("truncated")?,
        text: result["text"].as_str().ok_or("text")?.to_owned(),
        pages,
    })
}

#[test]
fn committed_fixtures_are_small_and_match_the_golden_records() -> TestResult {
    let golden = golden()?;
    let records = golden["files"].as_array().ok_or("files")?;
    let mut on_disk = Vec::new();
    for directory in ["corpus", "hostile"] {
        for entry in fs::read_dir(fixture_root().join(directory))? {
            let entry = entry?;
            let size = entry.metadata()?.len();
            assert!(size < 200 * 1024, "{} is too large", entry.path().display());
            on_disk.push(format!(
                "{directory}/{}",
                entry.file_name().to_string_lossy()
            ));
        }
    }
    on_disk.sort();
    // The "generated/" records are built in memory at test time, so they have no committed file.
    let mut recorded_names: Vec<String> = records
        .iter()
        .map(|record| record["name"].as_str().unwrap_or_default().to_owned())
        .filter(|name| !name.starts_with("generated/"))
        .collect();
    recorded_names.sort();
    assert_eq!(on_disk, recorded_names);
    assert!(
        golden["extractor"]
            .as_str()
            .is_some_and(|version| version.starts_with("python-pypdf:")),
    );
    assert_eq!(golden["limits"]["maxFileBytes"], MAX_FILE_BYTES as u64);
    assert_eq!(golden["limits"]["maxPageCount"], u64::from(MAX_PAGE_COUNT));
    assert_eq!(
        golden["limits"]["maxCharactersLimit"],
        MAX_CHARACTERS_LIMIT as u64
    );
    Ok(())
}

#[test]
fn recorded_canonical_views_are_reproduced_from_their_pages() -> TestResult {
    let golden = golden()?;
    for record in golden["files"].as_array().ok_or("files")? {
        if record["ok"] != true {
            // Only the success records carry a canonical view. A failure record must say so with
            // the error envelope, and it is checked by the CLI qualification test.
            assert!(
                record["error"]["code"].is_string(),
                "{} is a failure record without an error code",
                record["name"]
            );
            continue;
        }
        let expected = recorded(&record["result"])?;
        // The page budget is capped without the '\n\n' separators, so a record above the maximum
        // budget keeps pages whose joined text is not its capped text. Its full canonical text cannot
        // be rebuilt from the record; the budget test in crates/cli/tests/pdf_worker.rs covers it.
        if expected.character_count > MAX_CHARACTERS_LIMIT {
            continue;
        }
        let texts = expected
            .pages
            .iter()
            .map(|page| page.text.clone())
            .collect();
        let canonical = canonical_extraction(texts);
        assert_eq!(canonical.text, expected.text, "{}", record["name"]);
        assert_eq!(canonical.character_count, expected.character_count);
        assert_eq!(canonical.page_count, expected.page_count);
        let view = extraction_view(&canonical, MAX_CHARACTERS_LIMIT, true);
        assert_eq!(view.pages, expected.pages, "{}", record["name"]);
        assert!(!view.truncated);
    }
    Ok(())
}

#[test]
fn budgets_cut_pages_in_order_in_utf16_units() -> TestResult {
    let golden = golden()?;
    let record = golden["files"]
        .as_array()
        .ok_or("files")?
        .iter()
        .find(|record| record["name"] == "corpus/two-pages.pdf")
        .ok_or("two-pages record")?;
    let expected = recorded(&record["result"])?;
    let canonical = canonical_extraction(expected.pages.iter().map(|p| p.text.clone()).collect());
    let view = extraction_view(&canonical, 20, true);
    assert!(view.truncated);
    assert_eq!(view.text, "Page one heading\nExp");
    assert_eq!(view.pages.len(), 1);
    assert_eq!(view.pages[0].text, "Page one heading\nExp");
    let without_pages = extraction_view(&canonical, 20, false);
    assert!(without_pages.pages.is_empty());
    Ok(())
}

#[test]
fn validation_follows_the_documented_order() {
    assert_eq!(
        validate_pdf_content(b"", MAX_FILE_BYTES),
        Err(PdfError::Validation {
            field: "path",
            reason: "file must be a PDF document".to_owned()
        })
    );
    let oversized = [b"%PDF-".as_slice(), &vec![b' '; MAX_FILE_BYTES]].concat();
    assert!(matches!(
        validate_pdf_content(&oversized, MAX_FILE_BYTES),
        Err(PdfError::Validation { field: "path", .. })
    ));
    assert!(validate_pdf_content(b"%PDF-1.4", MAX_FILE_BYTES).is_ok());
    assert!(ExtractionOptions::new(Some(250_001), Some(true)).is_err());
    let defaults = ExtractionOptions::new(None, None);
    assert_eq!(defaults.map(|options| options.max_characters), Ok(100_000));
}

#[test]
fn extractor_versions_differ_from_the_recorded_cache_key() -> TestResult {
    let golden = golden()?;
    let recorded_version = golden["extractor"].as_str().ok_or("extractor")?;
    assert_ne!(recorded_version, EXTRACTOR_VERSION);
    assert!(recorded_version.ends_with(":canonical-250000-pages-utf16-v2"));
    assert!(EXTRACTOR_VERSION.ends_with(":canonical-250000-pages-utf16-v1"));
    Ok(())
}

#[test]
fn request_and_response_bodies_round_trip() -> TestResult {
    let directory = std::env::temp_dir();
    let body = encode_request(PdfOperation::Extract, &directory, 0, b"%PDF-1.4")?;
    let decoded = decode_request(&body).ok_or("decode")?;
    assert_eq!(decoded.operation, PdfOperation::Extract);
    assert_eq!(decoded.content, b"%PDF-1.4");
    let canonical = canonical_extraction(vec!["Résumé".to_owned()]);
    assert_eq!(
        decode_extract_response(&encode_extract_response(&canonical))?,
        canonical
    );
    let mut truncated = body.clone();
    truncated.truncate(3);
    assert!(decode_request(&truncated).is_none());
    assert_eq!(
        decode_extract_response(b"not json"),
        Err(PdfError::Extraction)
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_worker_program_is_the_sanitized_extraction_failure() -> TestResult {
    let supervisor = WorkerSupervisor::new(
        std::env::temp_dir().join("jobniac-no-such-pdf-worker"),
        Vec::new(),
    );
    let content = fs::read(fixture_root().join("corpus/resume-simple.pdf"))?;
    let options = ExtractionOptions::new(None, None)?;
    let outcome = extract_text(
        &supervisor,
        &std::env::temp_dir(),
        &content,
        "resume.pdf",
        options,
        std::future::pending(),
    )
    .await;
    assert_eq!(outcome, Err(PdfError::Extraction));
    Ok(())
}

#[tokio::test]
async fn render_refuses_bad_input_before_any_child_starts() -> TestResult {
    let supervisor = WorkerSupervisor::new(
        std::env::temp_dir().join("jobniac-no-such-pdf-worker"),
        Vec::new(),
    );
    let directory = std::env::temp_dir();
    let content = fs::read(fixture_root().join("corpus/resume-simple.pdf"))?;
    let outcome: Result<RenderedPage, PdfError> = render_page(
        &supervisor,
        &directory,
        &content,
        MAX_PAGE_COUNT,
        std::future::pending(),
    )
    .await;
    assert_eq!(outcome, Err(PdfError::Render));
    let outcome = render_page(&supervisor, &directory, b"%PDF-", 0, std::future::pending()).await;
    assert_eq!(outcome, Err(PdfError::Render));
    Ok(())
}
