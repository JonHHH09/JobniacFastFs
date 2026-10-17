//! The parent side of the PDF worker operations: text extraction and page rendering.
//!
//! The operations run in the CLI binary re-executed as `__worker pdf-extract` or `__worker
//! pdf-render`. This module owns everything that does not need PDFium: the request and response
//! codecs, the validation order and messages, the text normalization, the canonical view and the
//! PNG response check. The child side (PDFium) lives in the CLI crate.
//!
//! Request body (both operations): one operation byte (`1` extract, `2` render), a big-endian `u16`
//! length and the absolute PDFium directory in UTF-8, a big-endian `u32` page index (render only),
//! then the PDF bytes. The PDFium directory is supplied by the caller (the composition root reads
//! `JOBNIAC_PDFIUM_DIR`); the child never searches the working directory.
//!
//! Extraction response body: the canonical view as JSON (`{"result": {...}}`),
//! or `{"error": "page_limit"}` when the document has more pages than the limit. Render response
//! body: three big-endian `u32` (page count, width, height) followed by a PNG.

use std::fmt;
use std::future::Future;
use std::path::Path;

use jobniac_text::java::{truncate_utf16, utf16_length};
use jobniac_text::str_rules::{is_text_whitespace, split_lines, strip_text};
use serde_json::{Map, Value, json};

use crate::workers::error::{WorkerError, WorkerErrorKind};
use crate::workers::kinds::WorkerKind;
use crate::workers::supervisor::WorkerSupervisor;

/// Largest PDF accepted by text extraction, in bytes (the import limit).
pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;

/// Largest PDF accepted by page rendering, in bytes (the preview worker limit).
pub const MAX_PREVIEW_PDF_BYTES: usize = 1024 * 1024;

/// Largest page count accepted by either operation.
pub const MAX_PAGE_COUNT: u32 = 200;

/// The character budget applied when the caller gives none.
pub const DEFAULT_MAX_CHARACTERS: usize = 100_000;

/// The largest character budget a caller may request. The child also returns its canonical view
/// capped at this budget.
pub const MAX_CHARACTERS_LIMIT: usize = 250_000;

/// Pixels per side of a rendered page.
pub const MAX_DIMENSION: u32 = 1600;

/// Largest accepted rendered response: the PNG bound plus the twelve-byte header.
pub const MAX_RENDER_RESPONSE_BYTES: usize =
    (MAX_DIMENSION as usize) * (MAX_DIMENSION as usize) * 4 + 65_536 + 12;

/// The extractor version string. Cached text whose extractor differs from this value is
/// recomputed on demand.
pub const EXTRACTOR_VERSION: &str =
    "pdfium-render-0.9.4:pdfium-chromium-7881:canonical-250000-pages-utf16-v1";

/// Operation byte of the extraction request.
pub const OPERATION_EXTRACT: u8 = 1;

/// Operation byte of the render request.
pub const OPERATION_RENDER: u8 = 2;

/// Largest accepted PDFium directory path, in bytes.
pub const MAX_PDFIUM_DIR_BYTES: usize = 1024;

/// Message of every validation failure of the extraction request.
pub const EXTRACTION_VALIDATION_MESSAGE: &str = "Invalid PDF text extraction request";

/// Fixed message of every extraction failure after validation.
pub const EXTRACT_FAILURE_MESSAGE: &str = "PDF text could not be extracted";

/// Fixed message of every render failure.
pub const RENDER_FAILURE_MESSAGE: &str = "Unable to preview this PDF safely.";

/// The PNG signature.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// The IHDR chunk header, length and type.
const PNG_IHDR: [u8; 8] = [0, 0, 0, 13, b'I', b'H', b'D', b'R'];

/// The IEND chunk, which ends every valid PNG.
const PNG_IEND: [u8; 12] = [0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, b'B', 0x60, 0x82];

/// A failure of a PDF operation. Messages are fixed and never carry parser or path text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfError {
    /// The request is invalid (reported with the extraction message).
    Validation {
        /// The request field that failed (`path` or `maxCharacters` or `includePages`).
        field: &'static str,
        /// The fixed reason.
        reason: String,
    },
    /// The document has more pages than the limit.
    PageLimit,
    /// Text extraction failed for any reason after validation.
    Extraction,
    /// Page rendering failed for any reason.
    Render,
    /// The caller cancelled the operation; the worker tree was killed. The operation tells which
    /// fixed message applies.
    Cancelled {
        /// The operation that was cancelled.
        operation: PdfOperation,
    },
}

