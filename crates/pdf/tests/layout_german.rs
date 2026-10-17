//! German A4 layout (sanitizer and binary32 layout) against the golden vectors.
//!
//! The vectors were recorded on synthetic inputs only. Every float is compared by its binary32 bit pattern.

use jobniac_pdf::PdfError;
use jobniac_pdf::layout::german::{
    AdditionalEntry, BOTTOM_Y, Block, Builder, CONTENT_WIDTH, Document, EducationEntry,
    ExperienceEntry, GermanCoverLetterContent, Kind, LanguageEntry, MARGIN_X, MIN_LAST_PAGE_USAGE,
    PAGE_CAPACITY, PAGE_HEIGHT, PAGE_WIDTH, PersonalField, RenderLine, SkillGroup,
    StructuredResumeContent, Style, TOP_Y, build_cover_letter_layout, build_resume_layout,
    sanitize_german_pdf_text, style_of,
};
use serde_json::Value;

const SOURCE: &str = include_str!("golden/german-pdf-source.json");
const RESUME_SOURCE: &str = include_str!("golden/german-resume-source.json");

fn document_json(text: &str) -> Value {
    serde_json::from_str(text).expect("golden JSON parses")
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key]
        .as_array()
        .unwrap_or_else(|| panic!("missing array {key}"))
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string {key} in {value}"))
}

fn bits(value: &Value, key: &str) -> u32 {
    u32::from_str_radix(text(value, key), 16).expect("binary32 hex")
}

fn document_of(name: &str) -> Document {
    match name {
        "resume" => Document::Resume,
        "cover-letter" => Document::CoverLetter,
        other => panic!("unknown document {other}"),
    }
}

fn kind_of(name: &str) -> Kind {
    Kind::ALL
        .into_iter()
        .find(|kind| kind.name() == name)
        .unwrap_or_else(|| panic!("unknown style {name}"))
}

fn style(document: Document, name: &str) -> Style {
    style_of(document, kind_of(name))
        .unwrap_or_else(|| panic!("{name} is not a {document:?} style"))
}

fn static_field(name: &str) -> &'static str {
    match name {
        "projectUrl" => "projectUrl",
        "contactUrl" => "contactUrl",
        other => panic!("unknown field {other}"),
    }
}

fn assert_line(actual: &RenderLine, expected: &Value) {
    assert_eq!(actual.text, text(expected, "text"), "row text");
    assert_eq!(
        actual.style.kind.name(),
        text(expected, "style"),
        "row style"
    );
    assert_eq!(
        actual.indent.to_bits(),
        bits(expected, "indent_bits"),
        "indent of {:?}",
        actual.text
    );
    assert_eq!(
        actual.height.to_bits(),
        bits(expected, "height_bits"),
        "height of {:?}",
        actual.text
    );
    assert_eq!(
        actual.font_size.to_bits(),
        bits(expected, "font_size_bits"),
        "font size of {:?}",
        actual.text
    );
}

/// The validation error envelope (`validation_error` code).
fn assert_validation(error: &PdfError, expected: &Value) {
    assert_eq!(expected["code"], "validation_error");
    assert_eq!(error.message(), text(expected, "message"));
    let (field, reason) = match error {
        PdfError::Validation { field, reason, .. } => (*field, *reason),
        other => panic!("expected a validation error, got {other:?}"),
    };
    let details = &expected["details"];
    assert_eq!(field, details.get("field").and_then(Value::as_str), "field");
    assert_eq!(
        reason,
        details.get("reason").and_then(Value::as_str),
        "reason"
    );
}

fn personal_fields(value: &Value) -> Vec<PersonalField> {
    array(value, "personal_fields")
        .iter()
        .map(|field| PersonalField {
            label: text(field, "label").to_owned(),
            value: text(field, "value").to_owned(),
        })
        .collect()
}

fn optional(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::to_owned)
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    array(value, key)
        .iter()
        .map(|item| item.as_str().expect("string item").to_owned())
        .collect()
}

fn resume_content(value: &Value) -> StructuredResumeContent {
    StructuredResumeContent {
        full_name: text(value, "full_name").to_owned(),
        language: text(value, "language").to_owned(),
        summary: optional(value, "summary"),
        personal_fields: personal_fields(value),
        experiences: array(value, "experiences")
            .iter()
            .map(|entry| ExperienceEntry {
                title: text(entry, "title").to_owned(),
                company: text(entry, "company").to_owned(),
                location: optional(entry, "location"),
                start_date: optional(entry, "start_date"),
                end_date: optional(entry, "end_date"),
                bullets: strings(entry, "bullets"),
            })
            .collect(),
        additional: array(value, "additional")
            .iter()
            .map(|entry| AdditionalEntry {
                title: text(entry, "title").to_owned(),
                organization: optional(entry, "organization"),
                bullets: strings(entry, "bullets"),
            })
            .collect(),
        education: array(value, "education")
            .iter()
            .map(|entry| EducationEntry {
                degree: text(entry, "degree").to_owned(),
                institution: text(entry, "institution").to_owned(),
                location: optional(entry, "location"),
                start_date: optional(entry, "start_date"),
                end_date: optional(entry, "end_date"),
                bullets: strings(entry, "bullets"),
            })
            .collect(),
        skill_groups: array(value, "skill_groups")
            .iter()
            .map(|group| SkillGroup {
                category: text(group, "category").to_owned(),
                skills: strings(group, "skills"),
            })
            .collect(),
        languages: array(value, "languages")
            .iter()
            .map(|row| LanguageEntry {
                language: text(row, "language").to_owned(),
                proficiency: optional(row, "proficiency"),
            })
            .collect(),
    }
}

