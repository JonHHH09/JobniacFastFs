//! The child side of the PDF worker operations.
//!
//! Runs inside the `__worker pdf-extract` and `__worker pdf-render` children only. PDFium is bound
//! from the absolute directory named in the request, never from the working directory. Every
//! failure returns a [`WorkerFault`], which the child turns into exit status 1 without diagnostics.
//!
//! Both operations (text extraction and page rendering) use PDFium.

use std::io::Write;
use std::path::Path;

use image::ExtendedColorType;
use image::ImageEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use jobniac_infra::workers::WorkerFault;
use jobniac_infra::workers::child::write_response;
use jobniac_infra::workers::pdf::{
    DecodedRequest, MAX_CHARACTERS_LIMIT, MAX_FILE_BYTES, MAX_PAGE_COUNT, MAX_PREVIEW_PDF_BYTES,
    PdfOperation, canonical_extraction, decode_render_response, decode_request,
    encode_extract_response, encode_page_limit_response, extraction_view, has_strict_structure,
    normalize_page_text, render_size, validate_pdf_content,
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
