//! The opt-in `add_job_from_link` tool over the MCP wire.
//!
//! Registration, the published schema and annotations, the `request` pass-through, the sanitized
//! envelopes for each fetch failure and for malformed arguments, the no-echo rule for userinfo,
//! query and fragment, and the unexpected-error mapping. Most cases run the real core use case over
//! in-memory doubles, so that the fetch port is provably not called when validation refuses. All data
//! is synthetic.

mod common;

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::clock::FixedClock;
use jobniac_core::errors::{AppError, not_found};
use jobniac_core::job_links::service::{JobLinkPageFetcher, JobLinkRepository, JobLinkService};
use jobniac_core::job_links::{JobLinkFetchFailure, JobLinkFetchResult};
use jobniac_core::jobs::JobAggregate;
use jobniac_core::profiles::repository::UnitOfWork;
use jobniac_core::transactions::TransactionOptions;
use jobniac_mcp::tools::job_links::JobLinkPort;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Map, Value, json};
use time::macros::datetime;

type TestResult = Result<(), Box<dyn Error>>;

const URL: &str = "https://203.0.113.7/jobs/1?token=SECRET#frag";
const USERINFO_URL: &str = "https://user:pw@203.0.113.7/jobs/1";
const REQUEST_PROPERTIES: [&str; 11] = [
    "url",
    "sourceLabel",
    "title",
    "company",
    "location",
    "description",
    "skills",
    "experienceRequirement",
    "employmentType",
    "seniority",
    "postedAt",
];

/// A unit of work whose transaction handle is `()`.
#[derive(Debug, Default, Clone, Copy)]
struct MemoryUow;

impl UnitOfWork for MemoryUow {
    type Tx = ();

    async fn transaction<T, F>(&self, _options: TransactionOptions, body: F) -> Result<T, AppError>
    where
        F: AsyncFnOnce(&mut ()) -> Result<T, AppError>,
    {
        let mut tx = ();
        body(&mut tx).await
    }
}

/// A repository that never finds a duplicate and records each write.
#[derive(Debug, Default, Clone)]
struct Repository {
    created: Arc<Mutex<Vec<JobAggregate>>>,
}

#[async_trait]
impl JobLinkRepository for Repository {
    type Tx = ();

    async fn find_by_normalized_url(
        &self,
        _tx: &mut (),
        _normalized_url: &str,
    ) -> Result<Option<JobAggregate>, AppError> {
        Ok(None)
    }

    async fn create_link_job(
        &self,
        _tx: &mut (),
        graph: &JobAggregate,
    ) -> Result<(JobAggregate, bool), AppError> {
        self.created
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(graph.clone());
        Ok((graph.clone(), true))
    }
}

/// A fetch port that counts its calls and answers with a failure or with a fixed page.
#[derive(Debug, Default, Clone)]
struct Fetch {
    failure: Option<JobLinkFetchFailure>,
    calls: Arc<Mutex<usize>>,
}

