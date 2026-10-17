//! The document use cases: store a PDF from the import root, read its
//! metadata, and extract its canonical text through the extraction cache.
//!
//! Validation runs before any transaction opens. Each repository call runs in its own
//! transaction. The PDF worker runs outside every transaction.

use std::sync::Arc;

use jobniac_text::str_rules::strip_text;
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::clock::{Clock, format_instant};
use crate::errors::{AppError, ApplicationError, ErrorCode, not_found, validation_error};
use crate::profiles::repository::{READ_ONLY, READ_WRITE, UnitOfWork};

use super::model::{
    DocumentMetadata, ExtractionRecord, INTEGRITY_MESSAGE, MISSING_DOCUMENT_MESSAGE,
    PDF_MEDIA_TYPE, PdfText, StoredExtraction,
};
use super::model::{
    EXTRACTION_VALIDATION_MESSAGE, MAX_PAGE_COUNT, PAGE_VALIDATION_MESSAGE, RENDER_FAILURE_MESSAGE,
    STORAGE_VALIDATION_MESSAGE,
};
use super::repository::{
    DocumentFileReader, DocumentRepository, PdfExtractor, PdfRenderer, RenderedPdfPage,
};
use super::view::{
    document_id_text, document_id_value, document_validation, extraction_options, extraction_view,
    optional_boolean, page_index_in_range, path_text, request_object, safe_basename,
    validate_pdf_content,
};

/// Largest PDF the page-render worker accepts, in bytes (infra `MAX_PREVIEW_PDF_BYTES`).
const MAX_PREVIEW_PDF_BYTES: u64 = 1024 * 1024;

/// Computes the SHA-256 digest of document content (up to 10 MiB).
///
/// The core is runtime-free, so it cannot move the hashing off an async worker itself. The default
/// is [`InlineHasher`]; the composed application supplies one that runs the digest in a blocking
/// context (`DocumentService::with_hasher`).
pub trait ContentHasher: std::fmt::Debug + Send + Sync {
    /// The lowercase hexadecimal SHA-256 digest of `content`.
    fn sha256_hex(&self, content: &[u8]) -> String;
}

/// Hashes on the calling thread. The default of [`DocumentService`].
#[derive(Debug, Clone, Copy, Default)]
pub struct InlineHasher;

impl ContentHasher for InlineHasher {
    fn sha256_hex(&self, content: &[u8]) -> String {
        sha256_hex(content)
    }
}

/// Stores, reads, extracts and renders PDF documents.
#[derive(Debug)]
pub struct DocumentService<U, R> {
    uow: U,
    repo: R,
    reader: Arc<dyn DocumentFileReader>,
    extractor: Arc<dyn PdfExtractor>,
    renderer: Option<Arc<dyn PdfRenderer>>,
    clock: Arc<dyn Clock>,
    hasher: Arc<dyn ContentHasher>,
}

