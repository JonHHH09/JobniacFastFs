//! Rendering: layout to PDF bytes with unembedded Standard-14 fonts, then structural readback.
//!
//! [`render_pdf`] is the generic Letter renderer. [`primitives`] and
//! [`writer`] are shared with the German renderer.

pub mod generic;
pub mod german;
pub mod primitives;
pub mod writer;

use crate::error::PdfError;
use crate::layout::{PAGE_HEIGHT, PAGE_WIDTH, PageNumberLocale, build_pdf_layout};
use crate::readback::verify_structure;

/// A rendered document and its page count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPdf {
    /// The PDF file bytes.
    pub data: Vec<u8>,
    /// The number of pages in `data`.
    pub page_count: usize,
}

impl RenderedPdf {
    /// The size of the PDF in bytes.
    pub fn byte_size(&self) -> usize {
        self.data.len()
    }
}

/// Renders a generic Letter document from validated input.
///
/// Layout errors and unsupported characters are returned as validation errors. A failure while
/// writing or reading back the document is [`PdfError::Internal`], with the fixed message.
///
/// # Errors
///
/// Returns [`PdfError::Validation`] for invalid input or unsupported glyphs, and
/// [`PdfError::Internal`] when the written document fails its structural readback.
pub fn render_pdf(
    title: Option<&str>,
    body: Option<&str>,
    compact: bool,
    page_number_locale: PageNumberLocale,
) -> Result<RenderedPdf, PdfError> {
    let layout = build_pdf_layout(title, body, compact, page_number_locale)?;
    let streams = generic::page_streams(&layout)?;
    let contents: Vec<Vec<u8>> = streams.into_iter().map(String::into_bytes).collect();
    let data = writer::write_document(&contents, PAGE_WIDTH, PAGE_HEIGHT);
    let page_count = verify_structure(&data, layout.pages.len(), PAGE_WIDTH, PAGE_HEIGHT)
        .map_err(|_| PdfError::Internal)?;
    Ok(RenderedPdf { data, page_count })
}
