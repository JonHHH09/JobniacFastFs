//! The profile PDF tools: `ingest_profile_from_stored_pdf` and `get_profile_pdf_source` (the
//! `ProfilePdfService` port).
//!
//! Each tool decodes its arguments in the shape of its signature (see [`decode`]) and then
//! calls one port method. The port is supplied by the composition root (the CLI); an absent port
//! means absent tools. The errors use the compact envelope text.

mod decode;

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use serde_json::Value;

use crate::registry::{Arguments, RegistryError, ToolHandler, ToolRegistry};
use crate::result::{EnvelopeStyle, ToolResult};

/// The catalog name of `ingest_profile_from_stored_pdf`.
pub const INGEST_PROFILE_FROM_STORED_PDF: &str = "ingest_profile_from_stored_pdf";
/// The catalog name of `get_profile_pdf_source`.
pub const GET_PROFILE_PDF_SOURCE: &str = "get_profile_pdf_source";

/// The envelope style of both profile PDF tools (`profile_pdf_mcp` reuses `profile_mcp._call`).
const STYLE: EnvelopeStyle = EnvelopeStyle::Compact;

/// The stored-PDF profile ingestion and the PDF source lookup (`ProfilePdfService`).
#[async_trait]
pub trait ProfilePdfPort: std::fmt::Debug + Send + Sync {
    /// Populate the profile schema from a stored PDF extraction and link the profile to it.
    async fn ingest_profile_from_stored_pdf(&self, request: &Value) -> Result<Value, AppError>;
    /// The one-to-one PDF extraction source link of a profile.
    async fn get_profile_pdf_source(&self, profile_id: &str) -> Result<Value, AppError>;
}

/// `ingest_profile_from_stored_pdf`: ingest a profile from a stored PDF.
#[derive(Debug)]
pub struct IngestProfileFromStoredPdfTool {
    port: Arc<dyn ProfilePdfPort>,
}

/// `get_profile_pdf_source`: the PDF source of a profile.
#[derive(Debug)]
pub struct GetProfilePdfSourceTool {
    port: Arc<dyn ProfilePdfPort>,
}

impl IngestProfileFromStoredPdfTool {
    /// A handler over the profile PDF port.
    pub fn new(port: Arc<dyn ProfilePdfPort>) -> Self {
        Self { port }
    }
}

impl GetProfilePdfSourceTool {
    /// A handler over the profile PDF port.
    pub fn new(port: Arc<dyn ProfilePdfPort>) -> Self {
        Self { port }
    }
}

#[async_trait]
impl ToolHandler for IngestProfileFromStoredPdfTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::ingest_profile_from_stored_pdf(arguments) {
            Ok(request) => ToolResult::from_outcome(
                self.port.ingest_profile_from_stored_pdf(&request).await,
                STYLE,
            ),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for GetProfilePdfSourceTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::get_profile_pdf_source(arguments) {
            Ok(profile_id) => {
                ToolResult::from_outcome(self.port.get_profile_pdf_source(&profile_id).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

/// Register the profile PDF tools whose port is supplied. An absent port leaves both unregistered.
///
/// # Errors
///
/// A [`RegistryError`] when a name is not in the catalog or is already registered.
pub fn register(
    registry: &mut ToolRegistry,
    port: Option<Arc<dyn ProfilePdfPort>>,
) -> Result<(), RegistryError> {
    let Some(port) = port else {
        return Ok(());
    };
    registry.register(
        INGEST_PROFILE_FROM_STORED_PDF,
        IngestProfileFromStoredPdfTool::new(Arc::clone(&port)),
    )?;
    registry.register(GET_PROFILE_PDF_SOURCE, GetProfilePdfSourceTool::new(port))
}