impl PdfError {
    /// The fixed message of the failure.
    #[must_use]
    pub fn message(&self) -> &'static str {
        match self {
            Self::Validation { .. } | Self::PageLimit => EXTRACTION_VALIDATION_MESSAGE,
            Self::Extraction
            | Self::Cancelled {
                operation: PdfOperation::Extract,
            } => EXTRACT_FAILURE_MESSAGE,
            Self::Render
            | Self::Cancelled {
                operation: PdfOperation::Render,
            } => RENDER_FAILURE_MESSAGE,
        }
    }
}

impl fmt::Display for PdfError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for PdfError {}

/// Which operation a request carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfOperation {
    /// Text extraction.
    Extract,
    /// Page rendering to PNG.
    Render,
}

/// The validated options of a text extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractionOptions {
    /// The character budget, in UTF-16 units.
    pub max_characters: usize,
    /// Whether the per-page texts are returned.
    pub include_pages: bool,
}

impl ExtractionOptions {
    /// Validates the optional request values in order: the budget first, then the flag.
    ///
    /// `max_characters` is `None` when absent (the default applies); `include_pages` is `None`
    /// when absent (`true` applies). Callers reject non-integer and non-boolean JSON values first,
    /// with the field and the reason `must be an integer` or `must be a boolean`.
    ///
    /// # Errors
    ///
    /// Returns [`PdfError::Validation`] with field `maxCharacters` for a budget outside
    /// `1..=250000`.
    pub fn new(max_characters: Option<i64>, include_pages: Option<bool>) -> Result<Self, PdfError> {
        let budget = max_characters
            .unwrap_or_else(|| i64::try_from(DEFAULT_MAX_CHARACTERS).unwrap_or(i64::MAX));
        let limit = i64::try_from(MAX_CHARACTERS_LIMIT).unwrap_or(i64::MAX);
        let reason = if budget <= 0 {
            "must be greater than zero".to_owned()
        } else if budget > limit {
            format!("must be less than or equal to {MAX_CHARACTERS_LIMIT}")
        } else {
            return Ok(Self {
                max_characters: usize::try_from(budget).unwrap_or(MAX_CHARACTERS_LIMIT),
                include_pages: include_pages.unwrap_or(true),
            });
        };
        Err(PdfError::Validation {
            field: "maxCharacters",
            reason,
        })
    }
}

/// One page of an extraction: its one-based number and normalized text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfPage {
    /// One-based page number.
    pub page_number: u32,
    /// The normalized text of the page.
    pub text: String,
}

/// A text extraction in the response shape of [`extraction_view`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfExtraction {
    /// The safe base name of the uploaded file.
    pub file_name: String,
    /// The number of pages of the document.
    pub page_count: u32,
    /// The UTF-16 length of the full text, before any budget.
    pub character_count: usize,
    /// Whether the full text is longer than the budget.
    pub truncated: bool,
    /// The full text, cut to the budget.
    pub text: String,
    /// The per-page texts, cut to the budget, when requested.
    pub pages: Vec<PdfPage>,
}

/// A rendered page: the document's page count, the PNG size and the PNG bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPage {
    /// The number of pages of the document.
    pub page_count: u32,
    /// The PNG width in pixels.
    pub width: u32,
    /// The PNG height in pixels.
    pub height: u32,
    /// The PNG bytes.
    pub png: Vec<u8>,
}

/// The content checks of a PDF: the size limit, then the `%PDF-` header.
///
/// # Errors
///
/// Returns [`PdfError::Validation`] for a size over the limit or a missing `%PDF-` header.
pub fn validate_pdf_content(content: &[u8], limit: usize) -> Result<(), PdfError> {
    if content.len() > limit {
        return Err(PdfError::Validation {
            field: "path",
            reason: format!("PDF file size exceeds limit of {limit} bytes"),
        });
    }
    if !content.starts_with(b"%PDF-") {
        return Err(PdfError::Validation {
            field: "path",
            reason: "file must be a PDF document".to_owned(),
        });
    }
    Ok(())
}

