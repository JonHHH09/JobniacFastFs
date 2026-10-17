//! Generic Letter renderer: header and footer bands, page labels, bold labels, section rules and
//! the generated bullet.

use crate::error::PdfError;
use crate::layout::number::format_number;
use crate::layout::text::{
    BODY_SIZE, CONTINUATION_X, is_bullet_continuation_line, is_bullet_line, is_labeled_line,
    is_section_heading, strip_bullet_prefix, strip_leading,
};
use crate::layout::{BODY_START_Y, MARGIN, PAGE_WIDTH, PageLines, PdfLayout};
use crate::render::primitives::{
    DARK, FontFace, GRAY, TextStyle, WHITE, gray_triplet, measure, push_text,
};

/// Section heading size in points.
const SECTION_SIZE: f64 = 10.0;
/// Footer label size in points.
const LABEL_SIZE: f64 = 9.0;
/// Position of the generated bullet glyph.
const BULLET_X: f64 = 66.0;

const BODY: TextStyle = TextStyle::new(FontFace::Body, BODY_SIZE, DARK);
const BOLD_LABEL: TextStyle = TextStyle::new(FontFace::Bold, BODY_SIZE, DARK);
const SECTION: TextStyle = TextStyle::new(FontFace::Bold, SECTION_SIZE, GRAY);
const FOOTER: TextStyle = TextStyle::new(FontFace::Bold, LABEL_SIZE, WHITE);

/// Content stream of one page.
///
/// `number` is the 1-based page number and the layout supplies the page count and footer locale.
///
/// # Errors
///
/// Returns the unsupported-characters validation error for text the font cannot draw.
pub fn page_commands(
    page: &PageLines,
    layout: &PdfLayout,
    number: usize,
) -> Result<String, PdfError> {
    let gray = gray_triplet(GRAY);
    let mut out = String::new();
    out.push_str("q\n1 1 1 rg\n0 0 612 792 re\nf\n");
    out.push_str(&gray);
    out.push_str(" rg\n0 762 612 30 re\nf\n0 0 612 24 re\nf\n");

    let total = layout.pages.len();
    let label = format!(
        "Page {number} {} {total}",
        layout.page_number_locale.connective()
    );
    let label_x = PAGE_WIDTH - MARGIN - measure(&label, FontFace::Bold, LABEL_SIZE, false)?;
    for baseline in [773.0, 10.0] {
        push_text(&mut out, &label, label_x, baseline, FOOTER)?;
    }

    let leading = layout.leading.value();
    for (index, line) in page.lines.iter().enumerate() {
        let baseline = BODY_START_Y - (index as f64) * leading;
        if page.start_index + index < layout.title_rows {
            let title = TextStyle::new(FontFace::Bold, layout.title_size, DARK);
            push_text(&mut out, line, MARGIN, baseline, title)?;
        } else if is_bullet_continuation_line(line) {
            push_text(
                &mut out,
                strip_leading(line),
                CONTINUATION_X,
                baseline,
                BODY,
            )?;
        } else if is_section_heading(line) {
            let separator_y = format_number(baseline + 9.0);
            out.push_str(&gray);
            out.push_str(" RG\n0.5 w\n54 ");
            out.push_str(&separator_y);
            out.push_str(" m\n558 ");
            out.push_str(&separator_y);
            out.push_str(" l\nS\n");
            push_text(&mut out, line, MARGIN, baseline, SECTION)?;
        } else if is_bullet_line(line) {
            let bullet = BODY.with_generated_bullet();
            push_text(&mut out, "\u{2022}", BULLET_X, baseline, bullet)?;
            push_text(
                &mut out,
                strip_bullet_prefix(line),
                CONTINUATION_X,
                baseline,
                BODY,
            )?;
        } else if is_labeled_line(line) {
            let colon = line.find(':').unwrap_or(0);
            let label = &line[..=colon];
            let value = strip_leading(&line[colon + 1..]);
            push_text(&mut out, label, MARGIN, baseline, BOLD_LABEL)?;
            let value_x = MARGIN + measure(&format!("{label} "), FontFace::Bold, BODY_SIZE, false)?;
            push_text(&mut out, value, value_x, baseline, BODY)?;
        } else if !line.is_empty() {
            push_text(&mut out, line, MARGIN, baseline, BODY)?;
        }
    }
    out.push_str("Q\n");
    Ok(out)
}

/// Renders every page of a layout into content streams, in page order.
///
/// # Errors
///
/// Returns the first encoding or width error of any page.
pub fn page_streams(layout: &PdfLayout) -> Result<Vec<String>, PdfError> {
    layout
        .pages
        .iter()
        .enumerate()
        .map(|(index, page)| page_commands(page, layout, index + 1))
        .collect()
}