impl Fetch {
    fn calls(&self) -> usize {
        *self.calls.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl JobLinkPageFetcher for Fetch {
    async fn fetch(&self, _retrieval_url: &str) -> Result<JobLinkFetchResult, JobLinkFetchFailure> {
        *self.calls.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        match self.failure {
            Some(failure) => Err(failure),
            None => Ok(JobLinkFetchResult {
                title: Some("Engineer".to_owned()),
                description: Some("Build things. Python.".to_owned()),
                http_status: 200,
            }),
        }
    }
}

/// The real use case as the MCP port.
#[derive(Debug)]
struct LinkPort(Arc<JobLinkService<MemoryUow, Repository>>);

#[async_trait]
impl JobLinkPort for LinkPort {
    async fn add_job_from_link(&self, request: &Value) -> Result<Value, AppError> {
        let result = self.0.add_job_from_link(request).await?;
        serde_json::to_value(result).map_err(|_| AppError::Internal)
    }
}

/// A port that records each request and answers with a fixed outcome.
#[derive(Debug, Default)]
struct Stub {
    result: Option<Value>,
    error: Option<AppError>,
    requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl JobLinkPort for Stub {
    async fn add_job_from_link(&self, request: &Value) -> Result<Value, AppError> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        match (&self.error, &self.result) {
            (Some(error), _) => Err(error.clone()),
            (None, Some(value)) => Ok(value.clone()),
            (None, None) => Err(AppError::Internal),
        }
    }
}

/// The real use case over `fetch`, with a fixed clock.
fn real(fetch: &Fetch) -> Arc<dyn JobLinkPort> {
    Arc::new(LinkPort(Arc::new(JobLinkService::new(
        MemoryUow,
        Repository::default(),
        Arc::new(fetch.clone()),
        Arc::new(FixedClock::new(datetime!(2026-10-04 12:30 UTC))),
    ))))
}

/// The default services with `port` as the link use case (or none).
fn services(port: Option<Arc<dyn JobLinkPort>>) -> jobniac_mcp::McpServices {
    let mut services = common::services();
    services.job_link_service = port;
    services
}

/// One call through the wire, with the arguments as the client sends them.
async fn call(
    services: jobniac_mcp::McpServices,
    arguments: Value,
) -> Result<CallToolResult, Box<dyn Error>> {
    let client = common::connect(services).await?;
    let map: Map<String, Value> = match arguments {
        Value::Object(map) => map,
        _ => return Err("arguments must be an object".into()),
    };
    Ok(client
        .call_tool(CallToolRequestParams::new("add_job_from_link").with_arguments(map))
        .await?)
}

/// The wire text of a result: the structured value and every text block, joined.
fn wire(result: &CallToolResult) -> String {
    let mut text = serde_json::to_string(&result.structured_content).unwrap_or_default();
    for block in &result.content {
        if let Some(block) = block.as_text() {
            text.push_str(&block.text);
        }
    }
    text
}

#[tokio::test]
async fn the_tool_is_registered_only_when_its_port_is_injected() -> TestResult {
    let client = common::connect(services(None)).await?;
    let names: Vec<String> = client
        .list_all_tools()
        .await?
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert!(!names.iter().any(|name| name == "add_job_from_link"));

    let fetch = Fetch::default();
    let client = common::connect(services(Some(real(&fetch)))).await?;
    let tools = client.list_all_tools().await?;
    let tool = tools
        .iter()
        .find(|tool| tool.name == "add_job_from_link")
        .ok_or("the injected tool must be listed")?;
    assert_eq!(
        tool.annotations.as_ref().and_then(|a| a.read_only_hint),
        Some(false)
    );
    assert_eq!(
        tool.annotations.as_ref().and_then(|a| a.open_world_hint),
        Some(true)
    );
    Ok(())
}

#[tokio::test]
async fn the_schema_wraps_the_request_with_its_keys_and_url_required() -> TestResult {
    let fetch = Fetch::default();
    let client = common::connect(services(Some(real(&fetch)))).await?;
    let tools = client.list_all_tools().await?;
    let tool = tools
        .iter()
        .find(|tool| tool.name == "add_job_from_link")
        .ok_or("the injected tool must be listed")?;
    let schema = Value::Object(tool.input_schema.as_ref().clone());
    let properties = schema["properties"]
        .as_object()
        .ok_or("the input schema has no properties")?;
    assert_eq!(properties.keys().collect::<Vec<_>>(), ["request"]);
    let request = &schema["properties"]["request"];
    let keys: Vec<&str> = request["properties"]
        .as_object()
        .ok_or("the request schema has no properties")?
        .keys()
        .map(String::as_str)
        .collect();
    let mut expected = REQUEST_PROPERTIES.to_vec();
    expected.sort_unstable();
    let mut actual = keys;
    actual.sort_unstable();
    assert_eq!(actual, expected);
    assert_eq!(request["required"], json!(["url"]));
    Ok(())
}

#[tokio::test]
async fn a_success_envelope_is_the_service_result_and_the_request_is_passed_unchanged() -> TestResult
{
    let value = json!({ "status": "created_job", "job": { "job": { "id": "x" }, "skills": [] } });
    let stub = Arc::new(Stub {
        result: Some(value.clone()),
        ..Stub::default()
    });
    let request = json!({ "url": "https://203.0.113.7/j", "title": "T", "skills": ["Go"] });
    let result = call(
        services(Some(stub.clone() as Arc<dyn JobLinkPort>)),
        json!({ "request": request }),
    )
    .await?;
    assert_eq!(result.is_error, Some(false));
    assert_eq!(result.structured_content, Some(value.clone()));
    let requests = stub
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(requests, vec![request]);
    Ok(())
}

#[tokio::test]
async fn every_fetch_failure_maps_to_the_sanitized_envelope() -> TestResult {
    let failures = [
        JobLinkFetchFailure::Unavailable,
        JobLinkFetchFailure::Interrupted,
        JobLinkFetchFailure::InvalidUrl,
        JobLinkFetchFailure::RedirectNotFollowed,
        JobLinkFetchFailure::UnsafeAddress,
        JobLinkFetchFailure::IpLiteralRequired,
    ];
    for failure in failures {
        let fetch = Fetch {
            failure: Some(failure),
            ..Fetch::default()
        };
        let result = call(
            services(Some(real(&fetch))),
            json!({ "request": { "url": URL } }),
        )
        .await?;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content,
            Some(json!({
                "code": "validation_error",
                "message": "Invalid job link ingestion request",
                "details": { "field": "url", "reason": failure.message() },
            }))
        );
        let text = wire(&result);
        for private in ["SECRET", "203.0.113.7", "frag"] {
            assert!(!text.contains(private), "{private} leaked in {text}");
        }
        assert_eq!(fetch.calls(), 1);
    }
    Ok(())
}

#[tokio::test]
async fn userinfo_is_refused_before_any_fetch_without_echoing_credentials() -> TestResult {
    let fetch = Fetch::default();
    let result = call(
        services(Some(real(&fetch))),
        json!({ "request": { "url": USERINFO_URL } }),
    )
    .await?;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(fetch.calls(), 0);
    let envelope = result.structured_content.clone().unwrap_or(Value::Null);
    assert_eq!(envelope["code"], "validation_error");
    let text = wire(&result);
    assert!(!text.replace("userinfo", "").contains("user"));
    assert!(!text.contains("pw"));
    Ok(())
}

#[tokio::test]
async fn malformed_arguments_get_the_envelope_and_never_echo_the_input() -> TestResult {
    let cases = [
        json!({ "request": null }),
        json!({ "request": USERINFO_URL }),
        json!({ "request": { "extra": USERINFO_URL } }),
        json!({ "request": { "url": 5, "title": USERINFO_URL } }),
        json!({ "request": [USERINFO_URL] }),
    ];
    for arguments in cases {
        let fetch = Fetch::default();
        let result = call(services(Some(real(&fetch))), arguments).await?;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(fetch.calls(), 0);
        let envelope = result.structured_content.clone().unwrap_or(Value::Null);
        assert_eq!(envelope["code"], "validation_error");
        let text = wire(&result);
        for private in ["user:", "pw", "SECRET", "input_value"] {
            assert!(!text.contains(private), "{private} leaked in {text}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_missing_request_is_rejected_before_the_port_with_no_value_text() -> TestResult {
    for arguments in [json!({}), json!({ "url": USERINFO_URL })] {
        let fetch = Fetch::default();
        let result = call(services(Some(real(&fetch))), arguments).await?;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content, None);
        assert_eq!(fetch.calls(), 0);
        let text = wire(&result);
        for private in ["user:", "pw", "SECRET", "frag"] {
            assert!(!text.contains(private), "{private} leaked in {text}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_created_job_never_exposes_userinfo_query_or_fragment() -> TestResult {
    let fetch = Fetch::default();
    let result = call(
        services(Some(real(&fetch))),
        json!({ "request": { "url": URL } }),
    )
    .await?;
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.clone().unwrap_or(Value::Null);
    assert_eq!(structured["status"], "created_job");
    let text = serde_json::to_string(&structured)?;
    for private in ["SECRET", "frag", "?token"] {
        assert!(!text.contains(private), "{private} leaked in {text}");
    }
    assert_eq!(
        structured["job"]["linkIngestion"]["url"],
        "https://203.0.113.7/jobs/1"
    );
    Ok(())
}

#[tokio::test]
async fn an_unexpected_error_is_sanitized_to_internal_error() -> TestResult {
    let stub = Arc::new(Stub {
        error: Some(AppError::Internal),
        ..Stub::default()
    });
    let result = call(
        services(Some(stub as Arc<dyn JobLinkPort>)),
        json!({ "request": { "url": URL } }),
    )
    .await?;
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.clone().unwrap_or(Value::Null);
    assert_eq!(structured["code"], "internal_error");
    assert!(!wire(&result).contains("secret"));
    Ok(())
}

#[tokio::test]
async fn an_application_error_passes_through_the_application_shape() -> TestResult {
    let error = AppError::from(not_found("gone", [("a", "b".to_owned())]));
    let stub = Arc::new(Stub {
        error: Some(error),
        ..Stub::default()
    });
    let result = call(
        services(Some(stub as Arc<dyn JobLinkPort>)),
        json!({ "request": { "url": "https://203.0.113.7/" } }),
    )
    .await?;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content,
        Some(json!({ "code": "not_found", "message": "gone", "details": { "a": "b" } }))
    );
    Ok(())
}
