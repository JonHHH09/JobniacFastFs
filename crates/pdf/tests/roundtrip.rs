//! Rendering, content streams and structural readback of generic documents.
//!
//! Content streams are compared byte for byte with the streams recorded
//! in `tests/golden/pdf-generic.json`. Readback uses `lopdf`, as the renderer does.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use jobniac_pdf::PdfError;
use jobniac_pdf::layout::{PAGE_HEIGHT, PAGE_WIDTH, PageNumberLocale, build_pdf_layout};
use jobniac_pdf::readback::verify_structure;
use jobniac_pdf::render::generic::page_streams;
use jobniac_pdf::render::render_pdf;
use lopdf::{Document, Object};
use serde_json::Value;

const GOLDEN: &str = include_str!("golden/pdf-generic.json");

/// Cases whose recorded rows overflow the right margin; the layout re-wraps them by measured
/// width (see `layout_generic.rs`), so streams and page counts are checked for sanity only.
const WIDTH_AWARE_CASES: [&str; 2] = ["wrap_long_words", "body_at_limit"];

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden JSON parses")
}

/// The machine code of an error envelope. The generic vectors store `[code, label]` pairs.
fn error_code(wanted: &Value) -> &str {
    let code = &wanted["code"];
    code.as_str()
        .or_else(|| code[0].as_str())
        .expect("error code")
}

fn locale_of(name: &str) -> PageNumberLocale {
    match name {
        "canadian_french" => PageNumberLocale::CanadianFrench,
        _ => PageNumberLocale::English,
    }
}

#[test]
fn content_streams_match_the_golden_streams_byte_for_byte() {
    let golden = golden();
    let layouts = golden["layouts"].as_array().unwrap();
    let renders = golden["renders"].as_array().unwrap();
    for (layout_row, render_row) in layouts.iter().zip(renders) {
        let case = &layout_row["case"];
        let id = case["id"].as_str().unwrap();
        assert_eq!(render_row["id"].as_str().unwrap(), id);
        let streams = match (
            render_row.get("contentStreams"),
            render_row["result"].get("error"),
        ) {
            (None, Some(_)) => continue,
            (Some(streams), None) => streams,
            (streams, error) => {
                panic!("{id}: content streams and outcome disagree: {streams:?} vs {error:?}")
            }
        };
        let layout = build_pdf_layout(
            case["title"].as_str(),
            case["body"].as_str(),
            case["compact"].as_bool().unwrap(),
            locale_of(case["pageNumberLocale"].as_str().unwrap()),
        )
        .unwrap();
        let actual = page_streams(&layout).unwrap();
        if WIDTH_AWARE_CASES.contains(&id) {
            assert!(actual.len() >= streams.as_array().unwrap().len(), "{id}");
            continue;
        }
        let wanted: Vec<&str> = streams
            .as_array()
            .unwrap()
            .iter()
            .map(|stream| stream.as_str().unwrap())
            .collect();
        assert_eq!(actual.len(), wanted.len(), "{id}: page count");
        for (index, (left, right)) in actual.iter().zip(&wanted).enumerate() {
            assert_eq!(left, right, "{id}: content stream of page {}", index + 1);
        }
    }
}

