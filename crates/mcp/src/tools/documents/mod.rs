//! The document tools: `store_document_file`, `get_document_metadata`, `extract_pdf_text` and
//! `extract_stored_pdf_text` (the `DocumentService` port).
//!
//! Each tool decodes its arguments in the shape of its signature (see [`decode`]) and then
//! calls one port method with the decoded request object or identifier. A port answers with the
//! service result, as a JSON value, or with an [`AppError`]. The port is
//! supplied by the composition root (the CLI), so this crate does not depend on the PDF worker or
//! the import root; an absent port means absent tools. The errors are reported with the default
//! envelope text.

mod decode;

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use serde_json::Value;

use crate::registry::{Arguments, RegistryError, ToolHandler, ToolRegistry};
use crate::result::{EnvelopeStyle, ToolResult};

/// The catalog name of `store_document_file`.
pub const STORE_DOCUMENT_FILE: &str = "store_document_file";
/// The catalog name of `get_document_metadata`.
pub const GET_DOCUMENT_METADATA: &str = "get_document_metadata";
/// The catalog name of `extract_pdf_text`.
pub const EXTRACT_PDF_TEXT: &str = "extract_pdf_text";
/// The catalog name of `extract_stored_pdf_text`.
pub const EXTRACT_STORED_PDF_TEXT: &str = "extract_stored_pdf_text";

/// The envelope style of every document tool (`document_mcp._call` uses the default separators).
const STYLE: EnvelopeStyle = EnvelopeStyle::Default;

/// The document use cases (`DocumentService`). Each method takes the decoded request of its tool,
/// with the keys the `TypedDict` declares; `get_document_metadata` takes the identifier text
/// as sent, which the service parses.
#[async_trait]
pub trait DocumentPort: std::fmt::Debug + Send + Sync {
    /// Store a PDF read from the import root, with its metadata only in the answer.
    async fn store_document_file(&self, request: &Value) -> Result<Value, AppError>;
    /// The metadata of one stored document, without its bytes.
    async fn get_document_metadata(&self, document_id: &str) -> Result<Value, AppError>;
    /// Bounded text of a PDF read from the import root, without storing it.
    async fn extract_pdf_text(&self, request: &Value) -> Result<Value, AppError>;
    /// Bounded text of a stored PDF, optionally persisting its extraction.
    async fn extract_stored_pdf_text(&self, request: &Value) -> Result<Value, AppError>;
}

/// `store_document_file`: store one PDF from the import root.
#[derive(Debug)]
pub struct StoreDocumentFileTool {
    port: Arc<dyn DocumentPort>,
}

/// `get_document_metadata`: the metadata of one stored document.
#[derive(Debug)]
pub struct GetDocumentMetadataTool {
    port: Arc<dyn DocumentPort>,
}

/// `extract_pdf_text`: bounded text of an import-root PDF.
#[derive(Debug)]
pub struct ExtractPdfTextTool {
    port: Arc<dyn DocumentPort>,
}

/// `extract_stored_pdf_text`: bounded text of a stored PDF.
#[derive(Debug)]
pub struct ExtractStoredPdfTextTool {
    port: Arc<dyn DocumentPort>,
}

impl StoreDocumentFileTool {
    /// A handler over the document port.
    pub fn new(port: Arc<dyn DocumentPort>) -> Self {
        Self { port }
    }
}

impl GetDocumentMetadataTool {
    /// A handler over the document port.
    pub fn new(port: Arc<dyn DocumentPort>) -> Self {
        Self { port }
    }
}

impl ExtractPdfTextTool {
    /// A handler over the document port.
    pub fn new(port: Arc<dyn DocumentPort>) -> Self {
        Self { port }
    }
}

impl ExtractStoredPdfTextTool {
    /// A handler over the document port.
    pub fn new(port: Arc<dyn DocumentPort>) -> Self {
        Self { port }
    }
}

#[async_trait]
impl ToolHandler for StoreDocumentFileTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::store_document_file(arguments) {
            Ok(request) => {
                ToolResult::from_outcome(self.port.store_document_file(&request).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for GetDocumentMetadataTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::get_document_metadata(arguments) {
            Ok(document_id) => {
                ToolResult::from_outcome(self.port.get_document_metadata(&document_id).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for ExtractPdfTextTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::extract_pdf_text(arguments) {
            Ok(request) => {
                ToolResult::from_outcome(self.port.extract_pdf_text(&request).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for ExtractStoredPdfTextTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::extract_stored_pdf_text(arguments) {
            Ok(request) => {
                ToolResult::from_outcome(self.port.extract_stored_pdf_text(&request).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

/// Register the document tools whose port is supplied. An absent port leaves all four unregistered,
/// as for a server without a document service.
///
/// # Errors
///
/// A [`RegistryError`] when a name is not in the catalog or is already registered.
pub fn register(
    registry: &mut ToolRegistry,
    port: Option<Arc<dyn DocumentPort>>,
) -> Result<(), RegistryError> {
    let Some(port) = port else {
        return Ok(());
    };
    registry.register(
        STORE_DOCUMENT_FILE,
        StoreDocumentFileTool::new(Arc::clone(&port)),
    )?;
    registry.register(
        GET_DOCUMENT_METADATA,
        GetDocumentMetadataTool::new(Arc::clone(&port)),
    )?;
    registry.register(EXTRACT_PDF_TEXT, ExtractPdfTextTool::new(Arc::clone(&port)))?;
    registry.register(EXTRACT_STORED_PDF_TEXT, ExtractStoredPdfTextTool::new(port))
}
