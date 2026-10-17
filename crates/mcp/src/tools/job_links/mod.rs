//! The opt-in link tool `add_job_from_link` (the `JobLinkService` port).
//!
//! The tool is registered only when its port is supplied, which the composition root does only when
//! `JOBNIAC_LINK_FETCH_ENABLED` is exactly `1`, the platform is supported and a database is
//! configured. The `request` argument is an untyped model: the SDK-level validation is only
//! "the key is present", and every other check (including the URL rules) is the service's, which
//! answers with the sanitized `validation_error` envelope. A rejected value is therefore never echoed
//! through an argument error (a URL may carry userinfo). The tool reports errors with the compact
//! envelope.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use serde_json::Value;

use crate::args::{ArgDecoder, REQUIRED, request_argument};
use crate::registry::{Arguments, RegistryError, ToolHandler, ToolRegistry};
use crate::result::{EnvelopeStyle, ToolResult};

/// The catalog name of `add_job_from_link`.
pub const ADD_JOB_FROM_LINK: &str = "add_job_from_link";

/// The envelope style of the tool (`job_link_mcp._call` uses compact separators).
const STYLE: EnvelopeStyle = EnvelopeStyle::Compact;

/// The link ingestion use case behind `add_job_from_link`.
#[async_trait]
pub trait JobLinkPort: std::fmt::Debug + Send + Sync {
    /// Ingest the job page named by the `JobLinkRequest` object and return the `{status, job}`
    /// result. The object is passed on unchanged; the service validates it.
    async fn add_job_from_link(&self, request: &Value) -> Result<Value, AppError>;
}

/// `add_job_from_link`: ingest a job from a URL.
#[derive(Debug)]
pub struct AddJobFromLinkTool {
    port: Arc<dyn JobLinkPort>,
}

impl AddJobFromLinkTool {
    /// A handler over the link port.
    pub fn new(port: Arc<dyn JobLinkPort>) -> Self {
        Self { port }
    }
}

#[async_trait]
impl ToolHandler for AddJobFromLinkTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        let mut decoder = ArgDecoder::new(ADD_JOB_FROM_LINK);
        decoder.field(arguments, "", "request", REQUIRED, any_value);
        match decoder.check() {
            Ok(()) => ToolResult::from_outcome(
                self.port
                    .add_job_from_link(&request_argument(arguments))
                    .await,
                STYLE,
            ),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

/// The `Any` argument model: every present value is accepted, so only a missing key is an SDK error.
fn any_value(
    _location: &str,
    _value: &Value,
    _issues: &mut Vec<crate::args::ArgIssue>,
) -> Option<()> {
    Some(())
}

/// Register `add_job_from_link` when its port is supplied. An absent port leaves it unregistered.
///
/// # Errors
///
/// A [`RegistryError`] when the name is not in the catalog or is already registered.
pub fn register(
    registry: &mut ToolRegistry,
    port: Option<Arc<dyn JobLinkPort>>,
) -> Result<(), RegistryError> {
    if let Some(port) = port {
        registry.register(ADD_JOB_FROM_LINK, AddJobFromLinkTool::new(port))?;
    }
    Ok(())
}