impl<U, R> DocumentService<U, R>
where
    U: UnitOfWork,
    R: DocumentRepository<Tx = U::Tx>,
{
    /// A service over a unit of work, a repository, the import-root reader, the PDF extractor
    /// and a clock.
    pub fn new(
        uow: U,
        repo: R,
        reader: Arc<dyn DocumentFileReader>,
        extractor: Arc<dyn PdfExtractor>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            uow,
            repo,
            reader,
            extractor,
            renderer: None,
            clock,
            hasher: Arc::new(InlineHasher),
        }
    }

    /// Replaces the digest function, so that the hashing of large documents runs in a blocking
    /// context instead of on an async worker.
    #[must_use]
    pub fn with_hasher(mut self, hasher: Arc<dyn ContentHasher>) -> Self {
        self.hasher = hasher;
        self
    }

    /// Adds the page renderer that [`Self::render_stored_page`] uses. Without one, every render is a
    /// render failure.
    #[must_use]
    pub fn with_renderer(mut self, renderer: Arc<dyn PdfRenderer>) -> Self {
        self.renderer = Some(renderer);
        self
    }

    /// Stores a PDF read from the import root.
    ///
    /// The request carries `path` and an optional `mediaType`, which must be `application/pdf`
    /// when present. The bytes are deduplicated by SHA-256 inside the repository.
    ///
    /// # Errors
    ///
    /// A storage validation error for a bad request or file, a persistence error otherwise.
    pub async fn store_document_file(&self, request: &Value) -> Result<DocumentMetadata, AppError> {
        let request = request_object(request, STORAGE_VALIDATION_MESSAGE)?;
        let media = match request.get("mediaType") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(text)) => text.clone(),
            Some(_) => {
                return Err(document_validation(
                    STORAGE_VALIDATION_MESSAGE,
                    "mediaType",
                    "must be a string",
                ));
            }
        };
        let media = match strip_text(&media).to_lowercase() {
            text if text.is_empty() => PDF_MEDIA_TYPE.to_owned(),
            text => text,
        };
        if media != PDF_MEDIA_TYPE {
            return Err(document_validation(
                STORAGE_VALIDATION_MESSAGE,
                "mediaType",
                "must be application/pdf",
            ));
        }
        let snapshot = self
            .read(request.get("path"), STORAGE_VALIDATION_MESSAGE)
            .await?;
        validate_pdf_content(&snapshot.content, true)?;
        let now = format_instant(self.clock.now());
        let metadata = DocumentMetadata {
            id: Uuid::new_v4(),
            original_file_name: stored_base_name(&snapshot.file_name),
            media_type: media,
            byte_size: i64::try_from(snapshot.content.len()).unwrap_or(i64::MAX),
            sha256: self.hasher.sha256_hex(&snapshot.content),
            created_at: now.clone(),
            updated_at: now,
        };
        let content = snapshot.content;
        self.uow
            .transaction(READ_WRITE, async |tx| {
                self.repo.save_file(tx, &metadata, &content).await
            })
            .await
    }

    /// The safe metadata of a stored document.
    ///
    /// # Errors
    ///
    /// A storage validation error for a malformed identifier, `not_found` when the document is
    /// missing.
    pub async fn get_document_metadata(
        &self,
        document_id: &str,
    ) -> Result<DocumentMetadata, AppError> {
        let parsed = document_id_text(document_id)?;
        let found = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo.find_metadata(tx, parsed).await
            })
            .await?;
        found.map(safe_metadata).ok_or_else(|| missing(parsed))
    }

    /// Extracts the text of a PDF read from the import root. Nothing
    /// is stored.
    ///
    /// # Errors
    ///
    /// An extraction validation error for a bad request or file, or the extractor's failure.
    pub async fn extract_pdf_text(&self, request: &Value) -> Result<PdfText, AppError> {
        let request = request_object(request, EXTRACTION_VALIDATION_MESSAGE)?;
        let options = extraction_options(request)?;
        let snapshot = self
            .read(request.get("path"), EXTRACTION_VALIDATION_MESSAGE)
            .await?;
        validate_pdf_content(&snapshot.content, false)?;
        let canonical = self
            .extractor
            .extract(&snapshot.content, &stored_base_name(&snapshot.file_name))
            .await?;
        Ok(extraction_view(
            &canonical,
            options.max_characters,
            options.include_pages,
        ))
    }

    /// Extracts the text of a stored document.
    ///
    /// The bytes are verified against their declared size and SHA-256 before any extraction
    /// record is consulted, including on a cache hit. A cached extraction is used only when its
    /// extractor version matches; otherwise the text is recomputed. With
    /// `persistExtraction` the canonical text is stored, refreshing the existing record in place.
    ///
    /// # Errors
    ///
    /// Storage validation errors, `not_found`, the internal integrity error, or the extractor's
    /// failure.
    pub async fn extract_stored_pdf_text(
        &self,
        request: &Value,
    ) -> Result<StoredExtraction, AppError> {
        let request = request_object(request, STORAGE_VALIDATION_MESSAGE)?;
        let document_id = document_id_value(request.get("documentId"))?;
        let options = extraction_options(request)?;
        let persist = optional_boolean(
            request,
            "persistExtraction",
            false,
            STORAGE_VALIDATION_MESSAGE,
        )?;
        let found = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo.find_content(tx, document_id).await
            })
            .await?;
        let (metadata, content) = found.ok_or_else(|| missing(document_id))?;
        let metadata = safe_metadata(metadata);
        if metadata.media_type != PDF_MEDIA_TYPE {
            return Err(document_validation(
                STORAGE_VALIDATION_MESSAGE,
                "documentId",
                "stored document must have media type application/pdf",
            ));
        }
        let size_matches = i64::try_from(content.len()).ok() == Some(metadata.byte_size);
        if !size_matches || self.hasher.sha256_hex(&content) != metadata.sha256 {
            return Err(AppError::from(ApplicationError::with_message(
                ErrorCode::InternalError,
                INTEGRITY_MESSAGE,
            )));
        }
        validate_pdf_content(&content, false)?;
        let existing = if persist {
            self.uow
                .transaction(READ_ONLY, async |tx| {
                    self.repo.find_extraction(tx, document_id).await
                })
                .await?
        } else {
            None
        };
        let version = self.extractor.version();
        let cached = existing
            .as_ref()
            .filter(|record| record.extractor == version);
        let canonical = match cached {
            Some(record) => PdfText {
                file_name: metadata.original_file_name.clone(),
                page_count: record.page_count,
                character_count: record.character_count,
                truncated: record.truncated,
                text: record.extracted_text.clone(),
                pages: record.pages.clone(),
            },
            None => {
                self.extractor
                    .extract(&content, &metadata.original_file_name)
                    .await?
            }
        };
        let extraction_id = if persist {
            match cached {
                Some(record) => Some(record.id),
                None => {
                    let record = ExtractionRecord {
                        id: existing.as_ref().map_or_else(Uuid::new_v4, |old| old.id),
                        file_id: document_id,
                        extractor: version.to_owned(),
                        character_count: canonical.character_count,
                        page_count: canonical.page_count,
                        truncated: canonical.truncated,
                        extracted_text: canonical.text.clone(),
                        pages: canonical.pages.clone(),
                        created_at: format_instant(self.clock.now()),
                    };
                    let saved = self
                        .uow
                        .transaction(READ_WRITE, async |tx| {
                            self.repo.save_extraction(tx, &record).await
                        })
                        .await?;
                    Some(saved.id)
                }
            }
        } else {
            None
        };
        Ok(StoredExtraction {
            document: metadata,
            extraction_id,
            extraction: extraction_view(&canonical, options.max_characters, options.include_pages),
        })
    }

    /// Renders one page of a stored PDF to PNG (page-image viewer).
    ///
    /// The index is refused before the document is read when it is not below [`MAX_PAGE_COUNT`].
    /// The stored bytes are verified (size and SHA-256) before the worker sees them, as for the
    /// extraction cache. The index is checked against the page count the worker reports.
    ///
    /// # Errors
    ///
    /// A validation error for an index the document does not have, `not_found` for a missing
    /// document, the internal integrity error, or a render failure (also when no renderer is set).
    pub async fn render_stored_page(
        &self,
        document_id: Uuid,
        page_index: u32,
    ) -> Result<RenderedPdfPage, AppError> {
        if page_index >= MAX_PAGE_COUNT {
            return Err(page_index_refusal());
        }
        let renderer = self.renderer.as_ref().ok_or_else(render_failure)?;
        let header = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo.find_metadata(tx, document_id).await
            })
            .await?;
        let header = safe_metadata(header.ok_or_else(|| missing(document_id))?);
        if header.media_type != PDF_MEDIA_TYPE {
            return Err(document_validation(
                PAGE_VALIDATION_MESSAGE,
                "documentId",
                "stored document must have media type application/pdf",
            ));
        }
        // The render worker refuses a larger file, so it is refused before the blob is read.
        if u64::try_from(header.byte_size).map_or(true, |size| size > MAX_PREVIEW_PDF_BYTES) {
            return Err(render_failure());
        }
        let found = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo.find_content(tx, document_id).await
            })
            .await?;
        let (metadata, content) = found.ok_or_else(|| missing(document_id))?;
        let metadata = safe_metadata(metadata);
        let size_matches = i64::try_from(content.len()).ok() == Some(metadata.byte_size);
        if !size_matches || self.hasher.sha256_hex(&content) != metadata.sha256 {
            return Err(AppError::from(ApplicationError::with_message(
                ErrorCode::InternalError,
                INTEGRITY_MESSAGE,
            )));
        }
        let rendered = renderer.render(&content, page_index).await?;
        if !page_index_in_range(page_index, rendered.page_count) {
            return Err(page_index_refusal());
        }
        Ok(rendered)
    }

    /// Reads the `path` member through the import-root reader, mapping refusals to the request's
    /// validation message. For storage, the `PDF file size` reason is shortened to `file size`.
    async fn read(
        &self,
        path: Option<&Value>,
        message: &str,
    ) -> Result<super::model::FileSnapshot, AppError> {
        let path = path_text(path, message)?;
        match self.reader.read(path).await {
            Ok(snapshot) => Ok(snapshot),
            Err(AppError::Application(error)) if error.code() == ErrorCode::ValidationError => {
                let mut reason = error
                    .details()
                    .get("reason")
                    .cloned()
                    .unwrap_or_else(|| "file is not readable".to_owned());
                if message == STORAGE_VALIDATION_MESSAGE && reason.starts_with("PDF file size") {
                    reason = reason.replacen("PDF ", "", 1);
                }
                Err(document_validation(message, "path", &reason))
            }
            Err(other) => Err(other),
        }
    }
}

