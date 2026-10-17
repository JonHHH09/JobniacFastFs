//! Document writer on `pdf-writer`: unembedded Standard-14 fonts, one content stream per page.
//!
//! Object numbers are fixed by position, the document ID is a constant and no timestamp is
//! written, so equal input yields equal bytes. Shared by the generic and German renderers.

use pdf_writer::{Name, Pdf, Rect, Ref};

/// Fixed document identifier written twice into the trailer (deterministic output).
const DOCUMENT_ID: &[u8] = b"jobniac-generated-pdf-v1";

const CATALOG_ID: Ref = Ref::new(1);
const PAGE_TREE_ID: Ref = Ref::new(2);
const BODY_FONT_ID: Ref = Ref::new(3);
const BOLD_FONT_ID: Ref = Ref::new(4);
const FIRST_PAGE_ID: i32 = 5;

/// Writes a document with one page per content stream, all of the same size.
///
/// The fonts are Helvetica (`/FBody`) and Helvetica-Bold (`/FBold`), both `WinAnsiEncoding`.
/// Each page's `/Resources` carries exactly those two entries.
pub fn write_document(pages: &[Vec<u8>], page_width: f64, page_height: f64) -> Vec<u8> {
    let mut pdf = Pdf::new();
    pdf.set_file_id((DOCUMENT_ID.to_vec(), DOCUMENT_ID.to_vec()));
    pdf.catalog(CATALOG_ID).pages(PAGE_TREE_ID);

    let page_ids: Vec<Ref> = (0..pages.len())
        .map(|index| Ref::new(FIRST_PAGE_ID + 2 * index as i32))
        .collect();
    pdf.pages(PAGE_TREE_ID)
        .kids(page_ids.iter().copied())
        .count(i32::try_from(pages.len()).unwrap_or(i32::MAX));

    for (font_id, base_font) in [
        (BODY_FONT_ID, "Helvetica"),
        (BOLD_FONT_ID, "Helvetica-Bold"),
    ] {
        let mut font = pdf.type1_font(font_id);
        font.base_font(Name(base_font.as_bytes()));
        font.encoding_predefined(Name(b"WinAnsiEncoding"));
    }

    let media_box = Rect::new(0.0, 0.0, page_width as f32, page_height as f32);
    for (index, page_content) in pages.iter().enumerate() {
        let page_id = page_ids[index];
        let content_id = Ref::new(FIRST_PAGE_ID + 2 * index as i32 + 1);
        {
            let mut page = pdf.page(page_id);
            page.parent(PAGE_TREE_ID)
                .media_box(media_box)
                .contents(content_id);
            let mut resources = page.resources();
            let mut fonts = resources.fonts();
            fonts.pair(Name(b"FBody"), BODY_FONT_ID);
            fonts.pair(Name(b"FBold"), BOLD_FONT_ID);
        }
        pdf.stream(content_id, page_content);
    }
    pdf.finish()
}
