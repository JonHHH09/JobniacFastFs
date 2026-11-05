//! Ports of the document use cases.
//!
//! * [`DocumentRepository`]: the stored bytes and metadata, and the canonical extraction records.
//!   Every call takes the transaction handle that the service opens with the unit of work, so no
//!   connection is ambient. The SQLite adapter lives in `jobniac_infra::sqlite::documents`.
//! * [`DocumentFileReader`]: reads one file from the configured import root. Refusals come back as
//!   validation errors whose `reason` detail is the caller-visible text.
//! * [`PdfExtractor`]: the canonical extraction of PDF bytes. The PDF worker client implements it.
//! * [`PdfInspector`]: the page sizes, document info and attachments of PDF bytes.
//! * [`PdfLayoutReader`]: the positioned line fragments with font information of PDF bytes.

use async_trait::async_trait;
use uuid::Uuid;

use crate::errors::AppError;
use crate::pagination::{Page, PageRequest};

use super::list::DocumentSummary;
use super::model::{
    DocumentMetadata, ExtractionRecord, FileSnapshot, PdfInspection, PdfLayout, PdfText,
};

/// Persistence port of stored documents and extraction records.
#[async_trait]
pub trait DocumentRepository: Send + Sync {
    /// The open transaction handle that repository calls take.
    type Tx: Send;

    /// Deduplicates the verified bytes by SHA-256 and inserts the document metadata. Returns the
    /// metadata as stored.
    async fn save_file(
        &self,
        tx: &mut Self::Tx,
        metadata: &DocumentMetadata,
        content: &[u8],
    ) -> Result<DocumentMetadata, AppError>;

    /// The metadata of a document, without its bytes or extraction text.
    async fn find_metadata(
        &self,
        tx: &mut Self::Tx,
        document_id: Uuid,
    ) -> Result<Option<DocumentMetadata>, AppError>;

    /// The metadata and the verified bytes of a document (at most 10 MiB).
    async fn find_content(
        &self,
        tx: &mut Self::Tx,
        document_id: Uuid,
    ) -> Result<Option<(DocumentMetadata, Vec<u8>)>, AppError>;

    /// The canonical extraction of a document, when one is stored.
    async fn find_extraction(
        &self,
        tx: &mut Self::Tx,
        document_id: Uuid,
    ) -> Result<Option<ExtractionRecord>, AppError>;

    /// Stores or refreshes the canonical extraction of a document, keeping its identifier.
    async fn save_extraction(
        &self,
        tx: &mut Self::Tx,
        record: &ExtractionRecord,
    ) -> Result<ExtractionRecord, AppError>;
}

/// Persistence port of the read-only document listing (desktop only).
#[async_trait]
pub trait DocumentListRepository: Send + Sync {
    /// The transaction handle of the adapter.
    type Tx: Send;

    /// One page of document summaries for `request`, ordered by creation time (newest first) then
    /// identifier.
    async fn list_documents(
        &self,
        tx: &mut Self::Tx,
        request: &PageRequest<'_>,
    ) -> Result<Page<DocumentSummary>, AppError>;
}

/// Reads one file from the document import root.
#[async_trait]
pub trait DocumentFileReader: Send + Sync + std::fmt::Debug {
    /// Reads `path`. A refusal is a validation error with the `reason` detail set. Adapters do the
    /// file IO off the async runtime (for example on a blocking pool).
    ///
    /// # Errors
    ///
    /// A validation error for every refusal of the import root, or an internal error.
    async fn read(&self, path: &str) -> Result<FileSnapshot, AppError>;
}

/// Extracts the canonical text of PDF bytes.
#[async_trait]
pub trait PdfExtractor: Send + Sync + std::fmt::Debug {
    /// The extractor version recorded with cached text; a different value means recompute.
    fn version(&self) -> &str;

