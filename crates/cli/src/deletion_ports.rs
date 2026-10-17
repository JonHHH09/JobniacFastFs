//! The MCP ports of `delete_profile` and `delete_job` over the deletion use cases.
//!
//! Like [`crate::generation_ports`], the composition root is the only place that knows both the MCP
//! port (`jobniac-mcp`) and the concrete use case (`jobniac-core` over `jobniac-infra`). An adapter
//! only forwards each call; the use case answers with the tool result objects.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::deletion::{JobDeletionService, ProfileDeletionService};
use jobniac_core::errors::AppError;
use jobniac_mcp::tools::deletion::{DeletionPorts, JobDeletionPort, ProfileDeletionPort};
use jobniac_runtime::deletion::DeletionServices;
use serde_json::Value;

/// The profile deletion use case as the `delete_profile` port.
#[derive(Debug)]
struct ProfileAdapter(Arc<ProfileDeletionService>);

/// The job deletion use case as the `delete_job` port.
#[derive(Debug)]
struct JobAdapter(Arc<JobDeletionService>);

/// The MCP ports of the composed deletion use cases. A use case that is not composed leaves its
/// tool unregistered.
#[must_use]
pub fn from_services(services: &DeletionServices) -> DeletionPorts {
    DeletionPorts {
        profile: services.profile.as_ref().map(|service| {
            Arc::new(ProfileAdapter(Arc::clone(service))) as Arc<dyn ProfileDeletionPort>
        }),
        job: services
            .job
            .as_ref()
            .map(|service| Arc::new(JobAdapter(Arc::clone(service))) as Arc<dyn JobDeletionPort>),
    }
}

#[async_trait]
impl ProfileDeletionPort for ProfileAdapter {
    async fn delete_profile(&self, profile_id: &str) -> Result<Value, AppError> {
        self.0.delete_profile(profile_id).await
    }
}

#[async_trait]
impl JobDeletionPort for JobAdapter {
    async fn delete_job(&self, job_id: &str) -> Result<Value, AppError> {
        self.0.delete_job(job_id).await
    }
}