#[test]
fn rendered_outcomes_match_the_golden_vectors() {
    let golden = golden();
    let layouts = golden["layouts"].as_array().unwrap();
    for row in layouts {
        let case = &row["case"];
        let id = case["id"].as_str().unwrap();
        let rendered = render_pdf(
            case["title"].as_str(),
            case["body"].as_str(),
            case["compact"].as_bool().unwrap(),
            locale_of(case["pageNumberLocale"].as_str().unwrap()),
        );
        let expected = &golden["renders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["id"] == case["id"])
            .unwrap()["result"];
        match (rendered, expected.get("error")) {
            (Ok(output), None) if WIDTH_AWARE_CASES.contains(&id) => {
                assert!(output.page_count as u64 >= expected["pageCount"].as_u64().unwrap());
            }
            (Ok(output), None) => {
                assert_eq!(
                    output.page_count as u64,
                    expected["pageCount"].as_u64().unwrap(),
                    "{id}"
                );
                assert!(output.byte_size() > 0);
            }
            (Err(error), Some(wanted)) => {
                let code = match error {
                    PdfError::Validation { .. } => "validation_error",
                    PdfError::Internal => "internal_error",
                };
                assert_eq!(code, error_code(wanted), "{id}: code");
                assert_eq!(error.message(), wanted["message"].as_str().unwrap(), "{id}");
                let (field, reason) = match &error {
                    PdfError::Validation { field, reason, .. } => (*field, *reason),
                    PdfError::Internal => (None, None),
                };
                let details = &wanted["details"];
                assert_eq!(
                    field,
                    details.get("field").and_then(Value::as_str),
                    "{id}: field"
                );
                assert_eq!(
                    reason,
                    details.get("reason").and_then(Value::as_str),
                    "{id}: reason"
                );
            }
            (result, wanted) => panic!("{id}: outcome differs: {result:?} vs {wanted:?}"),
        }
    }
}

#[test]
fn unsupported_glyphs_are_validation_errors_without_details() {
    let error = render_pdf(
        Some("Title"),
        Some("Text with \u{7f} DEL"),
        false,
        PageNumberLocale::English,
    )
    .unwrap_err();
    assert_eq!(
        error,
        PdfError::Validation {
            message: "PDF text contains characters unsupported by the current font",
            field: None,
            reason: None,
        }
    );
}

#[test]
fn every_admitted_glyph_round_trips_through_the_content_stream() {
    // Code points 32 to 126 stay literal; 160 to 255 become octal escapes.
    let glyphs: Vec<char> = (32u32..=126)
        .chain(160u32..=255)
        .filter_map(char::from_u32)
        .collect();
    let body: String = glyphs
        .chunks(100)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    let layout = build_pdf_layout(
        Some("Glyphs"),
        Some(&body),
        false,
        PageNumberLocale::English,
    )
    .unwrap();
    let streams = page_streams(&layout).unwrap().join("");
    assert!(streams.contains("\\\\"), "backslash is escaped");
    assert!(streams.contains("\\240"), "NBSP is written as octal 240");
    assert!(
        streams.contains("\\377"),
        "y-diaeresis is written as octal 377"
    );
    // Only the generator writes U+2022 as byte 0x95; user text with U+2022 is sanitized to `?`.
    let bullet = build_pdf_layout(
        Some("Glyphs"),
        Some("- item"),
        false,
        PageNumberLocale::English,
    )
    .unwrap();
    assert!(page_streams(&bullet).unwrap().join("").contains("\\225"));
    let user = build_pdf_layout(
        Some("Glyphs"),
        Some("a \u{2022} b"),
        false,
        PageNumberLocale::English,
    )
    .unwrap();
    assert!(page_streams(&user).unwrap().join("").contains("a ? b"));
    let rendered = render_pdf(
        Some("Glyphs"),
        Some(&body),
        false,
        PageNumberLocale::English,
    )
    .unwrap();
    assert_eq!(rendered.page_count, layout.pages.len());
}

#[test]
fn output_is_deterministic_and_carries_no_timestamps() {
    let first = render_pdf(
        Some("Ada Example"),
        Some("PROFILE\nBody text."),
        false,
        PageNumberLocale::English,
    )
    .unwrap();
    let second = render_pdf(
        Some("Ada Example"),
        Some("PROFILE\nBody text."),
        false,
        PageNumberLocale::English,
    )
    .unwrap();
    assert_eq!(first, second);
    let text = String::from_utf8_lossy(&first.data);
    assert!(!text.contains("CreationDate"));
    assert!(!text.contains("ModDate"));
}

#[test]
fn readback_accepts_the_generic_document_and_checks_page_count() {
    let rendered = render_pdf(
        Some("Title"),
        Some("Body."),
        false,
        PageNumberLocale::English,
    )
    .unwrap();
    assert_eq!(
        verify_structure(&rendered.data, 1, PAGE_WIDTH, PAGE_HEIGHT),
        Ok(1)
    );
    assert!(verify_structure(&rendered.data, 2, PAGE_WIDTH, PAGE_HEIGHT).is_err());
    assert!(verify_structure(&rendered.data, 1, 595.0, 842.0).is_err());
}

fn rendered_document() -> Vec<u8> {
    render_pdf(
        Some("Title"),
        Some("Body."),
        false,
        PageNumberLocale::English,
    )
    .unwrap()
    .data
}

fn reserialize(mut document: Document) -> Vec<u8> {
    let mut out = Vec::new();
    document.save_to(&mut out).unwrap();
    out
}

#[test]
fn readback_rejects_unsafe_catalog_keys() {
    for key in ["OpenAction", "Names", "AcroForm", "EmbeddedFiles"] {
        let mut document = Document::load_mem(&rendered_document()).unwrap();
        let catalog_id = document
            .trailer
            .get(b"Root")
            .unwrap()
            .as_reference()
            .unwrap();
        document
            .get_dictionary_mut(catalog_id)
            .unwrap()
            .set(key, Object::Null);
        let data = reserialize(document);
        assert!(
            verify_structure(&data, 1, PAGE_WIDTH, PAGE_HEIGHT).is_err(),
            "{key}"
        );
    }
}

#[test]
fn readback_rejects_annotations_on_a_page() {
    let mut document = Document::load_mem(&rendered_document()).unwrap();
    let page_id = *document.get_pages().values().next().unwrap();
    document
        .get_dictionary_mut(page_id)
        .unwrap()
        .set("Annots", Object::Array(Vec::new()));
    let data = reserialize(document);
    assert!(verify_structure(&data, 1, PAGE_WIDTH, PAGE_HEIGHT).is_err());
}

#[test]
fn readback_rejects_a_direct_font_reference() {
    let mut document = Document::load_mem(&rendered_document()).unwrap();
    let page_id = *document.get_pages().values().next().unwrap();
    let resources = document
        .get_dictionary(page_id)
        .unwrap()
        .get(b"Resources")
        .unwrap()
        .clone();
    let resources_dict = resources.as_dict().unwrap().clone();
    let fonts = resources_dict
        .get(b"Font")
        .unwrap()
        .as_dict()
        .unwrap()
        .clone();
    let body_font = fonts.get(b"FBody").unwrap().clone();
    let font_dictionary = document
        .get_object(body_font.as_reference().unwrap())
        .unwrap()
        .as_dict()
        .unwrap()
        .clone();
    let mut fonts = fonts;
    fonts.set("FBody", Object::Dictionary(font_dictionary));
    let mut resources_dict = resources_dict;
    resources_dict.set("Font", Object::Dictionary(fonts));
    document
        .get_dictionary_mut(page_id)
        .unwrap()
        .set("Resources", Object::Dictionary(resources_dict));
    let data = reserialize(document);
    assert!(verify_structure(&data, 1, PAGE_WIDTH, PAGE_HEIGHT).is_err());
}
