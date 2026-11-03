//! PDF worker parent side without PDFium: codecs, validation order, the canonical shape and
//! the recorded extractor output of the synthetic corpus.
//!
//! The end-to-end runs of the PDFium children (qualification and hostile set) are in
//! `crates/cli/tests/pdf_worker.rs`, because only the CLI crate can name the built binary.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use jobniac_core::documents::model::{
    MAX_ATTACHMENT_BYTES, MAX_ATTACHMENT_TOTAL_BYTES, MAX_INSPECT_ATTACHMENT_NAME_CHARS,
    MAX_INSPECT_ATTACHMENTS, MAX_INSPECT_METADATA_CHARS, PdfAttachmentInfo, PdfInspection,
    PdfPageBox,
};
use jobniac_infra::workers::WorkerSupervisor;
use jobniac_infra::workers::pdf::{
    EXTRACTOR_VERSION, ExtractionOptions, MAX_CHARACTERS_LIMIT, MAX_FILE_BYTES, MAX_PAGE_COUNT,
    PdfError, PdfExtraction, PdfOperation, PdfPage, RenderedPage, attachment_entry,
    canonical_extraction, decode_extract_response, decode_inspect_response, decode_request,
    encode_extract_response, encode_inspect_response, encode_page_limit_response, encode_request,
    extract_text, extraction_view, normalize_metadata_text, page_box, render_page,
    sanitize_attachment_name, select_attachments, validate_pdf_content,
};
use serde_json::{Value, json};

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

// Inspection: the pure helpers and the parent-side decoder of the `pdf-inspect` response.

fn sample_inspection() -> PdfInspection {
    PdfInspection {
        page_count: 2,
        pages: vec![
            PdfPageBox {
                width_centipoints: 59_500,
                height_centipoints: 84_200,
            },
            PdfPageBox {
                width_centipoints: 61_200,
                height_centipoints: 79_200,
            },
        ],
        title: Some("Europass CV".to_owned()),
        producer: None,
        creator: Some("Editor".to_owned()),
        attachments: vec![
            PdfAttachmentInfo {
                name: "attachment.xml".to_owned(),
                byte_size: 5,
                content: Some(b"<?xml".to_vec()),
            },
            PdfAttachmentInfo {
                name: "big.bin".to_owned(),
                byte_size: MAX_ATTACHMENT_BYTES + 1,
                content: None,
            },
        ],
        attachments_truncated: false,
    }
}

/// The inspection response with one member of `result` replaced.
fn inspect_body_with(member: &str, value: Value) -> Vec<u8> {
    let mut body: Value =
        serde_json::from_slice(&encode_inspect_response(&sample_inspection())).expect("json");
    body["result"][member] = value;
    body.to_string().into_bytes()
}

#[test]
fn page_boxes_are_centipoints_rounded_half_away_from_zero() {
    let a4 = page_box(595.0, 842.0);
    assert_eq!(
        a4,
        Some(PdfPageBox {
            width_centipoints: 59_500,
            height_centipoints: 84_200
        })
    );
    let letter = page_box(612.0, 792.0).expect("letter");
    assert_eq!(
        (letter.width_centipoints, letter.height_centipoints),
        (61_200, 79_200)
    );
    // 0.125 and 0.375 points are exact halves of a centipoint (12.5 and 37.5) and round up.
    assert_eq!(page_box(0.125, 1.0).map(|b| b.width_centipoints), Some(13));
    assert_eq!(page_box(0.375, 1.0).map(|b| b.width_centipoints), Some(38));
    assert_eq!(
        page_box(595.276, 841.89).map(|b| b.width_centipoints),
        Some(59_528)
    );
    assert_eq!(page_box(0.0, 10.0), None);
    assert_eq!(page_box(0.004, 10.0), None);
    assert_eq!(page_box(-1.0, 10.0), None);
    assert_eq!(page_box(f32::NAN, 10.0), None);
    assert_eq!(page_box(f32::INFINITY, 10.0), None);
    assert_eq!(
        page_box(f32::MAX, 10.0).map(|b| b.width_centipoints),
        Some(u32::MAX)
    );
}

