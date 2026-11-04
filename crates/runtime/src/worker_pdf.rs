//! The child side of the PDF worker operations.
//!
//! Runs inside the `__worker pdf-extract`, `__worker pdf-render`, `__worker pdf-inspect`
//! and `__worker pdf-layout` children only. PDFium is bound
//! from the absolute directory named in the request, never from the working directory. Every
//! failure returns a [`WorkerFault`], which the child turns into exit status 1 without diagnostics.
//!
//! All operations (text extraction, page rendering, inspection and layout) use PDFium.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use image::ExtendedColorType;
use image::ImageEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use jobniac_core::documents::model::{
    MAX_INSPECT_ATTACHMENT_NAMES, PageLayout, PdfInspection, PdfLayout, PdfPageBox,
};
use jobniac_infra::workers::WorkerFault;
use jobniac_infra::workers::child::write_response;
use jobniac_infra::workers::pdf::{
    DecodedRequest, MAX_CHARACTERS_LIMIT, MAX_FILE_BYTES, MAX_PAGE_COUNT, MAX_PREVIEW_PDF_BYTES,
    PdfOperation, attachment_entry, canonical_extraction, decode_inspect_response,
    decode_render_response, decode_request, encode_extract_response, encode_inspect_response,
    encode_page_limit_response, extraction_view, has_strict_structure, normalize_metadata_text,
    normalize_page_text, page_box, render_size, select_attachments, validate_pdf_content,
};
use jobniac_infra::workers::pdf_layout::{
    FontStyle, LayoutBudget, LayoutChar, decode_layout_response, encode_layout_response,
    group_page_chars, is_bold_font, sfnt_style,
};
use jobniac_infra::workers::sandbox::{self, SandboxProfile};
use pdfium_render::prelude::*;

/// Binds PDFium from the library file inside `directory`, then confines the child.
///
/// The library is loaded first, because the confinement removes all file system access. The
/// document is parsed only after this returns, so PDFium never handles untrusted bytes unconfined.
/// A failed confinement is a fault: the child does not parse.
fn bind(directory: &str, profile: SandboxProfile) -> Result<Pdfium, WorkerFault> {
    let path = Path::new(directory);
    if !path.is_absolute() {
        return Err(WorkerFault);
    }
    let library = Pdfium::pdfium_platform_library_name_at_path(path);
    let bindings = Pdfium::bind_to_library(library).map_err(|_| WorkerFault)?;
    let pdfium = Pdfium::new(bindings);
    sandbox::apply(profile)?;
    Ok(pdfium)
}

/// Refuses any document with a security handler, owner-password-only ones included. PDFium would
/// otherwise open them with an empty password.
fn refuse_encrypted(document: &PdfDocument<'_>) -> Result<(), WorkerFault> {
    match document.permissions().security_handler_revision() {
        Ok(PdfSecurityHandlerRevision::Unprotected) => Ok(()),
        _ => Err(WorkerFault),
    }
}

/// Parses the request, which must be of the expected operation.
fn parse(request: &[u8], operation: PdfOperation) -> Result<DecodedRequest<'_>, WorkerFault> {
    decode_request(request)
        .filter(|decoded| decoded.operation == operation)
        .ok_or(WorkerFault)
}

/// Text extraction operation (`pdf-extract`): writes the canonical view as the response.
///
/// The child returns the view capped at the largest budget; the parent applies the caller's
/// budget. A document with more than 200 pages gets the page-limit response.
///
/// # Errors
///
/// Returns [`WorkerFault`] for a malformed request, a PDFium failure or a failed write.
pub fn extract(request: &[u8], output: &mut dyn Write) -> Result<(), WorkerFault> {
    let decoded = parse(request, PdfOperation::Extract)?;
    // Confine first: only the framing and the header decode ran before this point.
    let pdfium = bind(decoded.pdfium_dir, SandboxProfile::PdfExtract)?;
    validate_pdf_content(decoded.content, MAX_FILE_BYTES).map_err(|_| WorkerFault)?;
    if !has_strict_structure(decoded.content) {
        return Err(WorkerFault);
    }
    let document = pdfium
        .load_pdf_from_byte_slice(decoded.content, None)
        .map_err(|_| WorkerFault)?;
    refuse_encrypted(&document)?;
    let pages = document.pages();
    let count = pages.len();
    if count > MAX_PAGE_COUNT as i32 {
        return write_response(output, &encode_page_limit_response());
    }
    let mut texts = Vec::new();
    for page in pages.iter() {
        let text = page.text().map_err(|_| WorkerFault)?.all();
        texts.push(normalize_page_text(&text));
    }
    let canonical = canonical_extraction(texts);
    let view = extraction_view(&canonical, MAX_CHARACTERS_LIMIT, true);
    write_response(output, &encode_extract_response(&view))
}

