//! Value types and limits of the document use cases.
//!
//! Instants are kept as wire strings (`format_instant`), exactly as they
//! pass between the service and the repository. Byte sizes and character counts are `i64`
//! because they are SQLite `INTEGER` values; they cross IPC only as strings.

use uuid::Uuid;

/// Largest PDF accepted for import or extraction, in bytes (`MAX_FILE_BYTES`, 10 MiB).
pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
/// The default display budget of extraction, in UTF-16 units (`DEFAULT_MAX_CHARACTERS`).
pub const DEFAULT_MAX_CHARACTERS: usize = 100_000;
/// The largest display budget and the canonical text bound, in UTF-16 units.
pub const MAX_CHARACTERS_LIMIT: usize = 250_000;
/// The largest page count of a canonical extraction (`MAX_PAGE_COUNT`).
pub const MAX_PAGE_COUNT: u32 = 200;
/// The only media type the document use cases store and extract.
pub const PDF_MEDIA_TYPE: &str = "application/pdf";
/// The fixed message of every storage-request validation error.
pub const STORAGE_VALIDATION_MESSAGE: &str = "Invalid document storage request";
/// The fixed message of every extraction-request validation error.
pub const EXTRACTION_VALIDATION_MESSAGE: &str = "Invalid PDF text extraction request";
/// The internal error of a stored document whose bytes do not match its metadata.
pub const INTEGRITY_MESSAGE: &str = "Stored document integrity check failed";
/// The not-found message of a missing stored document.
pub const MISSING_DOCUMENT_MESSAGE: &str = "Stored document was not found";
/// The file name used when a name has no usable base name (`safe_basename`).
pub const FALLBACK_FILE_NAME: &str = "stored.pdf";
/// The fixed message of every page-render request validation error (page index or document).
pub const PAGE_VALIDATION_MESSAGE: &str = "Invalid PDF page request";
/// The fixed message of a page render that fails for any reason.
pub const RENDER_FAILURE_MESSAGE: &str = "Unable to preview this PDF safely.";

/// Immutable bytes read from one securely opened local source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSnapshot {
    /// The requested file name; the service reduces it to a safe base name.
    pub file_name: String,
    /// The file content, already bounded by the reader.
    pub content: Vec<u8>,
}

/// The metadata of one stored document (seven keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentMetadata {
    /// The document identifier.
    pub id: Uuid,
    /// The safe base name of the original file.
    pub original_file_name: String,
    /// The media type, always `application/pdf` for stored PDFs.
    pub media_type: String,
    /// The byte size of the content, between 1 and [`MAX_FILE_BYTES`].
    pub byte_size: i64,
    /// Lowercase hexadecimal SHA-256 of the content.
    pub sha256: String,
    /// Creation instant in the wire form.
    pub created_at: String,
    /// Last update instant in the wire form; never before `created_at`.
    pub updated_at: String,
}

/// One page of a canonical or display extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageText {
    /// The one-based page number.
    pub page_number: u32,
    /// The normalized page text, possibly cut by a display budget.
    pub text: String,
}

/// A PDF text extraction: the canonical value, or a display view of it (the two share this
/// shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfText {
    /// The safe base name of the source.
    pub file_name: String,
    /// The number of pages of the document, at least one.
    pub page_count: u32,
    /// The full UTF-16 length of the text, which may exceed the text kept here.
    pub character_count: i64,
    /// Whether the text was cut (`character_count` is greater than the budget).
    pub truncated: bool,
    /// The joined page text, bounded by the budget.
    pub text: String,
    /// The per-page text in page order, bounded by the budget when pages are included.
    pub pages: Vec<PageText>,
}

/// The durable canonical extraction of a stored document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionRecord {
    /// The extraction identifier, stable across refreshes of the same document.
    pub id: Uuid,
    /// The document the extraction belongs to.
    pub file_id: Uuid,
    /// The extractor version that produced the text (cache key).
    pub extractor: String,
    /// The full UTF-16 length of the text.
    pub character_count: i64,
    /// The page count, between 1 and [`MAX_PAGE_COUNT`].
    pub page_count: u32,
    /// Whether the extracted text was cut at [`MAX_CHARACTERS_LIMIT`].
    pub truncated: bool,
    /// The canonical text, at most [`MAX_CHARACTERS_LIMIT`] UTF-16 units.
    pub extracted_text: String,
    /// The canonical page projections.
    pub pages: Vec<PageText>,
    /// The creation instant in the wire form.
    pub created_at: String,
}

/// The display limits of one extraction request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractionOptions {
    /// The display budget in UTF-16 units, between 1 and [`MAX_CHARACTERS_LIMIT`].
    pub max_characters: usize,
    /// Whether per-page text is included in the view.
    pub include_pages: bool,
}

/// The result of extracting a stored document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredExtraction {
    /// The safe metadata of the document.
    pub document: DocumentMetadata,
    /// The extraction record identifier, present only when the result was persisted.
    pub extraction_id: Option<Uuid>,
    /// The display view of the canonical extraction.
    pub extraction: PdfText,
}