#[test]
fn metadata_strings_are_normalized_emptied_and_capped() {
    assert_eq!(
        normalize_metadata_text("  a \t b\u{00A0}c  ").as_deref(),
        Some("a b c")
    );
    assert_eq!(normalize_metadata_text(" \n\t "), None);
    assert_eq!(normalize_metadata_text(""), None);
    let long = "x".repeat(MAX_INSPECT_METADATA_CHARS + 50);
    let capped = normalize_metadata_text(&long).expect("capped");
    assert_eq!(capped.chars().count(), MAX_INSPECT_METADATA_CHARS);
    // The cap counts characters, not bytes, and a cut at a space leaves no trailing space.
    let wide = "é".repeat(MAX_INSPECT_METADATA_CHARS + 1);
    assert_eq!(
        normalize_metadata_text(&wide).map(|text| text.chars().count()),
        Some(MAX_INSPECT_METADATA_CHARS)
    );
    let spaced = format!("{} tail", "y".repeat(MAX_INSPECT_METADATA_CHARS - 1));
    assert_eq!(
        normalize_metadata_text(&spaced).map(|text| text.chars().count()),
        Some(MAX_INSPECT_METADATA_CHARS - 1)
    );
}

#[test]
fn attachment_names_lose_control_characters_and_are_capped() {
    assert_eq!(
        sanitize_attachment_name("a\u{0}b\r\nc\u{7f}.xml"),
        "abc.xml"
    );
    let long = "n".repeat(MAX_INSPECT_ATTACHMENT_NAME_CHARS + 10);
    assert_eq!(
        sanitize_attachment_name(&long).chars().count(),
        MAX_INSPECT_ATTACHMENT_NAME_CHARS
    );
    assert_eq!(sanitize_attachment_name("\u{1}\u{2}"), "");
}

#[test]
fn attachments_are_selected_with_xml_first_and_cut_at_sixteen() {
    assert_eq!(select_attachments::<&str>(0, &[]), (vec![], false));
    // Several large attachments come before the XML in document order; the XML is still listed
    // first and so is the first to get content within the budgets.
    let names = [
        "a.bin",
        "b.bin",
        "c.bin",
        "d.bin",
        "attachment.XML",
        "e.bin",
    ];
    let (order, truncated) = select_attachments(names.len(), &names);
    assert_eq!(order, vec![4, 0, 1, 2, 3, 5]);
    assert!(!truncated);
    let mut total = 0_u64;
    let size = MAX_ATTACHMENT_BYTES;
    let contents: Vec<bool> = order
        .iter()
        .map(|_| {
            attachment_entry("x", size, &mut total, || {
                Some(vec![0; usize::try_from(size).expect("size")])
            })
            .content
            .is_some()
        })
        .collect();
    assert_eq!(contents, vec![true, true, true, true, false, false]);
    // With more than sixteen attachments, a late XML is still listed and the list is truncated.
    let mut many: Vec<String> = (0..40).map(|index| format!("f{index}.bin")).collect();
    many[35] = "late.xml".to_owned();
    let (order, truncated) = select_attachments(many.len(), &many);
    assert_eq!(order.len(), MAX_INSPECT_ATTACHMENTS);
    assert_eq!(order[0], 35);
    assert_eq!(&order[1..4], &[0, 1, 2]);
    assert!(truncated);
    // Only the names that were read are considered; the reported total decides the flag.
    let (order, truncated) = select_attachments(60_000, &names);
    assert_eq!(order.len(), 6);
    assert!(truncated);
    assert!(!select_attachments(16, &["a"; 16]).1);
    assert!(select_attachments(17, &["a"; 16]).1);
}

