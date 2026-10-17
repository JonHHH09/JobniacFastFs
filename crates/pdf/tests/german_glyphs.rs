//! German glyph round trip: the 218 dedicated glyphs encode, render and read back.
//!
//! The glyph vectors hold every glyph in code points 32 to 126 and 160 to 255, the 26
//! Windows-1252 extras and U+2022. The rendered document is read back with `lopdf`, and the
//! bytes of its text operators are compared with the glyph bytes.

use jobniac_pdf::layout::german::{GermanCoverLetterContent, PersonalField};
use jobniac_pdf::metrics::{encode_german_font_text, german_font_text_width};
use jobniac_pdf::render::german::render_cover_letter_pdf;
use jobniac_pdf::render::primitives::literal;
use lopdf::content::Content;
use lopdf::{Document, Object};
use serde_json::Value;

const GLYPHS: &str = include_str!("golden/german_glyphs.json");

fn glyph_records() -> Vec<Value> {
    let document: Value = serde_json::from_str(GLYPHS).expect("glyph JSON parses");
    document["glyphs"].as_array().expect("glyphs array").clone()
}

fn character_of(record: &Value) -> char {
    let codepoint = record["codepoint"].as_u64().expect("codepoint");
    char::from_u32(u32::try_from(codepoint).expect("code point")).expect("scalar value")
}

fn bits(record: &Value, key: &str) -> u32 {
    u32::from_str_radix(record[key].as_str().expect("hex"), 16).expect("binary32 hex")
}

fn hex_bytes(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex byte"))
        .collect()
}

/// The bytes of every `Tj` operand on a page, in content order.
fn text_bytes(document: &Document, page: lopdf::ObjectId) -> (Vec<u8>, Vec<u8>) {
    let content = document.get_page_content(page);
    let operations = Content::decode(&content)
        .expect("content decodes")
        .operations;
    let mut shown = Vec::new();
    for operation in operations {
        if operation.operator == "Tj"
            && let Some(Object::String(bytes, _)) = operation.operands.first()
        {
            shown.extend_from_slice(bytes);
        }
    }
    (shown, content)
}

#[test]
fn every_glyph_matches_the_encoding_literal_and_widths() {
    let records = glyph_records();
    assert_eq!(records.len(), 218);
    for record in &records {
        let character = character_of(record);
        let text = character.to_string();
        let encoded = encode_german_font_text(&text).expect("admitted glyph");
        assert_eq!(
            encoded,
            hex_bytes(record["bytes"].as_str().expect("bytes")),
            "{character:?}"
        );
        assert_eq!(
            literal(&encoded),
            record["literal"].as_str().expect("literal"),
            "literal of {character:?}"
        );
        let normal = german_font_text_width(&text, false, 1000.0).expect("width");
        let bold = german_font_text_width(&text, true, 1000.0).expect("width");
        assert_eq!(
            normal.to_bits(),
            bits(record, "width_normal_bits"),
            "{character:?}"
        );
        assert_eq!(
            bold.to_bits(),
            bits(record, "width_bold_bits"),
            "{character:?}"
        );
    }
}

#[test]
fn all_glyphs_render_and_read_back_in_order() {
    let glyphs: Vec<char> = glyph_records()
        .iter()
        .map(character_of)
        .filter(|character| *character != ' ')
        .collect();
    assert_eq!(
        glyphs.len(),
        217,
        "every glyph but the plain space forms a paragraph"
    );

    // Four paragraphs without ASCII spaces, so each keeps every glyph in one row run.
    let chunk = glyphs.len().div_ceil(4);
    let paragraphs: Vec<String> = glyphs
        .chunks(chunk)
        .map(|part| part.iter().collect())
        .collect();
    let mut expected = Vec::new();
    for paragraph in &paragraphs {
        expected.extend(encode_german_font_text(paragraph).expect("admitted paragraph"));
    }

    let content = GermanCoverLetterContent {
        sender_name: "Ada Example".to_owned(),
        personal_fields: vec![PersonalField {
            label: "E-Mail".to_owned(),
            value: "ada@example.com".to_owned(),
        }],
        recipient_company: None,
        recipient_location: None,
        subject: "Bewerbung".to_owned(),
        salutation: "Sehr geehrte Damen und Herren,".to_owned(),
        paragraphs,
        closing: "Mit freundlichen Grüßen".to_owned(),
        signature: "Ada Example".to_owned(),
    };
    let rendered = render_cover_letter_pdf(&content).expect("German glyphs render");
    assert_eq!(rendered.page_count, 1);

    let document = Document::load_mem(&rendered.data).expect("rendered PDF reads back");
    let pages = document.get_pages();
    assert_eq!(pages.len(), 1);
    let page = *pages.values().next().expect("one page");
    let (shown, content_bytes) = text_bytes(&document, page);

    assert!(
        shown
            .windows(expected.len())
            .any(|window| window == expected.as_slice()),
        "the glyph bytes appear in order in the text operators"
    );
    assert!(
        shown
            .windows(b"Seite 1 / 1".len())
            .any(|window| window == b"Seite 1 / 1")
    );
    // Bytes 160 to 255 are octal escapes, 32 to 126 stay literal, the bullet is 0x95.
    assert!(content_bytes.windows(4).any(|window| window == b"\\240"));
    assert!(content_bytes.windows(4).any(|window| window == b"\\377"));
    assert!(content_bytes.windows(4).any(|window| window == b"\\225"));
    assert!(content_bytes.windows(4).any(|window| window == b"\\200"));
}
