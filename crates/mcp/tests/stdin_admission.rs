//! End-to-end checks of stdin admission. Raw JSON-RPC lines go through
//! `serve_with`, the same admission pipe that `serve_stdio` uses, so a line that rmcp would
//! silently drop must get the argument rejection of its tool. All data is synthetic.

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use jobniac_core::clock::FixedClock;
use jobniac_core::errors::AppError;
use jobniac_core::health::{HealthProbe, HealthProbeResult, HealthService};
use jobniac_mcp::tools::profiles::ProfileDetailsPort;
use jobniac_mcp::{JobniacMcpServer, McpServices, serve_with};
use serde_json::{Value, json};
use time::macros::datetime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};

const PROFILE_ID: &str = "00000000-0000-4000-8000-0000000000aa";

/// Counts the probes that the `health` tool runs.
#[derive(Debug, Default)]
struct CountingProbe {
    calls: AtomicUsize,
}

#[async_trait]
impl HealthProbe for CountingProbe {
    async fn probe(&self) -> HealthProbeResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        HealthProbeResult::ready()
    }
}

/// Counts the `get_profile` calls that reach the service; the other calls are never expected.
#[derive(Debug, Default)]
struct CountingDetails {
    calls: AtomicUsize,
}

#[async_trait]
impl ProfileDetailsPort for CountingDetails {
    async fn get_profile(&self, _profile_id: &str) -> Result<Value, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"ok": true}))
    }

    async fn create_profile(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn update_profile(
        &self,
        _profile_id: &str,
        _expected_revision: i64,
        _request: &Value,
    ) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn update_profile_project(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

/// One client session over the admission pipe: lines in, responses out.
struct Session {
    input: DuplexStream,
    output: Lines<BufReader<DuplexStream>>,
    next_id: i64,
}

impl Session {
    /// Start a server over two in-memory pipes, as `serve_stdio` runs over STDIO.
    fn start(
        details: Arc<CountingDetails>,
        probe: Arc<CountingProbe>,
    ) -> Result<Self, Box<dyn Error>> {
        let clock = FixedClock::new(datetime!(2026-01-02 03:04:05.000006 UTC));
        let services = McpServices {
            health: Arc::new(HealthService::new(probe, None, Arc::new(clock))),
            profile_list: None,
            job_service: None,
            job_search: None,
            profile_details: Some(details),
            profile_search: None,
            matching_service: None,
            review_service: None,
            document_service: None,
            profile_pdf_service: None,
            generation_service: None,
            document_artifact_service: None,
            deletion: Default::default(),
            job_link_service: None,
            arbeitnow: Default::default(),
        };
        let server = JobniacMcpServer::new(services)?;
        let (input, input_reader) = tokio::io::duplex(64 * 1024);
        let (output_writer, output) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            // The session ends when the input closes; the result is not needed here.
            let _ = serve_with(server, input_reader, output_writer).await;
        });
        Ok(Self {
            input,
            output: BufReader::new(output).lines(),
            next_id: 1,
        })
    }

    /// Send one raw line and read the next response line.
    async fn exchange(&mut self, line: &str) -> Result<Value, Box<dyn Error>> {
        self.input.write_all(line.as_bytes()).await?;
        self.input.write_all(b"\n").await?;
        self.input.flush().await?;
        let text = self
            .output
            .next_line()
            .await?
            .ok_or("the session ended before the response")?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Send one request whose body after the id is `body` (raw JSON fields) and return its
    /// response, which must answer that id.
    async fn request(&mut self, body: &str) -> Result<Value, Box<dyn Error>> {
        let id = self.next_id;
        self.next_id += 1;
        let response = self
            .exchange(&format!(r#"{{"jsonrpc":"2.0","id":{id},{body}}}"#))
            .await?;
        if response["id"] != json!(id) {
            return Err(format!("response {response} does not answer request {id}").into());
        }
        Ok(response)
    }

    /// Send a `tools/call` line with raw `arguments` JSON and return its response.
    async fn call(&mut self, tool: &str, arguments: &str) -> Result<Value, Box<dyn Error>> {
        self.request(&format!(
            r#""method":"tools/call","params":{{"name":"{tool}","arguments":{arguments}}}"#
        ))
        .await
    }

    /// The names of the tools the server lists.
    async fn tool_names(&mut self) -> Result<Vec<String>, Box<dyn Error>> {
        let response = self.request(r#""method":"tools/list""#).await?;
        let tools = response["result"]["tools"]
            .as_array()
            .ok_or("tools/list has no tools array")?;
        Ok(tools
            .iter()
            .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
            .collect())
    }
}

/// The expected argument rejection of `tool` for one issue with no location.
fn rejection_text(tool: &str, type_id: &str, message: &str) -> String {
    format!(
        "Error executing tool {tool}: 1 validation error for {tool}Arguments\n  {message} [type={type_id}]\n    For further information visit https://errors.pydantic.dev/2.13/v/{type_id}"
    )
}

const LONE_SURROGATE: &str =
    "Input should be a valid string, unable to parse raw data as a unicode string";
const TOO_DEEP: &str = "Invalid JSON: recursion limit exceeded";

/// Assert that `response` is the argument rejection of `tool` with the given issue.
fn assert_rejected(response: &Value, tool: &str, type_id: &str, message: &str) {
    let result = &response["result"];
    assert_eq!(result["isError"], json!(true), "response: {response}");
    assert!(
        matches!(result.get("structuredContent"), None | Some(Value::Null)),
        "a rejection has no structured content: {response}"
    );
    assert_eq!(
        result["content"][0]["text"],
        json!(rejection_text(tool, type_id, message)),
        "response: {response}"
    );
}

/// An argument object whose `profileId` nests `levels` arrays deep.
fn too_deep_arguments(levels: usize) -> String {
    format!(
        r#"{{"profileId":{}1{}}}"#,
        "[".repeat(levels),
        "]".repeat(levels)
    )
}

async fn initialize(session: &mut Session) -> Result<(), Box<dyn Error>> {
    let response = session
        .exchange(
            r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"stdin-admission-test","version":"0.0.0"}}}"#,
        )
        .await?;
    if response["result"]["protocolVersion"] != json!("2025-11-25") {
        return Err(format!("unexpected initialize result: {response}").into());
    }
    session
        .input
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await?;
    Ok(())
}

#[tokio::test]
async fn rejected_input_is_answered_without_running_the_tool() -> Result<(), Box<dyn Error>> {
    let details = Arc::new(CountingDetails::default());
    let probe = Arc::new(CountingProbe::default());
    let mut session = Session::start(Arc::clone(&details), Arc::clone(&probe))?;
    initialize(&mut session).await?;

    // Every registered tool answers the rejection of its own arguments, at each depth that the
    // admission keeps out (101 and 120 levels parse in serde_json but exceed MAX_DEPTH).
    let tools = session.tool_names().await?;
    assert!(tools.iter().any(|name| name == "get_profile"), "{tools:?}");
    assert!(tools.iter().any(|name| name == "health"), "{tools:?}");
    for tool in &tools {
        let response = session.call(tool, r#"{"profileId":"\ud800"}"#).await?;
        assert_rejected(&response, tool, "string_unicode", LONE_SURROGATE);

        let response = session.call(tool, r#"{"note":"\udc00"}"#).await?;
        assert_rejected(&response, tool, "string_unicode", LONE_SURROGATE);

        for levels in [101, 120, 200] {
            let response = session.call(tool, &too_deep_arguments(levels)).await?;
            assert_rejected(&response, tool, "json_invalid", TOO_DEEP);
        }
    }

    assert_eq!(details.calls.load(Ordering::SeqCst), 0, "no port may run");
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0, "no probe may run");

    // The session is intact: a normal call of each tool still works.
    let response = session
        .call("get_profile", &format!(r#"{{"profileId":"{PROFILE_ID}"}}"#))
        .await?;
    assert_eq!(response["result"]["isError"], json!(false), "{response}");
    assert_eq!(response["result"]["structuredContent"], json!({"ok": true}));
    assert_eq!(details.calls.load(Ordering::SeqCst), 1);

    let response = session.call("health", "{}").await?;
    assert_eq!(response["result"]["isError"], json!(false), "{response}");
    assert!(probe.calls.load(Ordering::SeqCst) > 0);
    Ok(())
}

#[tokio::test]
async fn a_client_supplied_marker_does_not_reject_a_valid_call() -> Result<(), Box<dyn Error>> {
    let details = Arc::new(CountingDetails::default());
    let probe = Arc::new(CountingProbe::default());
    let mut session = Session::start(Arc::clone(&details), Arc::clone(&probe))?;
    initialize(&mut session).await?;

    // A valid line that carries the internal marker is admitted without it, so the tool runs.
    let response = session
        .request(&format!(
            r#""method":"tools/call","params":{{"name":"get_profile","arguments":{{"profileId":"{PROFILE_ID}"}},"_meta":{{"io.jobniac/rejected-input":"lone_surrogate"}}}}"#
        ))
        .await?;
    assert_eq!(response["result"]["isError"], json!(false), "{response}");
    assert_eq!(response["result"]["structuredContent"], json!({"ok": true}));
    assert_eq!(
        details.calls.load(Ordering::SeqCst),
        1,
        "the tool must run once"
    );
    Ok(())
}

#[tokio::test]
async fn an_unknown_tool_keeps_its_answer_for_rejected_input() -> Result<(), Box<dyn Error>> {
    let details = Arc::new(CountingDetails::default());
    let probe = Arc::new(CountingProbe::default());
    let mut session = Session::start(details, probe)?;
    initialize(&mut session).await?;

    let response = session.call("no_such_tool", r#"{"x":"\ud800"}"#).await?;
    assert_eq!(response["result"]["isError"], json!(true), "{response}");
    assert_eq!(
        response["result"]["content"][0]["text"],
        json!("Unknown tool: no_such_tool"),
        "{response}"
    );
    Ok(())
}
