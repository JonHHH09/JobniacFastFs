//! The opt-in Arbeitnow tools `scan_arbeitnow_jobs` and `import_arbeitnow_job`.
//!
//! Each tool is registered only when its port is supplied, which the composition root does only when
//! `JOBNIAC_ARBEITNOW_ENABLED` is exactly `1` and the services are composed. The scan `request` is
//! optional and every key of it is optional and nullable; the import `request` is required and its
//! `candidateToken` key is required but may be `null`. Both tools report any failure with the compact
//! envelope (`arbeitnow_mcp._call`).
//!
//! The adapter ports are typed (`ScanRequest` in, `ScanResult` out; `ImportRequest` in,
//! `ImportOutcome` out), so this crate depends on the use-case types of `jobniac-core` and on no
//! infrastructure. A blank or missing import token is refused here, before the service call, with
//! the bare `candidateToken is required` message and empty details.

mod output;

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::arbeitnow::import::{ImportOutcome, ImportRequest};
use jobniac_core::arbeitnow::scan::{ScanRequest, ScanResult};
use jobniac_core::errors::{ApplicationError, validation_error};
use jobniac_text::java::java_strip;
use serde_json::Value;

use crate::args::{
    ArgDecoder, ArgIssue, OPTIONAL_NULLABLE, REQUIRED, REQUIRED_NULLABLE, Slot, dict_arg, list_arg,
    strict_bool, strict_int, strict_string,
};
use crate::registry::{Arguments, RegistryError, ToolHandler, ToolRegistry};
use crate::result::{EnvelopeStyle, ToolResult};

/// The catalog name of `scan_arbeitnow_jobs`.
pub const SCAN_ARBEITNOW_JOBS: &str = "scan_arbeitnow_jobs";
/// The catalog name of `import_arbeitnow_job`.
pub const IMPORT_ARBEITNOW_JOB: &str = "import_arbeitnow_job";

/// The envelope style of both tools (`arbeitnow_mcp._call` uses compact separators).
const STYLE: EnvelopeStyle = EnvelopeStyle::Compact;

/// The message of a missing or blank import token (no `field` or `reason` details, unlike the service).
const TOKEN_REQUIRED: &str = "candidateToken is required";

/// The scan use case behind `scan_arbeitnow_jobs`.
#[async_trait]
pub trait ArbeitnowScanPort: std::fmt::Debug + Send + Sync {
    /// Scan the board for `request`. The service validates every bound.
    async fn scan(&self, request: ScanRequest) -> Result<ScanResult, ApplicationError>;
}

/// The trusted import use case behind `import_arbeitnow_job`.
#[async_trait]
pub trait ArbeitnowImportPort: std::fmt::Debug + Send + Sync {
    /// Import the candidate named by `request` without refetching it.
    async fn import(&self, request: ImportRequest) -> Result<ImportOutcome, ApplicationError>;
}

/// The two optional Arbeitnow ports of the server; the default has neither, which leaves both tools
/// unregistered.
#[derive(Debug, Clone, Default)]
pub struct ArbeitnowPorts {
    /// The scan port; `None` leaves `scan_arbeitnow_jobs` unregistered.
    pub scan: Option<Arc<dyn ArbeitnowScanPort>>,
    /// The import port; `None` leaves `import_arbeitnow_job` unregistered.
    pub import: Option<Arc<dyn ArbeitnowImportPort>>,
}

/// `scan_arbeitnow_jobs`: read-only scan of the Arbeitnow board.
#[derive(Debug)]
pub struct ScanArbeitnowJobsTool {
    port: Arc<dyn ArbeitnowScanPort>,
}

impl ScanArbeitnowJobsTool {
    /// A handler over the scan port.
    pub fn new(port: Arc<dyn ArbeitnowScanPort>) -> Self {
        Self { port }
    }
}

#[async_trait]
impl ToolHandler for ScanArbeitnowJobsTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        let request = match decode_scan(arguments) {
            Ok(request) => request,
            Err(errors) => return ToolResult::from(errors),
        };
        match self.port.scan(request).await {
            Ok(result) => ToolResult::Success(output::scan_result(&result)),
            Err(error) => ToolResult::from_outcome(Err(error.into()), STYLE),
        }
    }
}

/// `import_arbeitnow_job`: import one signed scan candidate.
#[derive(Debug)]
pub struct ImportArbeitnowJobTool {
    port: Arc<dyn ArbeitnowImportPort>,
}

impl ImportArbeitnowJobTool {
    /// A handler over the import port.
    pub fn new(port: Arc<dyn ArbeitnowImportPort>) -> Self {
        Self { port }
    }
}

#[async_trait]
impl ToolHandler for ImportArbeitnowJobTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        let token = match decode_import(arguments) {
            Ok(token) => token,
            Err(errors) => return ToolResult::from(errors),
        };
        let Some(token) = token.filter(|token| !java_strip(token).is_empty()) else {
            let error = validation_error(TOKEN_REQUIRED, None, None);
            return ToolResult::from_outcome(Err(error.into()), STYLE);
        };
        let request = ImportRequest {
            candidate_token: Some(token),
        };
        match self.port.import(request).await {
            Ok(outcome) => ToolResult::from_outcome(output::import_outcome(&outcome), STYLE),
            Err(error) => ToolResult::from_outcome(Err(error.into()), STYLE),
        }
    }
}