/// Inspection operation (`pdf-inspect`): writes the page sizes, the document-info strings and the
/// attachments as the response.
///
/// It runs the same checks as [`extract`] under the same confinement. Attachment bytes are read
/// only for the attachments that may be returned. PDFium decodes an attachment's whole stream to
/// size it (`PdfAttachment::len`), so a decompression-bomb attachment exhausts the worker limits
/// and the whole inspection fails closed; the binding exposes no cheaper declared size. A document with more than 200 pages gets the
/// page-limit response.
///
/// # Errors
///
/// Returns [`WorkerFault`] for a malformed request, a page with a degenerate size, a PDFium failure
/// or a failed write.
pub fn inspect(request: &[u8], output: &mut dyn Write) -> Result<(), WorkerFault> {
    let decoded = parse(request, PdfOperation::Inspect)?;
    // Confine first: only the framing and the header decode ran before this point.
    let pdfium = bind(decoded.pdfium_dir, SandboxProfile::PdfExtract)?;
    validate_pdf_content(decoded.content, MAX_FILE_BYTES).map_err(|_| WorkerFault)?;
    if !has_strict_structure(decoded.content) {
        return Err(WorkerFault);
    }
    let document = pdfium
        .load_pdf_from_byte_slice(decoded.content, None)
        .map_err(|_| WorkerFault)?;
    refuse_encrypted(&document)?;
    let pages = document.pages();
    if pages.len() > MAX_PAGE_COUNT as i32 {
        return write_response(output, &encode_page_limit_response());
    }
    let mut boxes: Vec<PdfPageBox> = Vec::new();
    for page in pages.iter() {
        boxes.push(page_box(page.width().value, page.height().value).ok_or(WorkerFault)?);
    }
    let metadata = document.metadata();
    let info = |tag| {
        metadata
            .get(tag)
            .and_then(|entry| normalize_metadata_text(entry.value()))
    };
    let attachments = document.attachments();
    // Names first: reading a name does not decode the stream. The count is the binding's 16-bit
    // value, so 65,536 or more attachments may wrap it (the binding exposes no raw count).
    let total = usize::from(attachments.len());
    let mut names = Vec::new();
    for index in 0..total.min(MAX_INSPECT_ATTACHMENT_NAMES) {
        let handle = attachments
            .get(u16::try_from(index).map_err(|_| WorkerFault)?)
            .map_err(|_| WorkerFault)?;
        names.push(handle.name());
    }
    let (order, attachments_truncated) = select_attachments(total, &names);
    let mut returned_total = 0_u64;
    let mut entries = Vec::new();
    for index in order {
        let attachment = attachments
            .get(u16::try_from(index).map_err(|_| WorkerFault)?)
            .map_err(|_| WorkerFault)?;
        let byte_size = u64::try_from(attachment.len()).map_err(|_| WorkerFault)?;
        entries.push(attachment_entry(
            &attachment.name(),
            byte_size,
            &mut returned_total,
            || attachment.save_to_bytes().ok(),
        ));
    }
    let inspection = PdfInspection {
        page_count: u32::try_from(boxes.len()).map_err(|_| WorkerFault)?,
        pages: boxes,
        title: info(PdfDocumentMetadataTagType::Title),
        producer: info(PdfDocumentMetadataTagType::Producer),
        creator: info(PdfDocumentMetadataTagType::Creator),
        attachments: entries,
        attachments_truncated,
    };
    let body = encode_inspect_response(&inspection);
    decode_inspect_response(&body).map_err(|_| WorkerFault)?;
    write_response(output, &body)
}