/// The lowercase hexadecimal SHA-256 digest of `content`.
#[must_use]
pub fn sha256_hex(content: &[u8]) -> String {
    Sha256::digest(content)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The not-found error of a missing stored document.
fn missing(document_id: Uuid) -> AppError {
    AppError::from(not_found(
        MISSING_DOCUMENT_MESSAGE,
        [("documentId", document_id.to_string())],
    ))
}

/// The metadata with its file name reduced to a safe base name.
fn safe_metadata(mut metadata: DocumentMetadata) -> DocumentMetadata {
    metadata.original_file_name = safe_basename(&metadata.original_file_name);
    metadata
}

/// The validation error of a page index the document does not have.
fn page_index_refusal() -> AppError {
    document_validation(
        PAGE_VALIDATION_MESSAGE,
        "pageIndex",
        "page index is outside the document",
    )
}

/// The base name kept for a stored document. A name that reduces to a relative marker (`.` or
/// `..`) is not a file name, so it becomes the fallback name instead of reaching a save-as path.
fn stored_base_name(file_name: &str) -> String {
    match safe_basename(file_name) {
        base if base == "." || base == ".." => safe_basename(""),
        base => base,
    }
}

/// The render failure of a page that cannot be shown (the fixed preview message).
fn render_failure() -> AppError {
    AppError::from(validation_error(
        RENDER_FAILURE_MESSAGE,
        Some("documentId"),
        Some(RENDER_FAILURE_MESSAGE),
    ))
}