#[test]
fn attachment_content_follows_the_size_and_total_limits() {
    let mut total = 0_u64;
    let small = attachment_entry("a.xml", 3, &mut total, || Some(vec![1, 2, 3]));
    assert_eq!(small.content, Some(vec![1, 2, 3]));
    assert_eq!(total, 3);
    // Over the per-attachment limit: the loader never runs and the size is still reported.
    let big = attachment_entry("big", MAX_ATTACHMENT_BYTES + 1, &mut total, || {
        panic!("an oversized attachment is not loaded")
    });
    assert_eq!(
        (big.byte_size, big.content),
        (MAX_ATTACHMENT_BYTES + 1, None)
    );
    assert_eq!(total, 3);
    // Exactly at the per-attachment limit is returned, until the running total is exhausted.
    let mut total = 0_u64;
    let mut returned = 0;
    for _ in 0..6 {
        let entry = attachment_entry("m", MAX_ATTACHMENT_BYTES, &mut total, || {
            Some(vec![
                0;
                usize::try_from(MAX_ATTACHMENT_BYTES).expect("size")
            ])
        });
        returned += usize::from(entry.content.is_some());
        assert_eq!(entry.byte_size, MAX_ATTACHMENT_BYTES);
    }
    assert_eq!(returned, 4, "4 MiB of one MiB attachments");
    assert_eq!(total, MAX_ATTACHMENT_TOTAL_BYTES);
    // Once the total is spent, even a one-byte attachment is listed without content.
    let skipped = attachment_entry("s", 1, &mut total, || Some(vec![0]));
    assert_eq!((skipped.byte_size, skipped.content), (1, None));
    // A failed load or a load of another size leaves the content out.
    let mut total = 0_u64;
    assert_eq!(attachment_entry("f", 2, &mut total, || None).content, None);
    assert_eq!(
        attachment_entry("w", 2, &mut total, || Some(vec![1])).content,
        None
    );
    assert_eq!(total, 0);
    assert_eq!(
        attachment_entry("c\u{0}d", 0, &mut total, || Some(Vec::new())).name,
        "cd"
    );
}

#[test]
fn inspect_response_round_trips_and_reports_page_limit() {
    let inspection = sample_inspection();
    assert_eq!(
        decode_inspect_response(&encode_inspect_response(&inspection)),
        Ok(inspection)
    );
    assert_eq!(
        decode_inspect_response(&encode_page_limit_response()),
        Err(PdfError::PageLimit)
    );
}

#[test]
fn the_inspect_request_round_trips_with_the_inspect_operation() -> TestResult {
    let directory = std::env::temp_dir();
    let body = encode_request(PdfOperation::Inspect, &directory, 0, b"%PDF-x")?;
    let decoded = decode_request(&body).ok_or("request")?;
    assert_eq!(decoded.operation, PdfOperation::Inspect);
    assert_eq!(decoded.content, b"%PDF-x");
    assert_eq!(PdfError::Inspection.message(), "PDF could not be inspected");
    Ok(())
}

