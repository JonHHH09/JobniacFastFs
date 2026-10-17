//! The opt-in `get_document_artifact` tool. The port is a recorder, so the tests cover registration, the schema, argument
//! handling and the envelope only. All data is synthetic.

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::errors::{AppError, ApplicationError, ErrorCode, not_found};
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::{Arguments, ToolRegistry};
use jobniac_mcp::result::ToolResult;
use jobniac_mcp::tools::artifacts::{self, ArtifactPort, GET_DOCUMENT_ARTIFACT};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

const DOCUMENT_ID: &str = "00000000-0000-4000-8000-0000000000aa";

/// Records each identifier it receives and answers with a configured outcome.
#[derive(Debug)]
struct Recorder {
    seen: Mutex<Vec<String>>,
    answer: Mutex<Result<Value, AppError>>,
}

impl Recorder {
    fn answering(answer: Result<Value, AppError>) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            answer: Mutex::new(answer),
        })
    }

    fn seen(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl ArtifactPort for Recorder {
    async fn get_document_artifact(&self, document_id: &str) -> Result<Value, AppError> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(document_id.to_owned());
        self.answer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

fn registry_with(port: Option<Arc<dyn ArtifactPort>>) -> Result<ToolRegistry, Box<dyn Error>> {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    artifacts::register(&mut registry, port)?;
    Ok(registry)
}

async fn call(registry: &ToolRegistry, arguments: Value) -> Result<ToolResult, Box<dyn Error>> {
    let arguments: Arguments = arguments
        .as_object()
        .cloned()
        .ok_or("arguments must be an object")?;
    registry
        .call(GET_DOCUMENT_ARTIFACT, &arguments)
        .await
        .ok_or_else(|| "get_document_artifact is not registered".into())
}

#[test]
fn the_tool_is_absent_without_an_injected_port() -> TestResult {
    let registry = registry_with(None)?;
    assert!(!registry.is_registered(GET_DOCUMENT_ARTIFACT));
    let names: Vec<String> = registry
        .listed_tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert!(!names.iter().any(|name| name == GET_DOCUMENT_ARTIFACT));
    Ok(())
}

#[test]
fn the_tool_is_listed_with_its_read_only_schema_when_the_port_is_present() -> TestResult {
    let registry = registry_with(Some(Recorder::answering(Ok(json!({})))))?;
    assert!(registry.is_registered(GET_DOCUMENT_ARTIFACT));
    let listed = registry
        .listed_tools()
        .into_iter()
        .find(|tool| tool.name == GET_DOCUMENT_ARTIFACT)
        .ok_or("the tool is not listed")?;
    let value = serde_json::to_value(&listed)?;
    assert_eq!(value["annotations"]["readOnlyHint"], json!(true));
    assert_eq!(value["annotations"]["destructiveHint"], json!(false));
    assert_eq!(value["annotations"]["idempotentHint"], json!(true));
    assert_eq!(value["annotations"]["openWorldHint"], json!(false));
    assert_eq!(value["inputSchema"]["required"], json!(["documentId"]));
    Ok(())
}

#[tokio::test]
async fn the_identifier_is_forwarded_as_sent_and_the_answer_is_returned() -> TestResult {
    let answer = json!({"document": {"id": DOCUMENT_ID}, "contentBase64": "JVBERi0="});
    let recorder = Recorder::answering(Ok(answer.clone()));
    let registry = registry_with(Some(recorder.clone()))?;
    let outcome = call(&registry, json!({"documentId": DOCUMENT_ID})).await?;
    assert_eq!(recorder.seen(), [DOCUMENT_ID]);
    assert!(matches!(outcome, ToolResult::Success(ref value) if *value == answer));
    Ok(())
}

#[tokio::test]
async fn a_missing_or_non_string_identifier_never_reaches_the_port() -> TestResult {
    let recorder = Recorder::answering(Ok(json!({})));
    let registry = registry_with(Some(recorder.clone()))?;
    for arguments in [
        json!({}),
        json!({"documentId": null}),
        json!({"documentId": 7}),
    ] {
        let outcome = call(&registry, arguments).await?;
        assert!(matches!(outcome, ToolResult::InvalidArguments(_)));
    }
    assert!(recorder.seen().is_empty());
    Ok(())
}

#[tokio::test]
async fn an_application_error_is_the_object_envelope_with_the_default_separators() -> TestResult {
    let error = not_found(
        "Stored document was not found",
        [("documentId", DOCUMENT_ID)],
    );
    let recorder = Recorder::answering(Err(AppError::from(error)));
    let registry = registry_with(Some(recorder))?;
    let outcome = call(&registry, json!({"documentId": DOCUMENT_ID})).await?;
    let ToolResult::Failure { envelope, .. } = outcome else {
        return Err("expected an application failure".into());
    };
    assert_eq!(envelope.code, ErrorCode::NotFound);
    assert_eq!(envelope.message, "Stored document was not found");
    assert_eq!(
        envelope.details.get("documentId").map(String::as_str),
        Some(DOCUMENT_ID)
    );
    Ok(())
}

#[tokio::test]
async fn an_unclassified_failure_does_not_expose_its_text() -> TestResult {
    let recorder = Recorder::answering(Err(AppError::Internal));
    let registry = registry_with(Some(recorder))?;
    let outcome = call(&registry, json!({"documentId": DOCUMENT_ID})).await?;
    let ToolResult::Failure { envelope, .. } = outcome else {
        return Err("expected an application failure".into());
    };
    assert_eq!(envelope.code, ErrorCode::InternalError);
    assert!(!envelope.message.contains('/'));
    assert!(!envelope.message.contains('\\'));
    Ok(())
}

#[tokio::test]
async fn the_database_unavailable_envelope_is_forwarded_unchanged() -> TestResult {
    let recorder = Recorder::answering(Err(AppError::from(
        ApplicationError::database_unavailable(),
    )));
    let registry = registry_with(Some(recorder))?;
    let outcome = call(&registry, json!({"documentId": DOCUMENT_ID})).await?;
    let ToolResult::Failure { envelope, .. } = outcome else {
        return Err("expected an application failure".into());
    };
    assert_eq!(envelope.message, "Local database is unavailable");
    Ok(())
}
