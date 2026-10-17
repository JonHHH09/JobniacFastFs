//! The MCP server: `ServerHandler` over the tool registry.
//!
//! Tools are dispatched through [`ToolRegistry`] (no `#[tool]` macros), so the tool list is the catalog data filtered by the registered handlers.

use std::borrow::Cow;
use std::sync::Arc;

use jobniac_core::health::HealthService;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, InitializeResult,
    ListPromptsResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProtocolVersion, Tool,
};
use rmcp::service::{RequestContext, ServerInitializeError};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, DuplexStream};

use crate::registry::{RegistryError, ToolRegistry};
use crate::stdin_admission::{REJECTED_INPUT_KEY, Rejection, admit_changed};
use crate::tools;
use crate::tools::arbeitnow::ArbeitnowPorts;
use crate::tools::artifacts::ArtifactPort;
use crate::tools::deletion::DeletionPorts;
use crate::tools::documents::DocumentPort;
use crate::tools::generation::GenerationPort;
use crate::tools::job_links::JobLinkPort;
use crate::tools::jobs::{JobPort, JobSearchPort};
use crate::tools::matching::MatchingPort;
use crate::tools::profile_pdf::ProfilePdfPort;
use crate::tools::profiles::{ProfileDetailsPort, ProfileListPort, ProfileSearchPort};
use crate::tools::reviews::MatchReviewPort;

/// The application services the MCP tools call.
///
/// Each service is an `Option<Arc<...>>` field; a `None` service leaves its tools unregistered.
#[derive(Debug, Clone)]
pub struct McpServices {
    /// Shared health use case behind the `health` tool.
    pub health: Arc<HealthService>,
    /// The keyset listing behind `list_profiles`; `None` leaves the tool unregistered.
    pub profile_list: Option<Arc<dyn ProfileListPort>>,
    /// The profile aggregate use cases behind `get_profile`, `create_profile`, `update_profile`
    /// and `update_profile_project`; `None` leaves all four unregistered.
    pub profile_details: Option<Arc<dyn ProfileDetailsPort>>,
    /// The bounded search behind `search_profiles`; `None` leaves the tool unregistered.
    pub profile_search: Option<Arc<dyn ProfileSearchPort>>,
    /// The job capture, get, keyset list and revision-checked update use cases behind
    /// `add_job_from_text`, `get_job`, `list_jobs` and `update_job`; `None` leaves all four
    /// unregistered.
    pub job_service: Option<Arc<dyn JobPort>>,
    /// The bounded job search behind `search_jobs`; `None` leaves the tool unregistered.
    pub job_search: Option<Arc<dyn JobSearchPort>>,
    /// The deterministic matching use cases behind `analyze_job_match`, `analyze_all_job_matches`,
    /// `get_match_report` and `list_match_reports`; `None` leaves all four unregistered.
    pub matching_service: Option<Arc<dyn MatchingPort>>,
    /// The advisory review and disagreement use cases behind `submit_match_review`,
    /// `get_match_review`, `list_match_reviews`, `list_match_disagreements`,
    /// `acknowledge_match_disagreement` and `link_match_disagreement`; `None` leaves all six
    /// unregistered.
    pub review_service: Option<Arc<dyn MatchReviewPort>>,
    /// The document use cases behind `store_document_file`, `get_document_metadata`,
    /// `extract_pdf_text` and `extract_stored_pdf_text`; `None` leaves all four unregistered.
    pub document_service: Option<Arc<dyn DocumentPort>>,
    /// The stored-PDF ingestion and PDF source use cases behind `ingest_profile_from_stored_pdf`
    /// and `get_profile_pdf_source`; `None` leaves both unregistered.
    pub profile_pdf_service: Option<Arc<dyn ProfilePdfPort>>,
    /// The generation use cases behind the six generation tools; `None` leaves all six
    /// unregistered. The facade itself refuses while generation is not enabled.
    pub generation_service: Option<Arc<dyn GenerationPort>>,
    /// The verified artifact read behind `get_document_artifact`. `None` leaves the tool unregistered:
    /// it is opt-in and composed only when `JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED` is exactly `1`.
    pub document_artifact_service: Option<Arc<dyn ArtifactPort>>,
    /// The opt-in deletion use cases behind `delete_profile` and `delete_job`. Each absent port
    /// leaves its tool unregistered; the default is both absent.
    pub deletion: DeletionPorts,
    /// The opt-in link ingestion use case behind `add_job_from_link`. `None` leaves the tool
    /// unregistered; it is composed only when `JOBNIAC_LINK_FETCH_ENABLED` is exactly `1`, the
    /// platform is supported and a database is configured.
    pub job_link_service: Option<Arc<dyn JobLinkPort>>,
    /// The opt-in Arbeitnow scan and import use cases behind `scan_arbeitnow_jobs` and
    /// `import_arbeitnow_job`. Each absent port leaves its tool unregistered; they are composed only
    /// when `JOBNIAC_ARBEITNOW_ENABLED` is exactly `1`.
    pub arbeitnow: ArbeitnowPorts,
}

