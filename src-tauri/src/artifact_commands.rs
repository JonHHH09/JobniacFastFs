//! Artifact command adapters.
//!
//! Each command takes the single-flight slot of the artifacts workspace, calls the core use case or the
//! page renderer, and maps the result to a DTO. The stored PDF is referenced by its canonical ID only.
//! The viewer holds the verified artifact and the page count in state ([`ArtifactSession`]); page turns
//! render from the held bytes. The page image is the artifact viewer's own resident raster of the
//! `jobniac-page` scheme (the `artifact/` path), separate from the documents viewer's raster.
//!
//! The export destination is chosen in a native save dialog that runs in Rust ([`PdfSavePicker`]); the
//! path is checked as absolute and handed to the no-replace exporter, and never crosses IPC. The
//! command bodies are generic over [`ArtifactPorts`] so the tests answer with fake ports and a fake
//! dialog, as the documents commands do.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jobniac_core::artifacts::model::windows_basename;
use jobniac_core::artifacts::{
    ArtifactExporter, ArtifactPageRenderer, DocumentArtifact, ExportFailure,
};
use jobniac_core::documents::RenderedPdfPage;
use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::ids::admit_uuid;
use jobniac_runtime::artifacts::ArtifactServices;
use jobniac_runtime::services::Services;
use jobniac_text::str_rules::strip_text;
use serde_json::Value;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

use crate::artifact_dto::{
    ArtifactCapabilitiesDto, ArtifactExportDto, ArtifactPreviewStateDto, artifact_preview_state,
};
use crate::document_commands::is_local_absolute;
use crate::error::CommandError;
use crate::job_commands::NOT_CONFIGURED;
use crate::match_commands::{parse_request, text_field};
use crate::page_image::{PageImageRefDto, PageViewer, page_image_ref};
use crate::state::{AppState, ArtifactSession};

/// Message id of a request whose stored document ID is malformed, or a request without one.
pub const ARTIFACT_INPUT: &str = "artifacts.input.id";
/// Message id of every preview, page or load failure (the copy of the failed viewer).
pub const ARTIFACT_PREVIEW_FAILED: &str = "artifacts.preview.failed";
/// Message id of an export refused before publication: no file was created.
pub const ARTIFACT_EXPORT_FAILED: &str = "artifacts.export.failed";
/// Message id of an export whose completion could not be verified.
pub const ARTIFACT_EXPORT_UNCERTAIN: &str = "artifacts.export.uncertain";
/// The title of the native save dialog. The operating system draws
/// the dialog, so its copy is kept in Rust rather than in the React catalogue.
pub const SAVE_DIALOG_TITLE: &str = "Save PDF to a new file";
/// The filter name of the native save dialog (the filter reads `PDF documents (*.pdf)`).
pub const SAVE_FILTER_NAME: &str = "PDF documents";
/// The only extension a published export carries, and the default suffix of the dialog.
const PDF_EXTENSION: &str = "pdf";
/// The name suggested when the stored file has no usable base name.
const DEFAULT_EXPORT_NAME: &str = "document.pdf";
/// The longest file name that may be returned to React, in UTF-8 bytes.
const MAX_EXPORT_NAME_BYTES: usize = 4096;

/// The read and render operations the artifact commands need. The composed adapter calls the artifact
/// use case and the page renderer over the held bytes; the tests answer with fakes.
pub trait ArtifactPorts: Send + Sync {
    /// The verified artifact of a stored document (`read_artifact`: enabled flag, size, magic, digest).
    fn read(
        &self,
        document_id: Uuid,
    ) -> impl Future<Output = Result<DocumentArtifact, AppError>> + Send;

    /// Renders one zero-based page from an artifact the viewer already verified (the bytes it holds,
    /// never a second read of the stored row).
    fn render(
        &self,
        artifact: &DocumentArtifact,
        page_index: u32,
    ) -> impl Future<Output = Result<RenderedPdfPage, AppError>> + Send;

    /// The no-replace exporter of verified artifacts.
    fn exporter(&self) -> Arc<dyn ArtifactExporter>;
}

/// The composed artifact ports: the artifact use case and exporter of the runtime, and the page
/// renderer over the verified bytes (the same PDF worker as the documents viewer).
pub struct ComposedArtifactPorts<'a> {
    artifacts: &'a ArtifactServices,
    pages: ArtifactPageRenderer,
}

impl<'a> ComposedArtifactPorts<'a> {
    /// The ports of the composed services, or `None` when the artifact services are not composed.
    #[must_use]
    pub fn from_services(services: &'a Services) -> Option<Self> {
        services.artifacts.as_ref().map(|artifacts| Self {
            artifacts,
            pages: ArtifactPageRenderer::new(services.documents.renderer.clone()),
        })
    }
}

impl ArtifactPorts for ComposedArtifactPorts<'_> {
    async fn read(&self, document_id: Uuid) -> Result<DocumentArtifact, AppError> {
        self.artifacts
            .service
            .read_artifact(&document_id.to_string())
            .await
    }

    async fn render(
        &self,
        artifact: &DocumentArtifact,
        page_index: u32,
    ) -> Result<RenderedPdfPage, AppError> {
        self.pages.render_page(artifact, page_index).await
    }

    fn exporter(&self) -> Arc<dyn ArtifactExporter> {
        let exporter: Arc<dyn ArtifactExporter> = self.artifacts.exporter.clone();
        exporter
    }
}

