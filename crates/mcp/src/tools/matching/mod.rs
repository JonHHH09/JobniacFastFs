//! The matching tools: `analyze_job_match`, `analyze_all_job_matches`, `get_match_report` and
//! `list_match_reports` (the `MatchingService` port).
//!
//! Each tool decodes its `request` in the shape of its `TypedDict` (see [`decode`]) and then
//! calls one port with the decoded request object. A port answers with the service result, as
//! a JSON value, or with an [`AppError`]. The port is supplied by the
//! composition root (the CLI), so this crate does not depend on the PostgreSQL adapter; an absent
//! port means absent tools. Every matching tool reports its errors with the default envelope text
//! (the default separators), and the service validates the values.

mod decode;

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use serde_json::Value;

use crate::registry::{Arguments, RegistryError, ToolHandler, ToolRegistry};
use crate::result::{EnvelopeStyle, ToolResult};

/// The catalog name of `analyze_job_match`.
pub const ANALYZE_JOB_MATCH: &str = "analyze_job_match";
/// The catalog name of `analyze_all_job_matches`.
pub const ANALYZE_ALL_JOB_MATCHES: &str = "analyze_all_job_matches";
/// The catalog name of `get_match_report`.
pub const GET_MATCH_REPORT: &str = "get_match_report";
/// The catalog name of `list_match_reports`.
pub const LIST_MATCH_REPORTS: &str = "list_match_reports";

/// The envelope style of every matching tool (`match_mcp._call` uses the default separators).
const STYLE: EnvelopeStyle = EnvelopeStyle::Default;

/// The deterministic matching use cases (`MatchingService`). Each method takes the decoded
/// `request` object of its tool, with the keys the `TypedDict` declares.
#[async_trait]
pub trait MatchingPort: std::fmt::Debug + Send + Sync {
    /// Create or reuse the report of one profile and one job.
    async fn analyze_job_match(&self, request: &Value) -> Result<Value, AppError>;
    /// Analyze one bounded page of a profile's jobs; pair failures are reported inside the page.
    async fn analyze_all_job_matches(&self, request: &Value) -> Result<Value, AppError>;
    /// One stored report with its staleness against the current source revisions.
    async fn get_match_report(&self, request: &Value) -> Result<Value, AppError>;
    /// One bounded page of stored reports, optionally filtered by profile and job.
    async fn list_match_reports(&self, request: &Value) -> Result<Value, AppError>;
}

/// `analyze_job_match`: create or reuse one report.
#[derive(Debug)]
pub struct AnalyzeJobMatchTool {
    port: Arc<dyn MatchingPort>,
}

/// `analyze_all_job_matches`: analyze one bounded page of jobs.
#[derive(Debug)]
pub struct AnalyzeAllJobMatchesTool {
    port: Arc<dyn MatchingPort>,
}

/// `get_match_report`: one stored report.
#[derive(Debug)]
pub struct GetMatchReportTool {
    port: Arc<dyn MatchingPort>,
}

/// `list_match_reports`: one bounded page of stored reports.
#[derive(Debug)]
pub struct ListMatchReportsTool {
    port: Arc<dyn MatchingPort>,
}

impl AnalyzeJobMatchTool {
    /// A handler over the matching port.
    pub fn new(port: Arc<dyn MatchingPort>) -> Self {
        Self { port }
    }
}

impl AnalyzeAllJobMatchesTool {
    /// A handler over the matching port.
    pub fn new(port: Arc<dyn MatchingPort>) -> Self {
        Self { port }
    }
}

impl GetMatchReportTool {
    /// A handler over the matching port.
    pub fn new(port: Arc<dyn MatchingPort>) -> Self {
        Self { port }
    }
}

impl ListMatchReportsTool {
    /// A handler over the matching port.
    pub fn new(port: Arc<dyn MatchingPort>) -> Self {
        Self { port }
    }
}

#[async_trait]
impl ToolHandler for AnalyzeJobMatchTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::analyze_job_match(arguments) {
            Ok(request) => {
                ToolResult::from_outcome(self.port.analyze_job_match(&request).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for AnalyzeAllJobMatchesTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::analyze_all_job_matches(arguments) {
            Ok(request) => {
                ToolResult::from_outcome(self.port.analyze_all_job_matches(&request).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for GetMatchReportTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::get_match_report(arguments) {
            Ok(request) => {
                ToolResult::from_outcome(self.port.get_match_report(&request).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

#[async_trait]
impl ToolHandler for ListMatchReportsTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode::list_match_reports(arguments) {
            Ok(request) => {
                ToolResult::from_outcome(self.port.list_match_reports(&request).await, STYLE)
            }
            Err(errors) => ToolResult::from(errors),
        }
    }
}

/// Register the matching tools whose port is supplied. An absent port leaves all four unregistered,
/// as for a server without a matching service.
///
/// # Errors
///
/// A [`RegistryError`] when a name is not in the catalog or is already registered.
pub fn register(
    registry: &mut ToolRegistry,
    port: Option<Arc<dyn MatchingPort>>,
) -> Result<(), RegistryError> {
    let Some(port) = port else {
        return Ok(());
    };
    registry.register(
        ANALYZE_JOB_MATCH,
        AnalyzeJobMatchTool::new(Arc::clone(&port)),
    )?;
    registry.register(
        ANALYZE_ALL_JOB_MATCHES,
        AnalyzeAllJobMatchesTool::new(Arc::clone(&port)),
    )?;
    registry.register(GET_MATCH_REPORT, GetMatchReportTool::new(Arc::clone(&port)))?;
    registry.register(LIST_MATCH_REPORTS, ListMatchReportsTool::new(port))
}
