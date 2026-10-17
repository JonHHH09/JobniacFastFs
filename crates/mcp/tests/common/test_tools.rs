//! A test-only tool module that answers the tools of the call goldens (`tests/golden/calls-*.json`).
//!
//! Each case configures one service outcome (a return value or a raised error). The handler
//! decodes the arguments with the framework decoders, in the shape of its tool, and then serves
//! the configured outcome through the envelope mapping of that tool. The profile tools are the
//! production handlers of `jobniac-mcp`, over ports that serve the same script; the other tools are test-only scripted handlers. Either way
//! the framework's argument decoding, result shapes and text layouts are checked byte for byte.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jobniac_core::arbeitnow::import::{ImportOutcome, ImportRequest, ImportStatus};
use jobniac_core::arbeitnow::instant::ArbeitnowInstant;
use jobniac_core::arbeitnow::scan::{ScanRequest, ScanResult, ScannedJob};
use jobniac_core::errors::{AppError, ApplicationError, ErrorCode};
use jobniac_core::jobs::JobAggregate;
use jobniac_mcp::args::{
    ArgDecoder, ArgErrors, REQUIRED, REQUIRED_NULLABLE, Slot, dict_arg, strict_string,
};
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::{Arguments, ToolHandler, ToolRegistry};
use jobniac_mcp::result::{EnvelopeStyle, ToolResult};
use jobniac_mcp::tools::arbeitnow::{
    self as arbeitnow_tools, ArbeitnowImportPort, ArbeitnowPorts, ArbeitnowScanPort,
};
use jobniac_mcp::tools::artifacts::{self as artifact_tools, ArtifactPort};
use jobniac_mcp::tools::deletion::{
    self as deletion_tools, DeletionPorts, JobDeletionPort, ProfileDeletionPort,
};
use jobniac_mcp::tools::documents::{self as document_tools, DocumentPort};
use jobniac_mcp::tools::generation::{self as generation_tools, GenerationPort};
use jobniac_mcp::tools::job_links::{self as link_tools, JobLinkPort};
use jobniac_mcp::tools::jobs::{self as job_tools, JobPort, JobSearchPort};
use jobniac_mcp::tools::matching::{self as match_tools, MatchingPort};
use jobniac_mcp::tools::profile_pdf::{self as profile_pdf_tools, ProfilePdfPort};
use jobniac_mcp::tools::profiles::{
    self as profile_tools, ProfileDetailsPort, ProfileListPort, ProfileSearchPort,
};
use jobniac_mcp::tools::reviews::{self as review_tools, MatchReviewPort};
use serde_json::Value;

/// The scripted tools the goldens call, with the envelope style of the module that owns
/// each. The profile, job, matching, review, document and profile PDF tools are registered as
/// production handlers (see [`registry`]).
pub const TOOLS: &[(&str, EnvelopeStyle)] = &[];

/// The service of the scripted tools: serves the outcome that the current case configures.
#[derive(Debug, Default)]
pub struct Script {
    state: Mutex<ScriptState>,
}

#[derive(Debug, Default)]
struct ScriptState {
    configured: Option<Value>,
    calls: Vec<String>,
}

impl Script {
    /// Configure the one service outcome of the next case from the golden's `service` value
    /// (`null` when the SDK rejects the arguments before any service runs).
    pub fn configure(&self, service: &Value) {
        if let Ok(mut state) = self.state.lock() {
            state.configured = if service.is_null() {
                None
            } else {
                Some(service.clone())
            };
            state.calls.clear();
        }
    }

    /// The methods that were called since the last configuration.
    pub fn calls(&self) -> Vec<String> {
        self.state
            .lock()
            .map(|state| state.calls.clone())
            .unwrap_or_default()
    }

    fn serve(&self, method: &str) -> Result<Value, AppError> {
        let Ok(mut state) = self.state.lock() else {
            return Err(AppError::Internal);
        };
        state.calls.push(method.to_owned());
        let Some(configured) = state.configured.as_ref() else {
            return Err(AppError::Internal);
        };
        if configured["method"].as_str() != Some(method) {
            return Err(AppError::Internal);
        }
        match configured["outcome"].as_str() {
            Some("return") => Ok(configured["value"].clone()),
            _ => Err(app_error(&configured["error"])),
        }
    }
}