fn cover_content(value: &Value) -> GermanCoverLetterContent {
    GermanCoverLetterContent {
        sender_name: text(value, "sender_name").to_owned(),
        personal_fields: personal_fields(value),
        recipient_company: optional(value, "recipient_company"),
        recipient_location: optional(value, "recipient_location"),
        subject: text(value, "subject").to_owned(),
        salutation: text(value, "salutation").to_owned(),
        paragraphs: strings(value, "paragraphs"),
        closing: text(value, "closing").to_owned(),
        signature: text(value, "signature").to_owned(),
    }
}

fn spacer_block(heights: &[u32], index: usize) -> Block {
    let spacer = style_of(Document::Resume, Kind::Spacer).expect("spacer");
    Block::new(
        heights
            .iter()
            .enumerate()
            .map(|(line, height)| RenderLine {
                // The block index travels in the first row so that pages can name their blocks.
                text: if line == 0 {
                    index.to_string()
                } else {
                    String::new()
                },
                style: spacer,
                indent: 0.0,
                height: f32::from_bits(*height),
                font_size: 0.0,
            })
            .collect(),
    )
}

#[test]
fn constants_are_the_binary32_values() {
    let records = array(&document_json(SOURCE), "constants").to_vec();
    assert_eq!(records.len(), 8);
    for record in records {
        let value = match text(&record, "name") {
            "PAGE_WIDTH" => PAGE_WIDTH as f32,
            "PAGE_HEIGHT" => PAGE_HEIGHT as f32,
            "MARGIN_X" => MARGIN_X as f32,
            "TOP_Y" => TOP_Y,
            "BOTTOM_Y" => BOTTOM_Y,
            "CONTENT_WIDTH" => CONTENT_WIDTH,
            "PAGE_CAPACITY" => PAGE_CAPACITY,
            "MIN_LAST_PAGE_USAGE" => MIN_LAST_PAGE_USAGE,
            other => panic!("unknown constant {other}"),
        };
        assert_eq!(
            value.to_bits(),
            bits(&record, "bits"),
            "{}",
            text(&record, "name")
        );
    }
}

#[test]
fn sanitizer_matches_the_golden_vectors() {
    let records = array(&document_json(SOURCE), "sanitize").to_vec();
    assert_eq!(records.len(), 19);
    for record in records {
        assert_eq!(
            sanitize_german_pdf_text(text(&record, "input")),
            text(&record, "output"),
            "input {:?}",
            text(&record, "input")
        );
    }
}

#[test]
fn wrapping_matches_the_golden_vectors_row_for_row() {
    let records = array(&document_json(SOURCE), "wrap").to_vec();
    assert_eq!(records.len(), 77);
    for record in records {
        let document = document_of(text(&record, "document"));
        let builder = Builder::new(document);
        let indent = f32::from_bits(bits(&record, "indent_bits"));
        let actual = builder
            .wrap(
                text(&record, "text"),
                style(document, text(&record, "style")),
                indent,
            )
            .expect("wrap");
        let expected = array(&record, "lines");
        assert_eq!(
            actual.len(),
            expected.len(),
            "row count of {:?}",
            text(&record, "text")
        );
        for (row, expected_row) in actual.iter().zip(expected) {
            assert_line(row, expected_row);
        }
    }
}

#[test]
fn fitted_link_lines_shrink_or_refuse_like_the_golden_vectors() {
    let records = array(&document_json(SOURCE), "fit").to_vec();
    assert_eq!(records.len(), 12);
    for record in records {
        let document = document_of(text(&record, "document"));
        let field = static_field(text(&record, "field"));
        let result = Builder::new(document).fitted_link_line(
            text(&record, "text"),
            style(document, text(&record, "style")),
            field,
        );
        if record["ok"] == true {
            let row = result.expect("fitted row");
            assert_eq!(row.text, text(&record, "text_out"));
            assert_eq!(row.font_size.to_bits(), bits(&record, "size_bits"));
        } else {
            assert_validation(&result.expect_err("refused link"), &record["error"]);
        }
    }
}

