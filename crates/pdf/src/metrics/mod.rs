//! Standard-14 Helvetica metrics and the qualified WinAnsi glyph admission.
//!
//! The width tables are generated from the Adobe Core14 AFM files (see `tables.rs`, the notice in
//! and `NOTICE-Adobe-AFM.txt`). Two layouts share the tables:
//! the generic layout measures in double precision, and the German layout measures with `f32`
//! rounding after every addition, in source order.

mod tables;

use std::fmt;

use tables::{GERMAN_EXTRA_WIDTHS, HELVETICA_BOLD_WIDTHS, HELVETICA_WIDTHS};

/// Message of the unsupported-character error.
pub const FONT_ERROR_MESSAGE: &str = "PDF text contains characters unsupported by the current font";

/// The generated bullet U+2022, admitted only where the caller grants generated-bullet permission
/// (the generic layout) and always admitted by the German encoding.
pub const GENERATED_BULLET: char = '\u{2022}';

/// Byte the generated bullet encodes to in WinAnsi.
const GENERATED_BULLET_BYTE: u8 = 0x95;

/// Index of the generated bullet in the width tables.
const GENERATED_BULLET_INDEX: usize = 191;

/// Windows-1252 bytes of the German extra glyphs, sorted by code point.
const GERMAN_EXTRA_BYTES: [(u32, u8); 26] = [
    (338, 0x8C),
    (339, 0x9C),
    (352, 0x8A),
    (353, 0x9A),
    (376, 0x9F),
    (381, 0x8E),
    (382, 0x9E),
    (402, 0x83),
    (710, 0x88),
    (732, 0x98),
    (8211, 0x96),
    (8212, 0x97),
    (8216, 0x91),
    (8217, 0x92),
    (8218, 0x82),
    (8220, 0x93),
    (8221, 0x94),
    (8222, 0x84),
    (8224, 0x86),
    (8225, 0x87),
    (8230, 0x85),
    (8240, 0x89),
    (8249, 0x8B),
    (8250, 0x9B),
    (8364, 0x80),
    (8482, 0x99),
];

/// A character that the qualified font encoding cannot represent.
///
/// Core maps this to the `validation_error` with [`FONT_ERROR_MESSAGE`] and empty details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedCharacter {
    /// The rejected character.
    pub character: char,
}

impl fmt::Display for UnsupportedCharacter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(FONT_ERROR_MESSAGE)
    }
}

impl std::error::Error for UnsupportedCharacter {}

/// Index of a character in the generic tables, or an error when the character is not admitted.
///
/// U+0020..=U+007E map to `cp - 32`, U+00A0..=U+00FF to `cp - 65`, and U+2022 to 191 only with
/// generated-bullet permission.
fn generic_index(character: char, generated_bullet: bool) -> Result<usize, UnsupportedCharacter> {
    let codepoint = u32::from(character);
    match codepoint {
        0x20..=0x7E => Ok((codepoint - 0x20) as usize),
        0xA0..=0xFF => Ok((codepoint - 65) as usize),
        0x2022 if generated_bullet => Ok(GENERATED_BULLET_INDEX),
        _ => Err(UnsupportedCharacter { character }),
    }
}

/// The German extra glyph (normal, bold) widths of a character, if it is one.
fn german_extra_width(character: char) -> Option<(u16, u16)> {
    let codepoint = u32::from(character);
    GERMAN_EXTRA_WIDTHS
        .binary_search_by_key(&codepoint, |&(code, _, _)| code)
        .ok()
        .map(|index| (GERMAN_EXTRA_WIDTHS[index].1, GERMAN_EXTRA_WIDTHS[index].2))
}

/// Round a double to the nearest `f32` and widen it back, as Java's float operations do.
fn round_f32(value: f64) -> f64 {
    f64::from(value as f32)
}

/// Encode text with the generic WinAnsi admission policy.
///
/// Each character must be printable ASCII (U+0020..=U+007E) or Latin-1 (U+00A0..=U+00FF), or the
/// generated bullet when `generated_bullet` is true. The generated bullet becomes byte `0x95`.
///
/// # Errors
///
/// Returns [`UnsupportedCharacter`] for the first character that is not admitted.
pub fn encode_font_text(
    text: &str,
    generated_bullet: bool,
) -> Result<Vec<u8>, UnsupportedCharacter> {
    text.chars()
        .map(|character| {
            let index = generic_index(character, generated_bullet)?;
            if index == GENERATED_BULLET_INDEX {
                Ok(GENERATED_BULLET_BYTE)
            } else {
                Ok(character as u32 as u8)
            }
        })
        .collect()
}

/// Width of admitted text in PDF points, for the generic layout.
///
/// The width is the exact integer sum of the 1000-unit glyph widths, multiplied by `size` and
/// divided by 1000, in double precision.
///
/// # Errors
///
/// Returns [`UnsupportedCharacter`] for the first character that is not admitted.
pub fn font_text_width(
    text: &str,
    bold: bool,
    size: f64,
    generated_bullet: bool,
) -> Result<f64, UnsupportedCharacter> {
    let widths = if bold {
        &HELVETICA_BOLD_WIDTHS
    } else {
        &HELVETICA_WIDTHS
    };
    let mut units: u64 = 0;
    for character in text.chars() {
        let index = generic_index(character, generated_bullet)?;
        units += u64::from(widths[index]);
    }
    Ok(units as f64 * size / 1000.0)
}

/// Encode text with the German (Windows-1252) admission policy.
///
/// The encoding admits the generic set, the generated bullet and the 26 extra Windows-1252
/// glyphs of [`GERMAN_EXTRA_WIDTHS`]. Each character encodes to its Windows-1252 byte.
///
/// # Errors
///
/// Returns [`UnsupportedCharacter`] for the first character that is not admitted.
pub fn encode_german_font_text(text: &str) -> Result<Vec<u8>, UnsupportedCharacter> {
    text.chars()
        .map(|character| {
            let codepoint = u32::from(character);
            if character == GENERATED_BULLET {
                return Ok(GENERATED_BULLET_BYTE);
            }
            if let Ok(index) =
                GERMAN_EXTRA_BYTES.binary_search_by_key(&codepoint, |&(code, _)| code)
            {
                return Ok(GERMAN_EXTRA_BYTES[index].1);
            }
            generic_index(character, false)?;
            Ok(codepoint as u8)
        })
        .collect()
}

/// Width of admitted text in PDF points, for the German layout.
///
/// Glyph widths accumulate in `f32` (each addition rounded), the sum is divided by 1000 and
/// multiplied by `size`, in that order.
///
/// # Errors
///
/// Returns [`UnsupportedCharacter`] for the first character that is not admitted.
pub fn german_font_text_width(
    text: &str,
    bold: bool,
    size: f32,
) -> Result<f32, UnsupportedCharacter> {
    let widths = if bold {
        &HELVETICA_BOLD_WIDTHS
    } else {
        &HELVETICA_WIDTHS
    };
    let mut units = 0.0_f64;
    for character in text.chars() {
        let width = match german_extra_width(character) {
            Some((normal, bold_width)) => {
                if bold {
                    bold_width
                } else {
                    normal
                }
            }
            None => widths[generic_index(character, true)?],
        };
        units = round_f32(units + f64::from(width));
    }
    let per_unit = round_f32(units / 1000.0);
    Ok((per_unit * f64::from(size)) as f32)
}