/// The strict structural preflight both children run before PDFium parses the file. It applies
/// the two strictness rules that matter for hostile input:
///
/// * the last `startxref` offset must point at a classic `xref` table or at an `N G obj` header
///   (a cross-reference stream), so a damaged cross-reference table is refused rather than repaired;
/// * a document with an `/Encrypt` token anywhere in its bytes is refused, as is every encrypted
///   document. This is conservative: the token is searched for in
///   the raw bytes, so an encryption dictionary held in a compressed object stream is found only
///   when its name appears uncompressed.
///
/// Returns `false` when the document fails either rule.
#[must_use]
pub fn has_strict_structure(content: &[u8]) -> bool {
    if content
        .windows(b"/Encrypt".len())
        .any(|window| window == b"/Encrypt")
    {
        return false;
    }
    let keyword = b"startxref";
    let Some(position) = content
        .windows(keyword.len())
        .rposition(|window| window == keyword)
    else {
        return false;
    };
    let rest = content[position + keyword.len()..].trim_ascii_start();
    let digit_count = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    if digit_count == 0 {
        return false;
    }
    let offset = rest[..digit_count].iter().try_fold(0_usize, |value, byte| {
        value.checked_mul(10)?.checked_add(usize::from(byte - b'0'))
    });
    let Some(offset) = offset else {
        return false;
    };
    let Some(target) = content.get(offset..) else {
        return false;
    };
    target.starts_with(b"xref") || starts_with_object_header(target)
}

/// Whether `bytes` starts with `N G obj` (two decimal numbers and the keyword `obj`).
fn starts_with_object_header(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    for _ in 0..2 {
        let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
        if digits == 0 {
            return false;
        }
        rest = rest[digits..].trim_ascii_start();
    }
    rest.starts_with(b"obj")
}

/// The scaled side of a rendered page in pixels: `round(side * scale)` (rounding
/// half to even on a double) clamped to `1..=MAX_DIMENSION`.
#[must_use]
pub fn scaled_side(value: f64) -> i32 {
    let rounded = value.round_ties_even();
    if rounded.is_finite() {
        // The clamp keeps the value in the i32 range; the cast cannot truncate.
        #[allow(clippy::cast_possible_truncation)]
        let pixels = rounded.clamp(1.0, f64::from(MAX_DIMENSION)) as i32;
        pixels
    } else {
        1
    }
}

/// The pixel size of a rendered page: the longer side becomes [`MAX_DIMENSION`] pixels and the
/// other side is scaled with it, each rounded as in [`scaled_side`].
#[must_use]
pub fn render_size(width: f64, height: f64) -> (i32, i32) {
    let longest = width.max(height);
    if !(longest.is_finite() && longest > 0.0) {
        return (1, 1);
    }
    let scale = f64::from(MAX_DIMENSION) / longest;
    (scaled_side(width * scale), scaled_side(height * scale))
}

/// The safe base name of a file name: both separators are treated as private path parts.
#[must_use]
pub fn safe_basename(value: &str) -> String {
    let replaced = value.replace('\\', "/");
    let last = replaced.rsplit('/').next().unwrap_or("");
    if last.is_empty() {
        "stored.pdf".to_owned()
    } else {
        last.to_owned()
    }
}