/// The most embedded font data one layout operation reads, in bytes. Past it, fonts are judged by
/// their descriptor and name only, which bounds the copying of large or many fonts.
const MAX_FONT_DATA_BYTES: usize = 128 * 1024 * 1024;

/// The largest embedded font program whose style is read, in bytes. A larger one is judged by its
/// descriptor and name only.
const MAX_FONT_PROGRAM_BYTES: usize = 8 * 1024 * 1024;

/// How many font programs of one font identity are read before the identity is trusted, when they
/// all agree.
const FONT_AGREEMENT_READS: u8 = 3;

/// The most text objects remembered by their bounds.
const MAX_REMEMBERED_OBJECTS: usize = 100_000;

/// How many characters beyond the remaining character budget are read from a page, as slack for
/// whitespace and characters that are not returned.
const CHARACTER_SLACK: usize = 100_000;

/// What is known about one font identity.
#[derive(Default)]
struct FontEntry {
    reads: u8,
    first: Option<FontStyle>,
    ambiguous: bool,
}

/// The style the embedded font programs declare, with each font read as rarely as possible.
///
/// Producers such as the Europass editor name the regular and the bold face alike and leave the
/// weight out of the font descriptor, so PDFium reports no difference between them; the font
/// program itself carries the weight. pdfium-render exposes no font handle and no way to ask the
/// size of a font program before it is copied, so a font is identified by its name, family,
/// descriptor flags, italic angle and metrics. The first [`FONT_AGREEMENT_READS`] programs of an
/// identity are read; when they agree the identity is trusted, and when they differ (the regular
/// and bold subsets above) every new text object of it is read, within the overall cap. Only
/// embedded fonts are read: PDFium would otherwise hand out a substitute system font and the
/// result would depend on the machine.
#[derive(Default)]
struct FontStyles {
    by_object: HashMap<([u32; 4], String), FontStyle>,
    by_font: HashMap<String, FontEntry>,
    read_bytes: usize,
}

impl FontStyles {
    /// The declared style of the font of the text object that holds `character`, or the default
    /// (nothing declared) when it cannot be read.
    fn style_of(&mut self, character: &PdfPageTextChar<'_>) -> FontStyle {
        let Ok(object) = character.text_object() else {
            return FontStyle::default();
        };
        let font = object.font();
        let name = font.name();
        let bounds = object.bounds().ok().map(|quad| {
            [
                quad.left().value.to_bits(),
                quad.right().value.to_bits(),
                quad.bottom().value.to_bits(),
                quad.top().value.to_bits(),
            ]
        });
        let object_key = bounds.map(|bounds| (bounds, name.clone()));
        if let Some(style) = object_key.as_ref().and_then(|key| self.by_object.get(key)) {
            return *style;
        }
        let style = self.style_of_font(&font, &name);
        if let Some(key) = object_key
            && self.by_object.len() < MAX_REMEMBERED_OBJECTS
        {
            self.by_object.insert(key, style);
        }
        style
    }

    /// The style of `font`, reading its program when its identity is not yet trusted.
    fn style_of_font(&mut self, font: &PdfFont<'_>, name: &str) -> FontStyle {
        if !matches!(font.is_embedded(), Ok(true)) {
            return FontStyle::default();
        }
        let metric = |value: Result<PdfPoints, PdfiumError>| value.map_or(0, |v| v.value.to_bits());
        let size = PdfPoints::new(1000.0);
        let identity = format!(
            "{name}|{}|{:?}|{:?}|{:?}|{}{}{}{}{}{}|{}|{}",
            font.family(),
            font.weight(),
            font.is_fixed_pitch(),
            font.is_serif(),
            u8::from(font.is_symbolic()),
            u8::from(font.is_italic()),
            u8::from(font.is_bold_reenforced()),
            u8::from(font.is_all_caps()),
            u8::from(font.is_small_caps()),
            u8::from(font.is_sans_serif()),
            metric(font.ascent(size)),
            metric(font.descent(size)),
        );
        let entry = self.by_font.entry(identity).or_default();
        if entry.reads >= FONT_AGREEMENT_READS && !entry.ambiguous {
            return entry.first.unwrap_or_default();
        }
        if self.read_bytes >= MAX_FONT_DATA_BYTES {
            return FontStyle::default();
        }
        // `PdfFont::data` allocates the whole program before any cap applies; the worker's memory
        // limit is what bounds an oversized one.
        let Ok(data) = font.data() else {
            return FontStyle::default();
        };
        self.read_bytes = self.read_bytes.saturating_add(data.len());
        let style = if data.len() > MAX_FONT_PROGRAM_BYTES {
            FontStyle::default()
        } else {
            sfnt_style(&data).unwrap_or_default()
        };
        entry.reads = entry.reads.saturating_add(1);
        match entry.first {
            None => entry.first = Some(style),
            Some(first) if first != style => entry.ambiguous = true,
            Some(_) => {}
        }
        style
    }
}

