//! Standard-14 encoding and widths against the golden vectors (synthetic inputs).
//! Widths are compared by IEEE bit pattern: the generic layout in `f64`, the German layout
//! in `f32`.

use jobniac_pdf::metrics::{
    FONT_ERROR_MESSAGE, encode_font_text, encode_german_font_text, font_text_width,
    german_font_text_width,
};
use serde_json::Value;

const VECTORS: &str = include_str!("golden/pdf-font.json");
const NOTICE: &str = include_str!("../src/metrics/NOTICE-Adobe-AFM.txt");

fn document() -> Value {
    serde_json::from_str(VECTORS).expect("pdf-font.json parses")
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

fn number(value: &Value, key: &str) -> u64 {
    value[key]
        .as_u64()
        .unwrap_or_else(|| panic!("missing integer {key} in {value}"))
}

fn flag(value: &Value, key: &str) -> bool {
    value[key]
        .as_bool()
        .unwrap_or_else(|| panic!("missing bool {key} in {value}"))
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    assert!(hex.len().is_multiple_of(2), "odd hex length in {hex}");
    (0..hex.len())
        .step_by(2)
        .map(|start| {
            u8::from_str_radix(&hex[start..start + 2], 16).expect("hex byte in golden vector")
        })
        .collect()
}

fn hex_u64(hex: &str) -> u64 {
    u64::from_str_radix(hex, 16).expect("hex bit pattern in golden vector")
}

fn hex_u32(hex: &str) -> u32 {
    u32::from_str_radix(hex, 16).expect("hex f32 bit pattern in golden vector")
}

fn character(codepoint: u64) -> char {
    char::from_u32(u32::try_from(codepoint).expect("code point fits u32"))
        .expect("valid code point in golden vector")
}

/// Outcome of one generic encode-and-measure call, as the vectors record it.
fn generic_result(text: &str, bold: bool, size: f64, generated: bool) -> Option<(Vec<u8>, u64)> {
    let bytes = encode_font_text(text, generated).ok()?;
    let width = font_text_width(text, bold, size, generated).ok()?;
    Some((bytes, width.to_bits()))
}

#[test]
fn golden_document_has_the_expected_sections() {
    let document = document();
    assert_eq!(array(&document, "glyphs").len(), 384, "glyph rows");
    assert_eq!(array(&document, "german_extras").len(), 26);
    assert!(array(&document, "admission").len() > 500);
    assert!(array(&document, "strings").len() > 500);
}

#[test]
fn glyph_rows_match_the_golden_vectors_bit_for_bit() {
    let document = document();
    for row in array(&document, "glyphs") {
        let codepoint = number(row, "codepoint");
        let bold = text(row, "face") == "helvetica-bold";
        let glyph = character(codepoint);

        let bytes = encode_font_text(&glyph.to_string(), true).expect("admitted glyph");
        assert_eq!(
            bytes,
            hex_bytes(text(row, "bytes")),
            "generic byte of U+{codepoint:04X}"
        );

        let units = font_text_width(&glyph.to_string(), bold, 1000.0, true).expect("width");
        assert_eq!(
            units,
            row["width_units"].as_f64().expect("units"),
            "units U+{codepoint:04X}"
        );

        let width = font_text_width(&glyph.to_string(), bold, 1.0, true).expect("width");
        assert_eq!(
            width.to_bits(),
            hex_u64(text(row, "width_bits")),
            "width bits U+{codepoint:04X}"
        );

        let german = encode_german_font_text(&glyph.to_string()).expect("German glyph");
        assert_eq!(
            german,
            hex_bytes(text(row, "german_bytes")),
            "German byte U+{codepoint:04X}"
        );

        let german_width = german_font_text_width(&glyph.to_string(), bold, 1.0).expect("width");
        assert_eq!(
            german_width.to_bits(),
            hex_u32(text(row, "german_width_bits")),
            "German width bits U+{codepoint:04X}"
        );
    }
}

#[test]
fn german_extra_glyphs_match_the_golden_vectors() {
    let document = document();
    for row in array(&document, "german_extras") {
        let glyph = character(number(row, "codepoint")).to_string();
        for (key, bold) in [("normal", false), ("bold", true)] {
            let expected = &row[key];
            let bytes = encode_german_font_text(&glyph).expect("German extra admitted");
            assert_eq!(
                bytes,
                hex_bytes(text(expected, "bytes")),
                "extra bytes {glyph}"
            );
            let width = german_font_text_width(&glyph, bold, 1000.0).expect("width");
            assert_eq!(
                width.to_bits(),
                hex_u32(text(expected, "width_bits")),
                "extra width {glyph}"
            );
        }
    }
}

#[test]
fn admission_matches_the_golden_vectors() {
    let document = document();
    for row in array(&document, "admission") {
        let codepoint = number(row, "codepoint");
        let glyph = character(codepoint).to_string();

        let generic = &row["generic"];
        match generic_result(&glyph, false, 1.0, false) {
            Some((bytes, bits)) => {
                assert!(flag(generic, "ok"), "generic admits U+{codepoint:04X}");
                assert_eq!(
                    bytes,
                    hex_bytes(text(generic, "bytes")),
                    "generic bytes U+{codepoint:04X}"
                );
                assert_eq!(
                    bits,
                    hex_u64(text(generic, "width_bits")),
                    "generic width U+{codepoint:04X}"
                );
            }
            None => assert!(!flag(generic, "ok"), "generic rejects U+{codepoint:04X}"),
        }

        let bullet = &row["generic_bullet"];
        match generic_result(&glyph, true, 1.0, true) {
            Some((bytes, bits)) => {
                assert!(
                    flag(bullet, "ok"),
                    "bullet permission admits U+{codepoint:04X}"
                );
                assert_eq!(bytes, hex_bytes(text(bullet, "bytes")));
                assert_eq!(bits, hex_u64(text(bullet, "width_bits")));
            }
            None => assert!(
                !flag(bullet, "ok"),
                "bullet permission rejects U+{codepoint:04X}"
            ),
        }

        let german = &row["german"];
        match (
            encode_german_font_text(&glyph),
            german_font_text_width(&glyph, false, 1.0),
        ) {
            (Ok(bytes), Ok(width)) => {
                assert!(flag(german, "ok"), "German admits U+{codepoint:04X}");
                assert_eq!(bytes, hex_bytes(text(german, "bytes")));
                assert_eq!(width.to_bits(), hex_u32(text(german, "width_bits")));
            }
            _ => assert!(!flag(german, "ok"), "German rejects U+{codepoint:04X}"),
        }
    }
}

#[test]
fn string_vectors_match_the_golden_vectors_bit_for_bit() {
    let document = document();
    for vector in array(&document, "strings") {
        let input = text(vector, "text");
        let bold = flag(vector, "bold");
        let size = vector["size"].as_f64().expect("size");
        let generated = flag(vector, "generated_bullet");

        let generic = &vector["generic"];
        match generic_result(input, bold, size, generated) {
            Some((bytes, bits)) => {
                assert!(flag(generic, "ok"), "generic accepts {input:?}");
                assert_eq!(
                    bytes,
                    hex_bytes(text(generic, "bytes")),
                    "generic bytes {input:?}"
                );
                assert_eq!(
                    bits,
                    hex_u64(text(generic, "width_bits")),
                    "generic width {input:?}"
                );
            }
            None => assert!(!flag(generic, "ok"), "generic rejects {input:?}"),
        }

        let german = &vector["german"];
        // The German layout takes an f32 size; the vectors record the double the caller passed.
        let size32 = size as f32;
        match (
            encode_german_font_text(input),
            german_font_text_width(input, bold, size32),
        ) {
            (Ok(bytes), Ok(width)) => {
                assert!(flag(german, "ok"), "German accepts {input:?}");
                assert_eq!(
                    bytes,
                    hex_bytes(text(german, "bytes")),
                    "German bytes {input:?}"
                );
                assert_eq!(
                    width.to_bits(),
                    hex_u32(text(german, "width_bits")),
                    "German width {input:?}"
                );
            }
            _ => assert!(!flag(german, "ok"), "German rejects {input:?}"),
        }
    }
}

#[test]
fn rejected_characters_use_the_font_error_message() {
    let error = encode_font_text("\u{0081}", false).expect_err("control code point is rejected");
    assert_eq!(error.to_string(), FONT_ERROR_MESSAGE);
    assert_eq!(
        FONT_ERROR_MESSAGE,
        "PDF text contains characters unsupported by the current font"
    );
    assert!(
        encode_font_text("\u{2022}", false).is_err(),
        "bullet needs permission"
    );
    assert_eq!(
        encode_font_text("\u{2022}", true).expect("bullet"),
        vec![0x95]
    );
}

#[test]
fn aliases_and_byte_mapping() {
    assert_eq!(
        encode_font_text("\u{00A0}\u{00AD}", false).expect("aliases"),
        vec![0xA0, 0xAD]
    );
    let space = font_text_width(" ", false, 1000.0, false).expect("space");
    let nbsp = font_text_width("\u{00A0}", false, 1000.0, false).expect("nbsp");
    assert_eq!(space.to_bits(), nbsp.to_bits(), "NBSP is measured as space");
    let hyphen = font_text_width("-", true, 1000.0, false).expect("hyphen");
    let soft = font_text_width("\u{00AD}", true, 1000.0, false).expect("soft hyphen");
    assert_eq!(
        hyphen.to_bits(),
        soft.to_bits(),
        "soft hyphen is measured as hyphen"
    );
    assert_eq!(
        encode_german_font_text("\u{20AC}").expect("euro"),
        vec![0x80]
    );
    assert_eq!(
        encode_german_font_text("\u{2022}").expect("bullet"),
        vec![0x95]
    );
}

#[test]
fn notice_file_carries_the_adobe_permission_text() {
    assert!(
        NOTICE.contains("Copyright (c) 1985, 1987, 1989, 1990, 1997 Adobe Systems Incorporated.")
    );
    assert!(
        NOTICE.contains("may be used, copied, and distributed for any purpose and without charge")
    );
}