/// The native save dialog behind a seam, so the command tests can answer for the user.
pub trait PdfSavePicker: Send + Sync {
    /// Shows the save dialog suggesting `suggested_name`. `None` when the user cancels. The call blocks
    /// until the dialog closes, so the caller runs it off the runtime.
    fn pick_save(&self, suggested_name: &str) -> Option<PathBuf>;
}

/// The operating-system save dialog of the desktop shell. It is called from Rust only, so the webview
/// needs no dialog permission.
#[derive(Clone)]
pub struct NativePdfSavePicker {
    app: AppHandle,
}

impl NativePdfSavePicker {
    /// A picker that shows its dialog for `app`.
    #[must_use]
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl PdfSavePicker for NativePdfSavePicker {
    fn pick_save(&self, suggested_name: &str) -> Option<PathBuf> {
        self.app
            .dialog()
            .file()
            .set_title(SAVE_DIALOG_TITLE)
            .add_filter(SAVE_FILTER_NAME, &[PDF_EXTENSION])
            .set_file_name(suggested_name)
            .blocking_save_file()
            .and_then(|file| file.into_path().ok())
    }
}

/// Reports whether the artifact viewer and the native export are admitted. Read-only: it takes no slot, so the screen can ask on mount.
#[tauri::command]
#[specta::specta]
pub async fn artifact_capabilities_get(
    state: State<'_, AppState>,
) -> Result<ArtifactCapabilitiesDto, CommandError> {
    let services = state.services()?;
    // The runtime composes both under the exact flag `1` and a database, so one answer serves both.
    let composed = services.artifacts.is_some();
    Ok(ArtifactCapabilitiesDto {
        artifacts: composed,
        export: composed,
    })
}

/// Loads a stored PDF and shows its first page.
#[tauri::command]
#[specta::specta]
pub async fn artifact_preview_document(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<ArtifactPreviewStateDto, CommandError> {
    let services = state.services()?;
    let ports = ComposedArtifactPorts::from_services(services);
    run_preview(&state, ports.as_ref(), &request_json).await
}

/// Shows one page of the loaded PDF.
#[tauri::command]
#[specta::specta]
pub async fn artifact_show_page(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<PageImageRefDto, CommandError> {
    let services = state.services()?;
    let ports = ComposedArtifactPorts::from_services(services);
    run_show_page(&state, ports.as_ref(), &request_json).await
}

/// Publishes a stored PDF to a new file the user picks in the native save dialog.
/// `null` when the dialog is cancelled; nothing else changes then.
#[tauri::command]
#[specta::specta]
pub async fn artifact_export_document(
    app: AppHandle,
    state: State<'_, AppState>,
    request_json: String,
) -> Result<Option<ArtifactExportDto>, CommandError> {
    let services = state.services()?;
    let ports = ComposedArtifactPorts::from_services(services);
    let picker: Arc<dyn PdfSavePicker> = Arc::new(NativePdfSavePicker::new(app));
    run_export(&state, ports.as_ref(), &request_json, picker).await
}

/// The preview command body: page 0 of the stored PDF replaces the viewer.
pub(crate) async fn run_preview<P: ArtifactPorts>(
    state: &AppState,
    ports: Option<&P>,
    request_json: &str,
) -> Result<ArtifactPreviewStateDto, CommandError> {
    let request = parse_request(request_json, ARTIFACT_INPUT)?;
    let document_id = document_id_field(&request)?;
    let ports = ports.ok_or_else(not_configured)?;
    let _guard = state.begin_artifact()?;
    // A new preview clears the viewer before it loads, so a failed load never leaves a stale page.
    state.clear_artifact_view();
    let (artifact, rendered) = load_page(state, ports, document_id, 0).await?;
    let page_count = rendered.page_count;
    let document = artifact.document.clone();
    let page = install(state, artifact, 0, rendered)?;
    Ok(artifact_preview_state(&document, page_count, page))
}

/// The page-turn command body: one page of the loaded PDF replaces the raster. The request names the
/// document of the viewer that turns the page; a session that holds another document refuses it.
pub(crate) async fn run_show_page<P: ArtifactPorts>(
    state: &AppState,
    ports: Option<&P>,
    request_json: &str,
) -> Result<PageImageRefDto, CommandError> {
    let request = parse_request(request_json, ARTIFACT_PREVIEW_FAILED)?;
    let page_index = page_index_field(&request)?;
    // The session is global but two viewers (the Artifacts screen and the generation preview) can
    // hold different documents: a page turn names the document it was made for.
    let document_id = document_id_field(&request)
        .map_err(|_| CommandError::new("validation_error", ARTIFACT_PREVIEW_FAILED))?;
    let ports = ports.ok_or_else(not_configured)?;
    let _guard = state.begin_artifact()?;
    let Some(session) = state.artifact_session() else {
        return Err(CommandError::new(
            "validation_error",
            ARTIFACT_PREVIEW_FAILED,
        ));
    };
    // Another viewer replaced the session: refuse without touching it, so that viewer keeps its page.
    if session.artifact.document.id.to_string() != document_id.to_string() {
        return Err(CommandError::new(
            "validation_error",
            ARTIFACT_PREVIEW_FAILED,
        ));
    }
    // An index the loaded document does not have is refused; the viewer keeps its page.
    if page_index >= session.page_count {
        return Err(CommandError::new(
            "validation_error",
            ARTIFACT_PREVIEW_FAILED,
        ));
    }
    // The page is rendered from the bytes verified at the preview: the stored row is not read again.
    let rendered = turn_page(state, ports, &session.artifact, page_index).await?;
    install(state, session.artifact, page_index, rendered)
}

/// The export command body: the verified artifact is published to the file the dialog names.
pub(crate) async fn run_export<P: ArtifactPorts>(
    state: &AppState,
    ports: Option<&P>,
    request_json: &str,
    picker: Arc<dyn PdfSavePicker>,
) -> Result<Option<ArtifactExportDto>, CommandError> {
    let request = parse_request(request_json, ARTIFACT_INPUT)?;
    let document_id = document_id_field(&request)?;
    let ports = ports.ok_or_else(not_configured)?;
    let _guard = state.begin_artifact()?;
    // The artifact is verified before the dialog opens, so a refused document never prompts.
    let artifact = ports
        .read(document_id)
        .await
        .map_err(|error| export_read_error(&error))?;
    let exporter = ports.exporter();
    let suggested = suggested_name(&artifact.document.original_file_name);
    // The dialog and the publication are blocking IO; both run off the runtime.
    let exported = tokio::task::spawn_blocking(move || {
        export_blocking(picker.as_ref(), exporter.as_ref(), &artifact, &suggested)
    })
    .await
    .map_err(|_| export_uncertain())?;
    Ok(exported?.map(|exported_name| ArtifactExportDto { exported_name }))
}

/// Shows the save dialog, checks the destination and publishes the artifact. `Ok(None)` on a cancel.
fn export_blocking(
    picker: &dyn PdfSavePicker,
    exporter: &dyn ArtifactExporter,
    artifact: &DocumentArtifact,
    suggested: &str,
) -> Result<Option<String>, CommandError> {
    let Some(path) = picker.pick_save(suggested) else {
        return Ok(None);
    };
    let destination = destination_text(&path).ok_or_else(export_refused)?;
    let name = exporter
        .export(artifact, destination)
        .map_err(|failure| match failure {
            ExportFailure::Refused => export_refused(),
            ExportFailure::Uncertain => export_uncertain(),
        })?;
    if !is_safe_export_name(&name) {
        return Err(export_uncertain());
    }
    Ok(Some(name))
}

/// Reads and renders one page of a stored PDF. The render uses the artifact that the read verified, so
/// the metadata and the page come from one read. Any failure is the viewer's failure copy, and it clears
/// the viewer: the failed load leaves no stale page or pager behind.
async fn load_page<P: ArtifactPorts>(
    state: &AppState,
    ports: &P,
    document_id: Uuid,
    page_index: u32,
) -> Result<(Arc<DocumentArtifact>, RenderedPdfPage), CommandError> {
    let loaded = async {
        let artifact = ports
            .read(document_id)
            .await
            .map_err(|error| preview_error(&error))?;
        let rendered = ports
            .render(&artifact, page_index)
            .await
            .map_err(|error| preview_error(&error))?;
        Ok::<_, CommandError>((Arc::new(artifact), rendered))
    }
    .await;
    if loaded.is_err() {
        state.clear_artifact_view();
    }
    loaded
}

/// Renders one page of the artifact the viewer already holds (a page turn). A failure clears the viewer.
async fn turn_page<P: ArtifactPorts>(
    state: &AppState,
    ports: &P,
    artifact: &DocumentArtifact,
    page_index: u32,
) -> Result<RenderedPdfPage, CommandError> {
    let rendered = ports
        .render(artifact, page_index)
        .await
        .map_err(|error| preview_error(&error));
    if rendered.is_err() {
        state.clear_artifact_view();
    }
    rendered
}

/// Puts a rendered page on screen as the artifact viewer's raster and holds the verified artifact as the
/// session. A failure clears the viewer (the raster and the session), so no stale page or pager remains.
fn install(
    state: &AppState,
    artifact: Arc<DocumentArtifact>,
    page_index: u32,
    rendered: RenderedPdfPage,
) -> Result<PageImageRefDto, CommandError> {
    let page_count = rendered.page_count;
    let raster = state
        .artifact_rasters()
        .replace(rendered.png, rendered.width, rendered.height);
    let Some(raster) = raster else {
        state.clear_artifact_view();
        return Err(CommandError::new("internal_error", ARTIFACT_PREVIEW_FAILED));
    };
    let document_id = artifact.document.id.to_string();
    state.set_artifact_session(ArtifactSession {
        artifact,
        page_count,
    });
    Ok(page_image_ref(
        PageViewer::Artifacts,
        &document_id,
        page_index,
        page_count,
        &raster,
    ))
}

/// The stored document ID of a request: required text, trimmed, lowercased, a canonical UUID.
fn document_id_field(request: &Value) -> Result<Uuid, CommandError> {
    let text = text_field(request, "documentId", ARTIFACT_INPUT)?;
    admit_uuid(&strip_text(text).to_ascii_lowercase())
        .map_err(|_| CommandError::new("validation_error", ARTIFACT_INPUT))
}

/// The zero-based page index of a request: a JSON integer that fits in `u32`.
fn page_index_field(request: &Value) -> Result<u32, CommandError> {
    request
        .get("pageIndex")
        .and_then(Value::as_u64)
        .and_then(|index| u32::try_from(index).ok())
        .ok_or_else(|| CommandError::new("validation_error", ARTIFACT_PREVIEW_FAILED))
}

/// The copy of a read or render failure of the viewer. `not_found` keeps its code; every other
/// failure is `preview_failed`, whatever its cause.
fn preview_error(error: &AppError) -> CommandError {
    if is_not_found(error) {
        CommandError::new("not_found", ARTIFACT_PREVIEW_FAILED)
    } else {
        CommandError::new("preview_failed", ARTIFACT_PREVIEW_FAILED)
    }
}

/// The export failure of a read before the dialog: `not_found` keeps its code, otherwise the export
/// is refused (nothing was created).
fn export_read_error(error: &AppError) -> CommandError {
    if is_not_found(error) {
        CommandError::new("not_found", ARTIFACT_EXPORT_FAILED)
    } else {
        export_refused()
    }
}

/// The refusal of an export before publication. No name was created.
fn export_refused() -> CommandError {
    CommandError::new("export_failed", ARTIFACT_EXPORT_FAILED)
}

/// The outcome of a publication whose completion could not be verified.
fn export_uncertain() -> CommandError {
    CommandError::new("export_uncertain", ARTIFACT_EXPORT_UNCERTAIN)
}

/// The unconfigured refusal: the artifact services are not composed (flag or database).
fn not_configured() -> CommandError {
    CommandError::new(NOT_CONFIGURED, ARTIFACT_PREVIEW_FAILED)
}

/// Whether a core failure is a `not_found` (the wire code is the only thing the copy depends on).
fn is_not_found(error: &AppError) -> bool {
    error_envelope(error).code.code() == "not_found"
}

/// The destination text the exporter receives: an absolute local path that is valid text. The path is
/// never echoed back; a refusal carries no part of it.
fn destination_text(path: &Path) -> Option<&str> {
    if !is_local_absolute(path) {
        return None;
    }
    path.to_str()
}

/// The name suggested by the dialog: the base name of the stored file, with a `.pdf` suffix.
fn suggested_name(original: &str) -> String {
    let base = windows_basename(original).trim();
    if base.is_empty() {
        return DEFAULT_EXPORT_NAME.to_owned();
    }
    if base.to_ascii_lowercase().ends_with(".pdf") {
        base.to_owned()
    } else {
        format!("{base}.pdf")
    }
}

/// A file name that may cross IPC: non-empty, at most 4096 UTF-8 bytes, with no separator and no NUL.
fn is_safe_export_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_EXPORT_NAME_BYTES && !name.contains(['/', '\\', '\0'])
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use jobniac_core::documents::model::DocumentMetadata;
    use jobniac_core::errors::{ApplicationError, ErrorCode, not_found};

    use super::*;
    use crate::state::ARTIFACT_BUSY;

    /// Synthetic identifiers and names only.
    const DOC_ID: &str = "00000000-0000-4000-8000-0000000000d1";
    /// An absolute local directory on the host: a drive path on Windows, a rooted path elsewhere
    /// (a drive path is not absolute on Linux, so the export tests would be refused there).
    #[cfg(windows)]
    const PRIVATE_DIR: &str = "C:\\private-folder-for-tests";
    #[cfg(not(windows))]
    const PRIVATE_DIR: &str = "/private-folder-for-tests";

    /// The test bodies use `?` on command results; a refusal reads as its message id.
    impl std::fmt::Display for CommandError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "{}", self.message_id)
        }
    }

    impl std::error::Error for CommandError {}

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// What the fake read or render answers.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Outcome {
        Ok,
        NotFound,
        Failed,
    }

    fn failure(outcome: Outcome) -> AppError {
        match outcome {
            Outcome::NotFound => AppError::from(not_found(
                "Document not found",
                [("documentId", DOC_ID.to_owned())],
            )),
            _ => AppError::Application(ApplicationError::with_message(
                ErrorCode::InternalError,
                "fixed test failure",
            )),
        }
    }

    fn metadata() -> DocumentMetadata {
        DocumentMetadata {
            id: Uuid::parse_str(DOC_ID).unwrap_or_default(),
            original_file_name: "folder\\Example CV.pdf".to_owned(),
            media_type: "application/pdf".to_owned(),
            byte_size: 2048,
            sha256: "0".repeat(64),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    /// The verified artifact of `content`: its digest and size are those of the bytes.
    fn artifact_with(content: &[u8]) -> DocumentArtifact {
        let mut document = metadata();
        document.byte_size = i64::try_from(content.len()).unwrap_or(0);
        document.sha256 = jobniac_core::documents::sha256_hex(content);
        DocumentArtifact {
            document,
            content: content.to_vec(),
        }
    }

    fn rendered(page_count: u32, marker: u8) -> RenderedPdfPage {
        RenderedPdfPage {
            page_count,
            width: 8,
            height: 6,
            png: vec![0x89, b'P', b'N', b'G', marker],
        }
    }

    /// Answers the read and the render, and the exporter. Records what it was asked: each read counts,
    /// and each render records the bytes of the artifact it was given.
    struct FakePorts {
        read: Outcome,
        render: Outcome,
        page_count: u32,
        /// The content the successive reads answer; the last one repeats (a row that changes).
        contents: Vec<Vec<u8>>,
        reads: std::sync::atomic::AtomicUsize,
        rendered: Mutex<Vec<(Vec<u8>, u32)>>,
        exporter: Arc<FakeExporter>,
    }

    impl ArtifactPorts for FakePorts {
        async fn read(&self, _document_id: Uuid) -> Result<DocumentArtifact, AppError> {
            let call = self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match self.read {
                Outcome::Ok => {
                    let index = call.min(self.contents.len().saturating_sub(1));
                    let content = self.contents.get(index).cloned().unwrap_or_default();
                    Ok(artifact_with(&content))
                }
                outcome => Err(failure(outcome)),
            }
        }

        async fn render(
            &self,
            artifact: &DocumentArtifact,
            page_index: u32,
        ) -> Result<RenderedPdfPage, AppError> {
            if let Ok(mut rendered) = self.rendered.lock() {
                rendered.push((artifact.content.clone(), page_index));
            }
            match self.render {
                Outcome::Ok => Ok(rendered(
                    self.page_count,
                    u8::try_from(page_index).unwrap_or(0),
                )),
                outcome => Err(failure(outcome)),
            }
        }

        fn exporter(&self) -> Arc<dyn ArtifactExporter> {
            let exporter: Arc<dyn ArtifactExporter> = self.exporter.clone();
            exporter
        }
    }

    fn ports(read: Outcome, render: Outcome) -> FakePorts {
        with_contents(read, render, vec![b"%PDF-1.4\n".to_vec()])
    }

    fn with_contents(read: Outcome, render: Outcome, contents: Vec<Vec<u8>>) -> FakePorts {
        FakePorts {
            read,
            render,
            page_count: 2,
            contents,
            reads: std::sync::atomic::AtomicUsize::new(0),
            rendered: Mutex::new(Vec::new()),
            exporter: Arc::new(FakeExporter::new(Ok("copy.pdf".to_owned()))),
        }
    }

    impl FakePorts {
        fn reads(&self) -> usize {
            self.reads.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn rendered_contents(&self) -> Vec<Vec<u8>> {
            self.rendered
                .lock()
                .map(|calls| calls.iter().map(|(content, _)| content.clone()).collect())
                .unwrap_or_default()
        }
    }

    /// Publishes nothing; answers with a fixed outcome and records the destinations it was given.
    #[derive(Debug)]
    struct FakeExporter {
        outcome: Result<String, ExportFailure>,
        destinations: Mutex<Vec<String>>,
    }

    impl FakeExporter {
        fn new(outcome: Result<String, ExportFailure>) -> Self {
            Self {
                outcome,
                destinations: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> usize {
            self.destinations
                .lock()
                .map(|calls| calls.len())
                .unwrap_or(0)
        }
    }

    impl ArtifactExporter for FakeExporter {
        fn export(
            &self,
            _artifact: &DocumentArtifact,
            destination: &str,
        ) -> Result<String, ExportFailure> {
            if let Ok(mut calls) = self.destinations.lock() {
                calls.push(destination.to_owned());
            }
            self.outcome.clone()
        }
    }

    /// Answers the save dialog with a fixed choice (`None` is a cancel) and records the suggestion.
    struct FakePicker {
        answer: Option<PathBuf>,
        suggestions: Mutex<Vec<String>>,
    }

    impl PdfSavePicker for FakePicker {
        fn pick_save(&self, suggested_name: &str) -> Option<PathBuf> {
            if let Ok(mut suggestions) = self.suggestions.lock() {
                suggestions.push(suggested_name.to_owned());
            }
            self.answer.clone()
        }
    }

    fn picker(answer: Option<PathBuf>) -> Arc<FakePicker> {
        Arc::new(FakePicker {
            answer,
            suggestions: Mutex::new(Vec::new()),
        })
    }

    fn request(document_id: &str) -> String {
        serde_json::json!({ "documentId": document_id }).to_string()
    }

    fn page_request(index: i64) -> String {
        serde_json::json!({ "documentId": DOC_ID, "pageIndex": index }).to_string()
    }

    fn state() -> AppState {
        AppState::new(Err(jobniac_runtime::services::StartupError))
    }

    fn absolute(name: &str) -> PathBuf {
        PathBuf::from(PRIVATE_DIR).join(name)
    }

    fn json_of<T: serde::Serialize>(value: &T) -> Result<String, serde_json::Error> {
        serde_json::to_string(value)
    }

    #[tokio::test]
    async fn a_malformed_or_missing_id_is_the_input_copy_and_changes_nothing() -> TestResult {
        let state = state();
        let fake = ports(Outcome::Ok, Outcome::Ok);
        let refusal = run_preview(&state, Some(&fake), &request("not-a-uuid")).await;
        assert_eq!(
            refusal.err(),
            Some(CommandError::new("validation_error", ARTIFACT_INPUT))
        );
        let missing = run_preview(&state, Some(&fake), "{}").await;
        assert_eq!(
            missing.err(),
            Some(CommandError::new("validation_error", ARTIFACT_INPUT))
        );
        let not_json = run_preview(&state, Some(&fake), "[1]").await;
        assert!(not_json.is_err());
        assert!(state.artifact_rasters().current().is_none());
        Ok(())
    }

    #[tokio::test]
    async fn without_the_composed_services_every_command_is_the_unconfigured_refusal() -> TestResult
    {
        let state = state();
        let none: Option<&FakePorts> = None;
        let refusal = CommandError::new(NOT_CONFIGURED, ARTIFACT_PREVIEW_FAILED);
        assert_eq!(
            run_preview(&state, none, &request(DOC_ID)).await.err(),
            Some(refusal.clone())
        );
        assert_eq!(
            run_show_page(&state, none, &page_request(0)).await.err(),
            Some(refusal.clone())
        );
        let picked = picker(None);
        assert_eq!(
            run_export(&state, none, &request(DOC_ID), picked.clone())
                .await
                .err(),
            Some(refusal)
        );
        assert_eq!(
            picked.suggestions.lock().map(|s| s.len()).unwrap_or(1),
            0,
            "no dialog opens without the services"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_preview_shows_page_zero_with_the_summary_and_holds_the_session() -> TestResult {
        let state = state();
        let fake = ports(Outcome::Ok, Outcome::Ok);
        let preview =
            run_preview(&state, Some(&fake), &request(&DOC_ID.to_ascii_uppercase())).await?;
        assert_eq!(preview.document_id, DOC_ID);
        assert_eq!(preview.page_count, 2);
        assert_eq!(preview.current_page, 0);
        assert_eq!(preview.summary.display_name, "Example CV.pdf");
        // The size is the one of the verified bytes the read returned (nine bytes here).
        assert_eq!(preview.summary.byte_size, "9");
        assert_eq!(preview.page.revision, "1");
        assert_eq!(preview.page.page_index, 0);
        let session = state.artifact_session();
        assert!(session.as_ref().is_some_and(|held| {
            held.document_id() == Uuid::parse_str(DOC_ID).unwrap_or_default()
                && held.page_count == 2
        }));
        assert!(state.artifact_rasters().current().is_some());
        // The reply carries the page URL and sizes, never a path.
        let json = json_of(&preview)?;
        assert!(!json.contains(PRIVATE_DIR) && !json.contains("folder"));
        assert!(json.contains("\"displayName\":\"Example CV.pdf\""));
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_read_or_render_clears_the_viewer_and_keeps_the_error_generic() -> TestResult {
        for (read, render, code) in [
            (Outcome::NotFound, Outcome::Ok, "not_found"),
            (Outcome::Failed, Outcome::Ok, "preview_failed"),
            (Outcome::Ok, Outcome::Failed, "preview_failed"),
        ] {
            let state = state();
            let good = ports(Outcome::Ok, Outcome::Ok);
            run_preview(&state, Some(&good), &request(DOC_ID)).await?;
            let bad = ports(read, render);
            let refusal = run_preview(&state, Some(&bad), &request(DOC_ID)).await;
            assert_eq!(
                refusal.err(),
                Some(CommandError::new(code, ARTIFACT_PREVIEW_FAILED))
            );
            assert!(state.artifact_rasters().current().is_none());
            assert_eq!(state.artifact_session(), None);
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_page_turn_renders_the_bytes_verified_at_the_preview_when_the_row_changed()
    -> TestResult {
        let first = b"%PDF-1.4\nfirst\n".to_vec();
        let changed = b"%PDF-1.4\nchanged\n".to_vec();
        let state = state();
        let fake = with_contents(
            Outcome::Ok,
            Outcome::Ok,
            vec![first.clone(), changed.clone()],
        );
        let preview = run_preview(&state, Some(&fake), &request(DOC_ID)).await?;
        let digest = artifact_with(&first).document.sha256;
        let next = run_show_page(&state, Some(&fake), &page_request(1)).await?;
        // The stored row was read once, at the preview; the page turn renders the held bytes.
        assert_eq!(fake.reads(), 1);
        assert_eq!(fake.rendered_contents(), vec![first.clone(), first.clone()]);
        assert_eq!(next.page_count, preview.page_count);
        let held = state.artifact_session();
        assert!(held.as_ref().is_some_and(|session| {
            session.artifact.document.sha256 == digest && session.artifact.content == first
        }));
        Ok(())
    }

    #[tokio::test]
    async fn a_page_turn_renders_the_requested_page_under_a_new_revision() -> TestResult {
        let state = state();
        let fake = ports(Outcome::Ok, Outcome::Ok);
        let first = run_preview(&state, Some(&fake), &request(DOC_ID)).await?;
        let next = run_show_page(&state, Some(&fake), &page_request(1)).await?;
        assert_eq!(next.page_index, 1);
        assert_eq!(next.page_count, 2);
        assert_ne!(next.revision, first.page.revision);
        assert_ne!(next.url, first.page.url);
        Ok(())
    }

    #[tokio::test]
    async fn a_page_turn_is_refused_without_a_loaded_document_or_outside_the_page_count()
    -> TestResult {
        let state = state();
        let fake = ports(Outcome::Ok, Outcome::Ok);
        let nothing = run_show_page(&state, Some(&fake), &page_request(0)).await;
        assert_eq!(
            nothing.err(),
            Some(CommandError::new(
                "validation_error",
                ARTIFACT_PREVIEW_FAILED
            ))
        );
        run_preview(&state, Some(&fake), &request(DOC_ID)).await?;
        for index in [2_i64, -1] {
            let refusal = run_show_page(&state, Some(&fake), &page_request(index)).await;
            assert_eq!(
                refusal.err(),
                Some(CommandError::new(
                    "validation_error",
                    ARTIFACT_PREVIEW_FAILED
                ))
            );
        }
        let fractional = serde_json::json!({ "pageIndex": 1.5 }).to_string();
        assert!(
            run_show_page(&state, Some(&fake), &fractional)
                .await
                .is_err()
        );
        // A refused index keeps the page on screen.
        assert!(state.artifact_rasters().current().is_some());
        assert!(state.artifact_session().is_some());
        Ok(())
    }

    #[tokio::test]
    async fn a_page_turn_for_another_document_is_refused_and_keeps_the_session() -> TestResult {
        let state = state();
        let fake = ports(Outcome::Ok, Outcome::Ok);
        run_preview(&state, Some(&fake), &request(DOC_ID)).await?;
        let revision = state
            .artifact_rasters()
            .current()
            .map(|raster| raster.revision);
        let other = serde_json::json!({
            "documentId": "00000000-0000-4000-8000-0000000000d2",
            "pageIndex": 1
        })
        .to_string();
        assert_eq!(
            run_show_page(&state, Some(&fake), &other).await.err(),
            Some(CommandError::new(
                "validation_error",
                ARTIFACT_PREVIEW_FAILED
            ))
        );
        // Nothing was rendered or replaced: the viewer that owns the session keeps its page.
        assert_eq!(
            state
                .artifact_rasters()
                .current()
                .map(|raster| raster.revision),
            revision
        );
        assert!(state.artifact_session().is_some());
        // A request without the document is malformed.
        let missing = serde_json::json!({ "pageIndex": 1 }).to_string();
        assert!(run_show_page(&state, Some(&fake), &missing).await.is_err());
        assert!(
            run_show_page(&state, Some(&fake), &page_request(1))
                .await
                .is_ok()
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_page_load_clears_the_viewer_and_its_pager() -> TestResult {
        let state = state();
        run_preview(
            &state,
            Some(&ports(Outcome::Ok, Outcome::Ok)),
            &request(DOC_ID),
        )
        .await?;
        let failing = ports(Outcome::Ok, Outcome::Failed);
        assert!(
            run_show_page(&state, Some(&failing), &page_request(1))
                .await
                .is_err()
        );
        assert!(state.artifact_rasters().current().is_none());
        assert_eq!(state.artifact_session(), None);
        Ok(())
    }

    #[tokio::test]
    async fn a_running_request_refuses_the_next_one_as_busy_and_keeps_the_viewer() -> TestResult {
        let state = state();
        let fake = ports(Outcome::Ok, Outcome::Ok);
        run_preview(&state, Some(&fake), &request(DOC_ID)).await?;
        let held = state.begin_artifact()?;
        let busy = CommandError::busy(ARTIFACT_BUSY);
        assert_eq!(
            run_preview(&state, Some(&fake), &request(DOC_ID))
                .await
                .err(),
            Some(busy.clone())
        );
        assert_eq!(
            run_show_page(&state, Some(&fake), &page_request(1))
                .await
                .err(),
            Some(busy.clone())
        );
        let picked = picker(Some(absolute("copy.pdf")));
        assert_eq!(
            run_export(&state, Some(&fake), &request(DOC_ID), picked.clone())
                .await
                .err(),
            Some(busy)
        );
        assert!(
            picked
                .suggestions
                .lock()
                .map(|s| s.is_empty())
                .unwrap_or(false)
        );
        assert!(state.artifact_rasters().current().is_some());
        drop(held);
        Ok(())
    }

    #[tokio::test]
    async fn a_cancelled_dialog_returns_null_and_publishes_nothing() -> TestResult {
        let state = state();
        let fake = ports(Outcome::Ok, Outcome::Ok);
        let outcome = run_export(&state, Some(&fake), &request(DOC_ID), picker(None)).await?;
        assert_eq!(outcome, None);
        assert_eq!(fake.exporter.calls(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn a_published_export_returns_the_file_name_and_keeps_the_viewer() -> TestResult {
        let state = state();
        let fake = ports(Outcome::Ok, Outcome::Ok);
        run_preview(&state, Some(&fake), &request(DOC_ID)).await?;
        let picked = picker(Some(absolute("copy.pdf")));
        let outcome = run_export(&state, Some(&fake), &request(DOC_ID), picked.clone()).await?;
        assert_eq!(
            outcome,
            Some(ArtifactExportDto {
                exported_name: "copy.pdf".to_owned()
            })
        );
        assert_eq!(fake.exporter.calls(), 1);
        // The dialog suggests the base name of the stored file, with the PDF suffix.
        assert_eq!(
            picked
                .suggestions
                .lock()
                .map(|s| s.clone())
                .unwrap_or_default(),
            vec!["Example CV.pdf".to_owned()]
        );
        assert!(state.artifact_rasters().current().is_some());
        assert!(state.artifact_session().is_some());
        Ok(())
    }

    #[tokio::test]
    async fn a_refused_export_is_the_export_copy_and_a_verification_gap_is_uncertain() -> TestResult
    {
        let state = state();
        let refused = FakePorts {
            read: Outcome::Ok,
            render: Outcome::Ok,
            page_count: 1,
            contents: vec![b"%PDF-1.4\n".to_vec()],
            reads: std::sync::atomic::AtomicUsize::new(0),
            rendered: Mutex::new(Vec::new()),
            exporter: Arc::new(FakeExporter::new(Err(ExportFailure::Refused))),
        };
        let error = run_export(
            &state,
            Some(&refused),
            &request(DOC_ID),
            picker(Some(absolute("a.pdf"))),
        )
        .await;
        assert_eq!(
            error.err(),
            Some(CommandError::new("export_failed", ARTIFACT_EXPORT_FAILED))
        );
        let uncertain = FakePorts {
            read: Outcome::Ok,
            render: Outcome::Ok,
            page_count: 1,
            contents: vec![b"%PDF-1.4\n".to_vec()],
            reads: std::sync::atomic::AtomicUsize::new(0),
            rendered: Mutex::new(Vec::new()),
            exporter: Arc::new(FakeExporter::new(Err(ExportFailure::Uncertain))),
        };
        let error = run_export(
            &state,
            Some(&uncertain),
            &request(DOC_ID),
            picker(Some(absolute("a.pdf"))),
        )
        .await;
        assert_eq!(
            error.err(),
            Some(CommandError::new(
                "export_uncertain",
                ARTIFACT_EXPORT_UNCERTAIN
            ))
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_relative_destination_is_refused_before_the_exporter_runs() -> TestResult {
        let state = state();
        let fake = ports(Outcome::Ok, Outcome::Ok);
        let relative = picker(Some(PathBuf::from("relative\\copy.pdf")));
        let error = run_export(&state, Some(&fake), &request(DOC_ID), relative).await;
        assert_eq!(
            error.err(),
            Some(CommandError::new("export_failed", ARTIFACT_EXPORT_FAILED))
        );
        assert_eq!(fake.exporter.calls(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn an_export_of_an_unknown_document_is_not_found_before_the_dialog_opens() -> TestResult {
        let state = state();
        let missing = ports(Outcome::NotFound, Outcome::Ok);
        let picked = picker(Some(absolute("a.pdf")));
        let error = run_export(&state, Some(&missing), &request(DOC_ID), picked.clone()).await;
        assert_eq!(
            error.err(),
            Some(CommandError::new("not_found", ARTIFACT_EXPORT_FAILED))
        );
        assert!(
            picked
                .suggestions
                .lock()
                .map(|s| s.is_empty())
                .unwrap_or(false)
        );
        Ok(())
    }

    #[tokio::test]
    async fn no_path_or_operating_system_text_reaches_an_error_or_a_dto() -> TestResult {
        let state = state();
        let fake = ports(Outcome::Ok, Outcome::Ok);
        let refused = FakePorts {
            read: Outcome::Ok,
            render: Outcome::Ok,
            page_count: 1,
            contents: vec![b"%PDF-1.4\n".to_vec()],
            reads: std::sync::atomic::AtomicUsize::new(0),
            rendered: Mutex::new(Vec::new()),
            exporter: Arc::new(FakeExporter::new(Err(ExportFailure::Refused))),
        };
        let error = run_export(
            &state,
            Some(&refused),
            &request(DOC_ID),
            picker(Some(absolute("a.pdf"))),
        )
        .await
        .err();
        let error_json = json_of(&error)?;
        assert!(!error_json.contains(PRIVATE_DIR));
        let exported = run_export(
            &state,
            Some(&fake),
            &request(DOC_ID),
            picker(Some(absolute("a.pdf"))),
        )
        .await?;
        assert!(!json_of(&exported)?.contains(PRIVATE_DIR));
        let preview = run_preview(&state, Some(&fake), &request(DOC_ID)).await?;
        assert!(!json_of(&preview)?.contains(PRIVATE_DIR));
        Ok(())
    }

    #[test]
    fn the_suggestion_and_the_name_rules_hold() {
        assert_eq!(suggested_name("dir/cv.PDF"), "cv.PDF");
        assert_eq!(suggested_name("cv"), "cv.pdf");
        assert_eq!(suggested_name("  "), DEFAULT_EXPORT_NAME);
        assert!(is_safe_export_name("copy.pdf"));
        assert!(!is_safe_export_name(""));
        assert!(!is_safe_export_name("a/b.pdf"));
        assert!(!is_safe_export_name("a\\b.pdf"));
        assert!(!is_safe_export_name("a\0b.pdf"));
        assert!(!is_safe_export_name(&"n".repeat(MAX_EXPORT_NAME_BYTES + 1)));
    }

    #[test]
    fn the_destination_must_be_an_absolute_local_path() {
        assert!(destination_text(Path::new("relative.pdf")).is_none());
        assert!(destination_text(Path::new("\\\\server\\share\\a.pdf")).is_none());
        assert!(destination_text(&absolute("a.pdf")).is_some() || cfg!(not(windows)));
    }
}