/// The page frame that character geometry is reported in: the displayed page, so `/Rotate` and a
/// MediaBox or CropBox that does not start at the origin are applied. PDFium maps a point of
/// the page to a device position; with the device sized in centipoints the position is the
/// top-left centipoint position the response uses.
struct PageFrame<'a> {
    page: &'a PdfPage<'a>,
    config: PdfRenderConfig,
    height: i32,
}

impl PageFrame<'_> {
    /// A page point as displayed-page points with the y axis pointing up, or `None` when PDFium
    /// cannot map it.
    fn map(&self, x: PdfPoints, y: PdfPoints) -> Option<(f32, f32)> {
        let (device_x, device_y) = self.page.points_to_pixels(x, y, &self.config).ok()?;
        let up = i64::from(self.height) - i64::from(device_y);
        #[allow(clippy::cast_precision_loss)]
        Some((device_x as f32 / 100.0, up as f32 / 100.0))
    }
}

/// One character of a page as the layout grouping wants it, or `None` for a character PDFium
/// cannot place (no Unicode value or no box).
fn layout_char(
    character: &PdfPageTextChar<'_>,
    frame: &PageFrame<'_>,
    fonts: &mut FontStyles,
) -> Option<LayoutChar> {
    let unicode = character.unicode_char()?;
    let generated = character.is_generated().unwrap_or(true);
    // `scaled_font_size` multiplies by the matrix's `d`, which is 0 for text rotated by a quarter
    // turn; the length of the matrix's vertical axis is the scale whatever the rotation.
    let size = match character.matrix() {
        Ok(matrix) => character.unscaled_font_size().value * matrix.c().hypot(matrix.d()),
        Err(_) => character.unscaled_font_size().value,
    };
    // Generated characters (spaces and breaks PDFium synthesizes) only separate words, so they
    // need no box.
    if generated {
        return Some(LayoutChar {
            character: unicode,
            generated,
            left: 0.0,
            right: 0.0,
            bottom: 0.0,
            top: 0.0,
            baseline: 0.0,
            font_size: size,
            bold: false,
            italic: false,
        });
    }
    let bounds = character.loose_bounds().ok()?;
    let (x_a, y_a) = frame.map(bounds.left(), bounds.top())?;
    let (x_b, y_b) = frame.map(bounds.right(), bounds.bottom())?;
    let baseline = match character.origin() {
        Ok((x, y)) => frame.map(x, y)?.1,
        Err(_) => y_a.min(y_b),
    };
    let program = fonts.style_of(character);
    let weight = character.font_weight().map(|weight| match weight {
        PdfFontWeight::Weight100 => 100,
        PdfFontWeight::Weight200 => 200,
        PdfFontWeight::Weight300 => 300,
        PdfFontWeight::Weight400Normal => 400,
        PdfFontWeight::Weight500 => 500,
        PdfFontWeight::Weight600 => 600,
        PdfFontWeight::Weight700Bold => 700,
        PdfFontWeight::Weight800 => 800,
        PdfFontWeight::Weight900 => 900,
        PdfFontWeight::Custom(value) => value,
    });
    Some(LayoutChar {
        character: unicode,
        generated,
        // A box may come out inverted (a rotated page, a negative scale): order the edges.
        left: x_a.min(x_b),
        right: x_a.max(x_b),
        bottom: y_a.min(y_b),
        top: y_a.max(y_b),
        baseline,
        font_size: size,
        bold: program.bold
            || is_bold_font(
                weight,
                character.font_is_bold_reenforced(),
                &character.font_name(),
            ),
        italic: program.italic || character.font_is_italic(),
    })
}

