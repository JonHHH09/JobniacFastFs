//! What the job tool handlers pass to their ports. The call goldens check the wire result; this file
//! checks the values the service receives: the identifier as sent, the optional listing arguments,
//! the request objects, the nullable skill items, and the rejections that never reach a port.
//! All data is synthetic.

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::{Arguments, ToolRegistry};
use jobniac_mcp::result::ToolResult;
use jobniac_mcp::tools::jobs::{self, JobPort, JobSearchPort};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

const JOB_ID: &str = "00000000-0000-4000-8000-0000000000BB";

/// Records one `(port method, value the port received)` pair per call and answers `{"ok": true}`.
#[derive(Debug, Default)]
struct Recorder {
    calls: Mutex<Vec<(&'static str, Value)>>,
}

impl Recorder {
    fn record(&self, method: &'static str, received: Value) -> Result<Value, AppError> {
        // A poisoned lock keeps its calls: the first panic stays the reported failure.
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((method, received));
        Ok(json!({"ok": true}))
    }

    fn calls(&self) -> Vec<(&'static str, Value)> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl JobPort for Recorder {
    async fn add_job_from_text(&self, request: &Value) -> Result<Value, AppError> {
        self.record("add_job_from_text", request.clone())
    }

    async fn list_jobs(&self, limit: Option<i64>, cursor: Option<&str>) -> Result<Value, AppError> {
        self.record("list_jobs", json!({"limit": limit, "cursor": cursor}))
    }

    async fn get_job(&self, job_id: &str) -> Result<Value, AppError> {
        self.record("get_job", json!(job_id))
    }

    async fn update_job(&self, request: &Value) -> Result<Value, AppError> {
        self.record("update_job", request.clone())
    }
}

#[async_trait]
impl JobSearchPort for Recorder {
    async fn search_jobs(&self, request: &Value) -> Result<Value, AppError> {
        self.record("search_jobs", request.clone())
    }
}

/// A registry with the job tools over the recorder, over the bundled catalog.
fn registry(recorder: &Arc<Recorder>) -> Result<ToolRegistry, Box<dyn Error>> {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    jobs::register(
        &mut registry,
        Some(Arc::clone(recorder) as Arc<dyn JobPort>),
        Some(Arc::clone(recorder) as Arc<dyn JobSearchPort>),
    )?;
    Ok(registry)
}

/// Calls one tool through the registry and returns its outcome.
async fn call(registry: &ToolRegistry, name: &str, arguments: Value) -> Result<ToolResult, String> {
    let Value::Object(map) = arguments else {
        return Err("arguments must be an object".to_owned());
    };
    let map: Arguments = map;
    registry
        .call(name, &map)
        .await
        .ok_or_else(|| format!("{name} is not registered"))
}

#[tokio::test]
async fn get_job_passes_the_identifier_as_sent() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let outcome = call(&registry, "get_job", json!({ "jobId": JOB_ID })).await?;
    assert!(matches!(outcome, ToolResult::Success(_)));
    assert_eq!(recorder.calls(), vec![("get_job", json!(JOB_ID))]);
    Ok(())
}

#[tokio::test]
async fn list_jobs_passes_the_optional_page_arguments() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    call(&registry, "list_jobs", json!({})).await?;
    call(&registry, "list_jobs", json!({ "request": null })).await?;
    call(
        &registry,
        "list_jobs",
        json!({ "request": { "limit": 2, "cursor": "synthetic-cursor", "unexpected": 1 } }),
    )
    .await?;
    assert_eq!(
        recorder.calls(),
        vec![
            ("list_jobs", json!({"limit": null, "cursor": null})),
            ("list_jobs", json!({"limit": null, "cursor": null})),
            (
                "list_jobs",
                json!({"limit": 2, "cursor": "synthetic-cursor"})
            ),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn add_job_from_text_passes_the_request_object_as_sent() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let request = json!({
        "text": "Synthetic posting text",
        "sourceLabel": null,
        "skills": ["Rust", "PostgreSQL"],
        "postedAt": "2026-01-02T03:04:05Z",
    });
    call(
        &registry,
        "add_job_from_text",
        json!({ "request": request.clone() }),
    )
    .await?;
    assert_eq!(recorder.calls(), vec![("add_job_from_text", request)]);
    Ok(())
}

#[tokio::test]
async fn update_job_passes_the_request_with_its_nullable_skill_items() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let request = json!({
        "jobId": JOB_ID,
        "expectedRevision": 3,
        "title": "Platform engineer",
        "skills": [null, "Go"],
    });
    call(
        &registry,
        "update_job",
        json!({ "request": request.clone() }),
    )
    .await?;
    assert_eq!(recorder.calls(), vec![("update_job", request)]);
    Ok(())
}

#[tokio::test]
async fn search_jobs_passes_the_request_object() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let request = json!({ "query": "rust", "limit": null });
    call(
        &registry,
        "search_jobs",
        json!({ "request": request.clone() }),
    )
    .await?;
    assert_eq!(recorder.calls(), vec![("search_jobs", request)]);
    Ok(())
}