/// A `list[str]` argument: the list, then each element strictly a string at `location.index`.
fn string_list(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<Vec<String>> {
    let items = list_arg(location, value, issues)?;
    let mut decoded = Vec::with_capacity(items.len());
    let mut valid = true;
    for (index, item) in items.iter().enumerate() {
        match strict_string(&format!("{location}.{index}"), item, issues) {
            Some(text) => decoded.push(text),
            None => valid = false,
        }
    }
    valid.then_some(decoded)
}

/// A scan bound (`limit`, `maxPages`): `StrictInt`. An integer above the `i64` range is
/// forwarded as `i64::MAX`, which the service's `1..=100` and `1..=5` checks refuse with the same
/// message (`Invalid Arbeitnow scan request`) as any other out-of-range value. Integers above
/// the `u64` range and negative integers below `i64` are not representable in serde_json without
/// `arbitrary_precision` and stay `int_type`.
fn scan_bound(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<i64> {
    if value.is_u64() && value.as_i64().is_none() {
        return Some(i64::MAX);
    }
    strict_int(location, value, issues)
}

/// Decode the optional `request` of the scan tool. An absent or null request is the default request;
/// a null or absent key is `None`, and an empty list stays an empty list (`null` differs from `[]`).
fn decode_scan(arguments: &Arguments) -> Result<ScanRequest, crate::args::ArgErrors> {
    let mut decoder = ArgDecoder::new(SCAN_ARBEITNOW_JOBS);
    let mut scan = ScanRequest::default();
    if let Slot::Value(request) =
        decoder.field(arguments, "", "request", OPTIONAL_NULLABLE, dict_arg)
    {
        let parent = "request";
        scan.query = decoder
            .field(request, parent, "query", OPTIONAL_NULLABLE, strict_string)
            .into_value();
        scan.location = decoder
            .field(
                request,
                parent,
                "location",
                OPTIONAL_NULLABLE,
                strict_string,
            )
            .into_value();
        scan.remote_only = decoder
            .field(
                request,
                parent,
                "remoteOnly",
                OPTIONAL_NULLABLE,
                strict_bool,
            )
            .into_value();
        scan.visa_sponsorship = decoder
            .field(
                request,
                parent,
                "visaSponsorship",
                OPTIONAL_NULLABLE,
                strict_bool,
            )
            .into_value();
        scan.tags = decoder
            .field(request, parent, "tags", OPTIONAL_NULLABLE, string_list)
            .into_value();
        scan.job_types = decoder
            .field(request, parent, "jobTypes", OPTIONAL_NULLABLE, string_list)
            .into_value();
        scan.limit = decoder
            .field(request, parent, "limit", OPTIONAL_NULLABLE, scan_bound)
            .into_value();
        scan.max_pages = decoder
            .field(request, parent, "maxPages", OPTIONAL_NULLABLE, scan_bound)
            .into_value();
        scan.cursor = decoder
            .field(request, parent, "cursor", OPTIONAL_NULLABLE, strict_string)
            .into_value();
    }
    decoder.check()?;
    Ok(scan)
}

/// Decode the required `request` of the import tool: its `candidateToken` key must be present and is
/// a string or `null` (the token itself is checked by the handler).
fn decode_import(arguments: &Arguments) -> Result<Option<String>, crate::args::ArgErrors> {
    let mut decoder = ArgDecoder::new(IMPORT_ARBEITNOW_JOB);
    let mut token = None;
    if let Slot::Value(request) = decoder.field(arguments, "", "request", REQUIRED, dict_arg) {
        token = decoder
            .field(
                request,
                "request",
                "candidateToken",
                REQUIRED_NULLABLE,
                strict_string,
            )
            .into_value();
    }
    decoder.check()?;
    Ok(token)
}

/// Register each Arbeitnow tool whose port is supplied; an absent port leaves its tool unregistered.
///
/// # Errors
///
/// A [`RegistryError`] when a name is not in the catalog or is already registered.
pub fn register_arbeitnow_tools(
    registry: &mut ToolRegistry,
    scan: Option<Arc<dyn ArbeitnowScanPort>>,
    import: Option<Arc<dyn ArbeitnowImportPort>>,
) -> Result<(), RegistryError> {
    if let Some(port) = scan {
        registry.register(SCAN_ARBEITNOW_JOBS, ScanArbeitnowJobsTool::new(port))?;
    }
    if let Some(port) = import {
        registry.register(IMPORT_ARBEITNOW_JOB, ImportArbeitnowJobTool::new(port))?;
    }
    Ok(())
}

/// Register the tools of `ports`, as [`register_arbeitnow_tools`] does.
///
/// # Errors
///
/// A [`RegistryError`] when a name is not in the catalog or is already registered.
pub fn register(registry: &mut ToolRegistry, ports: ArbeitnowPorts) -> Result<(), RegistryError> {
    register_arbeitnow_tools(registry, ports.scan, ports.import)
}
