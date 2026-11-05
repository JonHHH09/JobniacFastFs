//! Document use cases: store a PDF from the
//! import root with SHA-256 deduplication, read metadata, and extract canonical text through the
//! extraction cache.
//!
//! The use cases are framework-free. The file reader, the PDF extractor and the repository are
//! ports: `jobniac_infra` provides the import-root reader, the PDF worker client and the
//! SQLite repository, and `jobniac_runtime` wires them. Bytes leave the database only inside
//! Rust; the wire forms are built by the adapters.

pub mod list;
pub mod model;
pub mod repository;
pub mod service;
pub mod view;

pub use list::{
    DOCUMENT_LIST_FILTER, DOCUMENT_LIST_SCOPE, DocumentListPage, DocumentListService,
    DocumentSummary,
};
pub use model::{
    DEFAULT_MAX_CHARACTERS, DocumentMetadata, ExtractionOptions, ExtractionRecord, FileSnapshot,
    INSPECTION_FAILURE_MESSAGE, LAYOUT_BOUNDS_TOLERANCE, LAYOUT_FAILURE_MESSAGE, LayoutLine,
    MAX_ATTACHMENT_BYTES, MAX_ATTACHMENT_TOTAL_BYTES, MAX_CHARACTERS_LIMIT, MAX_FILE_BYTES,
    MAX_INSPECT_ATTACHMENT_NAME_CHARS, MAX_INSPECT_ATTACHMENT_NAMES, MAX_INSPECT_ATTACHMENTS,
    MAX_INSPECT_METADATA_CHARS, MAX_LAYOUT_FONT_SIZE, MAX_LAYOUT_LINE_CHARS, MAX_LAYOUT_LINES,
    MAX_PAGE_COUNT, PAGE_VALIDATION_MESSAGE, PageLayout, PageText, PdfAttachmentInfo,
    PdfInspection, PdfLayout, PdfPageBox, PdfText, RENDER_FAILURE_MESSAGE, StoredExtraction,
};
pub use repository::{
    DocumentFileReader, DocumentListRepository, DocumentRepository, PdfExtractor, PdfInspector,
    PdfLayoutReader, PdfRenderer, RenderedPdfPage,
};
pub use service::{ContentHasher, DocumentService, InlineHasher, sha256_hex};
pub use view::{extraction_view, page_index_in_range, safe_basename, validate_pdf_content};
