//! What the profile PDF tool handlers pass to their port: the declared keys of the ingestion request
//! with their nulls, the required document identifier, the compact envelope text of a failure, and
//! the rejections that never reach the port. All data is synthetic.

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::errors::{AppError, not_found};
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::{Arguments, ToolRegistry};
use jobniac_mcp::result::ToolResult;
use jobniac_mcp::tools::profile_pdf::{self, ProfilePdfPort};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

const DOCUMENT_ID: &str = "00000000-0000-4000-8000-0000000000CC";
const PROFILE_ID: &str = "00000000-0000-4000-8000-0000000000AA";

/// Records one `(port method, value the port received)` pair per call. When `fail` is set, every
/// call answers a not-found application error.
#[derive(Debug, Default)]
struct Recorder {
    calls: Mutex<Vec<(&'static str, Value)>>,
    fail: bool,
}

impl Recorder {
    fn record(&self, method: &'static str, received: Value) -> Result<Value, AppError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((method, received));
        if self.fail {
            return Err(AppError::from(not_found(
                "Profile PDF source was not found",
                [("profileId", PROFILE_ID)],
            )));
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
impl ProfilePdfPort for Recorder {
    async fn ingest_profile_from_stored_pdf(&self, request: &Value) -> Result<Value, AppError> {
        self.record("ingest_profile_from_stored_pdf", request.clone())
    }

    async fn get_profile_pdf_source(&self, profile_id: &str) -> Result<Value, AppError> {
        self.record("get_profile_pdf_source", json!(profile_id))
    }
}

fn registry(recorder: Arc<Recorder>) -> Result<ToolRegistry, Box<dyn Error>> {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    profile_pdf::register(&mut registry, Some(recorder as Arc<dyn ProfilePdfPort>))?;
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
async fn ingestion_passes_the_declared_keys_and_the_nulls() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(Arc::clone(&recorder))?;
    let outcome = call(
        &registry,
        "ingest_profile_from_stored_pdf",
        json!({"request": {
            "documentId": DOCUMENT_ID,
            "existingProfileId": null,
            "overwriteExistingProfile": true,
            "maxCharacters": null,
            "expectedRevision": 3,
            "unknown": "dropped"
        }}),
    )
    .await?;
    assert!(matches!(outcome, ToolResult::Success(_)), "{outcome:?}");
    assert_eq!(
        recorder.calls(),
        vec![(
            "ingest_profile_from_stored_pdf",
            json!({
                "documentId": DOCUMENT_ID,
                "existingProfileId": null,
                "overwriteExistingProfile": true,
                "maxCharacters": null,
                "expectedRevision": 3
            })
        )]
    );
    Ok(())
}

#[tokio::test]
async fn ingestion_rejects_a_missing_or_null_document_identifier() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(Arc::clone(&recorder))?;
    for rejected in [
        json!({"request": {}}),
        json!({"request": {"documentId": null}}),
        json!({"request": {"documentId": DOCUMENT_ID, "expectedRevision": 2.0}}),
    ] {
        let outcome = call(&registry, "ingest_profile_from_stored_pdf", rejected).await?;
        assert!(
            matches!(outcome, ToolResult::InvalidArguments(_)),
            "{outcome:?}"
        );
    }
    assert!(
        recorder.calls().is_empty(),
        "rejected calls must not reach the port"
    );
    Ok(())
}

#[tokio::test]
async fn source_passes_the_profile_identifier_text() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(Arc::clone(&recorder))?;
    let outcome = call(
        &registry,
        "get_profile_pdf_source",
        json!({"profileId": PROFILE_ID}),
    )
    .await?;
    assert!(matches!(outcome, ToolResult::Success(_)), "{outcome:?}");
    assert_eq!(
        recorder.calls(),
        vec![("get_profile_pdf_source", json!(PROFILE_ID))]
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_source_lookup_uses_the_compact_envelope_text() -> TestResult {
    let recorder = Arc::new(Recorder {
        calls: Mutex::default(),
        fail: true,
    });
    let registry = registry(recorder)?;
    let result = call(
        &registry,
        "get_profile_pdf_source",
        json!({"profileId": PROFILE_ID}),
    )
    .await?
    .into_call_result();
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
            "{{\"code\":\"not_found\",\"message\":\"Profile PDF source was not found\",\"details\":{{\"profileId\":\"{PROFILE_ID}\"}}}}"
        )
    );
    Ok(())
}

#[tokio::test]
async fn max_characters_beyond_the_u64_range_is_refused_at_the_sdk_layer() -> TestResult {
    // The service answers `must be less than or equal to 250000`. An integer beyond u64 is refused as int_type before
    // the port runs; an integer within the u64 range reaches the port unchanged.
    let recorder = Arc::new(Recorder::default());
    let registry = registry(Arc::clone(&recorder))?;
    let within_u64 = json!({"request": {"documentId": DOCUMENT_ID, "maxCharacters": 9_223_372_036_854_775_808_u64}});
    let outcome = call(&registry, "ingest_profile_from_stored_pdf", within_u64).await?;
    assert!(matches!(outcome, ToolResult::Success(_)), "{outcome:?}");
    assert_eq!(
        recorder.calls()[0].1["maxCharacters"],
        json!(9_223_372_036_854_775_808_u64)
    );

    let beyond: Value = serde_json::from_str(&format!(
        r#"{{"request": {{"documentId": "{DOCUMENT_ID}", "maxCharacters": 100000000000000000000}}}}"#
    ))?;
    let outcome = call(&registry, "ingest_profile_from_stored_pdf", beyond).await?;
    let ToolResult::InvalidArguments(errors) = outcome else {
        return Err("a maxCharacters beyond u64 is not an argument error".into());
    };
    let rendered = errors.render();
    assert!(rendered.contains("request.maxCharacters"), "{rendered}");
    assert!(rendered.contains("[type=int_type]"), "{rendered}");
    assert_eq!(
        recorder.calls().len(),
        1,
        "a refused integer must not reach the port"
    );
    Ok(())
}

#[tokio::test]
async fn an_absent_port_leaves_both_tools_unregistered() -> TestResult {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    profile_pdf::register(&mut registry, None)?;
    assert!(!registry.is_registered("ingest_profile_from_stored_pdf"));
    assert!(!registry.is_registered("get_profile_pdf_source"));
    Ok(())
}
