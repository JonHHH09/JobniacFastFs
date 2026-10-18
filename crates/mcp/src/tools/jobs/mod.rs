//! The job tools: `add_job_from_text`, `get_job`, `list_jobs` and `update_job` (the
//! `JobService` port) and `search_jobs` (the `JobSearchService` port, the same module).
//!
//! Each tool decodes its arguments in the shape of its signature (see [`decode`]) and then
//! calls one port. A port answers with the service result, as a JSON value,
//! or with an [`AppError`]. The ports are supplied by the composition root (the CLI), so this crate
//! does not depend on the SQLite adapter; an absent port means absent tools. Every job tool
//! reports its errors with the compact envelope, and validation of the request
//! happens in the service, so a rejected value is never echoed.

mod decode;

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use serde_json::Value;

use crate::args::request_argument;
use crate::registry::{Arguments, RegistryError, ToolHandler, ToolRegistry};
use crate::result::{EnvelopeStyle, ToolResult};

/// The catalog name of `add_job_from_text`.
pub const ADD_JOB_FROM_TEXT: &str = "add_job_from_text";
/// The catalog name of `get_job`.
pub const GET_JOB: &str = "get_job";
/// The catalog name of `list_jobs`.
pub const LIST_JOBS: &str = "list_jobs";
/// The catalog name of `update_job`.
pub const UPDATE_JOB: &str = "update_job";
/// The catalog name of `search_jobs`.
pub const SEARCH_JOBS: &str = "search_jobs";

/// The envelope style of every job tool (`job_mcp._call` uses compact separators).
const STYLE: EnvelopeStyle = EnvelopeStyle::Compact;

/// The job capture, get, keyset list and revision-checked update use cases (`JobService`).
#[async_trait]
pub trait JobPort: std::fmt::Debug + Send + Sync {
    /// Capture pasted text from the `JobTextRequest` object and return the `{status, job}` result.
    async fn add_job_from_text(&self, request: &Value) -> Result<Value, AppError>;
    /// One page of job rows: the service dictionary with `jobs` and `nextCursor`.
    async fn list_jobs(&self, limit: Option<i64>, cursor: Option<&str>) -> Result<Value, AppError>;
    /// The aggregate of one job. `job_id` is the text of the argument, as the client sent it.
    async fn get_job(&self, job_id: &str) -> Result<Value, AppError>;
    /// Patch a job from the `JobUpdateRequest` object under its revision check.
    async fn update_job(&self, request: &Value) -> Result<Value, AppError>;
}

/// The bounded job search (`JobSearchService`).
#[async_trait]
pub trait JobSearchPort: std::fmt::Debug + Send + Sync {
    /// Search from the `JobSearchRequest` object and return the search envelope.
    async fn search_jobs(&self, request: &Value) -> Result<Value, AppError>;
}

/// `add_job_from_text`: capture pasted text as a job.
#[derive(Debug)]
pub struct AddJobFromTextTool {
    port: Arc<dyn JobPort>,
}

/// `get_job`: one job aggregate by its identifier.
#[derive(Debug)]
pub struct GetJobTool {
    port: Arc<dyn JobPort>,
}

/// `list_jobs`: one bounded page of job rows.
#[derive(Debug)]
pub struct ListJobsTool {
    port: Arc<dyn JobPort>,
}

/// `update_job`: patch a job under the revision check.
#[derive(Debug)]
pub struct UpdateJobTool {
    port: Arc<dyn JobPort>,
}

/// `search_jobs`: bounded search over the job posting index.
#[derive(Debug)]
pub struct SearchJobsTool {
    port: Arc<dyn JobSearchPort>,
}

impl AddJobFromTextTool {
    /// A handler over the job port.
    pub fn new(port: Arc<dyn JobPort>) -> Self {
        Self { port }
    }
}

impl GetJobTool {
    /// A handler over the job port.
    pub fn new(port: Arc<dyn JobPort>) -> Self {
        Self { port }
    }
}

impl ListJobsTool {
    /// A handler over the job port.
    pub fn new(port: Arc<dyn JobPort>) -> Self {
        Self { port }
    }
}

impl UpdateJobTool {
    /// A handler over the job port.
    pub fn new(port: Arc<dyn JobPort>) -> Self {
        Self { port }
    }
}

impl SearchJobsTool {
    /// A handler over the search port.
    pub fn new(port: Arc<dyn JobSearchPort>) -> Self {
        Self { port }
    }
}

#[async_trait]
impl ToolHandler for AddJobFromTextTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::add_job_from_text(arguments) {
            Ok(()) => ToolResult::from_outcome(
                self.port
                    .add_job_from_text(&request_argument(arguments))
                    .await,
                STYLE,
            ),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for GetJobTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::get_job(arguments) {
            Ok(job_id) => ToolResult::from_outcome(self.port.get_job(&job_id).await, STYLE),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for ListJobsTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::list_jobs(arguments) {
            Ok(page) => ToolResult::from_outcome(
                self.port
                    .list_jobs(page.limit, page.cursor.as_deref())
                    .await,
                STYLE,
            ),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for UpdateJobTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::update_job(arguments) {
            Ok(()) => ToolResult::from_outcome(
                self.port.update_job(&request_argument(arguments)).await,
                STYLE,
            ),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for SearchJobsTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::search_jobs(arguments) {
            Ok(()) => ToolResult::from_outcome(
                self.port.search_jobs(&request_argument(arguments)).await,
                STYLE,
            ),
            Err(errors) => ToolResult::from(errors),
        }
    }
}

/// Register the job tools whose ports are supplied. The capture, get, list and update tools are gated
/// on the job port. `search_jobs` is gated on its own search port: the separate gate is a
/// composition choice (the UI reports `hasJobSearch` apart from `hasJobService`). An absent port
/// leaves its tools unregistered.
///
/// # Errors
///
/// A [`RegistryError`] when a name is not in the catalog or is already registered.
pub fn register(
    registry: &mut ToolRegistry,
    service: Option<Arc<dyn JobPort>>,
    search: Option<Arc<dyn JobSearchPort>>,
) -> Result<(), RegistryError> {
    if let Some(port) = service {
        registry.register(
            ADD_JOB_FROM_TEXT,
            AddJobFromTextTool::new(Arc::clone(&port)),
        )?;
        registry.register(GET_JOB, GetJobTool::new(Arc::clone(&port)))?;
        registry.register(LIST_JOBS, ListJobsTool::new(Arc::clone(&port)))?;
        registry.register(UPDATE_JOB, UpdateJobTool::new(port))?;
    }
    if let Some(port) = search {
        registry.register(SEARCH_JOBS, SearchJobsTool::new(port))?;
    }
    Ok(())
}
