//! The MCP job ports over the PostgreSQL-backed job use cases.
//!
//! Like [`crate::profile_ports`], the composition root is the only place that knows both the MCP
//! ports (`jobniac-mcp`) and the concrete use cases (`jobniac-core` over `jobniac-infra`). Each
//! adapter wraps one concrete use case and converts its typed result into the service dictionary the
//! tools return.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_core::jobs::{JobSearchService, JobService};
use jobniac_infra::postgres::jobs::PgJobRepository;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use jobniac_mcp::tools::jobs::{JobPort, JobSearchPort};
use jobniac_runtime::services::JobServices;
use serde_json::Value;

use crate::profile_ports::to_dictionary;

/// The job capture, get, keyset list and update use case over PostgreSQL.
pub type JobUseCase = JobService<PgUnitOfWork, PgJobRepository>;
/// The bounded job search use case over PostgreSQL.
pub type JobSearchUseCase = JobSearchService<PgUnitOfWork, PgJobRepository>;

/// The MCP ports of the job tools. An absent port leaves its tools unregistered.
#[derive(Debug, Clone, Default)]
pub struct JobPorts {
    /// The capture, get, list and update use case behind the four job tools.
    pub service: Option<Arc<dyn JobPort>>,
    /// The search behind `search_jobs`.
    pub search: Option<Arc<dyn JobSearchPort>>,
}

/// The ports of the composed job services.
pub fn from_services(jobs: &JobServices) -> JobPorts {
    build(Arc::clone(&jobs.service), Arc::clone(&jobs.search))
}

/// The ports over the given use cases.
fn build(service: Arc<JobUseCase>, search: Arc<JobSearchUseCase>) -> JobPorts {
    JobPorts {
        service: Some(Arc::new(ServiceAdapter(service)) as Arc<dyn JobPort>),
        search: Some(Arc::new(SearchAdapter(search)) as Arc<dyn JobSearchPort>),
    }
}

/// The four job tools over the capture, get, list and update use case.
#[derive(Debug)]
struct ServiceAdapter(Arc<JobUseCase>);

#[async_trait]
impl JobPort for ServiceAdapter {
    async fn add_job_from_text(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.add_job_from_text(request).await?)
    }

    async fn list_jobs(&self, limit: Option<i64>, cursor: Option<&str>) -> Result<Value, AppError> {
        to_dictionary(&self.0.list_jobs(limit, cursor).await?)
    }

    async fn get_job(&self, job_id: &str) -> Result<Value, AppError> {
        to_dictionary(&self.0.get_job(job_id).await?)
    }

    async fn update_job(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.update_job(request).await?)
    }
}

/// `search_jobs` over the bounded search.
#[derive(Debug)]
struct SearchAdapter(Arc<JobSearchUseCase>);

#[async_trait]
impl JobSearchPort for SearchAdapter {
    async fn search_jobs(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.search_jobs(request).await?)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;

    use jobniac_core::clock::SystemClock;
    use jobniac_core::jobs::{JobSearchService, JobService};
    use jobniac_core::pagination::{CursorCodec, CursorKey};
    use jobniac_infra::postgres::config::load_database_config;
    use jobniac_infra::postgres::jobs::PgJobRepository;
    use jobniac_infra::postgres::migrations::migrate;
    use jobniac_infra::postgres::pool::PgPool;
    use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
    use jobniac_mcp::catalog::Catalog;
    use jobniac_mcp::registry::{Arguments, ToolRegistry};
    use jobniac_mcp::result::ToolResult;
    use jobniac_mcp::tools::jobs;
    use jobniac_testkit::pg::TestServer;
    use serde_json::{Value, json};

    use super::build;

    type TestResult = Result<(), Box<dyn Error>>;

    const CURSOR_SECRET: &str = "job-ports-cursor-secret-0123456789abcdef";

    /// Calls one tool through the registry and returns its outcome.
    async fn call(
        registry: &ToolRegistry,
        name: &str,
        arguments: Value,
    ) -> Result<ToolResult, String> {
        let Value::Object(map) = arguments else {
            return Err("arguments must be an object".to_owned());
        };
        let map: Arguments = map;
        registry
            .call(name, &map)
            .await
            .ok_or_else(|| format!("{name} is not registered"))
    }

    /// The structured success of a call, or the failure as a message.
    fn success(outcome: ToolResult) -> Result<Value, String> {
        match outcome {
            ToolResult::Success(value) => Ok(value),
            other => Err(format!("expected a success, got {other:?}")),
        }
    }

    /// The `code` of a failure envelope, or the outcome as a message.
    fn failure_code(outcome: ToolResult) -> Result<String, String> {
        match outcome {
            ToolResult::Failure { envelope, .. } => Ok(envelope.code.code().to_owned()),
            other => Err(format!("expected a failure, got {other:?}")),
        }
    }

    /// The number of job rows on the first page of `list_jobs` (limit 100).
    async fn listed_job_count(registry: &ToolRegistry) -> Result<usize, Box<dyn Error>> {
        let listed = success(
            call(
                registry,
                "list_jobs",
                json!({ "request": { "limit": 100 } }),
            )
            .await?,
        )?;
        Ok(listed["jobs"].as_array().map_or(0, Vec::len))
    }

    fn capture_request(title: &str) -> Value {
        json!({
            "text": "Synthetic posting text: Rust and PostgreSQL in a small team",
            "sourceLabel": "synthetic-board",
            "title": title,
            "company": "Example Corp",
            "location": null,
            "skills": ["Rust", "PostgreSQL"],
        })
    }

