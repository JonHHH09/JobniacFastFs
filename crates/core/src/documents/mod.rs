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
    MAX_CHARACTERS_LIMIT, MAX_FILE_BYTES, MAX_PAGE_COUNT, PAGE_VALIDATION_MESSAGE, PageText,
    PdfText, RENDER_FAILURE_MESSAGE, StoredExtraction,
};
pub use repository::{
    DocumentFileReader, DocumentListRepository, DocumentRepository, PdfExtractor, PdfRenderer,
    RenderedPdfPage,
};
pub use service::{ContentHasher, DocumentService, InlineHasher, sha256_hex};
pub use view::{extraction_view, page_index_in_range, safe_basename, validate_pdf_content};
