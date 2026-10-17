//! Every recorded tool call must reproduce the recorded `CallToolResult` byte for byte.
//!
//! The goldens are `tests/golden/calls-success.json`, `calls-errors.json` and `calls-arguments.json`.
//! Each case configures the service outcome of the
//! test-only tool module, calls the tool over the in-process transport and compares the complete
//! result: content text (byte for byte), `structuredContent` and `isError`. `resultType` is not
//! compared (see `without_result_type`); the goldens are client-side model dumps, not wire captures.

mod common;

use std::error::Error;
use std::sync::Arc;

use common::test_tools::{Script, registry};
use jobniac_mcp::JobniacMcpServer;
use rmcp::model::CallToolRequestParams;
use serde_json::Value;

const GOLDENS: &[(&str, &str)] = &[
    (
        "calls-success.json",
        include_str!("golden/calls-success.json"),
    ),
    (
        "calls-errors.json",
        include_str!("golden/calls-errors.json"),
    ),
    (
        "calls-arguments.json",
        include_str!("golden/calls-arguments.json"),
    ),
];

/// `resultType` is a protocol-2026-07-28 field. rmcp omits it for an older negotiated version
/// (the client here), while the recorded dump carries the model default `"complete"`. It is not
/// compared; every other key, including the text bytes, is.
fn without_result_type(mut result: Value) -> Value {
    if let Some(object) = result.as_object_mut() {
        object.remove("resultType");
    }
    result
}

#[tokio::test]
async fn every_recorded_call_reproduces_the_recorded_result() -> Result<(), Box<dyn Error>> {
    let mut checked = 0_usize;
    for (file, text) in GOLDENS {
        let cases: Vec<Value> = serde_json::from_str(text)?;
        for case in cases {
            let id = case["id"].as_str().unwrap_or_default();
            let tool = case["tool"].as_str().ok_or("case without a tool")?;
            let arguments = case["arguments"]
                .as_object()
                .cloned()
                .ok_or_else(|| format!("{file}: {id} has no argument object"))?;

            let script = Arc::new(Script::default());
            script.configure(&case["service"]);
            let server = JobniacMcpServer::with_registry(registry(&script)?);
            let client = common::connect_server(server).await?;

            let params = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
            let result = client.call_tool(params).await?;
            let actual = without_result_type(serde_json::to_value(&result)?);
            let expected = without_result_type(case["result"].clone());
            assert_eq!(actual, expected, "{file}: case {id}");

            let expected_calls = usize::from(!case["service"].is_null());
            assert_eq!(
                script.calls().len(),
                expected_calls,
                "{file}: case {id} service calls"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 372, "the goldens hold 372 recorded calls");
    Ok(())
}

#[tokio::test]
async fn texts_are_byte_identical_to_the_recorded_text() -> Result<(), Box<dyn Error>> {
    // A spot check that reads the expected text straight from the golden, so a failure shows the
    // exact recorded bytes rather than a structural difference.
    let cases: Vec<Value> = serde_json::from_str(include_str!("golden/calls-errors.json"))?;
    let case = cases
        .iter()
        .find(|case| case["id"] == "error-analyze-job-match-default-separators")
        .ok_or("golden case missing")?;
    let expected = case["result"]["content"][0]["text"]
        .as_str()
        .ok_or("golden text missing")?;

    let script = Arc::new(Script::default());
    script.configure(&case["service"]);
    let client =
        common::connect_server(JobniacMcpServer::with_registry(registry(&script)?)).await?;
    let arguments = case["arguments"].as_object().cloned().unwrap_or_default();
    let result = client
        .call_tool(
            CallToolRequestParams::new("analyze_job_match".to_owned()).with_arguments(arguments),
        )
        .await?;
    let text = result
        .content
        .first()
        .and_then(|block| block.as_text())
        .map(|text| text.text.as_str())
        .ok_or("no text content")?;
    assert_eq!(text, expected);
    assert!(text.contains("\\u00fc") && text.contains("\\ud83d\\ude42"));
    Ok(())
}

/// Appends a unique canary to every string value of an argument tree. Object keys are left alone.
fn with_canaries(value: &mut Value, counter: &mut usize) {
    match value {
        Value::String(text) => {
            *counter += 1;
            text.push_str(&format!(" CANARY{:04}X", *counter));
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| with_canaries(item, counter)),
        Value::Object(map) => map
            .values_mut()
            .for_each(|item| with_canaries(item, counter)),
        _ => {}
    }
}

#[tokio::test]
async fn rejected_argument_values_are_never_echoed() -> Result<(), Box<dyn Error>> {
    // A rejected value must not reach any error text or structured content.
    // Every string value of each argument-rejection golden carries a canary; the call must still
    // be rejected, must not reach a service, and the canary must not appear in the result.
    let cases: Vec<Value> = serde_json::from_str(include_str!("golden/calls-arguments.json"))?;
    let mut checked = 0_usize;
    for case in cases {
        let id = case["id"].as_str().unwrap_or_default();
        let tool = case["tool"].as_str().ok_or("case without a tool")?;
        let mut arguments = case["arguments"].clone();
        let mut counter = 0_usize;
        with_canaries(&mut arguments, &mut counter);
        let arguments = arguments
            .as_object()
            .cloned()
            .ok_or_else(|| format!("{id} has no argument object"))?;

        let script = Arc::new(Script::default());
        script.configure(&case["service"]);
        let server = JobniacMcpServer::with_registry(registry(&script)?);
        let client = common::connect_server(server).await?;
        let params = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
        let result = client.call_tool(params).await?;
        let wire = serde_json::to_value(&result)?;

        assert_eq!(wire["isError"], Value::Bool(true), "{id}: still rejected");
        assert!(
            script.calls().is_empty(),
            "{id}: a rejected call reached a service"
        );
        let rendered = wire.to_string();
        assert!(
            !rendered.contains("CANARY"),
            "{id}: a rejected value is echoed: {rendered}"
        );
        checked += 1;
    }
    assert_eq!(checked, 187, "every argument-rejection golden is checked");
    Ok(())
}
