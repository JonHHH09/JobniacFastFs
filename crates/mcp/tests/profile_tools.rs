//! What the profile tool handlers pass to their ports. The call goldens check the wire result; this
//! file checks the values the service receives: normalized identifiers, the revision, the request
//! object and the optional listing arguments. All data is synthetic.

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::clock::FixedClock;
use jobniac_core::errors::AppError;
use jobniac_core::health::{HealthProbe, HealthProbeResult, HealthService};
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::{Arguments, ToolRegistry};
use jobniac_mcp::result::ToolResult;
use jobniac_mcp::tools::profiles::{self, ProfileDetailsPort, ProfileListPort, ProfileSearchPort};
use jobniac_mcp::{JobniacMcpServer, McpServices, serve_with};
use serde_json::{Value, json};
use time::macros::datetime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

type TestResult = Result<(), Box<dyn Error>>;

const PROFILE_ID: &str = "00000000-0000-4000-8000-0000000000aa";

/// Records one line per port call and answers every call with `{"ok": true}`.
#[derive(Debug, Default)]
struct Recorder {
    calls: Mutex<Vec<String>>,
}

impl Recorder {
    fn record(&self, line: String) -> Result<Value, AppError> {
        // A poisoned lock keeps its calls: the first panic stays the reported failure.
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line);
        Ok(json!({"ok": true}))
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl ProfileListPort for Recorder {
    async fn list_profiles(
        &self,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<Value, AppError> {
        self.record(format!("list_profiles:{limit:?}:{cursor:?}"))
    }
}

#[async_trait]
impl ProfileDetailsPort for Recorder {
    async fn get_profile(&self, profile_id: &str) -> Result<Value, AppError> {
        self.record(format!("get_profile:{profile_id}"))
    }

    async fn create_profile(&self, request: &Value) -> Result<Value, AppError> {
        self.record(format!("create_profile:{}", request["fullName"]))
    }

    async fn update_profile(
        &self,
        profile_id: &str,
        expected_revision: i64,
        request: &Value,
    ) -> Result<Value, AppError> {
        self.record(format!(
            "update_profile:{profile_id}:{expected_revision}:{}",
            request["fullName"]
        ))
    }

    async fn update_profile_project(&self, request: &Value) -> Result<Value, AppError> {
        self.record(format!(
            "update_profile_project:{}:{}",
            request["projectId"], request["expectedRevision"]
        ))
    }
}

#[async_trait]
impl ProfileSearchPort for Recorder {
    async fn search_profiles(&self, request: &Value) -> Result<Value, AppError> {
        self.record(format!("search_profiles:{}", request["query"]))
    }
}

/// A registry with the three profile ports over one recorder.
fn registry(recorder: &Arc<Recorder>) -> Result<ToolRegistry, Box<dyn Error>> {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    profiles::register(
        &mut registry,
        Some(Arc::clone(recorder) as Arc<dyn ProfileListPort>),
        Some(Arc::clone(recorder) as Arc<dyn ProfileDetailsPort>),
        Some(Arc::clone(recorder) as Arc<dyn ProfileSearchPort>),
    )?;
    Ok(registry)
}

/// The arguments of one call, from a JSON object.
fn arguments(value: Value) -> Arguments {
    match value {
        Value::Object(map) => map,
        _ => Arguments::new(),
    }
}

async fn call(
    registry: &ToolRegistry,
    name: &str,
    args: Value,
) -> Result<ToolResult, Box<dyn Error>> {
    registry
        .call(name, &arguments(args))
        .await
        .ok_or_else(|| format!("{name} is not registered").into())
}

/// Answers every health probe as ready; the health tool is not called in this file.
#[derive(Debug)]
struct ReadyProbe;

#[async_trait]
impl HealthProbe for ReadyProbe {
    async fn probe(&self) -> HealthProbeResult {
        HealthProbeResult::ready()
    }
}

/// Runs one `tools/call` of `get_profile` whose raw `arguments` text is sent as written, through
/// `serve_with` (the admission pipe of `serve_stdio`). A `Value` cannot hold a repeated key, so the
/// raw text is the only way to submit one. Returns the response of the call.
async fn call_raw(recorder: &Arc<Recorder>, arguments: &str) -> Result<Value, Box<dyn Error>> {
    let services = McpServices {
        health: Arc::new(HealthService::new(
            Arc::new(ReadyProbe),
            None,
            Arc::new(FixedClock::new(datetime!(2026-01-02 03:04:05.000006 UTC))),
        )),
        profile_list: None,
        profile_details: Some(Arc::clone(recorder) as Arc<dyn ProfileDetailsPort>),
        profile_search: None,
        job_service: None,
        job_search: None,
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
    let (mut input, input_reader) = tokio::io::duplex(64 * 1024);
    let (output_writer, output) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        // The session ends when the input closes; the result is not needed here.
        let _ = serve_with(server, input_reader, output_writer).await;
    });
    let mut output = BufReader::new(output).lines();
    input
        .write_all(
            br#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"profile-tools-test","version":"0.0.0"}}}"#,
        )
        .await?;
    input.write_all(b"\n").await?;
    input.flush().await?;
    output
        .next_line()
        .await?
        .ok_or("the session ended before the initialize response")?;
    let call = format!(
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}
{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"get_profile","arguments":{arguments}}}}}"#
    );
    input.write_all(call.as_bytes()).await?;
    input.write_all(b"\n").await?;
    input.flush().await?;
    let text = output
        .next_line()
        .await?
        .ok_or("the session ended before the call response")?;
    Ok(serde_json::from_str(&text)?)
}