/// Normalizes the text of one page: each line of the `splitlines`-style split is reduced to its whitespace-separated words joined by one space, the lines
/// are joined with `\n`, and the result is stripped of surrounding whitespace.
#[must_use]
pub fn normalize_page_text(raw: &str) -> String {
    let lines: Vec<String> = split_lines(raw)
        .into_iter()
        .map(|line| {
            line.split(is_text_whitespace)
                .filter(|word| !word.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    strip_text(&lines.join("\n")).to_owned()
}

/// The display view of an extraction: applies a display budget to a canonical extraction without
/// changing the canonical value. Pages are cut in order, and each page takes what remains.
#[must_use]
pub fn extraction_view(
    canonical: &PdfExtraction,
    maximum: usize,
    include_pages: bool,
) -> PdfExtraction {
    let mut pages = Vec::new();
    if include_pages {
        let mut remaining = maximum;
        for page in &canonical.pages {
            if remaining == 0 {
                break;
            }
            let text = truncate_utf16(&page.text, remaining).to_owned();
            remaining = remaining.saturating_sub(utf16_length(&text));
            pages.push(PdfPage {
                page_number: page.page_number,
                text,
            });
        }
    }
    PdfExtraction {
        file_name: safe_basename(&canonical.file_name),
        page_count: canonical.page_count,
        character_count: canonical.character_count,
        truncated: canonical.character_count > maximum,
        text: truncate_utf16(&canonical.text, maximum).to_owned(),
        pages,
    }
}

/// Joins the normalized page texts with blank lines and builds the canonical extraction. The result still has the full text; apply
/// [`extraction_view`] with [`MAX_CHARACTERS_LIMIT`] before it is encoded.
#[must_use]
pub fn canonical_extraction(page_texts: Vec<String>) -> PdfExtraction {
    let page_count = u32::try_from(page_texts.len()).unwrap_or(u32::MAX);
    let pages: Vec<PdfPage> = page_texts
        .into_iter()
        .enumerate()
        .map(|(index, text)| PdfPage {
            page_number: u32::try_from(index + 1).unwrap_or(u32::MAX),
            text,
        })
        .collect();
    let full_text = pages
        .iter()
        .map(|page| page.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    PdfExtraction {
        file_name: "stored.pdf".to_owned(),
        page_count,
        character_count: utf16_length(&full_text),
        truncated: false,
        text: full_text,
        pages,
    }
}

/// Encodes a request body.
///
/// # Errors
///
/// Returns [`PdfError::Validation`] when the PDFium directory is not an absolute UTF-8 path of at
/// most [`MAX_PDFIUM_DIR_BYTES`] bytes. Every other input is passed through unchecked here; the
/// child validates it again.
pub fn encode_request(
    operation: PdfOperation,
    pdfium_dir: &Path,
    page: u32,
    content: &[u8],
) -> Result<Vec<u8>, PdfError> {
    let dir = pdfium_dir
        .to_str()
        .filter(|dir| dir.len() <= MAX_PDFIUM_DIR_BYTES && pdfium_dir.is_absolute())
        .ok_or(PdfError::Render)?;
    let mut body = Vec::with_capacity(content.len() + dir.len() + 8);
    body.push(match operation {
        PdfOperation::Extract => OPERATION_EXTRACT,
        PdfOperation::Render => OPERATION_RENDER,
    });
    let dir_length = u16::try_from(dir.len()).map_err(|_| PdfError::Render)?;
    body.extend_from_slice(&dir_length.to_be_bytes());
    body.extend_from_slice(dir.as_bytes());
    if operation == PdfOperation::Render {
        body.extend_from_slice(&page.to_be_bytes());
    }
    body.extend_from_slice(content);
    Ok(body)
}

/// A decoded request, borrowed from the request body (child side).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedRequest<'a> {
    /// The operation.
    pub operation: PdfOperation,
    /// The absolute PDFium directory.
    pub pdfium_dir: &'a str,
    /// The page index (zero for extraction).
    pub page: u32,
    /// The PDF bytes.
    pub content: &'a [u8],
}

/// Decodes a request body. Returns `None` for any malformed body.
#[must_use]
pub fn decode_request(body: &[u8]) -> Option<DecodedRequest<'_>> {
    let (&operation_byte, rest) = body.split_first()?;
    let operation = match operation_byte {
        OPERATION_EXTRACT => PdfOperation::Extract,
        OPERATION_RENDER => PdfOperation::Render,
        _ => return None,
    };
    let (length_bytes, rest) = rest.split_at_checked(2)?;
    let dir_length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
    if dir_length > MAX_PDFIUM_DIR_BYTES {
        return None;
    }
    let (dir_bytes, rest) = rest.split_at_checked(dir_length)?;
    let pdfium_dir = std::str::from_utf8(dir_bytes).ok()?;
    if !Path::new(pdfium_dir).is_absolute() {
        return None;
    }
    let (page, content) = match operation {
        PdfOperation::Extract => (0, rest),
        PdfOperation::Render => {
            let (page_bytes, content) = rest.split_at_checked(4)?;
            (
                u32::from_be_bytes([page_bytes[0], page_bytes[1], page_bytes[2], page_bytes[3]]),
                content,
            )
        }
    };
    Some(DecodedRequest {
        operation,
        pdfium_dir,
        page,
        content,
    })
}

/// Encodes the canonical extraction as the child's response body.
#[must_use]
pub fn encode_extract_response(canonical: &PdfExtraction) -> Vec<u8> {
    let pages: Vec<Value> = canonical
        .pages
        .iter()
        .map(|page| json!({"pageNumber": page.page_number, "text": page.text}))
        .collect();
    let mut result = Map::new();
    result.insert("fileName".to_owned(), json!(canonical.file_name));
    result.insert("pageCount".to_owned(), json!(canonical.page_count));
    result.insert(
        "characterCount".to_owned(),
        json!(canonical.character_count),
    );
    result.insert("truncated".to_owned(), json!(canonical.truncated));
    result.insert("text".to_owned(), json!(canonical.text));
    result.insert("pages".to_owned(), Value::Array(pages));
    json!({"result": Value::Object(result)})
        .to_string()
        .into_bytes()
}

/// Encodes the page-limit response body of the child.
#[must_use]
pub fn encode_page_limit_response() -> Vec<u8> {
    br#"{"error":"page_limit"}"#.to_vec()
}

/// Decodes the child's extraction response body.
///
/// # Errors
///
/// Returns [`PdfError::PageLimit`] for the page-limit response, and [`PdfError::Extraction`] for
/// any other malformed body.
pub fn decode_extract_response(body: &[u8]) -> Result<PdfExtraction, PdfError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| PdfError::Extraction)?;
    if value.get("error").and_then(Value::as_str) == Some("page_limit") {
        return Err(PdfError::PageLimit);
    }
    let result = value
        .get("result")
        .and_then(Value::as_object)
        .ok_or(PdfError::Extraction)?;
    let page_count = result
        .get("pageCount")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(PdfError::Extraction)?;
    let character_count = result
        .get("characterCount")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(PdfError::Extraction)?;
    let truncated = result
        .get("truncated")
        .and_then(Value::as_bool)
        .ok_or(PdfError::Extraction)?;
    let text = string_field(result, "text")?;
    let file_name = string_field(result, "fileName")?;
    let pages = result
        .get("pages")
        .and_then(Value::as_array)
        .ok_or(PdfError::Extraction)?
        .iter()
        .map(|page| {
            let page_number = page
                .get("pageNumber")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or(PdfError::Extraction)?;
            Ok(PdfPage {
                page_number,
                text: string_field(page.as_object().ok_or(PdfError::Extraction)?, "text")?,
            })
        })
        .collect::<Result<Vec<_>, PdfError>>()?;
    Ok(PdfExtraction {
        file_name,
        page_count,
        character_count,
        truncated,
        text,
        pages,
    })
}

