//! The opt-in `delete_profile` and `delete_job` tools.
//! Registration, the schemas and annotations, the identifier as sent, the envelopes and the
//! sanitized unexpected error. All data is synthetic.

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::errors::{AppError, ErrorCode, conflict, not_found};
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::{Arguments, ToolRegistry};
use jobniac_mcp::result::{EnvelopeStyle, ToolResult};
use jobniac_mcp::tools::deletion::{self, DeletionPorts, JobDeletionPort, ProfileDeletionPort};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

const ID: &str = "3f2b8c1e-6a4d-4e7b-9c10-2d5e8f9a0b1c";

/// Records each call and answers with a fixed outcome.
#[derive(Debug, Default)]
struct Fake {
    calls: Mutex<Vec<(&'static str, String)>>,
    deleted: bool,
    error: Mutex<Option<AppError>>,
}

impl Fake {
    fn answering(deleted: bool) -> Arc<Self> {
        Arc::new(Self {
            deleted,
            ..Self::default()
        })
    }

    fn failing(error: AppError) -> Arc<Self> {
        Arc::new(Self {
            deleted: true,
            calls: Mutex::new(Vec::new()),
            error: Mutex::new(Some(error)),
        })
    }

    fn calls(&self) -> Vec<(&'static str, String)> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn respond(&self, method: &'static str, id: &str) -> Result<Value, AppError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((method, id.to_owned()));
        if let Some(error) = self
            .error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return Err(error);
        }
        // The service answers with the canonical lowercase form of the identifier.
        Ok(json!({"profileId": id.to_lowercase(), "deleted": self.deleted}))
    }
}

#[async_trait]
impl ProfileDeletionPort for Fake {
    async fn delete_profile(&self, profile_id: &str) -> Result<Value, AppError> {
        self.respond("delete_profile", profile_id)
    }
}

#[async_trait]
impl JobDeletionPort for Fake {
    async fn delete_job(&self, job_id: &str) -> Result<Value, AppError> {
        self.respond("delete_job", job_id)
    }
}