fn write_request(full_name: &str) -> Value {
    json!({
        "fullName": full_name, "email": null, "summary": null, "contacts": null,
        "links": null, "skills": null, "languages": null, "education": null,
        "experiences": null, "projects": null,
    })
}

#[tokio::test]
async fn get_profile_passes_the_canonical_lowercase_identifier() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let upper = PROFILE_ID.to_ascii_uppercase();
    let result = call(&registry, "get_profile", json!({ "profileId": upper })).await?;
    assert!(matches!(result, ToolResult::Success(_)));
    assert_eq!(recorder.calls(), [format!("get_profile:{PROFILE_ID}")]);
    Ok(())
}

#[tokio::test]
async fn non_canonical_spellings_reach_the_service_unchanged() -> TestResult {
    // A braced identifier is accepted as a UUID spelling, so the SDK does not refuse it. It
    // is not normalized here; the service refuses it with `validation_error`.
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let braced = format!("{{{PROFILE_ID}}}");
    call(&registry, "get_profile", json!({ "profileId": braced })).await?;
    assert_eq!(recorder.calls(), [format!("get_profile:{braced}")]);
    Ok(())
}

#[tokio::test]
async fn update_profile_passes_the_revision_and_the_request_object() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let args = json!({
        "profileId": PROFILE_ID,
        "expectedRevision": 7,
        "request": write_request("Example Candidate"),
    });
    call(&registry, "update_profile", args).await?;
    assert_eq!(
        recorder.calls(),
        [format!(
            "update_profile:{PROFILE_ID}:7:\"Example Candidate\""
        )]
    );
    Ok(())
}

#[tokio::test]
async fn create_profile_passes_the_request_object() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    call(
        &registry,
        "create_profile",
        json!({ "request": write_request("Example Candidate") }),
    )
    .await?;
    assert_eq!(recorder.calls(), ["create_profile:\"Example Candidate\""]);
    Ok(())
}

#[tokio::test]
async fn update_profile_project_passes_the_patch_object() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let args = json!({
        "request": {
            "profileId": PROFILE_ID,
            "projectId": "00000000-0000-4000-8000-0000000000a1",
            "expectedRevision": 3,
            "name": null,
            "technologies": null,
        }
    });
    call(&registry, "update_profile_project", args).await?;
    assert_eq!(
        recorder.calls(),
        ["update_profile_project:\"00000000-0000-4000-8000-0000000000a1\":3"]
    );
    Ok(())
}

#[tokio::test]
async fn search_profiles_passes_the_query_object() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let result = call(
        &registry,
        "search_profiles",
        json!({ "request": { "query": "rust", "limit": null } }),
    )
    .await?;
    assert!(matches!(result, ToolResult::Success(_)));
    assert_eq!(recorder.calls(), ["search_profiles:\"rust\""]);
    Ok(())
}