fn string_field(object: &Map<String, Value>, key: &str) -> Result<String, PdfError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(PdfError::Extraction)
}

/// Checks a render response body and splits it into its parts.
///
/// # Errors
///
/// Returns [`PdfError::Render`] for any length, header, dimension or PNG structure outside the
/// accepted bounds.
pub fn decode_render_response(body: &[u8]) -> Result<RenderedPage, PdfError> {
    if !(45..=MAX_RENDER_RESPONSE_BYTES).contains(&body.len()) {
        return Err(PdfError::Render);
    }
    let page_count = read_u32(body, 0)?;
    let width = read_u32(body, 4)?;
    let height = read_u32(body, 8)?;
    let png = &body[12..];
    let valid = (1..=MAX_PAGE_COUNT).contains(&page_count)
        && (1..=MAX_DIMENSION).contains(&width)
        && (1..=MAX_DIMENSION).contains(&height)
        && png.starts_with(&PNG_SIGNATURE)
        && png[8..16] == PNG_IHDR
        && read_u32(png, 16)? == width
        && read_u32(png, 20)? == height
        && png.ends_with(&PNG_IEND);
    if !valid {
        return Err(PdfError::Render);
    }
    Ok(RenderedPage {
        page_count,
        width,
        height,
        png: png.to_vec(),
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, PdfError> {
    let window = bytes.get(offset..offset + 4).ok_or(PdfError::Render)?;
    Ok(u32::from_be_bytes([
        window[0], window[1], window[2], window[3],
    ]))
}

/// Maps a supervisor failure to the fixed failure of the operation.
fn map_worker_error(error: WorkerError, operation: PdfOperation, failure: PdfError) -> PdfError {
    match error.kind() {
        WorkerErrorKind::Cancelled => PdfError::Cancelled { operation },
        _ => failure,
    }
}

/// Extracts the text of a PDF through the `pdf-extract` worker, in the response shape of [`extraction_view`].
///
/// The request is validated in order (budget, then content). `file_name` is reduced to
/// its safe base name. Any worker failure is the fixed extraction failure.
///
/// # Errors
///
/// Returns a [`PdfError`] for validation, page-limit, extraction and cancellation outcomes.
pub async fn extract_text<C: Future<Output = ()>>(
    supervisor: &WorkerSupervisor,
    pdfium_dir: &Path,
    content: &[u8],
    file_name: &str,
    options: ExtractionOptions,
    cancel: C,
) -> Result<PdfExtraction, PdfError> {
    validate_pdf_content(content, MAX_FILE_BYTES)?;
    let body = encode_request(PdfOperation::Extract, pdfium_dir, 0, content)
        .map_err(|_| PdfError::Extraction)?;
    let response = supervisor
        .run(WorkerKind::PdfExtract, &body, cancel)
        .await
        .map_err(|error| map_worker_error(error, PdfOperation::Extract, PdfError::Extraction))?;
    let canonical = decode_extract_response(&response)?;
    let mut view = extraction_view(&canonical, options.max_characters, options.include_pages);
    view.file_name = safe_basename(file_name);
    Ok(view)
}

/// Renders one page of a PDF to PNG through the `pdf-render` worker.
///
/// Content is checked against the preview limit (1 MiB) and the page index against the limit of
/// 200 pages; the page must exist in the document. Any failure is the fixed render failure.
///
/// # Errors
///
/// Returns [`PdfError::Render`] for every failure except cancellation, which is
/// [`PdfError::Cancelled`] with the render operation.
pub async fn render_page<C: Future<Output = ()>>(
    supervisor: &WorkerSupervisor,
    pdfium_dir: &Path,
    content: &[u8],
    page: u32,
    cancel: C,
) -> Result<RenderedPage, PdfError> {
    if content.len() <= 5
        || content.len() > MAX_PREVIEW_PDF_BYTES
        || !content.starts_with(b"%PDF-")
        || page >= MAX_PAGE_COUNT
    {
        return Err(PdfError::Render);
    }
    let body = encode_request(PdfOperation::Render, pdfium_dir, page, content)?;
    let response = supervisor
        .run(WorkerKind::PdfRender, &body, cancel)
        .await
        .map_err(|error| map_worker_error(error, PdfOperation::Render, PdfError::Render))?;
    let rendered = decode_render_response(&response)?;
    if page >= rendered.page_count {
        return Err(PdfError::Render);
    }
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_follow_the_validation_order_and_defaults() {
        assert_eq!(
            ExtractionOptions::new(None, None),
            Ok(ExtractionOptions {
                max_characters: DEFAULT_MAX_CHARACTERS,
                include_pages: true
            })
        );
        assert!(matches!(
            ExtractionOptions::new(Some(0), None),
            Err(PdfError::Validation {
                field: "maxCharacters",
                ..
            })
        ));
        assert!(matches!(
            ExtractionOptions::new(Some(250_001), None),
            Err(PdfError::Validation {
                field: "maxCharacters",
                ..
            })
        ));
    }

    #[test]
    fn normalization_collapses_words_and_keeps_line_breaks() {
        assert_eq!(
            normalize_page_text("  a \t b \r\n\n c\u{00A0}d  \n"),
            "a b\n\nc d"
        );
        assert_eq!(normalize_page_text("\u{1C}x\u{1F}"), "x");
    }

    #[test]
    fn view_truncates_pages_in_order_and_flags_the_full_length() {
        let canonical = canonical_extraction(vec!["abcd".to_owned(), "efgh".to_owned()]);
        let view = extraction_view(&canonical, 6, true);
        assert!(view.truncated);
        assert_eq!(view.text, "abcd\n\n");
        assert_eq!(view.pages.len(), 2);
        assert_eq!(view.pages[1].text, "ef");
        let no_pages = extraction_view(&canonical, 6, false);
        assert!(no_pages.pages.is_empty());
    }

    #[test]
    fn safe_basename_treats_both_separators_as_private_paths() {
        assert_eq!(safe_basename("C:\\a\\b.pdf"), "b.pdf");
        assert_eq!(safe_basename("dir/"), "stored.pdf");
    }

    #[test]
    fn request_round_trips_and_refuses_relative_directories() -> Result<(), PdfError> {
        let dir = std::env::temp_dir();
        let body = encode_request(PdfOperation::Render, &dir, 7, b"%PDF-x")?;
        let decoded = decode_request(&body).ok_or(PdfError::Render)?;
        assert_eq!(decoded.operation, PdfOperation::Render);
        assert_eq!(decoded.page, 7);
        assert_eq!(decoded.content, b"%PDF-x");
        assert!(encode_request(PdfOperation::Extract, Path::new("relative"), 0, b"").is_err());
        assert!(decode_request(&[9, 0, 0]).is_none());
        Ok(())
    }

    #[test]
    fn extract_response_round_trips_and_reports_page_limit() -> Result<(), PdfError> {
        let canonical = canonical_extraction(vec!["one".to_owned(), String::new()]);
        let decoded = decode_extract_response(&encode_extract_response(&canonical))?;
        assert_eq!(decoded, canonical);
        assert_eq!(
            decode_extract_response(&encode_page_limit_response()),
            Err(PdfError::PageLimit)
        );
        Ok(())
    }

    #[test]
    fn render_response_checks_the_png_structure() {
        assert_eq!(decode_render_response(&[0; 44]), Err(PdfError::Render));
        let mut body = Vec::new();
        body.extend_from_slice(&1_u32.to_be_bytes());
        body.extend_from_slice(&2_u32.to_be_bytes());
        body.extend_from_slice(&3_u32.to_be_bytes());
        body.extend_from_slice(&PNG_SIGNATURE);
        body.extend_from_slice(&PNG_IHDR);
        body.extend_from_slice(&2_u32.to_be_bytes());
        body.extend_from_slice(&3_u32.to_be_bytes());
        body.extend_from_slice(&[0; 5]);
        body.extend_from_slice(&PNG_IEND);
        let Ok(rendered) = decode_render_response(&body) else {
            panic!("a structurally valid response is accepted");
        };
        assert_eq!(
            (rendered.page_count, rendered.width, rendered.height),
            (1, 2, 3)
        );
        let mut wrong = body.clone();
        wrong[4..8].copy_from_slice(&9_u32.to_be_bytes());
        assert_eq!(decode_render_response(&wrong), Err(PdfError::Render));
    }

    #[test]
    fn scaled_sides_round_half_to_even_and_stay_in_bounds() {
        assert_eq!(scaled_side(2.5), 2);
        assert_eq!(scaled_side(3.5), 4);
        assert_eq!(scaled_side(2.4), 2);
        assert_eq!(scaled_side(0.2), 1);
        assert_eq!(scaled_side(1e9), 1600);
        assert_eq!(scaled_side(f64::NAN), 1);
        assert_eq!(render_size(1600.0, 800.0), (1600, 800));
        // A zero-width page clamps its width to one pixel; the child refuses such pages earlier.
        assert_eq!(render_size(0.0, 10.0), (1, 1600));
        assert_eq!(render_size(0.0, 0.0), (1, 1));
    }

    fn minimal_pdf(startxref: &str, extra: &str) -> Vec<u8> {
        format!(
            "%PDF-1.4\n1 0 obj\n<< >>\nendobj\nxref\n0 1\n0000000000 65535 f \n\
             trailer\n<< /Root 1 0 R {extra}>>\nstartxref\n{startxref}\n%%EOF\n"
        )
        .into_bytes()
    }

    #[test]
    fn strict_structure_checks_the_last_startxref_and_the_encryption_token() {
        let xref_at = b"%PDF-1.4\n1 0 obj\n<< >>\nendobj\n".len().to_string();
        assert!(has_strict_structure(&minimal_pdf(&xref_at, "")));
        assert!(!has_strict_structure(&minimal_pdf("99999999", "")));
        assert!(!has_strict_structure(&minimal_pdf(
            &xref_at,
            "/Encrypt 2 0 R "
        )));
        assert!(!has_strict_structure(b"%PDF-1.4\n%%EOF\n"));
        assert!(!has_strict_structure(b"%PDF-1.4\nstartxref\n\n%%EOF\n"));
        assert!(!has_strict_structure(
            b"%PDF-1.4\nstartxref\n99999999999999999999999999\n"
        ));
    }

    #[test]
    fn strict_structure_accepts_a_cross_reference_stream_offset() {
        let stream_at = b"%PDF-1.5\n".len();
        let mut content = b"%PDF-1.5\n".to_vec();
        content.extend_from_slice(format!("{stream_at} 0 obj\n<< /Type /XRef >>\n").as_bytes());
        content.extend_from_slice(format!("startxref\n{stream_at}\n%%EOF\n").as_bytes());
        assert!(has_strict_structure(&content));
    }
}