/// The application error that a golden's error description stands for.
fn app_error(spec: &Value) -> AppError {
    match spec["kind"].as_str() {
        Some("application") => {
            let code = match spec["code"].as_str() {
                Some("VALIDATION_ERROR") => ErrorCode::ValidationError,
                Some("NOT_FOUND") => ErrorCode::NotFound,
                Some("CONFLICT") => ErrorCode::Conflict,
                Some("AUTHORIZATION_ERROR") => ErrorCode::AuthorizationError,
                Some("UPSTREAM_RATE_LIMITED") => ErrorCode::UpstreamRateLimited,
                Some("UPSTREAM_INVALID_RESPONSE") => ErrorCode::UpstreamInvalidResponse,
                Some("UPSTREAM_UNAVAILABLE") => ErrorCode::UpstreamUnavailable,
                _ => ErrorCode::InternalError,
            };
            let details: Vec<(String, String)> = spec["details"]
                .as_object()
                .map(|object| {
                    object
                        .iter()
                        .map(|(key, value)| {
                            (key.clone(), value.as_str().unwrap_or_default().to_owned())
                        })
                        .collect()
                })
                .unwrap_or_default();
            AppError::Application(ApplicationError::new(
                code,
                spec["message"].as_str(),
                details,
            ))
        }
        Some("domain_value") => {
            AppError::DomainValue(spec["message"].as_str().unwrap_or_default().to_owned())
        }
        Some("persistence_constraint") => AppError::PersistenceConstraint {
            sqlstate: spec["sqlstate"].as_str().map(str::to_owned),
            constraint: spec["constraint"].as_str().map(str::to_owned),
        },
        _ => AppError::Internal,
    }
}

/// One scripted tool.
#[derive(Debug)]
pub struct ScriptedTool {
    name: &'static str,
    style: EnvelopeStyle,
    script: Arc<Script>,
}

impl ScriptedTool {
    /// The scripted handler of `name`; its envelope style comes from [`TOOLS`].
    pub fn new(name: &'static str, script: Arc<Script>) -> Self {
        let style = TOOLS
            .iter()
            .find(|(tool, _)| *tool == name)
            .map_or(EnvelopeStyle::Compact, |(_, style)| *style);
        Self {
            name,
            style,
            script,
        }
    }
}

#[async_trait]
impl ToolHandler for ScriptedTool {
    async fn call(&self, arguments: &Arguments) -> ToolResult {
        match decode(self.name, arguments) {
            Err(errors) => ToolResult::from(errors),
            Ok(()) => ToolResult::from_outcome(self.script.serve(self.name), self.style),
        }
    }
}

/// The profile ports of the golden script: every port serves the outcome of the current case,
/// under the method name that the golden configures.
#[derive(Debug)]
pub struct ScriptPort(pub Arc<Script>);

#[async_trait]
impl ProfileListPort for ScriptPort {
    async fn list_profiles(
        &self,
        _limit: Option<i64>,
        _cursor: Option<&str>,
    ) -> Result<Value, AppError> {
        self.0.serve("list_profiles")
    }
}

#[async_trait]
impl ProfileDetailsPort for ScriptPort {
    async fn get_profile(&self, _profile_id: &str) -> Result<Value, AppError> {
        self.0.serve("get_profile")
    }

    async fn create_profile(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("create_profile")
    }

    async fn update_profile(
        &self,
        _profile_id: &str,
        _expected_revision: i64,
        _request: &Value,
    ) -> Result<Value, AppError> {
        self.0.serve("update_profile")
    }

    async fn update_profile_project(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("update_profile_project")
    }
}

#[async_trait]
impl ProfileSearchPort for ScriptPort {
    async fn search_profiles(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("search_profiles")
    }
}

#[async_trait]
impl JobPort for ScriptPort {
    async fn add_job_from_text(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("add_job_from_text")
    }

