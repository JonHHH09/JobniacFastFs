//! The MCP ports of `scan_arbeitnow_jobs` and `import_arbeitnow_job` over the composed use cases.
//!
//! The composition root is the only place that knows both the MCP ports (`jobniac-mcp`) and the
//! concrete use cases (`jobniac-core` over `jobniac-infra`). The ports are present only when the
//! flag is exactly `1`, the keys are valid and a database is configured; otherwise the default
//! ports register neither tool. No key or secret is read here beyond the runtime composition.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::arbeitnow::import::{ImportOutcome, ImportRequest};
use jobniac_core::arbeitnow::scan::{ScanRequest, ScanResult};
use jobniac_core::env::Environment;
use jobniac_core::errors::ApplicationError;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_mcp::tools::arbeitnow::{ArbeitnowImportPort, ArbeitnowPorts, ArbeitnowScanPort};
use jobniac_runtime::arbeitnow::{ArbeitnowServices, compose_arbeitnow};
use jobniac_runtime::services::StartupError;

/// The composed use cases as the scan port.
#[derive(Debug)]
struct ScanAdapter(ArbeitnowServices);

/// The composed use cases as the import port.
#[derive(Debug)]
struct ImportAdapter(ArbeitnowServices);

#[async_trait]
impl ArbeitnowScanPort for ScanAdapter {
    async fn scan(&self, request: ScanRequest) -> Result<ScanResult, ApplicationError> {
        self.0.scan(request).await
    }
}

#[async_trait]
impl ArbeitnowImportPort for ImportAdapter {
    async fn import(&self, request: ImportRequest) -> Result<ImportOutcome, ApplicationError> {
        self.0.import(request).await
    }
}

/// Both ports of the composed services, or the empty ports when they were not composed.
#[must_use]
pub fn from_services(services: Option<ArbeitnowServices>) -> ArbeitnowPorts {
    match services {
        None => ArbeitnowPorts::default(),
        Some(services) => ArbeitnowPorts {
            scan: Some(Arc::new(ScanAdapter(services.clone()))),
            import: Some(Arc::new(ImportAdapter(services))),
        },
    }
}

/// Compose the Arbeitnow ports from the environment snapshot and the database configuration.
///
/// # Errors
///
/// The sanitized [`StartupError`] when the flag is exactly `1` but the platform or the keys are
/// unusable.
pub fn from_environment(
    env: &Environment,
    database: Option<&SqlitePool>,
) -> Result<ArbeitnowPorts, StartupError> {
    compose_arbeitnow(env, database)
        .map(from_services)
        .map_err(|_| StartupError)
}
