//! Generic Letter layout: validated text to fixed rows and pages.
//!
//! Everything here is pure. Widths and glyph encoding live in [`fonts`]; drawing lives in
//! [`crate::render`].

pub mod fonts;
pub mod german;
pub mod number;
pub mod pagination;
pub mod text;

use jobniac_text::java::{BudgetField, PreparationBudget, java_is_blank, java_strip, utf16_length};

use crate::error::PdfError;
pub use pagination::PageLines;

/// Page width in points (Letter).
pub const PAGE_WIDTH: f64 = 612.0;
/// Page height in points (Letter).
pub const PAGE_HEIGHT: f64 = 792.0;
/// Left and right margin in points.
pub const MARGIN: f64 = 54.0;
/// Baseline of the first body row.
pub const BODY_START_Y: f64 = 726.0;
/// Lowest baseline that still fits a body row.
pub const BODY_END_Y: f64 = 60.0;
/// Body row spacing in points.
pub const NORMAL_LEADING: f64 = 13.5;
/// Compact row spacing in points.
pub const COMPACT_LEADING: f64 = 10.75;
/// Wrap width of body text in characters.
pub const BODY_WRAP_CHARACTERS: usize = text::BODY_WRAP_CHARACTERS;

/// Largest body in UTF-16 units after the Java strip.
pub const MAX_BODY_UTF16_UNITS: usize = 50_000;

/// Title used when the caller gives none or only blanks.
pub const DEFAULT_TITLE: &str = "Generated PDF";

/// Row spacing of a layout. Only the two qualified values exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leading {
    /// 13.5 points.
    Normal,
    /// 10.75 points.
    Compact,
}

impl Leading {
    /// The leading in points.
    pub fn value(self) -> f64 {
        match self {
            Self::Normal => NORMAL_LEADING,
            Self::Compact => COMPACT_LEADING,
        }
    }

    /// The leading for a compact flag.
    pub fn from_compact(compact: bool) -> Self {
        if compact { Self::Compact } else { Self::Normal }
    }
}

/// Locale of the `Page N of M` footer label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageNumberLocale {
    /// `Page N of M`.
    English,
    /// `Page N sur M`.
    CanadianFrench,
}

impl PageNumberLocale {
    /// The connective word between the page number and the page count.
    pub fn connective(self) -> &'static str {
        match self {
            Self::English => "of",
            Self::CanadianFrench => "sur",
        }
    }
}

/// A complete generic layout: pages of rows, the leading and the footer locale.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfLayout {
    /// Pages in order. Never empty.
    pub pages: Vec<PageLines>,
    /// The row spacing used for pagination and drawing.
    pub leading: Leading,
    /// The footer locale.
    pub page_number_locale: PageNumberLocale,
    /// How many rows at the start of the layout are the title (more than one for long titles).
    pub title_rows: usize,
    /// The point size the title rows are drawn at.
    pub title_size: f64,
}

/// Validates the body after the Java strip.
///
/// Returns the stripped body. Blank and over-long bodies are validation errors.
pub fn validate_body(body: Option<&str>) -> Result<String, PdfError> {
    let body = java_strip(body.unwrap_or("")).to_owned();
    let reason = if java_is_blank(&body) {
        Some("must not be blank")
    } else if utf16_length(&body) > MAX_BODY_UTF16_UNITS {
        Some("must be less than or equal to 50000 characters")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(PdfError::invalid_field("body", reason)),
        None => Ok(body),
    }
}

/// Builds the generic layout.
///
/// The title is stripped and sanitized (or replaced by `Generated PDF` when blank); the body is
/// validated, sanitized and wrapped. Preparation counts UTF-16 units of the raw inputs first.
pub fn build_pdf_layout(
    title: Option<&str>,
    body: Option<&str>,
    compact: bool,
    page_number_locale: PageNumberLocale,
) -> Result<PdfLayout, PdfError> {
    let mut budget = PreparationBudget::new(BudgetField::Text);
    budget.add_entries(2)?;
    budget.add_text(title)?;
    budget.add_text(body)?;
    let validated_body = validate_body(body)?;
    let resolved_title = match title {
        Some(title) if !java_is_blank(title) => java_strip(title).to_owned(),
        _ => DEFAULT_TITLE.to_owned(),
    };
    let leading = Leading::from_compact(compact);
    let (lines, title_rows, title_size) =
        text::body_lines(&text::sanitize_pdf_text(&resolved_title), &validated_body);
    Ok(PdfLayout {
        pages: pagination::paginate_lines(&lines, leading),
        leading,
        page_number_locale,
        title_rows,
        title_size,
    })
}
