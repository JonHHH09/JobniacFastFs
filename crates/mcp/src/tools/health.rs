//! The `health` tool: the shared health report, passed through unchanged.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_core::health::HealthService;
use serde_json::Value;

use crate::registry::{Arguments, ToolHandler};
use crate::result::{EnvelopeStyle, ToolResult};

/// The catalog name of the tool.
pub const NAME: &str = "health";

/// Answers `health` from the shared health use case.
#[derive(Debug)]
pub struct HealthTool {
    service: Arc<HealthService>,
}

impl HealthTool {
    /// A handler over the shared health service.
    pub const fn new(service: Arc<HealthService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl ToolHandler for HealthTool {
    async fn call(&self, _arguments: &Arguments) -> ToolResult {
        let report = self.service.check().await;
        report_result(serde_json::to_value(&report))
    }
}

/// Maps the serialized health report to the tool result.
///
/// A failed serialization answers with the sanitized `internal_error` envelope, not the SDK's
/// generic `Error executing tool health` text; the serializer's own text is dropped, so it never
/// reaches the client.
pub(crate) fn report_result(serialized: Result<Value, serde_json::Error>) -> ToolResult {
    match serialized {
        Ok(value) => ToolResult::Success(value),
        Err(_) => ToolResult::from_outcome(Err(AppError::Internal), EnvelopeStyle::Compact),
    }
}

#[cfg(test)]
mod tests {
    use super::report_result;

    #[test]
    fn a_failed_report_is_the_sanitized_internal_error() -> Result<(), Box<dyn std::error::Error>> {
        let failure = serde_json::from_str::<serde_json::Value>("{ secret-detail").err();
        let error = failure.ok_or("the probe input parsed")?;
        let wire = serde_json::to_value(report_result(Err(error)).into_call_result())?;

        assert_eq!(wire["isError"], serde_json::Value::Bool(true));
        assert_eq!(wire["structuredContent"]["code"], "internal_error");
        assert!(!wire.to_string().contains("secret-detail"));
        assert!(
            !wire.to_string().contains("column"),
            "serializer text leaked"
        );
        Ok(())
    }
}
