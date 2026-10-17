//! The MCP port of `get_document_artifact` over the composed artifact use case.
//!
//! The composition root is the only place that knows both the MCP port (`jobniac-mcp`) and the
//! concrete use case (`jobniac-core` over `jobniac-infra`). The port is absent unless the artifact
//! services were composed, which happens only when the flag is exactly `1` and a database is set.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_mcp::tools::artifacts::ArtifactPort;
use jobniac_runtime::artifacts::{ArtifactServices, ArtifactUseCase};
use serde_json::Value;

/// The `get_document_artifact` port over the artifact use case.
#[derive(Debug)]
struct ArtifactAdapter(Arc<ArtifactUseCase>);

#[async_trait]
impl ArtifactPort for ArtifactAdapter {
    async fn get_document_artifact(&self, document_id: &str) -> Result<Value, AppError> {
        self.0.get_document_artifact(document_id).await
    }
}

/// The MCP port of the composed artifact services, or `None` when they were not composed.
#[must_use]
pub fn from_services(services: &Option<ArtifactServices>) -> Option<Arc<dyn ArtifactPort>> {
    services.as_ref().map(|services| {
        Arc::new(ArtifactAdapter(Arc::clone(&services.service))) as Arc<dyn ArtifactPort>
    })
}