#[test]
fn pagination_matches_the_golden_vectors() {
    let records = array(&document_json(SOURCE), "paginate").to_vec();
    assert_eq!(records.len(), 24);
    for record in records {
        let document = document_of(text(&record, "document"));
        let blocks: Vec<Block> = array(&record, "block_heights_bits")
            .iter()
            .enumerate()
            .map(|(index, heights)| {
                let parsed: Vec<u32> = heights
                    .as_array()
                    .expect("heights")
                    .iter()
                    .map(|hex| u32::from_str_radix(hex.as_str().expect("hex"), 16).expect("bits"))
                    .collect();
                spacer_block(&parsed, index)
            })
            .collect();
        let result = Builder::new(document).paginate(blocks);
        if record["ok"] == true {
            let pages = result.expect("paginated");
            let expected = array(&record, "pages");
            assert_eq!(pages.len(), expected.len(), "page count");
            for (page, expected_page) in pages.iter().zip(expected) {
                let indices: Vec<u64> = page
                    .blocks
                    .iter()
                    .map(|block| block.lines[0].text.parse().expect("block index"))
                    .collect();
                let expected_indices: Vec<u64> = expected_page["blocks"]
                    .as_array()
                    .expect("indices")
                    .iter()
                    .map(|index| index.as_u64().expect("index"))
                    .collect();
                assert_eq!(indices, expected_indices, "block order");
                assert_eq!(page.used_height.to_bits(), bits(expected_page, "used_bits"));
            }
        } else {
            assert_validation(&result.expect_err("refused pagination"), &record["error"]);
        }
    }
}

#[test]
fn widths_match_the_golden_vectors() {
    let records = array(&document_json(SOURCE), "width").to_vec();
    assert_eq!(records.len(), 6);
    for record in records {
        let document = document_of(text(&record, "document"));
        let width = Builder::new(document)
            .width(
                text(&record, "text"),
                style(document, text(&record, "style")),
            )
            .expect("width");
        assert_eq!(
            width.to_bits(),
            bits(&record, "width_bits"),
            "{:?}",
            text(&record, "text")
        );
    }
}

#[test]
fn contact_headers_match_the_golden_vectors() {
    let records = array(&document_json(SOURCE), "contact_header").to_vec();
    assert_eq!(records.len(), 8);
    for record in records {
        let document = document_of(text(&record, "document"));
        let fields: Vec<PersonalField> = array(&record, "personal_fields")
            .iter()
            .map(|field| PersonalField {
                label: text(field, "label").to_owned(),
                value: text(field, "value").to_owned(),
            })
            .collect();
        let block = Builder::new(document)
            .header(text(&record, "name"), &fields)
            .expect("header");
        let expected = array(&record, "lines");
        assert_eq!(block.lines.len(), expected.len(), "header rows");
        for (row, expected_row) in block.lines.iter().zip(expected) {
            assert_line(row, expected_row);
        }
        assert_eq!(block.height().to_bits(), bits(&record, "height_bits"));
    }
}

#[test]
fn block_heights_use_the_exact_sum() {
    let records = array(&document_json(SOURCE), "block_height").to_vec();
    assert_eq!(records.len(), 8);
    for record in records {
        let heights: Vec<u32> = array(&record, "heights_bits")
            .iter()
            .map(|hex| u32::from_str_radix(hex.as_str().expect("hex"), 16).expect("bits"))
            .collect();
        let block = spacer_block(&heights, 0);
        assert_eq!(
            block.height().to_bits(),
            bits(&record, "height_bits"),
            "{heights:?}"
        );
    }
}

#[test]
fn full_layouts_match_the_golden_vectors() {
    let records = array(&document_json(RESUME_SOURCE), "records").to_vec();
    assert_eq!(records.len(), 27);
    for record in records {
        let document = document_of(text(&record, "document"));
        let result = match document {
            Document::Resume => build_resume_layout(&resume_content(&record["content"])),
            Document::CoverLetter => build_cover_letter_layout(&cover_content(&record["content"])),
        };
        if record["ok"] == true {
            let layout = result.expect("layout");
            assert_eq!(
                layout.german_footer,
                record["german_footer"].as_bool().expect("flag")
            );
            let pages = array(&record, "pages");
            assert_eq!(layout.pages.len(), pages.len(), "page count");
            for (page, expected_page) in layout.pages.iter().zip(pages) {
                assert_eq!(page.used_height.to_bits(), bits(expected_page, "used_bits"));
                let blocks = array(expected_page, "blocks");
                assert_eq!(page.blocks.len(), blocks.len(), "block count");
                for (block, expected_block) in page.blocks.iter().zip(blocks) {
                    let expected_rows = expected_block.as_array().expect("rows");
                    assert_eq!(block.lines.len(), expected_rows.len(), "row count");
                    for (row, expected_row) in block.lines.iter().zip(expected_rows) {
                        assert_line(row, expected_row);
                    }
                }
            }
        } else {
            assert_validation(&result.expect_err("refused layout"), &record["error"]);
        }
    }
}

#[test]
fn record_counts_are_the_documented_golden_totals() {
    let source = document_json(SOURCE);
    let total: usize = [
        "constants",
        "sanitize",
        "wrap",
        "fit",
        "paginate",
        "width",
        "contact_header",
        "block_height",
    ]
    .iter()
    .map(|key| array(&source, key).len())
    .sum();
    assert_eq!(total, 162);
}
