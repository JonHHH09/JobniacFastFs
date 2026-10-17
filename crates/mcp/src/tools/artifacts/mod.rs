//! The opt-in artifact tool `get_document_artifact`.
//!
//! The tool is registered only when the composition root supplies the artifact port, which it does
//! only when `JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED` is exactly `1` and a database is configured. Every
//! failure is answered with the envelope text in the default separators and `isError` set, so this
//! module reuses the document envelope style.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use serde_json::Value;

use crate::args::{ArgDecoder, REQUIRED, strict_string};
use crate::registry::{Arguments, RegistryError, ToolHandler, ToolRegistry};
use crate::result::{EnvelopeStyle, ToolResult};

/// The catalog name of `get_document_artifact`.
pub const GET_DOCUMENT_ARTIFACT: &str = "get_document_artifact";

/// The envelope style of the tool (the inline handler uses the default separators).
const STYLE: EnvelopeStyle = EnvelopeStyle::Default;

/// The artifact use case behind the tool: the `{document, contentBase64}` answer for one document.
#[async_trait]
pub trait ArtifactPort: std::fmt::Debug + Send + Sync {
    /// The verified artifact of one stored document, or an application error.
    async fn get_document_artifact(&self, document_id: &str) -> Result<Value, AppError>;
}

/// `get_document_artifact`: read-only retrieval of one verified stored PDF.
#[derive(Debug)]
pub struct GetDocumentArtifactTool {
    port: Arc<dyn ArtifactPort>,
}

impl GetDocumentArtifactTool {
    /// A handler over the artifact port.
    pub fn new(port: Arc<dyn ArtifactPort>) -> Self {
        Self { port }
    }
}

#[async_trait]
impl ToolHandler for GetDocumentArtifactTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        let mut decoder = ArgDecoder::new(GET_DOCUMENT_ARTIFACT);
        let document_id = decoder.field(arguments, "", "documentId", REQUIRED, strict_string);
        match decoder.check() {
            Ok(()) => {
                let document_id = document_id.into_value().unwrap_or_default();
                ToolResult::from_outcome(self.port.get_document_artifact(&document_id).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

/// Register `get_document_artifact` when its port is supplied. An absent port leaves it
/// unregistered, so the tool is absent from `tools/list` (default-off).
///
/// # Errors
///
/// A [`RegistryError`] when the name is not in the catalog or is already registered.
pub fn register(
    registry: &mut ToolRegistry,
    port: Option<Arc<dyn ArtifactPort>>,
) -> Result<(), RegistryError> {
    let Some(port) = port else {
        return Ok(());
    };
    registry.register(GET_DOCUMENT_ARTIFACT, GetDocumentArtifactTool::new(port))
}