#[test]
fn the_decoder_refuses_malformed_and_over_limit_responses() {
    let refused = |body: Vec<u8>| {
        assert_eq!(decode_inspect_response(&body), Err(PdfError::Inspection));
    };
    refused(b"not json".to_vec());
    refused(b"{}".to_vec());
    refused(br#"{"result":[]}"#.to_vec());
    refused(br#"{"error":"other"}"#.to_vec());
    // Page count and page list.
    refused(inspect_body_with("pageCount", json!(3)));
    refused(inspect_body_with("pageCount", json!(0)));
    refused(inspect_body_with("pageCount", json!(-1)));
    refused(inspect_body_with(
        "pages",
        json!([[59_500, 84_200], [0, 10]]),
    ));
    refused(inspect_body_with("pages", json!([[59_500, 84_200], [1]])));
    refused(inspect_body_with(
        "pages",
        json!([[59_500, 84_200], [4_294_967_296_u64, 1]]),
    ));
    refused(inspect_body_with("pages", json!("x")));
    let many_pages = vec![json!([100, 100]); MAX_PAGE_COUNT as usize + 1];
    let mut body: Value =
        serde_json::from_slice(&encode_inspect_response(&sample_inspection())).expect("json");
    body["result"]["pages"] = Value::Array(many_pages);
    body["result"]["pageCount"] = json!(MAX_PAGE_COUNT + 1);
    refused(body.to_string().into_bytes());
    // Document-info strings.
    refused(inspect_body_with("title", json!(5)));
    refused(inspect_body_with("title", json!("")));
    refused(inspect_body_with("title", json!(" padded ")));
    refused(inspect_body_with(
        "producer",
        json!("x".repeat(MAX_INSPECT_METADATA_CHARS + 1)),
    ));
    refused(inspect_body_with("creator", json!("two  spaces")));
    // Attachments.
    let attachment = |name: Value, size: Value, content: Value| json!([{"name": name, "byteSize": size, "content": content}]);
    refused(inspect_body_with("attachments", json!("x")));
    refused(inspect_body_with("attachments", json!([5])));
    refused(inspect_body_with(
        "attachments",
        attachment(json!("a\u{0}b"), json!(0), Value::Null),
    ));
    refused(inspect_body_with(
        "attachments",
        attachment(
            json!("n".repeat(MAX_INSPECT_ATTACHMENT_NAME_CHARS + 1)),
            json!(0),
            Value::Null,
        ),
    ));
    refused(inspect_body_with(
        "attachments",
        attachment(json!("a"), json!(-1), Value::Null),
    ));
    refused(inspect_body_with(
        "attachments",
        attachment(json!("a"), json!(1), json!("zz")),
    ));
    refused(inspect_body_with(
        "attachments",
        attachment(json!("a"), json!(2), json!("00")),
    ));
    refused(inspect_body_with(
        "attachments",
        attachment(json!("a"), json!(1), json!(7)),
    ));
    let oversized = MAX_ATTACHMENT_BYTES + 1;
    refused(inspect_body_with(
        "attachments",
        attachment(
            json!("a"),
            json!(oversized),
            json!("00".repeat(usize::try_from(oversized).expect("size"))),
        ),
    ));
    let too_many: Vec<Value> = (0..=MAX_INSPECT_ATTACHMENTS)
        .map(|_| json!({"name": "a", "byteSize": 0, "content": null}))
        .collect();
    refused(inspect_body_with("attachments", Value::Array(too_many)));
    // The sum of the returned content is capped.
    let full = "00".repeat(usize::try_from(MAX_ATTACHMENT_BYTES).expect("size"));
    let over_total: Vec<Value> = (0..5)
        .map(|_| json!({"name": "a", "byteSize": MAX_ATTACHMENT_BYTES, "content": full}))
        .collect();
    refused(inspect_body_with("attachments", Value::Array(over_total)));
    // The truncation flag must be a boolean and agree with a full list.
    refused(inspect_body_with("attachmentsTruncated", json!("yes")));
    refused(inspect_body_with("attachmentsTruncated", json!(true)));
}

#[test]
fn a_full_attachment_list_with_content_stays_under_the_worker_output_cap() {
    let content = vec![0xAB_u8; usize::try_from(MAX_ATTACHMENT_BYTES).expect("size")];
    let inspection = PdfInspection {
        page_count: MAX_PAGE_COUNT,
        pages: vec![
            PdfPageBox {
                width_centipoints: u32::MAX,
                height_centipoints: u32::MAX
            };
            MAX_PAGE_COUNT as usize
        ],
        title: Some("t".repeat(MAX_INSPECT_METADATA_CHARS)),
        producer: Some("p".repeat(MAX_INSPECT_METADATA_CHARS)),
        creator: Some("c".repeat(MAX_INSPECT_METADATA_CHARS)),
        attachments: (0..MAX_INSPECT_ATTACHMENTS)
            .map(|index| PdfAttachmentInfo {
                name: "n".repeat(MAX_INSPECT_ATTACHMENT_NAME_CHARS),
                byte_size: MAX_ATTACHMENT_BYTES,
                content: (index < 4).then(|| content.clone()),
            })
            .collect(),
        attachments_truncated: true,
    };
    let body = encode_inspect_response(&inspection);
    assert!(body.len() < 10 * 1024 * 1024, "{} bytes", body.len());
    assert_eq!(decode_inspect_response(&body), Ok(inspection));
}