#[tokio::test]
async fn list_profiles_without_arguments_passes_no_limit_or_cursor() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    call(&registry, "list_profiles", json!({})).await?;
    call(&registry, "list_profiles", json!({ "request": null })).await?;
    call(
        &registry,
        "list_profiles",
        json!({ "request": { "limit": 2, "cursor": "synthetic-cursor" } }),
    )
    .await?;
    assert_eq!(
        recorder.calls(),
        [
            "list_profiles:None:None",
            "list_profiles:None:None",
            "list_profiles:Some(2):Some(\"synthetic-cursor\")",
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_rejected_call_never_reaches_the_port() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let result = call(&registry, "get_profile", json!({ "profileId": 12345 })).await?;
    assert!(matches!(result, ToolResult::InvalidArguments(_)));
    let result = call(
        &registry,
        "update_profile",
        json!({ "profileId": PROFILE_ID }),
    )
    .await?;
    assert!(matches!(result, ToolResult::InvalidArguments(_)));
    assert!(recorder.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_malformed_profile_id_answers_the_generic_text_without_echoing_it() -> TestResult {
    // The port answers with the generic `uuid_parsing` text and never repeats the submitted
    // string.
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let submitted = "not-a-uuid-Secret123";
    let cases = [
        ("get_profile", json!({ "profileId": submitted })),
        (
            "update_profile",
            json!({
                "profileId": submitted,
                "expectedRevision": 1,
                "request": write_request("Example Candidate"),
            }),
        ),
    ];
    for (tool, args) in cases {
        let result = call(&registry, tool, args).await?;
        let ToolResult::InvalidArguments(errors) = result.clone() else {
            return Err(format!("{tool}: a malformed identifier is not an argument error").into());
        };
        let expected = format!(
            "Error executing tool {tool}: 1 validation error for {tool}Arguments\n\
             profileId\n  Input should be a valid UUID [type=uuid_parsing]\n    \
             For further information visit https://errors.pydantic.dev/2.13/v/uuid_parsing"
        );
        assert_eq!(errors.render(), expected, "{tool}");
        let wire = serde_json::to_value(result.into_call_result())?;
        assert_eq!(wire["isError"], json!(true), "{tool}");
        assert!(!wire.to_string().contains(submitted), "{tool}");
    }
    assert!(recorder.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_repeated_key_keeps_the_last_value() -> TestResult {
    // A repeated key keeps its last value. The first `profileId`
    // is malformed, the last one is valid, and the valid one must reach the port.
    let recorder = Arc::new(Recorder::default());
    let arguments = format!(r#"{{"profileId":"not-a-uuid","profileId":"{PROFILE_ID}"}}"#);
    let response = call_raw(&recorder, &arguments).await?;
    assert_eq!(response["result"]["isError"], json!(false), "{response}");
    assert_eq!(recorder.calls(), [format!("get_profile:{PROFILE_ID}")]);
    Ok(())
}

#[tokio::test]
async fn integers_beyond_i64_are_refused_at_the_sdk_layer() -> TestResult {
    // The port refuses an integer beyond `i64` as `int_type` before the service runs. This test
    // pins the refusal.
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let huge_limit: Value =
        serde_json::from_str(r#"{"request": {"limit": 100000000000000000000}}"#)?;
    let result = call(&registry, "list_profiles", huge_limit).await?;
    let ToolResult::InvalidArguments(errors) = result else {
        return Err("a limit beyond i64 is not an argument error".into());
    };
    assert_eq!(
        errors.render(),
        "Error executing tool list_profiles: 1 validation error for list_profilesArguments\n\
         request.limit\n  Input should be a valid integer [type=int_type]\n    \
         For further information visit https://errors.pydantic.dev/2.13/v/int_type"
    );

    let huge_revision: Value = serde_json::from_str(&format!(
        r#"{{"profileId": "{PROFILE_ID}", "expectedRevision": 9223372036854775808, "request": {{"fullName": null, "email": null, "summary": null, "contacts": null, "links": null, "skills": null, "languages": null, "education": null, "experiences": null, "projects": null}}}}"#
    ))?;
    let result = call(&registry, "update_profile", huge_revision).await?;
    assert!(matches!(result, ToolResult::InvalidArguments(_)));

    let huge_display_order: Value = serde_json::from_str(
        r#"{"profileId": "00000000-0000-4000-8000-0000000000aa", "expectedRevision": 1, "request": {"fullName": null, "email": null, "summary": null, "contacts": null, "links": null, "skills": [{"id": null, "skill": null, "normalizedSkill": null, "category": null, "displayOrder": 9223372036854775808}], "languages": null, "education": null, "experiences": null, "projects": null}}"#,
    )?;
    let result = call(&registry, "update_profile", huge_display_order).await?;
    let ToolResult::InvalidArguments(errors) = result else {
        return Err("a displayOrder beyond i64 is not an argument error".into());
    };
    let rendered = errors.render();
    assert!(rendered.contains("displayOrder"), "{rendered}");
    assert!(rendered.contains("[type=int_type]"), "{rendered}");

    let huge_search_limit: Value =
        serde_json::from_str(r#"{"request": {"query": "rust", "limit": 100000000000000000000}}"#)?;
    let result = call(&registry, "search_profiles", huge_search_limit).await?;
    let ToolResult::InvalidArguments(errors) = result else {
        return Err("a search limit beyond i64 is not an argument error".into());
    };
    let rendered = errors.render();
    assert!(rendered.contains("limit"), "{rendered}");
    assert!(rendered.contains("[type=int_type]"), "{rendered}");

    assert!(recorder.calls().is_empty());
    Ok(())
}