#[tokio::test]
async fn rejected_arguments_never_reach_a_port() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let rejected = [
        ("add_job_from_text", json!({})),
        ("add_job_from_text", json!({ "request": { "title": "x" } })),
        ("get_job", json!({ "jobId": 5 })),
        ("list_jobs", json!({ "request": { "limit": "5" } })),
        ("search_jobs", json!({ "request": { "limit": 5 } })),
        (
            "update_job",
            json!({ "request": { "jobId": 5, "expectedRevision": "3" } }),
        ),
        (
            "update_job",
            json!({ "request": { "jobId": JOB_ID, "expectedRevision": 3, "skills": [5] } }),
        ),
        ("update_job", json!({ "request": null })),
        (
            "update_job",
            json!({ "request": { "jobId": null, "expectedRevision": 3 } }),
        ),
        (
            "update_job",
            json!({ "request": { "jobId": JOB_ID, "expectedRevision": null } }),
        ),
        (
            "update_job",
            json!({ "request": { "jobId": JOB_ID, "expectedRevision": true } }),
        ),
        (
            "update_job",
            json!({ "request": { "jobId": JOB_ID, "expectedRevision": 0.0 } }),
        ),
        (
            "update_job",
            json!({ "request": { "jobId": JOB_ID, "expectedRevision": 3, "skills": "Java" } }),
        ),
    ];
    for (name, arguments) in rejected {
        let outcome = call(&registry, name, arguments).await?;
        assert!(
            matches!(outcome, ToolResult::InvalidArguments(_)),
            "{name} must reject its arguments: {outcome:?}"
        );
    }
    assert!(
        recorder.calls().is_empty(),
        "a rejected call reached a port"
    );
    Ok(())
}

#[tokio::test]
async fn integers_beyond_i64_are_refused_at_the_sdk_layer() -> TestResult {
    // A serde_json integer is held as i64 or u64 only, so the job decoders refuse an integer
    // beyond i64 as int_type before the port runs.
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let cases = [
        (
            "update_job",
            json!({"request": {"jobId": JOB_ID, "expectedRevision": 9223372036854775808_u64}}),
            "request.expectedRevision",
        ),
        (
            "list_jobs",
            json!({"request": {"limit": 9223372036854775808_u64}}),
            "request.limit",
        ),
        (
            "search_jobs",
            json!({"request": {"query": "rust", "limit": 9223372036854775808_u64}}),
            "request.limit",
        ),
    ];
    for (name, arguments, location) in cases {
        let result = call(&registry, name, arguments).await?;
        let ToolResult::InvalidArguments(errors) = result else {
            return Err(format!("{name}: an integer beyond i64 is not an argument error").into());
        };
        let rendered = errors.render();
        assert!(rendered.contains(location), "{rendered}");
        assert!(rendered.contains("[type=int_type]"), "{rendered}");
    }
    assert!(
        recorder.calls().is_empty(),
        "a rejected call reached a port"
    );
    Ok(())
}

#[test]
fn search_jobs_registers_on_its_own_port_without_the_job_port() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let mut search_only = ToolRegistry::new(Catalog::load()?);
    jobs::register(
        &mut search_only,
        None,
        Some(Arc::clone(&recorder) as Arc<dyn JobSearchPort>),
    )?;
    let names: Vec<String> = search_only
        .listed_tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert_eq!(names, ["search_jobs"]);
    Ok(())
}

#[test]
fn job_tools_are_listed_in_catalog_order_only_when_their_ports_are_supplied() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let full = registry(&recorder)?;
    let names: Vec<String> = full
        .listed_tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert_eq!(
        names,
        [
            "search_jobs",
            "update_job",
            "list_jobs",
            "get_job",
            "add_job_from_text",
        ]
    );

    let mut bare = ToolRegistry::new(Catalog::load()?);
    jobs::register(&mut bare, None, None)?;
    assert!(bare.listed_tools().is_empty());
    Ok(())
}