    async fn list_jobs(
        &self,
        _limit: Option<i64>,
        _cursor: Option<&str>,
    ) -> Result<Value, AppError> {
        self.0.serve("list_jobs")
    }

    async fn get_job(&self, _job_id: &str) -> Result<Value, AppError> {
        self.0.serve("get_job")
    }

    async fn update_job(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("update_job")
    }
}

#[async_trait]
impl JobSearchPort for ScriptPort {
    async fn search_jobs(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("search_jobs")
    }
}

/// Ports of a server whose services are not reachable: they never answer a call. Used where only
/// the tool list matters.
#[derive(Debug)]
pub struct Unavailable;

#[async_trait]
impl ProfileListPort for Unavailable {
    async fn list_profiles(
        &self,
        _limit: Option<i64>,
        _cursor: Option<&str>,
    ) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl ProfileDetailsPort for Unavailable {
    async fn get_profile(&self, _profile_id: &str) -> Result<Value, AppError> {
        Err(AppError::Internal)
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

#[async_trait]
impl ProfileSearchPort for Unavailable {
    async fn search_profiles(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl JobPort for Unavailable {
    async fn add_job_from_text(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn list_jobs(
        &self,
        _limit: Option<i64>,
        _cursor: Option<&str>,
    ) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn get_job(&self, _job_id: &str) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn update_job(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl JobSearchPort for Unavailable {
    async fn search_jobs(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl MatchingPort for ScriptPort {
    async fn analyze_job_match(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("analyze_job_match")
    }

    async fn analyze_all_job_matches(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("analyze_all_job_matches")
    }

    async fn get_match_report(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("get_match_report")
    }

    async fn list_match_reports(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("list_match_reports")
    }
}

#[async_trait]
impl MatchReviewPort for ScriptPort {
    async fn submit_match_review(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("submit_match_review")
    }

    async fn get_match_review(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("get_match_review")
    }

    async fn list_match_reviews(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("list_match_reviews")
    }

    async fn list_match_disagreements(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("list_match_disagreements")
    }

    async fn acknowledge_match_disagreement(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("acknowledge_match_disagreement")
    }

    async fn link_match_disagreement(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("link_match_disagreement")
    }
}

#[async_trait]
impl MatchingPort for Unavailable {
    async fn analyze_job_match(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn analyze_all_job_matches(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn get_match_report(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn list_match_reports(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl MatchReviewPort for Unavailable {
    async fn submit_match_review(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn get_match_review(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn list_match_reviews(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn list_match_disagreements(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn acknowledge_match_disagreement(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn link_match_disagreement(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl DocumentPort for ScriptPort {
    async fn store_document_file(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("store_document_file")
    }

    async fn get_document_metadata(&self, _document_id: &str) -> Result<Value, AppError> {
        self.0.serve("get_document_metadata")
    }

    async fn extract_pdf_text(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("extract_pdf_text")
    }

    async fn extract_stored_pdf_text(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("extract_stored_pdf_text")
    }
}

#[async_trait]
impl GenerationPort for ScriptPort {
    async fn generate_pdf_file(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("generate_pdf_file")
    }

    async fn generate_pdf_resume(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("generate_pdf_resume")
    }

    async fn generate_canadian_pdf_resume(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("generate_canadian_pdf_resume")
    }

    async fn generate_canadian_french_pdf_resume(
        &self,
        _request: &Value,
    ) -> Result<Value, AppError> {
        self.0.serve("generate_canadian_french_pdf_resume")
    }

    async fn generate_german_tailored_resume(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("generate_german_tailored_resume")
    }

    async fn generate_german_cover_letter(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("generate_german_cover_letter")
    }
}

#[async_trait]
impl ProfilePdfPort for ScriptPort {
    async fn ingest_profile_from_stored_pdf(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("ingest_profile_from_stored_pdf")
    }

    async fn get_profile_pdf_source(&self, _profile_id: &str) -> Result<Value, AppError> {
        self.0.serve("get_profile_pdf_source")
    }
}

#[async_trait]
impl DocumentPort for Unavailable {
    async fn store_document_file(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn get_document_metadata(&self, _document_id: &str) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn extract_pdf_text(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn extract_stored_pdf_text(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl GenerationPort for Unavailable {
    async fn generate_pdf_file(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn generate_pdf_resume(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn generate_canadian_pdf_resume(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn generate_canadian_french_pdf_resume(
        &self,
        _request: &Value,
    ) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn generate_german_tailored_resume(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn generate_german_cover_letter(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl ProfilePdfPort for Unavailable {
    async fn ingest_profile_from_stored_pdf(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }

    async fn get_profile_pdf_source(&self, _profile_id: &str) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl ProfileDeletionPort for ScriptPort {
    async fn delete_profile(&self, _profile_id: &str) -> Result<Value, AppError> {
        self.0.serve("delete_profile")
    }
}

#[async_trait]
impl JobDeletionPort for ScriptPort {
    async fn delete_job(&self, _job_id: &str) -> Result<Value, AppError> {
        self.0.serve("delete_job")
    }
}

#[async_trait]
impl JobLinkPort for ScriptPort {
    async fn add_job_from_link(&self, _request: &Value) -> Result<Value, AppError> {
        self.0.serve("add_job_from_link")
    }
}

#[async_trait]
impl ArtifactPort for ScriptPort {
    async fn get_document_artifact(&self, _document_id: &str) -> Result<Value, AppError> {
        self.0.serve("get_document_artifact")
    }
}

/// The application error of a served outcome, for the typed Arbeitnow ports. Any outcome that is
/// not an application error is the unclassified failure of the adapter.
fn application_error_of(error: AppError) -> ApplicationError {
    match error {
        AppError::Application(application) => application,
        _ => internal_error(),
    }
}

/// The unclassified failure of a typed Arbeitnow port: the adapter's `internal_error`.
fn internal_error() -> ApplicationError {
    ApplicationError::new(
        ErrorCode::InternalError,
        None,
        Vec::<(String, String)>::new(),
    )
}

/// A string of the observed golden that the typed Arbeitnow result keeps as a static constant.
fn known_text(text: &str) -> Option<&'static str> {
    const KNOWN: &[&str] = &[
        "arbeitnow",
        "https://www.arbeitnow.com/api/job-board-api",
        "Upstream results are mutable; this scan is not a snapshot.",
    ];
    KNOWN.iter().find(|known| **known == text).copied()
}

fn text_of(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::to_owned)
}

fn texts_of(value: &Value, key: &str) -> Option<Vec<String>> {
    value[key]
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
}

/// One observed scanned job, rebuilt as the service type. `postedAt` is the instant's parts.
fn scanned_job_of(value: &Value) -> Option<ScannedJob> {
    let posted = &value["postedAt"];
    let posted_at = if posted.is_null() {
        None
    } else {
        let second = posted["epochSecond"].as_i64()?;
        let nanosecond = u32::try_from(posted["nanosecond"].as_u64()?).ok()?;
        Some(ArbeitnowInstant::new(second, nanosecond).ok()?)
    };
    Some(ScannedJob {
        slug: text_of(value, "slug")?,
        canonical_url: text_of(value, "canonicalUrl")?,
        company: text_of(value, "company")?,
        title: text_of(value, "title")?,
        location: text_of(value, "location")?,
        remote: value["remote"].as_bool()?,
        posted_at,
        tags: texts_of(value, "tags")?,
        job_types: texts_of(value, "jobTypes")?,
        description_excerpt: text_of(value, "descriptionExcerpt")?,
        candidate_token: text_of(value, "candidateToken")?,
    })
}

/// One observed scan result, rebuilt as the service type.
fn scan_result_of(value: &Value) -> Option<ScanResult> {
    let jobs = value["jobs"]
        .as_array()?
        .iter()
        .map(scanned_job_of)
        .collect::<Option<Vec<_>>>()?;
    Some(ScanResult {
        source: known_text(value["source"].as_str()?)?,
        source_docs_url: known_text(value["sourceDocsUrl"].as_str()?)?,
        jobs,
        pages_scanned: u32::try_from(value["pagesScanned"].as_u64()?).ok()?,
        records_inspected: i32::try_from(value["recordsInspected"].as_i64()?).ok()?,
        next_cursor: text_of(value, "nextCursor"),
        consistency_notice: known_text(value["consistencyNotice"].as_str()?)?,
    })
}

#[async_trait]
impl ArbeitnowScanPort for ScriptPort {
    async fn scan(&self, _request: ScanRequest) -> Result<ScanResult, ApplicationError> {
        let value = self.0.serve("scan").map_err(application_error_of)?;
        scan_result_of(&value).ok_or_else(internal_error)
    }
}

#[async_trait]
impl ArbeitnowImportPort for ScriptPort {
    async fn import(&self, _request: ImportRequest) -> Result<ImportOutcome, ApplicationError> {
        let value = self.0.serve("import_job").map_err(application_error_of)?;
        let status = match value["status"].as_str() {
            Some("created_job") => ImportStatus::CreatedJob,
            Some("reused_existing_job") => ImportStatus::ReusedExistingJob,
            _ => return Err(internal_error()),
        };
        let job: JobAggregate =
            serde_json::from_value(value["job"].clone()).map_err(|_| internal_error())?;
        Ok(ImportOutcome { status, job })
    }
}

#[async_trait]
impl ProfileDeletionPort for Unavailable {
    async fn delete_profile(&self, _profile_id: &str) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl JobDeletionPort for Unavailable {
    async fn delete_job(&self, _job_id: &str) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl JobLinkPort for Unavailable {
    async fn add_job_from_link(&self, _request: &Value) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl ArtifactPort for Unavailable {
    async fn get_document_artifact(&self, _document_id: &str) -> Result<Value, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl ArbeitnowScanPort for Unavailable {
    async fn scan(&self, _request: ScanRequest) -> Result<ScanResult, ApplicationError> {
        Err(internal_error())
    }
}

#[async_trait]
impl ArbeitnowImportPort for Unavailable {
    async fn import(&self, _request: ImportRequest) -> Result<ImportOutcome, ApplicationError> {
        Err(internal_error())
    }
}

/// A registry with every scripted tool and the production profile tools, over the bundled catalog.
pub fn registry(script: &Arc<Script>) -> Result<ToolRegistry, Box<dyn std::error::Error>> {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    for &(name, _) in TOOLS {
        registry.register(name, ScriptedTool::new(name, Arc::clone(script)))?;
    }
    let port = Arc::new(ScriptPort(Arc::clone(script)));
    profile_tools::register(
        &mut registry,
        Some(port.clone()),
        Some(port.clone()),
        Some(port.clone()),
    )?;
    job_tools::register(&mut registry, Some(port.clone()), Some(port.clone()))?;
    match_tools::register(&mut registry, Some(port.clone()))?;
    review_tools::register(&mut registry, Some(port.clone()))?;
    document_tools::register(&mut registry, Some(port.clone()))?;
    profile_pdf_tools::register(&mut registry, Some(port.clone()))?;
    generation_tools::register(&mut registry, Some(port.clone()))?;
    deletion_tools::register(
        &mut registry,
        DeletionPorts {
            profile: Some(port.clone()),
            job: Some(port.clone()),
        },
    )?;
    link_tools::register(&mut registry, Some(port.clone()))?;
    artifact_tools::register(&mut registry, Some(port.clone()))?;
    arbeitnow_tools::register(
        &mut registry,
        ArbeitnowPorts {
            scan: Some(port.clone()),
            import: Some(port),
        },
    )?;
    Ok(registry)
}

/// Decode the arguments of `tool` in the shape of its signature.
fn decode(tool: &'static str, arguments: &Arguments) -> Result<(), ArgErrors> {
    let mut decoder = ArgDecoder::new(tool);
    if tool == "generate_pdf_resume"
        && let Slot::Value(request) = decoder.field(arguments, "", "request", REQUIRED, dict_arg)
    {
        decoder.field(
            request,
            "request",
            "profileId",
            REQUIRED_NULLABLE,
            strict_string,
        );
    }
    decoder.check()
}