    /// Extracts the canonical extraction of `content`. The result carries the full canonical
    /// text bounded at [`super::model::MAX_CHARACTERS_LIMIT`] and `character_count` of the full
    /// text.
    ///
    /// # Errors
    ///
    /// A validation error for an invalid or over-long document, or an extraction failure.
    async fn extract(&self, content: &[u8], file_name: &str) -> Result<PdfText, AppError>;
}

/// Inspects PDF bytes: page sizes, document-info strings and embedded files (the `pdf-inspect`
/// operation of the PDF worker). Nothing is persisted.
#[async_trait]
pub trait PdfInspector: Send + Sync + std::fmt::Debug {
    /// Inspects `content`, which the caller has already checked for size and the PDF header.
    ///
    /// Attachments are listed with `.xml` names first, then in document order. PDFium decodes an
    /// attachment's whole stream to size it and again to read it, and the pdfium-render binding
    /// exposes no cheaper declared size. A decompression-bomb attachment therefore exhausts the
    /// worker limits (memory, CPU, wall time) and the whole inspection fails closed with the fixed
    /// inspection failure; no partial result is returned. The attachment count is reported through
    /// a 16-bit value by the binding, so a document with 65,536 or more attachments may report a
    /// wrapped count.
    ///
    /// # Errors
    ///
    /// A validation error for an invalid, encrypted or over-long document, or the fixed inspection
    /// failure for any other outcome (including cancellation).
    async fn inspect(&self, content: &[u8]) -> Result<PdfInspection, AppError>;
}

/// Reads the layout of PDF bytes: positioned line fragments with font information (the
/// `pdf-layout` operation of the PDF worker). Nothing is persisted.
#[async_trait]
pub trait PdfLayoutReader: Send + Sync + std::fmt::Debug {
    /// Reads the layout of `content`, which the caller has already checked for size and the PDF
    /// header.
    ///
    /// Characters are walked in content order and grouped into fragments: a fragment continues
    /// while the next character's baseline is within half a font size of the current baseline and
    /// the horizontal gap from the previous character is below one and a half font sizes, so two
    /// columns on one baseline give two fragments, and so does a change of bold or italic (a bold
    /// label in front of a regular value). Coordinates are integer centipoints with the origin at
    /// the top-left of the page, clamped to the page. The font size is the median over the
    /// fragment. Boldness comes from the font weight, PDFium's bold flag, the font name or, as
    /// many producers name every face alike, the `OS/2` and `head` tables of the embedded font.
    ///
    /// The bounds are the page limit of text extraction ([`super::MAX_PAGE_COUNT`]), at most
    /// [`super::MAX_LAYOUT_LINES`] fragments per document, at most
    /// [`super::MAX_LAYOUT_LINE_CHARS`] characters per fragment and in total at most
    /// [`super::MAX_CHARACTERS_LIMIT`] characters; a bound that cuts output sets `truncated`.
    /// Rotated text is grouped by its unrotated bounds and pages whose box does not start at the
    /// origin are not translated, so such pages may lose or clamp fragments.
    ///
    /// # Errors
    ///
    /// A validation error for an invalid, encrypted or over-long document, or the fixed layout
    /// failure for any other outcome (including cancellation).
    async fn read_layout(&self, content: &[u8]) -> Result<PdfLayout, AppError>;
}

/// One page rendered to PNG by the PDF worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPdfPage {
    /// The number of pages of the document.
    pub page_count: u32,
    /// The PNG width in pixels.
    pub width: u32,
    /// The PNG height in pixels.
    pub height: u32,
    /// The PNG bytes.
    pub png: Vec<u8>,
}

/// Renders one page of PDF bytes to PNG (the `pdf-render` operation of the PDF worker).
#[async_trait]
pub trait PdfRenderer: Send + Sync + std::fmt::Debug {
    /// Renders the zero-based `page_index` of `content`. The page count of the document is returned
    /// with the image so that the caller can check the index against it.
    ///
    /// # Errors
    ///
    /// A validation error for a page the document does not have, or a render failure for any other
    /// outcome (including cancellation).
    async fn render(&self, content: &[u8], page_index: u32) -> Result<RenderedPdfPage, AppError>;
}
