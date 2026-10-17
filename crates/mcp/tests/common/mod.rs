//! Shared fixtures for the in-process MCP tests. All data is synthetic.

#![allow(dead_code)]

pub mod test_tools;

use std::error::Error;
use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::clock::FixedClock;
use jobniac_core::health::{HealthProbe, HealthProbeResult, HealthService};
use jobniac_mcp::{JobniacMcpServer, McpServices};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;
use time::macros::datetime;

/// A probe that always reports the service as ready.
#[derive(Debug)]
pub struct ReadyProbe;

#[async_trait]
impl HealthProbe for ReadyProbe {
    async fn probe(&self) -> HealthProbeResult {
        HealthProbeResult::ready()
    }
}

/// Services whose clock matches the golden snapshot (`checkedAt` 2026-01-02T03:04:05.000006Z).
pub fn services() -> McpServices {
    let clock = FixedClock::new(datetime!(2026-01-02 03:04:05.000006 UTC));
    McpServices {
        health: Arc::new(HealthService::new(
            Arc::new(ReadyProbe),
            None,
            Arc::new(clock),
        )),
        profile_list: Some(Arc::new(test_tools::Unavailable)),
        profile_details: Some(Arc::new(test_tools::Unavailable)),
        profile_search: Some(Arc::new(test_tools::Unavailable)),
        job_service: Some(Arc::new(test_tools::Unavailable)),
        job_search: Some(Arc::new(test_tools::Unavailable)),
        matching_service: Some(Arc::new(test_tools::Unavailable)),
        review_service: Some(Arc::new(test_tools::Unavailable)),
        document_service: Some(Arc::new(test_tools::Unavailable)),
        profile_pdf_service: Some(Arc::new(test_tools::Unavailable)),
        generation_service: Some(Arc::new(test_tools::Unavailable)),
        document_artifact_service: None,
        deletion: Default::default(),
        job_link_service: None,
        arbeitnow: Default::default(),
    }
}

/// The services of one catalog configuration: the default composition plus the opt-in groups
/// named in `groups` (`deletion`, `link`, `arbeitnow`, `artifacts`), each with fake ports that are
/// never called by a listing. The groups are the ones the `JOBNIAC_*_ENABLED` flags compose.
pub fn services_for(groups: &[&str]) -> McpServices {
    let mut services = services();
    if groups.contains(&"deletion") {
        services.deletion = jobniac_mcp::tools::deletion::DeletionPorts {
            profile: Some(Arc::new(test_tools::Unavailable)),
            job: Some(Arc::new(test_tools::Unavailable)),
        };
    }
    if groups.contains(&"link") {
        services.job_link_service = Some(Arc::new(test_tools::Unavailable));
    }
    if groups.contains(&"arbeitnow") {
        services.arbeitnow = jobniac_mcp::tools::arbeitnow::ArbeitnowPorts {
            scan: Some(Arc::new(test_tools::Unavailable)),
            import: Some(Arc::new(test_tools::Unavailable)),
        };
    }
    if groups.contains(&"artifacts") {
        services.document_artifact_service = Some(Arc::new(test_tools::Unavailable));
    }
    services
}

/// Connect a client to the default server (health only) over an in-memory duplex pipe.
pub async fn connect(
    services: McpServices,
) -> Result<RunningService<RoleClient, ()>, Box<dyn Error>> {
    connect_server(JobniacMcpServer::new(services)?).await
}

/// Connect a client to any server running in this process over an in-memory duplex pipe.
pub async fn connect_server(
    server: JobniacMcpServer,
) -> Result<RunningService<RoleClient, ()>, Box<dyn Error>> {
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    let (client_read, client_write) = tokio::io::split(client_side);
    tokio::spawn(async move {
        if let Ok(running) = server.serve((server_read, server_write)).await {
            // The session ends when the client disconnects; the join result is not needed.
            let _ = running.waiting().await;
        }
    });
    Ok(().serve((client_read, client_write)).await?)
}

/// Parse a golden file embedded at compile time.
pub fn golden(text: &str) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_str(text)?)
}
