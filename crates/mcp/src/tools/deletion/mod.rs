//! The destructive tools `delete_profile` and `delete_job` (the `ProfileDeletionService` and
//! `JobDeletionService` ports).
//!
//! The tools are opt-in. Each one registers only when its port is supplied, so the composition root
//! adds them only when `JOBNIAC_DELETION_ENABLED` is exactly `1` and a database is configured. The
//! MCP contract has no revision and no confirmation: the desktop confirmation token is a
//! desktop concept and never reaches this module. The identifier is passed on as sent; the service
//! admits it as a UUID and answers `validation_error` for a bad one. Both tools report errors with
//! the compact envelope.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use serde_json::Value;

use crate::args::{ArgDecoder, ArgErrors, REQUIRED, strict_string};
use crate::registry::{Arguments, RegistryError, ToolHandler, ToolRegistry};
use crate::result::{EnvelopeStyle, ToolResult};

/// The catalog name of `delete_profile`.
pub const DELETE_PROFILE: &str = "delete_profile";
/// The catalog name of `delete_job`.
pub const DELETE_JOB: &str = "delete_job";

/// The envelope style of both tools (`entity_deletion_mcp._call` uses compact separators).
const STYLE: EnvelopeStyle = EnvelopeStyle::Compact;

/// The profile hard-delete use case behind `delete_profile`.
#[async_trait]
pub trait ProfileDeletionPort: std::fmt::Debug + Send + Sync {
    /// Delete the profile named by `profile_id` and return `{profileId, deleted}`.
    async fn delete_profile(&self, profile_id: &str) -> Result<Value, AppError>;
}

/// The job hard-delete use case behind `delete_job`.
#[async_trait]
pub trait JobDeletionPort: std::fmt::Debug + Send + Sync {
    /// Delete the job named by `job_id` and return `{jobId, deleted}`.
    async fn delete_job(&self, job_id: &str) -> Result<Value, AppError>;
}

/// The optional deletion ports of the server. An absent port leaves its tool unregistered.
#[derive(Debug, Clone, Default)]
pub struct DeletionPorts {
    /// The port behind `delete_profile`.
    pub profile: Option<Arc<dyn ProfileDeletionPort>>,
    /// The port behind `delete_job`.
    pub job: Option<Arc<dyn JobDeletionPort>>,
}

/// `delete_profile`: hard delete of a profile aggregate.
#[derive(Debug)]
pub struct DeleteProfileTool {
    port: Arc<dyn ProfileDeletionPort>,
}

/// `delete_job`: hard delete of a stored job.
#[derive(Debug)]
pub struct DeleteJobTool {
    port: Arc<dyn JobDeletionPort>,
}

impl DeleteProfileTool {
    /// A handler over the profile deletion port.
    pub fn new(port: Arc<dyn ProfileDeletionPort>) -> Self {
        Self { port }
    }
}

impl DeleteJobTool {
    /// A handler over the job deletion port.
    pub fn new(port: Arc<dyn JobDeletionPort>) -> Self {
        Self { port }
    }
}

#[async_trait]
impl ToolHandler for DeleteProfileTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode_id(DELETE_PROFILE, "profileId", arguments) {
            Ok(profile_id) => {
                ToolResult::from_outcome(self.port.delete_profile(&profile_id).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for DeleteJobTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode_id(DELETE_JOB, "jobId", arguments) {
            Ok(job_id) => ToolResult::from_outcome(self.port.delete_job(&job_id).await, STYLE),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

/// Decodes the one required bare string argument of a deletion tool.
fn decode_id(tool: &'static str, key: &str, arguments: &Arguments) -> Result<String, ArgErrors> {
    let mut decoder = ArgDecoder::new(tool);
    let id = decoder.field(arguments, "", key, REQUIRED, strict_string);
    decoder.check()?;
    Ok(id.into_value().unwrap_or_default())
}

/// Register the deletion tools whose ports are supplied. Each tool is gated on its own port.
///
/// # Errors
///
/// A [`RegistryError`] when a name is not in the catalog or is already registered.
pub fn register(registry: &mut ToolRegistry, ports: DeletionPorts) -> Result<(), RegistryError> {
    if let Some(port) = ports.profile {
        registry.register(DELETE_PROFILE, DeleteProfileTool::new(port))?;
    }
    if let Some(port) = ports.job {
        registry.register(DELETE_JOB, DeleteJobTool::new(port))?;
    }
    Ok(())
}
