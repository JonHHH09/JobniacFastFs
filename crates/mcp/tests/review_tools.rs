//! What the review tool handlers pass to their port: the declared keys of each request object, the
//! nested `components` and `evidence` items normalized the same way, the enumerations and strict
//! flags checked before the port runs, and the rejections that never reach it. All data is synthetic.

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::{Arguments, ToolRegistry};
use jobniac_mcp::result::ToolResult;
use jobniac_mcp::tools::reviews::{self, MatchReviewPort};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

const REPORT_ID: &str = "00000000-0000-4000-8000-0000000000EE";
const DISAGREEMENT_ID: &str = "00000000-0000-4000-8000-0000000000D1";

/// Records one `(port method, value the port received)` pair per call and answers `{"ok": true}`.
#[derive(Debug, Default)]
struct Recorder {
    calls: Mutex<Vec<(&'static str, Value)>>,
}

impl Recorder {
    fn record(&self, method: &'static str, received: &Value) -> Result<Value, AppError> {
        // A poisoned lock keeps its calls: the first panic stays the reported failure.
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((method, received.clone()));
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
impl MatchReviewPort for Recorder {
    async fn submit_match_review(&self, request: &Value) -> Result<Value, AppError> {
        self.record("submit_match_review", request)
    }

    async fn get_match_review(&self, request: &Value) -> Result<Value, AppError> {
        self.record("get_match_review", request)
    }

    async fn list_match_reviews(&self, request: &Value) -> Result<Value, AppError> {
        self.record("list_match_reviews", request)
    }

    async fn list_match_disagreements(&self, request: &Value) -> Result<Value, AppError> {
        self.record("list_match_disagreements", request)
    }

    async fn acknowledge_match_disagreement(&self, request: &Value) -> Result<Value, AppError> {
        self.record("acknowledge_match_disagreement", request)
    }

    async fn link_match_disagreement(&self, request: &Value) -> Result<Value, AppError> {
        self.record("link_match_disagreement", request)
    }
}

/// A registry with the six review tools over `recorder`.
fn registry(recorder: &Arc<Recorder>) -> Result<ToolRegistry, Box<dyn Error>> {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    reviews::register(
        &mut registry,
        Some(Arc::clone(recorder) as Arc<dyn MatchReviewPort>),
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

/// A complete `ReviewRequest` whose nested items carry one key outside their TypedDict.
fn review() -> Value {
    json!({
        "reportId": REPORT_ID,
        "reviewer": "synthetic-reviewer",
        "model": null,
        "reviewVersion": "v1",
        "overallScore": 72,
        "outcome": "INSUFFICIENT_EVIDENCE",
        "blockerMismatch": true,
        "components": [{
            "component": "HARD_REQUIREMENTS",
            "earnedPoints": 0,
            "availablePoints": 10,
            "status": "MISMATCH",
            "note": "dropped"
        }],
        "evidence": [{
            "component": null,
            "status": "UNKNOWN",
            "sourceType": "posting",
            "fact": "Synthetic requirement",
            "outcomeChangingDefect": true,
            "note": "dropped"
        }],
        "summary": null,
        "unknown": "dropped"
    })
}

#[tokio::test]
async fn submit_passes_the_declared_keys_with_normalized_items() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let outcome = call(
        &registry,
        "submit_match_review",
        json!({ "request": review() }),
    )
    .await?;
    assert!(matches!(outcome, ToolResult::Success(_)), "{outcome:?}");
    let expected = json!({
        "reportId": REPORT_ID,
        "reviewer": "synthetic-reviewer",
        "model": null,
        "reviewVersion": "v1",
        "overallScore": 72,
        "outcome": "INSUFFICIENT_EVIDENCE",
        "blockerMismatch": true,
        "components": [{
            "component": "HARD_REQUIREMENTS",
            "earnedPoints": 0,
            "availablePoints": 10,
            "status": "MISMATCH"
        }],
        "evidence": [{
            "component": null,
            "status": "UNKNOWN",
            "sourceType": "posting",
            "fact": "Synthetic requirement",
            "outcomeChangingDefect": true
        }],
        "summary": null
    });
    assert_eq!(recorder.calls(), vec![("submit_match_review", expected)]);
    Ok(())
}

#[tokio::test]
async fn integers_beyond_i64_reach_the_port_for_the_service_to_range_check() -> TestResult {
    // Any integer is forwarded; the service answers `overallScore must be 0-100` and the
    // point messages, so the port must see the value unchanged.
    let big = 9_223_372_036_854_775_808_u64;
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let mut request = review();
    request["overallScore"] = json!(big);
    request["components"][0]["earnedPoints"] = json!(big);
    request["components"][0]["availablePoints"] = json!(big);
    let outcome = call(
        &registry,
        "submit_match_review",
        json!({ "request": request }),
    )
    .await?;
    assert!(matches!(outcome, ToolResult::Success(_)), "{outcome:?}");
    call(
        &registry,
        "list_match_reviews",
        json!({"request": {"limit": big}}),
    )
    .await?;
    let calls = recorder.calls();
    assert_eq!(calls.len(), 2);
    let (method, submitted) = &calls[0];
    assert_eq!(*method, "submit_match_review");
    assert_eq!(submitted["overallScore"], json!(big));
    assert_eq!(submitted["components"][0]["earnedPoints"], json!(big));
    assert_eq!(submitted["components"][0]["availablePoints"], json!(big));
    assert_eq!(calls[1], ("list_match_reviews", json!({"limit": big})));
    Ok(())
}

#[tokio::test]
async fn rejected_reviews_never_reach_the_port() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let mut bad_outcome = review();
    bad_outcome["outcome"] = json!("WEAK_MATCH_PLUS");
    let mut bad_component = review();
    bad_component["components"][0]["component"] = json!("SALARY");
    let mut bad_flag = review();
    bad_flag["blockerMismatch"] = json!("true");
    let mut missing = review();
    if let Some(object) = missing.as_object_mut() {
        object.remove("summary");
    }
    let rejected = [
        ("submit_match_review", json!({})),
        ("submit_match_review", json!({ "request": bad_outcome })),
        ("submit_match_review", json!({ "request": bad_component })),
        ("submit_match_review", json!({ "request": bad_flag })),
        ("submit_match_review", json!({ "request": missing })),
        ("get_match_review", json!({ "request": {} })),
        ("list_match_reviews", json!({ "request": { "limit": "5" } })),
        (
            "list_match_disagreements",
            json!({ "request": { "reportId": 7 } }),
        ),
        (
            "acknowledge_match_disagreement",
            json!({ "request": { "disagreementId": DISAGREEMENT_ID } }),
        ),
        ("link_match_disagreement", json!({ "request": null })),
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
async fn reads_and_disagreement_writes_pass_their_declared_keys() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    call(
        &registry,
        "get_match_review",
        json!({ "request": { "id": REPORT_ID, "x": 1 } }),
    )
    .await?;
    call(
        &registry,
        "list_match_reviews",
        json!({ "request": { "reportId": null, "limit": 10, "cursor": "c" } }),
    )
    .await?;
    call(
        &registry,
        "list_match_disagreements",
        json!({ "request": {} }),
    )
    .await?;
    call(
        &registry,
        "acknowledge_match_disagreement",
        json!({ "request": { "disagreementId": DISAGREEMENT_ID, "linearIssueId": "SYN-1" } }),
    )
    .await?;
    call(
        &registry,
        "link_match_disagreement",
        json!({ "request": { "disagreementId": null, "linearIssueId": null } }),
    )
    .await?;
    assert_eq!(
        recorder.calls(),
        vec![
            ("get_match_review", json!({ "id": REPORT_ID })),
            (
                "list_match_reviews",
                json!({ "reportId": null, "limit": 10, "cursor": "c" }),
            ),
            ("list_match_disagreements", json!({})),
            (
                "acknowledge_match_disagreement",
                json!({ "disagreementId": DISAGREEMENT_ID, "linearIssueId": "SYN-1" }),
            ),
            (
                "link_match_disagreement",
                json!({ "disagreementId": null, "linearIssueId": null }),
            ),
        ]
    );
    Ok(())
}

#[test]
fn the_six_tools_are_listed_in_catalog_order_only_with_a_port() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let names: Vec<String> = registry(&recorder)?
        .listed_tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert_eq!(
        names,
        [
            "submit_match_review",
            "get_match_review",
            "list_match_reviews",
            "list_match_disagreements",
            "acknowledge_match_disagreement",
            "link_match_disagreement",
        ]
    );

    let mut bare = ToolRegistry::new(Catalog::load()?);
    reviews::register(&mut bare, None)?;
    assert!(bare.listed_tools().is_empty());
    Ok(())
}
