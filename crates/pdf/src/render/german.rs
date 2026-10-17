//! German A4 renderer: layout to PDF bytes with the dedicated Windows-1252 glyph set.
//!
//! The writer, the Standard-14 font resources and the structural readback are shared with the
//! generic renderer. Text is encoded with [`crate::metrics::encode_german_font_text`], so the 218
//! German glyphs and U+2022 round trip.

use crate::error::{FONT_MESSAGE, PdfError};
use crate::layout::german::{
    Block, GermanCoverLetterContent, GermanLayout, Kind, MARGIN_X, PAGE_HEIGHT, PAGE_WIDTH,
    PageLayout, Rgb, StructuredResumeContent, TOP_Y, build_cover_letter_layout,
    build_resume_layout,
};
use crate::layout::number::format_number;
use crate::metrics::{encode_german_font_text, german_font_text_width};
use crate::readback::verify_structure;
use crate::render::RenderedPdf;
use crate::render::primitives::literal;
use crate::render::writer::write_document;

/// Footer label size in points.
const FOOTER_SIZE: f32 = 7.5;
/// Footer baseline in points.
const FOOTER_Y: f32 = 20.0;
/// Footer colour, the muted grey of the German layouts.
const MUTED: Rgb = [91, 99, 107];
/// Position of the generated bullet glyph.
const BULLET_X: f32 = 44.0;
/// Size of the generated bullet glyph in points.
const BULLET_SIZE: f32 = 9.0;

/// Renders a German resume.
///
/// # Errors
///
/// Returns the layout's validation errors, and [`PdfError::Internal`] when the output fails its
/// structural readback.
pub fn render_resume_pdf(content: &StructuredResumeContent) -> Result<RenderedPdf, PdfError> {
    render_layout(&build_resume_layout(content)?)
}

/// Renders a German cover letter.
///
/// # Errors
///
/// Returns the layout's validation errors, and [`PdfError::Internal`] when the output fails its
/// structural readback.
pub fn render_cover_letter_pdf(
    content: &GermanCoverLetterContent,
) -> Result<RenderedPdf, PdfError> {
    render_layout(&build_cover_letter_layout(content)?)
}

/// Writes a layout to a document and checks it.
fn render_layout(layout: &GermanLayout) -> Result<RenderedPdf, PdfError> {
    let mut contents = Vec::with_capacity(layout.pages.len());
    for (index, page) in layout.pages.iter().enumerate() {
        contents.push(page_commands(page, layout, index + 1)?);
    }
    let data = write_document(&contents, PAGE_WIDTH, PAGE_HEIGHT);
    let page_count = verify_structure(&data, layout.pages.len(), PAGE_WIDTH, PAGE_HEIGHT)
        .map_err(|_| PdfError::Internal)?;
    Ok(RenderedPdf { data, page_count })
}

/// A PDF colour operand triplet: each channel divided by 255 and formatted.
fn rgb_triplet(color: Rgb) -> String {
    color
        .iter()
        .map(|&channel| format_number(f64::from(channel) / 255.0))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Appends one `BT ... ET` text operator.
fn text_operator(
    out: &mut String,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    color: Rgb,
    bold: bool,
) -> Result<(), PdfError> {
    let encoded = encode_german_font_text(text).map_err(|_| PdfError::validation(FONT_MESSAGE))?;
    out.push_str(&rgb_triplet(color));
    out.push_str(" rg\nBT\n");
    out.push_str(if bold { "/FBold " } else { "/FBody " });
    out.push_str(&format_number(f64::from(size)));
    out.push_str(" Tf\n1 0 0 1 ");
    out.push_str(&format_number(f64::from(x)));
    out.push(' ');
    out.push_str(&format_number(f64::from(y)));
    out.push_str(" Tm\n");
    out.push_str(&literal(&encoded));
    out.push_str(" Tj\nET\n");
    Ok(())
}

/// The content stream of one page.
///
/// `number` is the 1-based page number. The footer reads `Seite N / M` for the German layouts and
/// `N / M` otherwise.
fn page_commands(
    page: &PageLayout,
    layout: &GermanLayout,
    number: usize,
) -> Result<Vec<u8>, PdfError> {
    let mut out = String::new();
    out.push_str("q\n1 1 1 rg\n0 0 ");
    out.push_str(&format_number(PAGE_WIDTH));
    out.push(' ');
    out.push_str(&format_number(PAGE_HEIGHT));
    out.push_str(" re\nf\n");

    let right = (PAGE_WIDTH - MARGIN_X) as f32;
    let mut y = TOP_Y;
    for block in &page.blocks {
        draw_block(&mut out, block, &mut y, right)?;
    }

    let prefix = if layout.german_footer { "Seite " } else { "" };
    let footer = format!("{prefix}{number} / {}", layout.pages.len());
    let width =
        german_font_text_width(&footer, false, FOOTER_SIZE).map_err(|_| PdfError::Internal)?;
    text_operator(
        &mut out,
        &footer,
        right - width,
        FOOTER_Y,
        FOOTER_SIZE,
        MUTED,
        false,
    )?;
    out.push_str("Q\n");
    Ok(out.into_bytes())
}

/// Draws the rows of one block, advancing the baseline by each row's height in binary32.
fn draw_block(out: &mut String, block: &Block, y: &mut f32, right: f32) -> Result<(), PdfError> {
    for row in &block.lines {
        *y -= row.height;
        let style = row.style;
        if style.kind == Kind::Spacer {
            continue;
        }
        if matches!(style.kind, Kind::Section | Kind::Divider) {
            let rule_y = *y
                + if style.kind == Kind::Section {
                    11.0
                } else {
                    6.0
                };
            out.push_str(&rgb_triplet(style.color));
            out.push_str(" RG\n0.6 w\n42 ");
            out.push_str(&format_number(f64::from(rule_y)));
            out.push_str(" m\n");
            out.push_str(&format_number(f64::from(right)));
            out.push(' ');
            out.push_str(&format_number(f64::from(rule_y)));
            out.push_str(" l\nS\n");
        }
        if style.kind == Kind::Divider {
            continue;
        }
        if style.kind == Kind::Bullet {
            text_operator(
                out,
                "\u{2022}",
                BULLET_X,
                *y,
                BULLET_SIZE,
                style.color,
                false,
            )?;
        }
        text_operator(
            out,
            &row.text,
            MARGIN_X as f32 + row.indent,
            *y,
            row.font_size,
            style.color,
            style.bold,
        )?;
    }
    Ok(())
}
