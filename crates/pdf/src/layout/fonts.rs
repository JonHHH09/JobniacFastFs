//! Thin adapter from the generic layout to the Standard-14 metrics for the generic renderer.

use crate::error::{FONT_MESSAGE, PdfError};
use crate::metrics::{self, UnsupportedCharacter};

impl From<UnsupportedCharacter> for PdfError {
    fn from(_: UnsupportedCharacter) -> Self {
        PdfError::validation(FONT_MESSAGE)
    }
}

/// WinAnsi bytes of text for the generic renderer. `generated_bullet` admits U+2022 (byte 0x95).
///
/// # Errors
///
/// Returns the unsupported-characters validation error for the first character that the font
/// cannot draw.
pub fn encode_text(text: &str, generated_bullet: bool) -> Result<Vec<u8>, PdfError> {
    Ok(metrics::encode_font_text(text, generated_bullet)?)
}

/// Width in points of text in Helvetica (`bold` false) or Helvetica-Bold (`bold` true) at `size`.
///
/// # Errors
///
/// Returns the unsupported-characters validation error for the first character that the font
/// cannot draw.
pub fn text_width(
    text: &str,
    bold: bool,
    size: f64,
    generated_bullet: bool,
) -> Result<f64, PdfError> {
    Ok(metrics::font_text_width(
        text,
        bold,
        size,
        generated_bullet,
    )?)
}
