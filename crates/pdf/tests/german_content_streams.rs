//! German page content streams compared byte for byte with the golden streams.
//!
//! `tests/golden/german-content-streams.json` records the raw content stream of every page for the
//! 27 synthetic sources of `german-resume-source.json` (row baselines, section and divider rules,
//! bullets and footers). The streams are read back from the rendered PDFs with `lopdf`, so the test
//! covers the bytes the renderer writes.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use jobniac_pdf::PdfError;
use jobniac_pdf::layout::german::{
    AdditionalEntry, EducationEntry, ExperienceEntry, GermanCoverLetterContent, LanguageEntry,
    PersonalField, SkillGroup, StructuredResumeContent,
};
use jobniac_pdf::render::german::{render_cover_letter_pdf, render_resume_pdf};
use lopdf::Document;
use serde_json::Value;

const STREAMS: &str = include_str!("golden/german-content-streams.json");
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

fn optional(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::to_owned)
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    array(value, key)
        .iter()
        .map(|item| item.as_str().expect("string item").to_owned())
        .collect()
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

/// The raw, decoded content stream of every page, in page order.
///
/// `get_page_content` would append a newline after each stream, so the stream object is read
/// directly. Each page must hold exactly one content stream.
fn page_streams(pdf: &[u8]) -> Vec<Vec<u8>> {
    let document = Document::load_mem(pdf).expect("PDF parses");
    document
        .get_pages()
        .values()
        .map(|&page| {
            let streams = document.get_page_contents(page);
            assert_eq!(streams.len(), 1, "one content stream per page");
            let stream = document
                .get_object(streams[0])
                .and_then(|object| object.as_stream())
                .expect("content stream object");
            stream
                .decompressed_content()
                .expect("content stream decodes")
        })
        .collect()
}

/// The validation or internal error envelope (`code`, `message`, `details`).
fn assert_error(error: &PdfError, expected: &Value) {
    assert_eq!(error.message(), text(expected, "message"), "message");
    let details = &expected["details"];
    match error {
        PdfError::Validation { field, reason, .. } => {
            assert_eq!(expected["code"], "validation_error", "code");
            assert_eq!(
                *field,
                details.get("field").and_then(Value::as_str),
                "field"
            );
            assert_eq!(
                *reason,
                details.get("reason").and_then(Value::as_str),
                "reason"
            );
        }
        PdfError::Internal => assert_eq!(expected["code"], "internal_error", "code"),
    }
}

#[test]
fn page_content_streams_match_the_golden_streams_byte_for_byte() {
    let records = array(&document_json(STREAMS), "records").to_vec();
    let sources = array(&document_json(RESUME_SOURCE), "records").to_vec();
    assert_eq!(records.len(), 27, "golden record count");
    assert_eq!(sources.len(), 27, "source record count");
    let mut compared_pages = 0;
    for (record, source) in records.iter().zip(&sources) {
        let document = text(record, "document");
        let label = format!("{document} #{}", record["index"]);
        assert_eq!(document, text(source, "document"), "{label}: source order");
        assert_eq!(record["ok"], source["ok"], "{label}: outcome");
        let rendered = match document {
            "resume" => render_resume_pdf(&resume_content(&source["content"])),
            "cover-letter" => render_cover_letter_pdf(&cover_content(&source["content"])),
            other => panic!("unknown document {other}"),
        };
        if record["ok"] == true {
            let output = rendered.unwrap_or_else(|error| panic!("{label}: {error:?}"));
            let wanted = array(record, "contentStreams");
            let actual = page_streams(&output.data);
            assert_eq!(actual.len(), wanted.len(), "{label}: page count");
            assert_eq!(output.page_count, wanted.len(), "{label}: page count");
            for (index, (left, right)) in actual.iter().zip(wanted).enumerate() {
                assert_eq!(
                    left.as_slice(),
                    right.as_str().expect("stream text").as_bytes(),
                    "{label}: content stream of page {}",
                    index + 1
                );
                compared_pages += 1;
            }
        } else {
            assert!(
                record.get("contentStreams").is_none(),
                "{label}: error rows carry no content streams"
            );
            match rendered {
                Ok(_) => panic!("{label}: Rust renders where the vectors expect a failure"),
                Err(error) => assert_error(&error, &record["error"]),
            }
        }
    }
    assert!(compared_pages > 0, "some pages were compared");
}
