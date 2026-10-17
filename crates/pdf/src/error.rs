//! Caller-safe errors of the PDF layout and renderer.

use jobniac_text::java::PreparationLimitExceeded;

/// Message of the internal failure.
pub const INTERNAL_MESSAGE: &str = "PDF file could not be generated";

/// Message of the validation failure for characters the font cannot draw.
pub const FONT_MESSAGE: &str = "PDF text contains characters unsupported by the current font";

/// Message of the common PDF generation request validation failure.
pub const REQUEST_MESSAGE: &str = "Invalid PDF generation request";

/// Failure of PDF layout or rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfError {
    /// A caller-correctable problem. `field` and `reason` are optional
    /// details; the message never contains the input text.
    Validation {
        /// The fixed, caller-safe message.
        message: &'static str,
        /// The offending request field, when one applies.
        field: Option<&'static str>,
        /// The fixed reason for the field, when one applies.
        reason: Option<&'static str>,
    },
    /// A library or structural failure. The message is fixed.
    Internal,
}

impl PdfError {
    /// A validation error without details (for example unsupported glyphs).
    pub fn validation(message: &'static str) -> Self {
        Self::Validation {
            message,
            field: None,
            reason: None,
        }
    }

    /// A validation error for one request field.
    pub fn invalid_field(field: &'static str, reason: &'static str) -> Self {
        Self::Validation {
            message: REQUEST_MESSAGE,
            field: Some(field),
            reason: Some(reason),
        }
    }

    /// The fixed caller-safe message of this error.
    pub fn message(&self) -> &'static str {
        match self {
            Self::Validation { message, .. } => message,
            Self::Internal => INTERNAL_MESSAGE,
        }
    }
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for PdfError {}

impl From<PreparationLimitExceeded> for PdfError {
    fn from(error: PreparationLimitExceeded) -> Self {
        Self::Validation {
            message: error.message(),
            field: Some(error.field().as_str()),
            reason: Some(error.reason()),
        }
    }
}
