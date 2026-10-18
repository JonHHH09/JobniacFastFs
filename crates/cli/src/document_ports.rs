//! The MCP ports of the document tools and the profile PDF tools over the SQLite-backed use
//! cases.
//!
//! Like [`crate::match_ports`], the composition root is the only place that knows both the MCP
//! ports (`jobniac-mcp`) and the concrete use cases (`jobniac-core` over `jobniac-infra`). Each
//! adapter converts the typed result of its use case into the service dictionary the tools
//! return, with the tool key names in their insertion order (the tool text keeps that order).

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::documents::{DocumentMetadata, DocumentService, PdfText, StoredExtraction};
use jobniac_core::errors::AppError;
use jobniac_infra::sqlite::documents::SqliteDocumentRepository;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use jobniac_mcp::tools::documents::DocumentPort;
use jobniac_mcp::tools::profile_pdf::ProfilePdfPort;
use jobniac_runtime::services::{DocumentServices, ProfilePdfServiceType};
use serde_json::{Value, json};

/// The document use cases over SQLite.
pub type DocumentUseCase = DocumentService<SqliteUnitOfWork, SqliteDocumentRepository>;

/// The MCP ports of the document and profile PDF tools. An absent port leaves its tools
/// unregistered.
#[derive(Debug, Clone, Default)]
pub struct DocumentPorts {
    /// The store, metadata and extraction use cases behind the four document tools.
    pub documents: Option<Arc<dyn DocumentPort>>,
    /// The ingestion and source use cases behind the two profile PDF tools.
    pub profile_pdf: Option<Arc<dyn ProfilePdfPort>>,
}

/// The ports of the composed document and profile PDF services.
pub fn from_services(
    documents: &DocumentServices,
    profile_pdf: &Arc<ProfilePdfServiceType>,
) -> DocumentPorts {
    from_use_cases(Arc::clone(&documents.service), Arc::clone(profile_pdf))
}

/// The ports over the given use cases (the composition root and the SQLite tests share this).
pub fn from_use_cases(
    documents: Arc<DocumentUseCase>,
    profile_pdf: Arc<ProfilePdfServiceType>,
) -> DocumentPorts {
    DocumentPorts {
        documents: Some(Arc::new(DocumentAdapter(documents)) as Arc<dyn DocumentPort>),
        profile_pdf: Some(Arc::new(ProfilePdfAdapter(profile_pdf)) as Arc<dyn ProfilePdfPort>),
    }
}

/// The four document tools over the document use cases.
#[derive(Debug)]
struct DocumentAdapter(Arc<DocumentUseCase>);

#[async_trait]
impl DocumentPort for DocumentAdapter {
    async fn store_document_file(&self, request: &Value) -> Result<Value, AppError> {
        Ok(metadata_value(&self.0.store_document_file(request).await?))
    }

    async fn get_document_metadata(&self, document_id: &str) -> Result<Value, AppError> {
        Ok(metadata_value(
            &self.0.get_document_metadata(document_id).await?,
        ))
    }

    async fn extract_pdf_text(&self, request: &Value) -> Result<Value, AppError> {
        Ok(pdf_text_value(&self.0.extract_pdf_text(request).await?))
    }

    async fn extract_stored_pdf_text(&self, request: &Value) -> Result<Value, AppError> {
        Ok(stored_extraction_value(
            &self.0.extract_stored_pdf_text(request).await?,
        ))
    }
}

/// The two profile PDF tools over the ingestion use case.
#[derive(Debug)]
struct ProfilePdfAdapter(Arc<ProfilePdfServiceType>);

#[async_trait]
impl ProfilePdfPort for ProfilePdfAdapter {
    async fn ingest_profile_from_stored_pdf(&self, request: &Value) -> Result<Value, AppError> {
        self.0.ingest_profile_from_stored_pdf(request).await
    }

    async fn get_profile_pdf_source(&self, profile_id: &str) -> Result<Value, AppError> {
        self.0.get_profile_pdf_source(profile_id).await
    }
}

/// A stored document's metadata dictionary, in the tool key order.
fn metadata_value(metadata: &DocumentMetadata) -> Value {
    json!({
        "id": metadata.id.to_string(),
        "originalFileName": metadata.original_file_name,
        "mediaType": metadata.media_type,
        "byteSize": metadata.byte_size,
        "sha256": metadata.sha256,
        "createdAt": metadata.created_at,
        "updatedAt": metadata.updated_at,
    })
}

/// A bounded text view, in the tool key order.
fn pdf_text_value(text: &PdfText) -> Value {
    let pages: Vec<Value> = text
        .pages
        .iter()
        .map(|page| json!({"pageNumber": page.page_number, "text": page.text}))
        .collect();
    json!({
        "fileName": text.file_name,
        "pageCount": text.page_count,
        "characterCount": text.character_count,
        "truncated": text.truncated,
        "text": text.text,
        "pages": pages,
    })
}

/// The stored-extraction dictionary: the document, the extraction identifier and the view.
fn stored_extraction_value(stored: &StoredExtraction) -> Value {
    json!({
        "document": metadata_value(&stored.document),
        "extractionId": stored.extraction_id.map(|id| id.to_string()),
        "extraction": pdf_text_value(&stored.extraction),
    })
}
