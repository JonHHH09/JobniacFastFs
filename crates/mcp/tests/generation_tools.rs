//! What the generation tool handlers pass to their port: the declared keys of each `TypedDict`
//! request with their nulls, the strict types of the identity, text and boolean fields, the
//! default-style envelope text of a failure, and the rejections that never reach the port. All
//! data is synthetic.

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::errors::{AppError, ApplicationError, ErrorCode};
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::{Arguments, ToolRegistry};
use jobniac_mcp::result::{EnvelopeStyle, ToolResult};
use jobniac_mcp::tools::generation::{self, GenerationPort, Operation};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

const PROFILE_ID: &str = "00000000-0000-4000-8000-0000000000AA";
const JOB_ID: &str = "00000000-0000-4000-8000-0000000000BB";
const RESUME_ID: &str = "00000000-0000-4000-8000-0000000000CC";

/// Records one `(tool, request the port received)` pair per call. When `refuse` is set, every call
/// answers the generic unavailable refusal of a disabled facade.
#[derive(Debug, Default)]
struct Recorder {
    calls: Mutex<Vec<(&'static str, Value)>>,
    refuse: bool,
}

impl Recorder {
    fn record(&self, tool: &'static str, request: &Value) -> Result<Value, AppError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((tool, request.clone()));
        if self.refuse {
            return Err(AppError::from(ApplicationError::with_message(
                ErrorCode::InternalError,
                "Generation is not available",
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
impl GenerationPort for Recorder {
    async fn generate_pdf_file(&self, request: &Value) -> Result<Value, AppError> {
        self.record("generate_pdf_file", request)
    }

    async fn generate_pdf_resume(&self, request: &Value) -> Result<Value, AppError> {
        self.record("generate_pdf_resume", request)
    }

    async fn generate_canadian_pdf_resume(&self, request: &Value) -> Result<Value, AppError> {
        self.record("generate_canadian_pdf_resume", request)
    }

    async fn generate_canadian_french_pdf_resume(
        &self,
        request: &Value,
    ) -> Result<Value, AppError> {
        self.record("generate_canadian_french_pdf_resume", request)
    }

    async fn generate_german_tailored_resume(&self, request: &Value) -> Result<Value, AppError> {
        self.record("generate_german_tailored_resume", request)
    }

    async fn generate_german_cover_letter(&self, request: &Value) -> Result<Value, AppError> {
        self.record("generate_german_cover_letter", request)
    }
}

fn registry(recorder: Arc<Recorder>) -> Result<ToolRegistry, Box<dyn Error>> {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    generation::register(&mut registry, Some(recorder as Arc<dyn GenerationPort>))?;
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

#[test]
fn all_six_tools_are_registered_only_with_a_port() -> TestResult {
    let mut empty = ToolRegistry::new(Catalog::load()?);
    generation::register(&mut empty, None)?;
    for operation in Operation::ALL {
        assert!(!empty.is_registered(operation.name()));
    }
    let registry = registry(Arc::new(Recorder::default()))?;
    for operation in Operation::ALL {
        assert!(
            registry.is_registered(operation.name()),
            "{}",
            operation.name()
        );
    }
    Ok(())
}

#[tokio::test]
async fn the_pdf_file_request_keeps_its_declared_keys_and_nulls() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(Arc::clone(&recorder))?;
    let outcome = call(
        &registry,
        "generate_pdf_file",
        json!({"request": {"fileName": null, "title": "Synthetic", "body": "Text", "extra": 1}}),
    )
    .await?;
    assert!(matches!(outcome, ToolResult::Success(_)), "{outcome:?}");
    assert_eq!(
        recorder.calls(),
        vec![(
            "generate_pdf_file",
            json!({"fileName": null, "title": "Synthetic", "body": "Text"})
        )]
    );
    Ok(())
}

#[tokio::test]
async fn the_profile_resumes_forward_the_profile_identifier() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(Arc::clone(&recorder))?;
    for name in [
        "generate_pdf_resume",
        "generate_canadian_pdf_resume",
        "generate_canadian_french_pdf_resume",
    ] {
        let outcome = call(
            &registry,
            name,
            json!({"request": {"profileId": PROFILE_ID}}),
        )
        .await?;
        assert!(
            matches!(outcome, ToolResult::Success(_)),
            "{name}: {outcome:?}"
        );
    }
    assert_eq!(
        recorder.calls(),
        vec![
            ("generate_pdf_resume", json!({"profileId": PROFILE_ID})),
            (
                "generate_canadian_pdf_resume",
                json!({"profileId": PROFILE_ID})
            ),
            (
                "generate_canadian_french_pdf_resume",
                json!({"profileId": PROFILE_ID})
            ),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn the_german_requests_keep_their_identities_and_the_boolean() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(Arc::clone(&recorder))?;
    let resume = call(
        &registry,
        "generate_german_tailored_resume",
        json!({"request": {"profileId": PROFILE_ID, "jobId": JOB_ID, "includeProjects": true}}),
    )
    .await?;
    assert!(matches!(resume, ToolResult::Success(_)), "{resume:?}");
    let letter = call(
        &registry,
        "generate_german_cover_letter",
        json!({"request": {"profileId": PROFILE_ID, "jobId": JOB_ID, "resumeId": RESUME_ID}}),
    )
    .await?;
    assert!(matches!(letter, ToolResult::Success(_)), "{letter:?}");
    assert_eq!(
        recorder.calls(),
        vec![
            (
                "generate_german_tailored_resume",
                json!({"profileId": PROFILE_ID, "jobId": JOB_ID, "includeProjects": true})
            ),
            (
                "generate_german_cover_letter",
                json!({"profileId": PROFILE_ID, "jobId": JOB_ID, "resumeId": RESUME_ID})
            ),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn non_strict_values_and_missing_keys_never_reach_the_port() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let registry = registry(Arc::clone(&recorder))?;
    let rejected = [
        (
            "generate_pdf_file",
            json!({"request": {"fileName": 7, "title": null, "body": null}}),
        ),
        (
            "generate_pdf_file",
            json!({"request": {"title": null, "body": null}}),
        ),
        ("generate_pdf_resume", json!({"request": {"profileId": 12}})),
        ("generate_pdf_resume", json!({"request": {}})),
        (
            "generate_german_tailored_resume",
            json!({"request": {"profileId": PROFILE_ID, "jobId": JOB_ID, "includeProjects": "yes"}}),
        ),
        (
            "generate_german_tailored_resume",
            json!({"request": {"profileId": PROFILE_ID, "jobId": JOB_ID, "includeProjects": 1}}),
        ),
        (
            "generate_german_cover_letter",
            json!({"request": {"profileId": PROFILE_ID, "jobId": 3, "resumeId": RESUME_ID}}),
        ),
        (
            "generate_german_cover_letter",
            json!({"request": "not an object"}),
        ),
        ("generate_canadian_pdf_resume", json!({})),
    ];
    for (name, arguments) in rejected {
        let outcome = call(&registry, name, arguments).await?;
        assert!(
            matches!(outcome, ToolResult::InvalidArguments(_)),
            "{name}: {outcome:?}"
        );
    }
    assert!(
        recorder.calls().is_empty(),
        "rejected calls must not reach the port"
    );
    Ok(())
}

#[tokio::test]
async fn a_refused_facade_answers_the_default_envelope_text() -> TestResult {
    let recorder = Arc::new(Recorder {
        refuse: true,
        ..Recorder::default()
    });
    let registry = registry(Arc::clone(&recorder))?;
    let outcome = call(
        &registry,
        "generate_pdf_resume",
        json!({"request": {"profileId": PROFILE_ID}}),
    )
    .await?;
    match outcome {
        ToolResult::Failure { envelope, style } => {
            assert_eq!(style, EnvelopeStyle::Default);
            assert_eq!(envelope.message, "Generation is not available");
        }
        other => return Err(format!("expected a failure envelope, got {other:?}").into()),
    }
    Ok(())
}