/// Failure while building or serving the MCP server.
#[derive(Debug, thiserror::Error)]
pub enum McpServeError {
    /// The bundled tool or initialize catalog is not valid JSON for the wire types.
    #[error("the MCP catalog is invalid: {0}")]
    Catalog(#[from] serde_json::Error),
    /// A tool handler could not be registered against the catalog.
    #[error("the MCP tool registry is invalid: {0}")]
    Registry(#[from] RegistryError),
    /// The client's `initialize` handshake failed (boxed to keep the error type small).
    #[error("MCP initialization failed: {0}")]
    Initialize(Box<rmcp::service::ServerInitializeError>),
    /// The background session task panicked or was aborted.
    #[error("MCP session task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// The Jobniac MCP server. Holds the tool registry, so the catalog is parsed and validated once.
#[derive(Debug, Clone)]
pub struct JobniacMcpServer {
    registry: Arc<ToolRegistry>,
}

impl JobniacMcpServer {
    /// Build the server from its services: the default tool handlers over the bundled catalog.
    pub fn new(services: McpServices) -> Result<Self, McpServeError> {
        Ok(Self::with_registry(tools::default_registry(&services)?))
    }

    /// Build the server over an already composed registry (tests compose their own tool modules).
    pub fn with_registry(registry: ToolRegistry) -> Self {
        Self {
            registry: Arc::new(registry),
        }
    }

    fn catalog_tools(&self) -> Vec<Tool> {
        self.registry.listed_tools()
    }
}

impl ServerHandler for JobniacMcpServer {
    /// The `initialize` result from `catalog/initialize.json`, which mirrors the golden
    /// `initialize.json`: `serverInfo` (name, title, empty version, description) and the
    /// capabilities the SDK advertises, including the prompts and resources entries.
    fn get_info(&self) -> InitializeResult {
        self.registry.catalog().initialize().clone()
    }

    /// The protocol revisions this server implements (up to 2025-11-25).
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::known_up_to(&ProtocolVersion::V_2025_11_25))
    }

    /// The SDK answers `prompts/list` and `resources/list` with empty lists, and so does
    /// this server. `prompts/get` and `resources/read` keep rmcp's `method not found` default.
    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult::default())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult::default())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: self.catalog_tools(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.as_ref();
        let arguments = request.arguments.unwrap_or_default();
        // Input that stdin admission could not carry is answered with the argument rejection of
        // its tool, and the tool does not run (see `stdin_admission`).
        let rejection = context
            .meta
            .get(REJECTED_INPUT_KEY)
            .and_then(Value::as_str)
            .and_then(Rejection::from_meta_value);
        let outcome = match rejection {
            Some(rejection) => self.registry.reject_input(name, rejection.issue_kind()),
            None => self.registry.call(name, &arguments).await,
        };
        let result = match outcome {
            Some(outcome) => outcome.into_call_result(),
            None => {
                CallToolResult::error(vec![ContentBlock::text(format!("Unknown tool: {name}"))])
            }
        };
        Ok(result.into())
    }
}

/// Serve the MCP server over this process's STDIO. Nothing else may write to stdout.
///
/// A stdin that closes before `initialize` ends the session without an error.
pub async fn serve_stdio(services: McpServices) -> Result<(), McpServeError> {
    let server = JobniacMcpServer::new(services)?;
    serve_with(server, tokio::io::stdin(), tokio::io::stdout()).await
}

/// Capacity of the pipe between stdin admission and the rmcp transport.
const ADMITTED_PIPE_CAPACITY: usize = 64 * 1024;

/// Serve the MCP server over `input` and `output`, the lines of `input` passing through stdin
/// admission first. [`serve_stdio`] calls this with the process's STDIO.
///
/// Each line is read whatever its length and repaired by [`admit_changed`] before rmcp parses it, so
/// a line that rmcp would silently drop still gets an answer. A line that is not UTF-8 is passed
/// on byte for byte, as before admission existed. The pump ends at the end of `input`, which ends
/// the session.
pub async fn serve_with<R, W>(
    server: JobniacMcpServer,
    input: R,
    output: W,
) -> Result<(), McpServeError>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let (admitted_reader, mut admitted_writer) = tokio::io::duplex(ADMITTED_PIPE_CAPACITY);
    tokio::spawn(async move {
        let mut input = BufReader::new(input);
        let mut line = Vec::new();
        loop {
            line.clear();
            match input.read_until(b'\n', &mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let body = strip_terminator(&line);
            let written = match std::str::from_utf8(body) {
                Ok(text) => match admit_changed(text) {
                    Some(admitted) => {
                        write_line(&mut admitted_writer, admitted.text.as_bytes()).await
                    }
                    None => write_line(&mut admitted_writer, body).await,
                },
                Err(_) => write_line(&mut admitted_writer, body).await,
            };
            if written.is_err() {
                break;
            }
        }
    });
    let running = match server.serve((admitted_reader, output)).await {
        Ok(running) => running,
        Err(ServerInitializeError::ConnectionClosed(_)) => return Ok(()),
        Err(error) => return Err(McpServeError::Initialize(Box::new(error))),
    };
    running.waiting().await?;
    Ok(())
}

/// The line without its `\n` or `\r\n` terminator.
fn strip_terminator(line: &[u8]) -> &[u8] {
    let without_lf = line.strip_suffix(b"\n").unwrap_or(line);
    without_lf.strip_suffix(b"\r").unwrap_or(without_lf)
}

/// Write one line and its `\n` terminator into the pipe.
async fn write_line(writer: &mut DuplexStream, body: &[u8]) -> std::io::Result<()> {
    writer.write_all(body).await?;
    writer.write_all(b"\n").await
}