/// A registry with the deletion tools of the given ports, over the bundled catalog.
fn registry(
    profile: Option<&Arc<Fake>>,
    job: Option<&Arc<Fake>>,
) -> Result<ToolRegistry, Box<dyn Error>> {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    let ports = DeletionPorts {
        profile: profile.map(|fake| Arc::clone(fake) as Arc<dyn ProfileDeletionPort>),
        job: job.map(|fake| Arc::clone(fake) as Arc<dyn JobDeletionPort>),
    };
    deletion::register(&mut registry, ports)?;
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

/// The names of the tools the registry lists.
fn listed(registry: &ToolRegistry) -> Vec<String> {
    registry
        .listed_tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect()
}

#[test]
fn tools_are_absent_without_ports_and_registered_independently() -> TestResult {
    let fake = Fake::answering(true);
    assert!(listed(&registry(None, None)?).is_empty());
    assert_eq!(
        listed(&registry(Some(&fake), None)?),
        vec!["delete_profile"]
    );
    assert_eq!(listed(&registry(None, Some(&fake))?), vec!["delete_job"]);
    Ok(())
}

#[test]
fn schemas_use_bare_required_ids_and_destructive_annotations() -> TestResult {
    let fake = Fake::answering(true);
    let registry = registry(Some(&fake), Some(&fake))?;
    let tools: Vec<Value> = registry
        .listed_tools()
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    let profile = tools
        .iter()
        .find(|tool| tool["name"] == "delete_profile")
        .ok_or("no delete_profile")?;
    let job = tools
        .iter()
        .find(|tool| tool["name"] == "delete_job")
        .ok_or("no delete_job")?;
    assert_eq!(
        profile["description"],
        "Delete a profile aggregate by profile UUID. Child profile data is removed by database cascade."
    );
    assert_eq!(profile["inputSchema"]["required"], json!(["profileId"]));
    assert_eq!(
        profile["inputSchema"]["properties"]
            .as_object()
            .map(|p| p.len()),
        Some(1)
    );
    assert_eq!(job["description"], "Hard delete a stored job by UUID.");
    assert_eq!(job["inputSchema"]["required"], json!(["jobId"]));
    assert_eq!(
        job["inputSchema"]["properties"]
            .as_object()
            .map(|p| p.len()),
        Some(1)
    );
    for tool in [profile, job] {
        let annotations = &tool["annotations"];
        assert_eq!(annotations["readOnlyHint"], false);
        assert_eq!(annotations["destructiveHint"], true);
        assert_eq!(annotations["openWorldHint"], false);
    }
    assert_eq!(profile["annotations"]["idempotentHint"], true);
    assert_eq!(job["annotations"]["idempotentHint"], false);
    Ok(())
}

#[tokio::test]
async fn delete_profile_reports_deleted_and_missing_without_error() -> TestResult {
    let fake = Fake::answering(true);
    let registry = registry(Some(&fake), None)?;
    let upper = ID.to_uppercase();
    let outcome = call(&registry, "delete_profile", json!({ "profileId": upper })).await?;
    assert!(
        matches!(outcome, ToolResult::Success(ref value) if value == &json!({"profileId": ID, "deleted": true}))
    );
    assert_eq!(fake.calls(), vec![("delete_profile", upper)]);

    let missing = Fake::answering(false);
    let registry = self::registry(Some(&missing), None)?;
    let outcome = call(&registry, "delete_profile", json!({ "profileId": ID })).await?;
    assert!(
        matches!(outcome, ToolResult::Success(ref value) if value == &json!({"profileId": ID, "deleted": false}))
    );
    Ok(())
}

#[tokio::test]
async fn delete_job_passes_the_identifier_as_sent() -> TestResult {
    let fake = Fake::answering(true);
    let registry = registry(None, Some(&fake))?;
    let outcome = call(&registry, "delete_job", json!({ "jobId": ID })).await?;
    assert!(matches!(outcome, ToolResult::Success(_)));
    assert_eq!(fake.calls(), vec![("delete_job", ID.to_owned())]);
    Ok(())
}

#[tokio::test]
async fn missing_job_is_a_not_found_envelope_in_the_compact_style() -> TestResult {
    let fake = Fake::failing(
        not_found(
            &format!("Job not found: {ID}"),
            [("resource", "job"), ("jobId", ID)],
        )
        .into(),
    );
    let registry = registry(None, Some(&fake))?;
    let outcome = call(&registry, "delete_job", json!({ "jobId": ID })).await?;
    let ToolResult::Failure { envelope, style } = outcome else {
        return Err("a not_found must be a tool failure".into());
    };
    assert_eq!(style, EnvelopeStyle::Compact);
    assert_eq!(envelope.code, ErrorCode::NotFound);
    assert_eq!(envelope.message, format!("Job not found: {ID}"));
    assert_eq!(
        envelope.details.get("resource").map(String::as_str),
        Some("job")
    );
    assert_eq!(envelope.details.get("jobId").map(String::as_str), Some(ID));
    Ok(())
}

#[tokio::test]
async fn invalid_ids_return_validation_envelopes_from_the_service() -> TestResult {
    // The service admits the identifier; the tool passes any string through.
    let fake = Fake::failing(
        jobniac_core::errors::validation_error(
            "Invalid profile request",
            Some("profileId"),
            Some("must be a valid UUID"),
        )
        .into(),
    );
    let registry = registry(Some(&fake), None)?;
    for value in ["", "   ", "not-a-uuid"] {
        let outcome = call(&registry, "delete_profile", json!({ "profileId": value })).await?;
        let ToolResult::Failure { envelope, .. } = outcome else {
            return Err("a validation refusal must be a tool failure".into());
        };
        assert_eq!(envelope.code, ErrorCode::ValidationError);
        assert_eq!(envelope.message, "Invalid profile request");
        assert_eq!(
            envelope.details.get("field").map(String::as_str),
            Some("profileId")
        );
        assert_eq!(
            envelope.details.get("reason").map(String::as_str),
            Some("must be a valid UUID")
        );
    }
    Ok(())
}

#[tokio::test]
async fn missing_argument_is_rejected_before_the_port() -> TestResult {
    let fake = Fake::answering(true);
    let registry = registry(None, Some(&fake))?;
    let outcome = call(&registry, "delete_job", json!({})).await?;
    assert!(matches!(outcome, ToolResult::InvalidArguments(_)));
    assert!(fake.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn pending_generation_conflict_is_passed_through() -> TestResult {
    let fake = Fake::failing(
        conflict(
            "Profile has a pending generation",
            [("reason", "generation_pending")],
        )
        .into(),
    );
    let registry = registry(Some(&fake), None)?;
    let outcome = call(&registry, "delete_profile", json!({ "profileId": ID })).await?;
    let ToolResult::Failure { envelope, .. } = outcome else {
        return Err("a conflict must be a tool failure".into());
    };
    assert_eq!(envelope.code, ErrorCode::Conflict);
    assert_eq!(envelope.message, "Profile has a pending generation");
    assert_eq!(
        envelope.details.get("reason").map(String::as_str),
        Some("generation_pending")
    );
    Ok(())
}

#[tokio::test]
async fn unexpected_errors_are_sanitized() -> TestResult {
    let profile = Fake::failing(AppError::Internal);
    let job = Fake::failing(AppError::Internal);
    let registry = registry(Some(&profile), Some(&job))?;
    for (name, argument) in [("delete_profile", "profileId"), ("delete_job", "jobId")] {
        let outcome = call(&registry, name, json!({ argument: ID })).await?;
        let ToolResult::Failure { envelope, .. } = outcome else {
            return Err("an unexpected error must be a tool failure".into());
        };
        assert_eq!(envelope.code, ErrorCode::InternalError);
        assert!(envelope.details.is_empty());
        assert!(!envelope.message.contains("secret"));
    }
    Ok(())
}
