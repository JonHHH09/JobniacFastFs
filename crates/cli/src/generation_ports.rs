//! The MCP port of the six generation tools over the generation facade.
//!
//! Like [`crate::document_ports`], the composition root is the only place that knows both the MCP
//! port (`jobniac-mcp`) and the concrete facade (`jobniac-core` over `jobniac-infra`). The facade
//! already answers with the tool metadata objects, so the adapter only forwards each call.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_core::generation::facade::GenerationService;
use jobniac_mcp::tools::generation::GenerationPort;
use serde_json::Value;

/// The six generation tools over the generation facade.
#[derive(Debug)]
pub struct GenerationAdapter(Arc<GenerationService>);

/// The MCP port of the composed generation facade.
#[must_use]
pub fn from_service(service: &Arc<GenerationService>) -> Arc<dyn GenerationPort> {
    Arc::new(GenerationAdapter(Arc::clone(service)))
}

#[async_trait]
impl GenerationPort for GenerationAdapter {
    async fn generate_pdf_file(&self, request: &Value) -> Result<Value, AppError> {
        Ok(self.0.generate_pdf_file(request).await?)
    }

    async fn generate_pdf_resume(&self, request: &Value) -> Result<Value, AppError> {
        Ok(self.0.generate_pdf_resume(request).await?)
    }

    async fn generate_canadian_pdf_resume(&self, request: &Value) -> Result<Value, AppError> {
        Ok(self.0.generate_canadian_pdf_resume(request).await?)
    }

    async fn generate_canadian_french_pdf_resume(
        &self,
        request: &Value,
    ) -> Result<Value, AppError> {
        Ok(self.0.generate_canadian_french_pdf_resume(request).await?)
    }

    async fn generate_german_tailored_resume(&self, request: &Value) -> Result<Value, AppError> {
        Ok(self.0.generate_german_tailored_resume(request).await?)
    }

    async fn generate_german_cover_letter(&self, request: &Value) -> Result<Value, AppError> {
        Ok(self.0.generate_german_cover_letter(request).await?)
    }
}
