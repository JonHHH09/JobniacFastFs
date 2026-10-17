//! What the document tool handlers pass to their port: the declared keys of each request object
//! with their nulls, the strict types checked before the port runs, the identifier as sent, and the
//! envelope text of an application failure. All data is synthetic.

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::errors::{AppError, ApplicationError, not_found};
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::{Arguments, ToolRegistry};
use jobniac_mcp::result::ToolResult;
use jobniac_mcp::tools::documents::{self, DocumentPort};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

const DOCUMENT_ID: &str = "00000000-0000-4000-8000-0000000000CC";

/// Records one `(port method, value the port received)` pair per call. The answer is configured
/// per test: a value, or an application error.
#[derive(Debug, Default)]
struct Recorder {
    calls: Mutex<Vec<(&'static str, Value)>>,
    fail_with: Mutex<Option<ApplicationError>>,
}

impl Recorder {
    fn record(&self, method: &'static str, received: Value) -> Result<Value, AppError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((method, received));
        if let Some(error) = self
            .fail_with
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return Err(AppError::Application(error));
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
impl DocumentPort for Recorder {
    async fn store_document_file(&self, request: &Value) -> Result<Value, AppError> {
        self.record("store_document_file", request.clone())
    }

    async fn get_document_metadata(&self, document_id: &str) -> Result<Value, AppError> {
        self.record("get_document_metadata", json!(document_id))
    }

    async fn extract_pdf_text(&self, request: &Value) -> Result<Value, AppError> {
        self.record("extract_pdf_text", request.clone())
    }

    async fn extract_stored_pdf_text(&self, request: &Value) -> Result<Value, AppError> {
        self.record("extract_stored_pdf_text", request.clone())
    }
}

fn registry(recorder: &Arc<Recorder>) -> Result<ToolRegistry, Box<dyn Error>> {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    documents::register(
        &mut registry,
        Some(Arc::clone(recorder) as Arc<dyn DocumentPort>),
    )?;
    Ok(registry)
}

async fn call(
    registry: &ToolRegistry,
    name: &str,
    arguments: Value,
) -> Result<ToolResult, Box<dyn Error>> {
    let arguments: Arguments = arguments
        .as_object()
        .cloned()
        .ok_or("arguments must be an object")?;
    registry
        .call(name, &arguments)
        .await
        .ok_or_else(|| format!("tool {name} is not registered").into())
}

#[tokio::test]
async fn store_passes_the_declared_keys_with_their_nulls() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let outcome = call(
        &registry,
        "store_document_file",
        json!({"request": {"path": "inbox/a.pdf", "mediaType": null, "extra": 1}}),
    )
    .await?;
    assert!(matches!(outcome, ToolResult::Success(_)), "{outcome:?}");
    assert_eq!(
        recorder.calls(),
        vec![(
            "store_document_file",
            json!({"path": "inbox/a.pdf", "mediaType": null})
        )]
    );
    Ok(())
}

#[tokio::test]
async fn metadata_passes_the_identifier_text_unchanged() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let outcome = call(
        &registry,
        "get_document_metadata",
        json!({"documentId": "not-parsed-by-the-handler"}),
    )
    .await?;
    assert!(matches!(outcome, ToolResult::Success(_)), "{outcome:?}");
    assert_eq!(
        recorder.calls(),
        vec![("get_document_metadata", json!("not-parsed-by-the-handler"))]
    );
    Ok(())
}

#[tokio::test]
async fn extraction_requests_keep_nulls_and_reject_loose_types() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let outcome = call(
        &registry,
        "extract_stored_pdf_text",
        json!({"request": {
            "documentId": DOCUMENT_ID,
            "maxCharacters": null,
            "includePages": null,
            "persistExtraction": true
        }}),
    )
    .await?;
    assert!(matches!(outcome, ToolResult::Success(_)), "{outcome:?}");

    for rejected in [
        json!({"request": {"path": "a.pdf", "maxCharacters": "10", "includePages": null}}),
        json!({"request": {"path": "a.pdf", "maxCharacters": null, "includePages": "yes"}}),
        json!({"request": {"path": "a.pdf", "maxCharacters": null}}),
    ] {
        let outcome = call(&registry, "extract_pdf_text", rejected).await?;
        assert!(
            matches!(outcome, ToolResult::InvalidArguments(_)),
            "{outcome:?}"
        );
    }
    assert_eq!(
        recorder.calls().len(),
        1,
        "rejected calls must not reach the port"
    );
    Ok(())
}

#[tokio::test]
async fn max_characters_beyond_the_u64_range_is_refused_at_the_sdk_layer() -> TestResult {
    // The service answers `must be less than or equal to 250000` or `must be greater than zero`.
    // A serde_json integer is held as i64 or u64 only, so an integer beyond u64 is refused as
    // int_type before the port runs. Integers within the u64 range reach the port unchanged, so
    // the service ranges them.
    let recorder = Arc::new(Recorder::default());
    let registry = registry(&recorder)?;
    let within_u64 = json!({"request": {"path": "a.pdf", "maxCharacters": 9_223_372_036_854_775_808_u64, "includePages": null}});
    let outcome = call(&registry, "extract_pdf_text", within_u64).await?;
    assert!(matches!(outcome, ToolResult::Success(_)), "{outcome:?}");
    assert_eq!(
        recorder.calls()[0].1["maxCharacters"],
        json!(9_223_372_036_854_775_808_u64)
    );

    for literal in ["100000000000000000000", "-100000000000000000000"] {
        let arguments: Value = serde_json::from_str(&format!(
            r#"{{"request": {{"path": "a.pdf", "maxCharacters": {literal}, "includePages": null}}}}"#
        ))?;
        let outcome = call(&registry, "extract_pdf_text", arguments).await?;
        let ToolResult::InvalidArguments(errors) = outcome else {
            return Err(format!("{literal} is not an argument error").into());
        };
        let rendered = errors.render();
        assert!(rendered.contains("request.maxCharacters"), "{rendered}");
        assert!(rendered.contains("[type=int_type]"), "{rendered}");
    }
    assert_eq!(
        recorder.calls().len(),
        1,
        "a refused integer must not reach the port"
    );
    Ok(())
}

#[tokio::test]
async fn an_application_failure_uses_the_default_separators_and_no_path() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let failure = not_found(
        "Stored document was not found",
        [("documentId", DOCUMENT_ID)],
    );
    *recorder
        .fail_with
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(failure);
    let registry = registry(&recorder)?;
    let outcome = call(
        &registry,
        "get_document_metadata",
        json!({"documentId": DOCUMENT_ID}),
    )
    .await?;
    let result = outcome.into_call_result();
    assert_eq!(result.is_error, Some(true));
    let text = result
        .content
        .first()
        .and_then(|block| block.as_text())
        .map(|text| text.text.clone())
        .ok_or("no text content")?;
    assert_eq!(
        text,
        format!(
            "{{\"code\": \"not_found\", \"message\": \"Stored document was not found\", \"details\": {{\"documentId\": \"{DOCUMENT_ID}\"}}}}"
        )
    );
    Ok(())
}

#[tokio::test]
async fn an_absent_port_leaves_the_four_tools_unregistered() -> TestResult {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    documents::register(&mut registry, None)?;
    assert!(!registry.is_registered("store_document_file"));
    assert!(!registry.is_registered("extract_pdf_text"));
    Ok(())
}
