//! Shared content-stream primitives: colours, WinAnsi literals and single-line text operators.
//!
//! Used by the generic renderer here and by the German renderer. Operators stay within
//! `q Q rg RG re f w m l S BT ET Tf Tm Tj`.

use crate::error::PdfError;
use crate::layout::fonts::{encode_text, text_width};
use crate::layout::number::format_number;

/// Body text shade, 14/255.
pub const DARK: f64 = 14.0 / 255.0;
/// Grey shade of rules and section headings, 64/255.
pub const GRAY: f64 = 64.0 / 255.0;
/// Full white, used for page labels.
pub const WHITE: f64 = 1.0;

/// The two Standard-14 faces. The resource names are the ones [`crate::readback`] checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontFace {
    /// Helvetica, resource `FBody`.
    Body,
    /// Helvetica-Bold, resource `FBold`.
    Bold,
}

impl FontFace {
    /// The page resource name of this face.
    pub fn resource_name(self) -> &'static str {
        match self {
            Self::Body => "FBody",
            Self::Bold => "FBold",
        }
    }

    /// Whether this is the bold face.
    pub fn is_bold(self) -> bool {
        matches!(self, Self::Bold)
    }
}

/// Writes a grey triplet `s s s` for a shade in 0..=1.
pub fn gray_triplet(shade: f64) -> String {
    let value = format_number(shade);
    format!("{value} {value} {value}")
}

/// A PDF string literal for WinAnsi bytes: `(`, `)` and `\` are escaped, and bytes below 32 or
/// at 127 and above become three-digit octal escapes.
pub fn literal(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() + 2);
    result.push('(');
    for &byte in bytes {
        match byte {
            b'(' | b')' | b'\\' => {
                result.push('\\');
                result.push(char::from(byte));
            }
            0..=31 | 127..=255 => result.push_str(&format!("\\{byte:03o}")),
            _ => result.push(char::from(byte)),
        }
    }
    result.push(')');
    result
}

/// The font, size, fill shade and bullet permission of one text operator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    /// The Standard-14 face.
    pub face: FontFace,
    /// Size in points.
    pub size: f64,
    /// Grey fill shade in 0..=1.
    pub shade: f64,
    /// Whether U+2022 may be encoded (byte 0x95). Only the generator sets this.
    pub generated_bullet: bool,
}

impl TextStyle {
    /// A style without generated-bullet permission.
    pub const fn new(face: FontFace, size: f64, shade: f64) -> Self {
        Self {
            face,
            size,
            shade,
            generated_bullet: false,
        }
    }

    /// The same style with generated-bullet permission.
    pub const fn with_generated_bullet(self) -> Self {
        Self {
            generated_bullet: true,
            ..self
        }
    }
}

/// Appends one `BT ... ET` text operator at `(x, y)`.
///
/// # Errors
///
/// Returns the unsupported-characters validation error when the text cannot be encoded.
pub fn push_text(
    out: &mut String,
    text: &str,
    x: f64,
    y: f64,
    style: TextStyle,
) -> Result<(), PdfError> {
    let encoded = encode_text(text, style.generated_bullet)?;
    let literal = literal(&encoded);
    out.push_str(&gray_triplet(style.shade));
    out.push_str(" rg\nBT\n/");
    out.push_str(style.face.resource_name());
    out.push(' ');
    out.push_str(&format_number(style.size));
    out.push_str(" Tf\n1 0 0 1 ");
    out.push_str(&format_number(x));
    out.push(' ');
    out.push_str(&format_number(y));
    out.push_str(" Tm\n");
    out.push_str(&literal);
    out.push_str(" Tj\nET\n");
    Ok(())
}

/// Width of text for a face at a size, delegating to the Standard-14 metrics.
///
/// # Errors
///
/// Returns the unsupported-characters validation error for characters the font cannot draw.
pub fn measure(
    text: &str,
    face: FontFace,
    size: f64,
    generated_bullet: bool,
) -> Result<f64, PdfError> {
    text_width(text, face.is_bold(), size, generated_bullet)
}

#[cfg(test)]
mod tests {
    use super::literal;

    #[test]
    fn literal_escapes_delimiters_and_high_bytes() {
        assert_eq!(literal(b"a(b)c\\"), "(a\\(b\\)c\\\\)");
        assert_eq!(literal(&[0x95, 0xA0, 0xFF, 0x0A]), "(\\225\\240\\377\\012)");
    }
}
