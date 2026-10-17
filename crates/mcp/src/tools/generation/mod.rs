//! The generation tools: `generate_pdf_file`, `generate_pdf_resume`, `generate_canadian_pdf_resume`,
//! `generate_canadian_french_pdf_resume`, `generate_german_tailored_resume` and
//! `generate_german_cover_letter` (the `GenerationService` port).
//!
//! Each tool decodes its one `request` argument in the shape of its `TypedDict` (see
//! [`decode`]) and forwards the declared keys to one facade method. The facade answers with the
//! metadata object, or with an [`AppError`]. Generation is default-registered: when generation is
//! not enabled or the root is not qualified, the facade refuses at call time with the same
//! configuration error, and no storage or render call is made. The errors use
//! the default envelope text, shared with the document tools.

mod decode;

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use serde_json::Value;

use crate::registry::{Arguments, RegistryError, ToolHandler, ToolRegistry};
use crate::result::{EnvelopeStyle, ToolResult};

/// The catalog name of `generate_pdf_file`.
pub const GENERATE_PDF_FILE: &str = "generate_pdf_file";
/// The catalog name of `generate_pdf_resume`.
pub const GENERATE_PDF_RESUME: &str = "generate_pdf_resume";
/// The catalog name of `generate_canadian_pdf_resume`.
pub const GENERATE_CANADIAN_PDF_RESUME: &str = "generate_canadian_pdf_resume";
/// The catalog name of `generate_canadian_french_pdf_resume`.
pub const GENERATE_CANADIAN_FRENCH_PDF_RESUME: &str = "generate_canadian_french_pdf_resume";
/// The catalog name of `generate_german_tailored_resume`.
pub const GENERATE_GERMAN_TAILORED_RESUME: &str = "generate_german_tailored_resume";
/// The catalog name of `generate_german_cover_letter`.
pub const GENERATE_GERMAN_COVER_LETTER: &str = "generate_german_cover_letter";

/// The envelope style of every generation tool (`generation_mcp` uses `document_mcp._call`).
const STYLE: EnvelopeStyle = EnvelopeStyle::Default;

/// The generation use cases (`GenerationService`). Each method takes the decoded request object of
/// its tool with the keys the `TypedDict` declares.
#[async_trait]
pub trait GenerationPort: std::fmt::Debug + Send + Sync {
    /// Generate a standalone stored PDF and return its committed file metadata.
    async fn generate_pdf_file(&self, request: &Value) -> Result<Value, AppError>;
    /// Generate and link the current profile master resume PDF.
    async fn generate_pdf_resume(&self, request: &Value) -> Result<Value, AppError>;
    /// Generate and link the current Canadian profile resume PDF.
    async fn generate_canadian_pdf_resume(&self, request: &Value) -> Result<Value, AppError>;
    /// Generate and link the current French Canadian profile resume PDF.
    async fn generate_canadian_french_pdf_resume(&self, request: &Value)
    -> Result<Value, AppError>;
    /// Generate the current job-specific English and German resume variants.
    async fn generate_german_tailored_resume(&self, request: &Value) -> Result<Value, AppError>;
    /// Generate a German cover letter for the exact selected current resume.
    async fn generate_german_cover_letter(&self, request: &Value) -> Result<Value, AppError>;
}

/// The six generation operations, one per catalog name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// `generate_pdf_file`.
    PdfFile,
    /// `generate_pdf_resume`.
    PdfResume,
    /// `generate_canadian_pdf_resume`.
    CanadianPdfResume,
    /// `generate_canadian_french_pdf_resume`.
    CanadianFrenchPdfResume,
    /// `generate_german_tailored_resume`.
    GermanTailoredResume,
    /// `generate_german_cover_letter`.
    GermanCoverLetter,
}

impl Operation {
    /// The catalog name of the operation.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PdfFile => GENERATE_PDF_FILE,
            Self::PdfResume => GENERATE_PDF_RESUME,
            Self::CanadianPdfResume => GENERATE_CANADIAN_PDF_RESUME,
            Self::CanadianFrenchPdfResume => GENERATE_CANADIAN_FRENCH_PDF_RESUME,
            Self::GermanTailoredResume => GENERATE_GERMAN_TAILORED_RESUME,
            Self::GermanCoverLetter => GENERATE_GERMAN_COVER_LETTER,
        }
    }

    /// Every operation, in catalog order.
    pub const ALL: [Self; 6] = [
        Self::PdfFile,
        Self::PdfResume,
        Self::CanadianPdfResume,
        Self::CanadianFrenchPdfResume,
        Self::GermanTailoredResume,
        Self::GermanCoverLetter,
    ];
}

/// One generation tool: the operation it forwards to the port.
#[derive(Debug)]
pub struct GenerationTool {
    port: Arc<dyn GenerationPort>,
    operation: Operation,
}

impl GenerationTool {
    /// A handler of `operation` over the generation port.
    #[must_use]
    pub fn new(port: Arc<dyn GenerationPort>, operation: Operation) -> Self {
        Self { port, operation }
    }
}

#[async_trait]
impl ToolHandler for GenerationTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        let name = self.operation.name();
        match decode::request(name, arguments) {
            Ok(request) => {
                let outcome = match self.operation {
                    Operation::PdfFile => self.port.generate_pdf_file(&request).await,
                    Operation::PdfResume => self.port.generate_pdf_resume(&request).await,
                    Operation::CanadianPdfResume => {
                        self.port.generate_canadian_pdf_resume(&request).await
                    }
                    Operation::CanadianFrenchPdfResume => {
                        self.port
                            .generate_canadian_french_pdf_resume(&request)
                            .await
                    }
                    Operation::GermanTailoredResume => {
                        self.port.generate_german_tailored_resume(&request).await
                    }
                    Operation::GermanCoverLetter => {
                        self.port.generate_german_cover_letter(&request).await
                    }
                };
                ToolResult::from_outcome(outcome, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

/// Register the six generation tools over `port`. An absent port leaves all six unregistered.
///
/// # Errors
///
/// A [`RegistryError`] when a name is not in the catalog or is already registered.
pub fn register(
    registry: &mut ToolRegistry,
    port: Option<Arc<dyn GenerationPort>>,
) -> Result<(), RegistryError> {
    let Some(port) = port else {
        return Ok(());
    };
    for operation in Operation::ALL {
        registry.register(
            operation.name(),
            GenerationTool::new(Arc::clone(&port), operation),
        )?;
    }
    Ok(())
}