/// Layout operation (`pdf-layout`): writes the positioned line fragments of every page, with font
/// information, as the response.
///
/// It runs the same checks as [`extract`] under the same confinement. The characters of each page
/// are walked in content order and grouped by [`group_page_chars`] under the document bounds (at
/// most 20,000 fragments, 1,000 characters per fragment and 250,000 characters in total); a page
/// is read only up to the remaining character budget plus a slack, and a cut sets `truncated`.
/// Geometry is in the displayed page, with `/Rotate` and the page box origin applied. A document
/// with more than 200 pages gets the page-limit response.
///
/// # Errors
///
/// Returns [`WorkerFault`] for a malformed request, a page with a degenerate size, a PDFium failure
/// or a failed write.
pub fn layout(request: &[u8], output: &mut dyn Write) -> Result<(), WorkerFault> {
    let decoded = parse(request, PdfOperation::Layout)?;
    // Confine first: only the framing and the header decode ran before this point.
    let pdfium = bind(decoded.pdfium_dir, SandboxProfile::PdfExtract)?;
    validate_pdf_content(decoded.content, MAX_FILE_BYTES).map_err(|_| WorkerFault)?;
    if !has_strict_structure(decoded.content) {
        return Err(WorkerFault);
    }
    let document = pdfium
        .load_pdf_from_byte_slice(decoded.content, None)
        .map_err(|_| WorkerFault)?;
    refuse_encrypted(&document)?;
    let pages = document.pages();
    if pages.len() > MAX_PAGE_COUNT as i32 {
        return write_response(output, &encode_page_limit_response());
    }
    let mut budget = LayoutBudget::new();
    let mut fonts = FontStyles::default();
    let mut laid_out = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        let size = page_box(page.width().value, page.height().value).ok_or(WorkerFault)?;
        let width = i32::try_from(size.width_centipoints).map_err(|_| WorkerFault)?;
        let height = i32::try_from(size.height_centipoints).map_err(|_| WorkerFault)?;
        let mut chars = Vec::new();
        let mut cut = false;
        if !budget.stopped {
            let frame = PageFrame {
                page: &page,
                config: PdfRenderConfig::new().set_target_size(width, height),
                height,
            };
            let text = page.text().map_err(|_| WorkerFault)?;
            let all = text.chars();
            let mut walk = all.iter();
            let limit = budget.chars_left.saturating_add(CHARACTER_SLACK);
            chars.extend(
                walk.by_ref()
                    .take(limit)
                    .filter_map(|character| layout_char(&character, &frame, &mut fonts)),
            );
            cut = walk.next().is_some();
        }
        let lines = group_page_chars(
            &chars,
            size.width_centipoints,
            size.height_centipoints,
            &mut budget,
        );
        if cut {
            budget.truncated = true;
            budget.stopped = true;
        }
        laid_out.push(PageLayout {
            page_number: u32::try_from(index + 1).map_err(|_| WorkerFault)?,
            width_centipoints: size.width_centipoints,
            height_centipoints: size.height_centipoints,
            lines,
        });
    }
    let layout = PdfLayout {
        page_count: u32::try_from(laid_out.len()).map_err(|_| WorkerFault)?,
        pages: laid_out,
        truncated: budget.truncated,
    };
    let body = encode_layout_response(&layout);
    decode_layout_response(&body).map_err(|_| WorkerFault)?;
    write_response(output, &body)
}