    /// The job tools through the MCP handler layer, over a disposable migrated database.
    /// Skips with a notice when no PostgreSQL server is configured for the tests.
    #[tokio::test]
    async fn job_tools_capture_read_update_search_and_list_through_the_mcp_handlers() -> TestResult
    {
        let Some(server) = TestServer::from_env() else {
            eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
            return Ok(());
        };
        let database = server.fresh_database().await;
        let config = load_database_config(&database.environment())
            .ok_or("the disposable database configuration is invalid")?;
        migrate(&config).await.map_err(|error| error.0)?;

        let uow = PgUnitOfWork::new(PgPool::new(Some(config)));
        let service = Arc::new(JobService::new(
            uow.clone(),
            PgJobRepository,
            Arc::new(SystemClock),
            CursorCodec::new(CursorKey::from_secret(CURSOR_SECRET)?),
        ));
        let search = Arc::new(JobSearchService::new(uow.clone(), PgJobRepository));
        let ports = build(service, search);
        let mut registry = ToolRegistry::new(Catalog::load()?);
        jobs::register(&mut registry, ports.service, ports.search)?;

        // Capture: a new job is created with its identifier and revision.
        let created = success(
            call(
                &registry,
                "add_job_from_text",
                json!({ "request": capture_request("Backend engineer") }),
            )
            .await?,
        )?;
        assert_eq!(created["status"], "created_job");
        let job_id = created["job"]["job"]["id"]
            .as_str()
            .ok_or("no job id")?
            .to_owned();
        let revision = created["job"]["job"]["revision"]
            .as_i64()
            .ok_or("no revision")?;

        // The same text is reused, and nothing new is written.
        let reused = success(
            call(
                &registry,
                "add_job_from_text",
                json!({ "request": capture_request("Backend engineer") }),
            )
            .await?,
        )?;
        assert_eq!(reused["status"], "reused_existing_job");
        assert_eq!(reused["job"]["job"]["id"], job_id.as_str());

        // Get: the identifier is accepted in uppercase and the aggregate matches the capture.
        let fetched = success(
            call(
                &registry,
                "get_job",
                json!({ "jobId": job_id.to_ascii_uppercase() }),
            )
            .await?,
        )?;
        assert_eq!(fetched["job"]["title"], "Backend engineer");
        assert_eq!(fetched["skills"].as_array().map(Vec::len), Some(2));

        // Update: the revision check accepts the current revision and advances it by one.
        let updated = success(
            call(
                &registry,
                "update_job",
                json!({ "request": {
                    "jobId": job_id,
                    "expectedRevision": revision,
                    "title": "Platform engineer",
                    "skills": ["Go"],
                } }),
            )
            .await?,
        )?;
        assert_eq!(updated["job"]["title"], "Platform engineer");
        assert_eq!(updated["job"]["revision"].as_i64(), Some(revision + 1));

        // A stale revision is a conflict, and nothing is written.
        let stale = call(
            &registry,
            "update_job",
            json!({ "request": {
                "jobId": job_id,
                "expectedRevision": revision,
                "title": "Stale engineer",
            } }),
        )
        .await?;
        assert_eq!(failure_code(stale)?, "conflict");
        let after_conflict =
            success(call(&registry, "get_job", json!({ "jobId": job_id })).await?)?;
        assert_eq!(after_conflict["job"]["title"], "Platform engineer");
        assert_eq!(
            after_conflict["job"]["revision"].as_i64(),
            Some(revision + 1)
        );

        // A malformed identifier is refused by the service, and an unknown one is not found.
        let malformed = call(&registry, "get_job", json!({ "jobId": "not-a-uuid" })).await?;
        let wire = serde_json::to_value(malformed.clone().into_call_result())?;
        assert_eq!(failure_code(malformed)?, "validation_error");
        assert!(
            !wire.to_string().contains("not-a-uuid"),
            "the malformed identifier was echoed: {wire}"
        );
        let missing = call(
            &registry,
            "get_job",
            json!({ "jobId": "00000000-0000-4000-8000-0000000000ff" }),
        )
        .await?;
        assert_eq!(failure_code(missing)?, "not_found");

        // Search finds the job by a token of its updated title.
        let found = success(
            call(
                &registry,
                "search_jobs",
                json!({ "request": { "query": "platform", "limit": 5 } }),
            )
            .await?,
        )?;
        let matches = found["jobs"].as_array().ok_or("no jobs array")?;
        assert!(
            matches
                .iter()
                .any(|hit| hit["job"]["id"] == job_id.as_str()),
            "the search must return the updated job: {found}"
        );

        // List: the first page holds the job and no cursor follows it.
        let listed = success(
            call(
                &registry,
                "list_jobs",
                json!({ "request": { "limit": 10 } }),
            )
            .await?,
        )?;
        let jobs = listed["jobs"].as_array().ok_or("no jobs array")?;
        assert!(jobs.iter().any(|job| job["id"] == job_id.as_str()));
        assert!(listed["nextCursor"].is_null());

        // A rejected request never reaches the database: the blank text is refused by the use case,
        // and the number of listed jobs does not change.
        let count_before = listed_job_count(&registry).await?;
        let blank = call(
            &registry,
            "add_job_from_text",
            json!({ "request": { "text": "   " } }),
        )
        .await?;
        assert_eq!(failure_code(blank)?, "validation_error");
        assert_eq!(listed_job_count(&registry).await?, count_before);
        Ok(())
    }
}
