//! The MCP port of `add_job_from_link` over the composed link use case.
//!
//! Like [`crate::job_ports`], the composition root is the only place that knows both the MCP port
//! (`jobniac-mcp`) and the concrete use case (`jobniac-core` over `jobniac-infra`). The adapter only
//! forwards the request and converts the typed `{status, job}` result into its dictionary.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_mcp::tools::job_links::JobLinkPort;
use jobniac_runtime::job_links::JobLinkUseCase;
use serde_json::Value;

use crate::profile_ports::to_dictionary;

/// The link use case as the `add_job_from_link` port.
#[derive(Debug)]
struct LinkAdapter(Arc<JobLinkUseCase>);

/// The MCP port of the composed link use case. Absent when the use case is not composed, which
/// leaves `add_job_from_link` unregistered.
#[must_use]
pub fn from_service(service: Option<&Arc<JobLinkUseCase>>) -> Option<Arc<dyn JobLinkPort>> {
    service.map(|service| Arc::new(LinkAdapter(Arc::clone(service))) as Arc<dyn JobLinkPort>)
}

#[async_trait]
impl JobLinkPort for LinkAdapter {
    async fn add_job_from_link(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.add_job_from_link(request).await?)
    }
}