/// Page rendering operation (`pdf-render`): writes the 12-byte header and the PNG.
///
/// The page is scaled so its longer side is [`MAX_DIMENSION`] pixels, and drawn over white.
///
/// # Errors
///
/// Returns [`WorkerFault`] for a malformed request, a document without the requested page, a page
/// with a degenerate size, a PDFium failure or a response that fails its own structural check.
pub fn render(request: &[u8], output: &mut dyn Write) -> Result<(), WorkerFault> {
    let decoded = parse(request, PdfOperation::Render)?;
    // Confine first: only the framing and the header decode ran before this point.
    let pdfium = bind(decoded.pdfium_dir, SandboxProfile::PdfRender)?;
    if decoded.content.len() > MAX_PREVIEW_PDF_BYTES {
        return Err(WorkerFault);
    }
    validate_pdf_content(decoded.content, MAX_PREVIEW_PDF_BYTES).map_err(|_| WorkerFault)?;
    if !has_strict_structure(decoded.content) {
        return Err(WorkerFault);
    }
    let index = i32::try_from(decoded.page).map_err(|_| WorkerFault)?;
    let document = pdfium
        .load_pdf_from_byte_slice(decoded.content, None)
        .map_err(|_| WorkerFault)?;
    refuse_encrypted(&document)?;
    let pages = document.pages();
    let count = pages.len();
    if !(1..=MAX_PAGE_COUNT as i32).contains(&count) || index >= count {
        return Err(WorkerFault);
    }
    for page in pages.iter() {
        let (width, height) = (page.width().value, page.height().value);
        if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
            return Err(WorkerFault);
        }
    }
    let page = pages.get(index).map_err(|_| WorkerFault)?;
    let (width, height) = (page.width().value, page.height().value);
    let (target_width, target_height) = render_size(f64::from(width), f64::from(height));
    let config = PdfRenderConfig::new()
        .set_target_width(target_width)
        .set_target_height(target_height)
        .set_clear_color(PdfColor::WHITE);
    let bitmap = page.render_with_config(&config).map_err(|_| WorkerFault)?;
    let image = bitmap.as_image().map_err(|_| WorkerFault)?.to_rgb8();
    if i64::from(image.width()) != i64::from(target_width)
        || i64::from(image.height()) != i64::from(target_height)
    {
        return Err(WorkerFault);
    }
    let png = encode_png(image.as_raw(), image.width(), image.height())?;
    let mut body = Vec::with_capacity(12 + png.len());
    body.extend_from_slice(&(count as u32).to_be_bytes());
    body.extend_from_slice(&image.width().to_be_bytes());
    body.extend_from_slice(&image.height().to_be_bytes());
    body.extend_from_slice(&png);
    decode_render_response(&body).map_err(|_| WorkerFault)?;
    write_response(output, &body)
}

/// Encodes RGB8 pixels as a PNG. The PNG only travels over the local page scheme, so encode time
/// is favoured over size: fast deflate with the cheap `Sub` filter instead of the default
/// adaptive filter search.
fn encode_png(pixels: &[u8], width: u32, height: u32) -> Result<Vec<u8>, WorkerFault> {
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Sub)
        .write_image(pixels, width, height, ExtendedColorType::Rgb8)
        .map_err(|_| WorkerFault)?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A page-like image: white with dark text-like stripes and a pseudo-random noisy band.
    fn page(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = vec![255_u8; (width * height * 3) as usize];
        let mut state = 0x9e37_79b9_u32;
        for y in 0..height {
            for x in 0..width {
                let at = ((y * width + x) * 3) as usize;
                if y % 14 < 7 && x > 100 && x < width - 100 {
                    pixels[at..at + 3].fill(30);
                }
                if y > height - 200 {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    pixels[at] = (state >> 24) as u8;
                }
            }
        }
        pixels
    }

    #[test]
    fn a_full_size_page_encodes_within_the_render_output_limit() {
        let (width, height) = (1600, 1131);
        let pixels = page(width, height);
        let started = Instant::now();
        let fast = encode_png(&pixels, width, height).expect("encode");
        let fast_time = started.elapsed();
        let started = Instant::now();
        let mut default = Vec::new();
        PngEncoder::new(&mut default)
            .write_image(&pixels, width, height, ExtendedColorType::Rgb8)
            .expect("default encode");
        let default_time = started.elapsed();
        eprintln!(
            "fast {} bytes in {fast_time:?}; default {} bytes in {default_time:?}",
            fast.len(),
            default.len()
        );
        assert!(fast.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(fast.len() < 10 * 1024 * 1024, "within the response cap");
    }
}
