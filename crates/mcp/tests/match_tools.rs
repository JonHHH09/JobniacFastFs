//! What the matching tool handlers pass to their port: the declared keys of each request object, the
//! identifiers as the client sent them, the nullable filters, the envelope style of failures, and the
//! rejections that never reach the port. The call goldens check the wire result; this file checks the
//! values the service receives. All data is synthetic.

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::{Arguments, ToolRegistry};
use jobniac_mcp::result::{EnvelopeStyle, ToolResult};
use jobniac_mcp::tools::matching::{self, MatchingPort};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

const PROFILE_ID: &str = "00000000-0000-4000-8000-0000000000AA";
const JOB_ID: &str = "00000000-0000-4000-8000-0000000000BB";
const REPORT_ID: &str = "00000000-0000-4000-8000-0000000000EE";

/// Records one `(port method, value the port received)` pair per call and answers `{"ok": true}`.
#[derive(Debug, Default)]
struct Recorder {
    calls: Mutex<Vec<(&'static str, Value)>>,
    fail: bool,
}

impl Recorder {
    fn record(&self, method: &'static str, received: &Value) -> Result<Value, AppError> {
        // A poisoned lock keeps its calls: the first panic stays the reported failure.
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((method, received.clone()));
        if self.fail {
            return Err(AppError::DomainValue(
                " profile id is not admitted ".to_owned(),
            ));
        }
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
impl MatchingPort for Recorder {
    async fn analyze_job_match(&self, request: &Value) -> Result<Value, AppError> {
        self.record("analyze_job_match", request)
    }

    async fn analyze_all_job_matches(&self, request: &Value) -> Result<Value, AppError> {
        self.record("analyze_all_job_matches", request)
    }

    async fn get_match_report(&self, request: &Value) -> Result<Value, AppError> {
        self.record("get_match_report", request)
    }

    async fn list_match_reports(&self, request: &Value) -> Result<Value, AppError> {
        self.record("list_match_reports", request)
    }
}

/// A registry with the four matching tools over `recorder`.
fn registry(recorder: &Arc<Recorder>) -> Result<ToolRegistry, Box<dyn Error>> {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    matching::register(
        &mut registry,
        Some(Arc::clone(recorder) as Arc<dyn MatchingPort>),
    )?;
    Ok(registry)
}

/// Calls one tool through the registry and returns its outcome.
async fn call(
    registry: &ToolRegistry,
    name: &str,
    arguments: Value,
) -> Result<ToolResult, Box<dyn Error>> {
    let Value::Object(map) = arguments else {
        return Err("arguments must be an object".into());
    };
    let map: Arguments = map;
    registry
        .call(name, &map)
        .await
        .ok_or_else(|| format!("{name} is not registered").into())
}

#[tokio::test]
async fn analyze_passes_only_the_declared_keys_as_sent() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let outcome = call(
        &registry,
        "analyze_job_match",
        json!({"request": {"profileId": PROFILE_ID, "jobId": JOB_ID.to_ascii_lowercase(), "x": 1}}),
    )
    .await?;
    assert!(matches!(outcome, ToolResult::Success(_)), "{outcome:?}");
    assert_eq!(
        recorder.calls(),
        vec![(
            "analyze_job_match",
            json!({"profileId": PROFILE_ID, "jobId": JOB_ID.to_ascii_lowercase()}),
        )]
    );
    Ok(())
}

#[tokio::test]
async fn batch_and_filters_pass_their_nullable_keys_through() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    call(
        &registry,
        "analyze_all_job_matches",
        json!({"request": {"profileId": PROFILE_ID, "limit": 25, "cursor": null}}),
    )
    .await?;
    call(
        &registry,
        "list_match_reports",
        json!({"request": {"profileId": null, "jobId": JOB_ID, "limit": null}}),
    )
    .await?;
    call(&registry, "list_match_reports", json!({"request": {}})).await?;
    call(
        &registry,
        "get_match_report",
        json!({"request": {"id": REPORT_ID}}),
    )
    .await?;
    assert_eq!(
        recorder.calls(),
        vec![
            (
                "analyze_all_job_matches",
                json!({"profileId": PROFILE_ID, "limit": 25, "cursor": null}),
            ),
            (
                "list_match_reports",
                json!({"profileId": null, "jobId": JOB_ID, "limit": null}),
            ),
            ("list_match_reports", json!({})),
            ("get_match_report", json!({"id": REPORT_ID})),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn rejected_requests_never_reach_the_port() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let rejected = [
        ("analyze_job_match", json!({})),
        (
            "analyze_job_match",
            json!({"request": {"profileId": PROFILE_ID}}),
        ),
        (
            "analyze_job_match",
            json!({"request": {"profileId": 5, "jobId": JOB_ID}}),
        ),
        (
            "analyze_all_job_matches",
            json!({"request": {"profileId": null}}),
        ),
        (
            "analyze_all_job_matches",
            json!({"request": {"profileId": PROFILE_ID, "limit": "5"}}),
        ),
        (
            "analyze_all_job_matches",
            json!({"request": {"profileId": PROFILE_ID, "cursor": 7}}),
        ),
        ("get_match_report", json!({"request": {}})),
        ("get_match_report", json!({"request": {"id": 12}})),
        ("list_match_reports", json!({"request": null})),
        ("list_match_reports", json!({"request": {"limit": true}})),
        ("list_match_reports", json!({})),
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
        "a rejected call reached the port"
    );
    Ok(())
}

#[tokio::test]
async fn integers_beyond_i64_reach_the_port_for_the_service_to_range_check() -> TestResult {
    // Any integer is forwarded, and the service answers `limit must be between 1 and 100`.
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    call(
        &registry,
        "analyze_all_job_matches",
        json!({"request": {"profileId": PROFILE_ID, "limit": 9_223_372_036_854_775_808_u64}}),
    )
    .await?;
    call(
        &registry,
        "list_match_reports",
        json!({"request": {"limit": 9_223_372_036_854_775_808_u64}}),
    )
    .await?;
    assert_eq!(
        recorder.calls(),
        vec![
            (
                "analyze_all_job_matches",
                json!({"profileId": PROFILE_ID, "limit": 9_223_372_036_854_775_808_u64}),
            ),
            (
                "list_match_reports",
                json!({"limit": 9_223_372_036_854_775_808_u64}),
            ),
        ]
    );
    // A float is still `int_type`, and nothing reaches the port.
    let outcome = call(
        &registry,
        "list_match_reports",
        json!({"request": {"limit": 2.5}}),
    )
    .await?;
    assert!(matches!(outcome, ToolResult::InvalidArguments(_)));
    assert_eq!(recorder.calls().len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_port_failure_uses_the_default_envelope_style() -> TestResult {
    let recorder = Arc::new(Recorder {
        calls: Mutex::default(),
        fail: true,
    });
    let registry = registry(&recorder)?;
    let outcome = call(
        &registry,
        "analyze_all_job_matches",
        json!({"request": {"profileId": PROFILE_ID}}),
    )
    .await?;
    let ToolResult::Failure { envelope, style } = outcome else {
        return Err("a domain value error must be an envelope".into());
    };
    assert_eq!(style, EnvelopeStyle::Default);
    assert_eq!(envelope.code.code(), "validation_error");
    assert_eq!(envelope.message, "profile id is not admitted");
    Ok(())
}

#[test]
fn the_four_tools_are_listed_in_catalog_order_only_with_a_port() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let names: Vec<String> = registry(&recorder)?
        .listed_tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert_eq!(
        names,
        [
            "analyze_job_match",
            "analyze_all_job_matches",
            "get_match_report",
            "list_match_reports",
        ]
    );

    let mut bare = ToolRegistry::new(Catalog::load()?);
    matching::register(&mut bare, None)?;
    assert!(bare.listed_tools().is_empty());
    Ok(())
}
