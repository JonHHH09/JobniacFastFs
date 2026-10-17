//! Document command adapters.
//!
//! Each command takes the single-flight slot of the documents workspace, calls one core use case and
//! maps the result to a DTO. The requests are JSON text, as the matching commands take them, so a lone
//! surrogate escape is refused with the operation's own copy. The stored-PDF preview always
//! uses a display budget of 100,000 UTF-16 units and no page list.
//!
//! The choose, import and preview actions are the selection commands below.
//! The chosen path is held in Rust state behind an unguessable token and never crosses IPC: the
//! native dialog runs behind [`PdfPicker`], so the command tests answer for the user with a fake.
//! Import and preview both read the file through the document service, which enforces the import
//! root, the file-safety rules, the size limit and the PDF magic.

#[cfg(windows)]
use std::path::{Component, Prefix};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jobniac_core::documents::{DEFAULT_MAX_CHARACTERS, MAX_PAGE_COUNT, safe_basename};
use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::ids::admit_uuid;
use jobniac_runtime::services::DOCUMENT_IMPORT_ROOT_VARIABLE;
use jobniac_text::str_rules::strip_text;
use serde_json::{Value, json};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

use crate::document_dto::{
    DocumentCapabilitiesDto, SelectedPdfDto, StoredDocumentDto, StoredDocumentPageDto,
    StoredPreviewDto, TextPreviewDto, stored_document, stored_preview, text_preview,
};
use crate::error::CommandError;
use crate::match_commands::{parse_request, require_database, text_field};
use crate::page_image::{PageImageRefDto, PageViewer, page_image_ref};
use crate::state::{AppState, PendingSelection};

/// Message id of a selection refused for its shape: not absolute, a UNC, device or drive-relative
/// form, not a `.pdf` file, or not text. The copy never names the path.
pub const DOCUMENT_SELECTION_INVALID: &str = "documents.selection.invalid";
/// Message id of an import or a preview while no PDF is selected (none chosen, or the selection was
/// dropped by a reset, an import or the shutdown).
pub const DOCUMENT_SELECTION_NONE: &str = "documents.selection.none";
/// The title of the native open dialog. The operating system
/// draws the dialog, so its copy is kept in Rust rather than in the React catalogue.
pub const DIALOG_TITLE: &str = "Choose a PDF from your configured import root";
/// The filter name of the native open dialog.
pub const DIALOG_FILTER_NAME: &str = "PDF documents";
/// The only extension a selection may carry (matched without regard to case).
const PDF_EXTENSION: &str = "pdf";

/// Message id of a document request while no local database is configured (the copy is the intro of
/// the unavailable state).
pub const DOCUMENT_UNCONFIGURED: &str = "documents.intro.unavailable";
/// Message id of a document request whose stored document ID is malformed.
pub const DOCUMENT_INPUT: &str = "documents.input.id";
/// Message id of every other document failure, whatever the code.
pub const DOCUMENT_ERROR: &str = "documents.error.generic";
/// The display budget of the stored preview, in UTF-16 units.
pub const PREVIEW_CHARACTERS: usize = DEFAULT_MAX_CHARACTERS;
/// The fixed page size of the stored-document listing. The pickers filter by name over the loaded
/// pages, so the page is larger than the 20 of the other histories (the codec allows 100).
pub const DOCUMENT_LIST_LIMIT: i64 = 50;
/// The largest zoom a render request may carry. The viewer never upscales, so the bound is 1.0.
pub const ZOOM_MAX: f64 = 1.0;

/// Reports whether the documents screen is usable, so it can show the unavailable intro and disable its
/// actions before any click. Read-only: it takes no slot, so the screen can ask on mount.
#[tauri::command]
#[specta::specta]
pub async fn documents_capabilities_get(
    state: State<'_, AppState>,
) -> Result<DocumentCapabilitiesDto, CommandError> {
    run_capabilities(&state)
}

fn run_capabilities(state: &AppState) -> Result<DocumentCapabilitiesDto, CommandError> {
    let services = state.services()?;
    // The same gate as `require_database` of every action of the screen.
    Ok(DocumentCapabilitiesDto {
        documents: services.database.is_some(),
    })
}

/// Lists one page of the stored documents, newest first, optionally from the cursor of the previous
/// page (desktop only: no MCP tool has this shape). Read-only: it has a slot of its own, so a
/// picker can list while the documents workspace holds its slot. No path, hash or content crosses.
#[tauri::command]
#[specta::specta]
pub async fn documents_list(
    state: State<'_, AppState>,
    cursor: Option<String>,
) -> Result<StoredDocumentPageDto, CommandError> {
    run_list(&state, cursor.as_deref()).await
}

async fn run_list(
    state: &AppState,
    cursor: Option<&str>,
) -> Result<StoredDocumentPageDto, CommandError> {
    let services = state.services()?;
    require_database(services, DOCUMENT_UNCONFIGURED)?;
    let _slot = state.begin_document_list()?;
    let page = services
        .documents
        .list
        .list_documents(Some(DOCUMENT_LIST_LIMIT), cursor)
        .await
        .map_err(|error| document_error(&error))?;
    Ok(StoredDocumentPageDto::from(&page))
}

/// Clears the documents view. The React state is cleared by the screen. The command drops the resident
/// page raster and cancels the PDF worker calls in flight, so a running preview or render ends as
/// cancelled instead of being waited for.
#[tauri::command]
#[specta::specta]
pub async fn document_reset(state: State<'_, AppState>) -> Result<(), CommandError> {
    state.release_document_work();
    Ok(())
}

/// Renders one page of a stored PDF for the page-image viewer. The PNG stays in Rust: the reply carries the `jobniac-page` URL of the resident
/// raster, its size and its revision, never a path or bytes.
#[tauri::command]
#[specta::specta]
pub async fn document_render_page(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<PageImageRefDto, CommandError> {
    run_render_page(&state, &request_json).await
}

async fn run_render_page(
    state: &AppState,
    request_json: &str,
) -> Result<PageImageRefDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, DOCUMENT_INPUT)?;
    let document_id = document_id_field(&request)?;
    let page_index = page_index_field(&request)?;
    zoom_field(&request)?;
    require_database(services, DOCUMENT_UNCONFIGURED)?;
    let _guard = state.begin_document()?;
    let rendered = match services
        .documents
        .service
        .render_stored_page(document_id, page_index)
        .await
    {
        Ok(rendered) => rendered,
        Err(error) => {
            // A failed preview leaves no stale page behind.
            state.page_rasters().clear();
            return Err(document_error(&error));
        }
    };
    let raster = state
        .page_rasters()
        .replace(rendered.png, rendered.width, rendered.height);
    let Some(raster) = raster else {
        state.page_rasters().clear();
        return Err(CommandError::new("internal_error", DOCUMENT_ERROR));
    };
    Ok(page_image_ref(
        PageViewer::Documents,
        &document_id.to_string(),
        page_index,
        rendered.page_count,
        &raster,
    ))
}

/// The native open dialog behind a seam, so the command tests can answer for the user.
pub trait PdfPicker: Send + Sync {
    /// Shows the open dialog for one PDF, starting in `start_dir` when one is given. `None` when the
    /// user cancels. The call blocks until the dialog closes, so the caller runs it off the runtime.
    fn pick_pdf(&self, start_dir: Option<&Path>) -> Option<PathBuf>;
}

/// The operating-system open dialog of the desktop shell. It is called from Rust only, so the
/// webview needs no dialog permission.
#[derive(Clone)]
pub struct NativePdfPicker {
    app: AppHandle,
}

impl NativePdfPicker {
    /// A picker that shows its dialog for `app`.
    #[must_use]
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl PdfPicker for NativePdfPicker {
    fn pick_pdf(&self, start_dir: Option<&Path>) -> Option<PathBuf> {
        let mut dialog = self
            .app
            .dialog()
            .file()
            .set_title(DIALOG_TITLE)
            .add_filter(DIALOG_FILTER_NAME, &[PDF_EXTENSION]);
        if let Some(directory) = start_dir {
            dialog = dialog.set_directory(directory);
        }
        dialog
            .blocking_pick_file()
            .and_then(|file| file.into_path().ok())
    }
}

/// The directory the dialog opens in: the configured import root when it is an absolute local
/// directory. The environment is read again here only for this hint; the import layer enforces the
/// root on every read.
#[must_use]
pub fn import_root_start_dir() -> Option<PathBuf> {
    let environment = jobniac_runtime::process_environment();
    let path = PathBuf::from(environment.get(DOCUMENT_IMPORT_ROOT_VARIABLE)?);
    (is_local_absolute(&path) && path.is_dir()).then_some(path)
}

/// Whether `path` is an absolute local path: a drive path on Windows, an absolute path elsewhere.
/// A UNC form, a device form (`\\?\`, `\\.\`) and a drive-relative form (`C:name`) are never local.
#[must_use]
pub fn is_local_absolute(path: &Path) -> bool {
    let text = path.as_os_str().to_string_lossy();
    if text.starts_with("\\\\") || text.starts_with("//") || !path.is_absolute() {
        return false;
    }
    has_disk_prefix(path)
}

#[cfg(windows)]
fn has_disk_prefix(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))
    )
}

#[cfg(not(windows))]
fn has_disk_prefix(_path: &Path) -> bool {
    true
}

/// The text of a selected path that the import and preview requests send: a local absolute path to
/// a `.pdf` file that is valid text.
///
/// # Errors
///
/// A validation error with the selection copy for any other path. The error never names the path.
pub fn admitted_path_text(path: &Path) -> Result<&str, CommandError> {
    let refusal = || CommandError::new("validation_error", DOCUMENT_SELECTION_INVALID);
    if !is_local_absolute(path) {
        return Err(refusal());
    }
    let is_pdf = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(PDF_EXTENSION));
    if !is_pdf {
        return Err(refusal());
    }
    path.to_str().ok_or_else(refusal)
}

/// Shows the native open dialog and holds the chosen PDF.
/// A cancel returns `null` and changes nothing; a refused choice drops the held selection.
#[tauri::command]
#[specta::specta]
pub async fn document_choose_file(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<SelectedPdfDto>, CommandError> {
    let picker: Arc<dyn PdfPicker> = Arc::new(NativePdfPicker::new(app));
    run_choose(&state, picker, import_root_start_dir()).await
}

async fn run_choose(
    state: &AppState,
    picker: Arc<dyn PdfPicker>,
    start_dir: Option<PathBuf>,
) -> Result<Option<SelectedPdfDto>, CommandError> {
    state.services()?;
    let _guard = state.begin_document()?;
    let picked = tokio::task::spawn_blocking(move || picker.pick_pdf(start_dir.as_deref()))
        .await
        .map_err(|_| CommandError::new("internal_error", DOCUMENT_ERROR))?;
    let Some(path) = picked else {
        return Ok(None);
    };
    // A choose starts from no selection, so a failed check leaves nothing held.
    state.clear_selection();
    admitted_path_text(&path)?;
    let display_name = safe_basename(
        &path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    let token = Uuid::new_v4().to_string();
    state.stage_selection(PendingSelection {
        token: token.clone(),
        path,
        display_name: display_name.clone(),
    });
    Ok(Some(SelectedPdfDto {
        selection_token: token,
        display_name,
    }))
}

/// Imports the held PDF into the store. The source is kept;
/// the selection is spent by a successful import.
#[tauri::command]
#[specta::specta]
pub async fn document_import_selected(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<StoredDocumentDto, CommandError> {
    run_import_selected(&state, &request_json).await
}

async fn run_import_selected(
    state: &AppState,
    request_json: &str,
) -> Result<StoredDocumentDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, DOCUMENT_SELECTION_INVALID)?;
    let token = selection_token_field(&request)?;
    require_database(services, DOCUMENT_UNCONFIGURED)?;
    let _guard = state.begin_document()?;
    let selection = held_selection_for(state, token)?;
    let path_text = admitted_path_text(&selection.path)?;
    let metadata = services
        .documents
        .service
        .store_document_file(&json!({ "path": path_text }))
        .await
        .map_err(|error| document_error(&error))?;
    state.clear_selection();
    stored_document(&metadata).map_err(|_| CommandError::new("internal_error", DOCUMENT_ERROR))
}

/// Previews the plain text of the held PDF without importing it. The text is untrusted and bounded by the display budget.
#[tauri::command]
#[specta::specta]
pub async fn document_preview_selected(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<TextPreviewDto, CommandError> {
    run_preview_selected(&state, &request_json).await
}

async fn run_preview_selected(
    state: &AppState,
    request_json: &str,
) -> Result<TextPreviewDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, DOCUMENT_SELECTION_INVALID)?;
    let token = selection_token_field(&request)?;
    let _guard = state.begin_document()?;
    let selection = held_selection_for(state, token)?;
    let path_text = admitted_path_text(&selection.path)?;
    let text = services
        .documents
        .service
        .extract_pdf_text(&json!({
            "path": path_text,
            "maxCharacters": PREVIEW_CHARACTERS,
            "includePages": false,
        }))
        .await
        .map_err(|error| document_error(&error))?;
    Ok(text_preview(&text))
}

/// The selection token of a request: required text.
fn selection_token_field(request: &Value) -> Result<&str, CommandError> {
    text_field(request, "selectionToken", DOCUMENT_SELECTION_INVALID)
}

/// The held selection when `token` names it. No selection is `selection.none`; another token is
/// `selection.invalid`. Neither refusal names the path.
fn held_selection_for(state: &AppState, token: &str) -> Result<PendingSelection, CommandError> {
    match state.held_selection() {
        None => Err(CommandError::new(
            "validation_error",
            DOCUMENT_SELECTION_NONE,
        )),
        Some(held) if held.token == token => Ok(held),
        Some(_) => Err(CommandError::new(
            "validation_error",
            DOCUMENT_SELECTION_INVALID,
        )),
    }
}

/// The zero-based page index of a render request: a JSON integer below the page limit.
fn page_index_field(request: &Value) -> Result<u32, CommandError> {
    request
        .get("pageIndex")
        .and_then(Value::as_u64)
        .and_then(|index| u32::try_from(index).ok())
        .filter(|index| *index < MAX_PAGE_COUNT)
        .ok_or_else(|| CommandError::new("validation_error", DOCUMENT_INPUT))
}

/// The zoom of a render request: a finite JSON number above zero and at most [`ZOOM_MAX`]. The
/// viewer never upscales, so the zoom is validated and not used to size the raster.
fn zoom_field(request: &Value) -> Result<f64, CommandError> {
    request
        .get("zoom")
        .and_then(Value::as_f64)
        .filter(|zoom| zoom.is_finite() && *zoom > 0.0 && *zoom <= ZOOM_MAX)
        .ok_or_else(|| CommandError::new("validation_error", DOCUMENT_INPUT))
}

/// Reads the metadata of a stored document by its ID.
#[tauri::command]
#[specta::specta]
pub async fn document_load_metadata(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<StoredDocumentDto, CommandError> {
    run_load_metadata(&state, &request_json).await
}

async fn run_load_metadata(
    state: &AppState,
    request_json: &str,
) -> Result<StoredDocumentDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, DOCUMENT_INPUT)?;
    let document_id = document_id_field(&request)?;
    require_database(services, DOCUMENT_UNCONFIGURED)?;
    let _guard = state.begin_document()?;
    let metadata = services
        .documents
        .service
        .get_document_metadata(&document_id.to_string())
        .await
        .map_err(|error| document_error(&error))?;
    stored_document(&metadata).map_err(|_| CommandError::new("internal_error", DOCUMENT_ERROR))
}

/// Previews the text of a stored document, optionally persisting the extraction.
#[tauri::command]
#[specta::specta]
pub async fn document_preview_stored(
    state: State<'_, AppState>,
    request_json: String,
) -> Result<StoredPreviewDto, CommandError> {
    run_preview_stored(&state, &request_json).await
}

async fn run_preview_stored(
    state: &AppState,
    request_json: &str,
) -> Result<StoredPreviewDto, CommandError> {
    let services = state.services()?;
    let request = parse_request(request_json, DOCUMENT_INPUT)?;
    let document_id = document_id_field(&request)?;
    let persist = persist_field(&request)?;
    require_database(services, DOCUMENT_UNCONFIGURED)?;
    let _guard = state.begin_document()?;
    let stored = services
        .documents
        .service
        .extract_stored_pdf_text(&json!({
            "documentId": document_id.to_string(),
            "maxCharacters": PREVIEW_CHARACTERS,
            "includePages": false,
            "persistExtraction": persist,
        }))
        .await
        .map_err(|error| document_error(&error))?;
    stored_preview(&stored).map_err(|_| CommandError::new("internal_error", DOCUMENT_ERROR))
}

/// The stored document ID of a request: required, text, and a UUID after normalization
/// (surrounding white space removed, lowercase).
fn document_id_field(request: &Value) -> Result<Uuid, CommandError> {
    let text = text_field(request, "documentId", DOCUMENT_INPUT)?;
    parse_document_id(text)
}

/// Parses one stored document ID with the input copy on refusal.
///
/// # Errors
///
/// A `validation_error` with the input copy for any text that is not a canonical UUID.
pub fn parse_document_id(text: &str) -> Result<Uuid, CommandError> {
    admit_uuid(&strip_text(text).to_ascii_lowercase())
        .map_err(|_| CommandError::new("validation_error", DOCUMENT_INPUT))
}

/// The optional `persist` flag: absent or null is false, any other non-boolean is refused.
fn persist_field(request: &Value) -> Result<bool, CommandError> {
    match request.get("persist") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(_) => Err(CommandError::new("validation_error", DOCUMENT_INPUT)),
    }
}

/// Maps a core failure to the typed command error. The wire code is kept; the copy is the generic one.
fn document_error(error: &AppError) -> CommandError {
    CommandError::new(error_envelope(error).code.code(), DOCUMENT_ERROR)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::state::AppState;
    use jobniac_core::env::Environment;
    use jobniac_runtime::services::{StartupError, compose};

    const ID: &str = "00000000-0000-4000-8000-0000000000AB";

    #[test]
    fn a_document_id_is_normalized_to_lowercase_uuid() {
        let parsed = parse_document_id(&format!("  {ID}  ")).map(|id| id.to_string());
        assert_eq!(
            parsed.ok().as_deref(),
            Some("00000000-0000-4000-8000-0000000000ab")
        );
        assert_eq!(
            parse_document_id("urn:uuid:00000000-0000-4000-8000-0000000000ab")
                .err()
                .map(|error| error.message_id),
            Some(DOCUMENT_INPUT.to_owned())
        );
    }

    #[test]
    fn a_missing_or_non_text_document_id_is_a_validation_error() {
        let missing = document_id_field(&json!({}));
        assert_eq!(
            missing.err(),
            Some(CommandError::new("validation_error", DOCUMENT_INPUT))
        );
        let numeric = document_id_field(&json!({"documentId": 7}));
        assert_eq!(
            numeric.err(),
            Some(CommandError::new("validation_error", DOCUMENT_INPUT))
        );
    }

    #[test]
    fn persist_defaults_to_false_and_refuses_other_types() {
        assert_eq!(persist_field(&json!({})).ok(), Some(false));
        assert_eq!(persist_field(&json!({"persist": true})).ok(), Some(true));
        assert!(persist_field(&json!({"persist": "yes"})).is_err());
    }

    #[test]
    fn a_core_failure_keeps_its_code_and_gets_the_generic_copy() {
        let error = AppError::from(jobniac_core::errors::not_found(
            "Stored document was not found",
            [("documentId", ID.to_owned())],
        ));
        assert_eq!(
            document_error(&error),
            CommandError::new("not_found", DOCUMENT_ERROR)
        );
    }

    #[test]
    fn the_reset_releases_the_raster_while_a_document_request_runs() {
        // The reset does not wait for the running request: it cancels it and drops the raster.
        let state = AppState::new(Err(StartupError));
        let running = state.begin_document();
        assert!(running.is_ok());
        let raster = state.page_rasters().replace(vec![0x89, b'P'], 1, 1);
        assert!(raster.is_some());
        state.release_document_work();
        assert!(state.page_rasters().current().is_none());
    }

    #[test]
    fn the_preview_budget_is_the_display_limit() {
        assert_eq!(PREVIEW_CHARACTERS, 100_000);
    }

    fn configured_state() -> AppState {
        AppState::new(compose(&Environment::default()))
    }

    #[tokio::test]
    async fn a_render_request_refuses_a_bad_document_id_before_the_database() {
        let state = configured_state();
        for request in [
            json!({"documentId": "urn:uuid:00000000-0000-4000-8000-0000000000ab", "pageIndex": 0, "zoom": 1.0}),
            json!({"documentId": 7, "pageIndex": 0, "zoom": 1.0}),
            json!({"pageIndex": 0, "zoom": 1.0}),
        ] {
            let refusal = run_render_page(&state, &request.to_string()).await.err();
            assert_eq!(
                refusal,
                Some(CommandError::new("validation_error", DOCUMENT_INPUT))
            );
        }
    }

    #[tokio::test]
    async fn a_render_request_refuses_a_page_index_outside_the_limit_or_not_an_integer() {
        let state = configured_state();
        for page in [json!(200), json!(-1), json!(0.5), json!("0"), json!(null)] {
            let request = json!({"documentId": ID, "pageIndex": page, "zoom": 1.0});
            let refusal = run_render_page(&state, &request.to_string()).await.err();
            assert_eq!(
                refusal,
                Some(CommandError::new("validation_error", DOCUMENT_INPUT))
            );
        }
    }

    #[tokio::test]
    async fn a_render_request_refuses_a_zoom_outside_the_bounds() {
        let state = configured_state();
        for zoom in [json!(0), json!(-0.5), json!(1.5), json!("1"), json!(null)] {
            let request = json!({"documentId": ID, "pageIndex": 0, "zoom": zoom});
            let refusal = run_render_page(&state, &request.to_string()).await.err();
            assert_eq!(
                refusal,
                Some(CommandError::new("validation_error", DOCUMENT_INPUT))
            );
        }
    }

    #[test]
    fn a_valid_zoom_and_page_index_are_admitted() {
        let page = page_index_field(&json!({"pageIndex": 199}));
        assert_eq!(page.ok(), Some(199));
        assert_eq!(zoom_field(&json!({"zoom": 1.0})).ok(), Some(1.0));
        assert_eq!(zoom_field(&json!({"zoom": 0.5})).ok(), Some(0.5));
    }

    #[test]
    fn the_capabilities_follow_the_database_and_the_startup() {
        let without_database = configured_state();
        assert_eq!(
            run_capabilities(&without_database).ok(),
            Some(DocumentCapabilitiesDto { documents: false })
        );
        let failed = AppState::new(Err(StartupError));
        assert_eq!(
            run_capabilities(&failed).err(),
            Some(CommandError::startup_failed())
        );
    }

    #[tokio::test]
    async fn the_list_without_a_database_is_the_unconfigured_copy() {
        let state = configured_state();
        assert_eq!(
            run_list(&state, None).await.err(),
            Some(CommandError::new("not_configured", DOCUMENT_UNCONFIGURED))
        );
        let failed = AppState::new(Err(StartupError));
        assert_eq!(
            run_list(&failed, None).await.err(),
            Some(CommandError::startup_failed())
        );
    }

    #[tokio::test]
    async fn the_list_has_its_own_slot_so_it_runs_while_the_documents_slot_is_held() {
        let state = configured_state();
        let _running = state.begin_document();
        // Still the unconfigured copy, never busy: the documents slot does not gate the listing.
        assert_eq!(
            run_list(&state, None).await.err(),
            Some(CommandError::new("not_configured", DOCUMENT_UNCONFIGURED))
        );
        // The database check comes first, so hold the list slot to see its own refusal.
        let _list = state.begin_document_list();
        assert_eq!(
            state.begin_document_list().err(),
            Some(CommandError::busy(crate::state::DOCUMENT_BUSY))
        );
    }

    #[tokio::test]
    async fn a_valid_render_request_without_a_database_is_the_unconfigured_copy() {
        let state = configured_state();
        let request = json!({"documentId": ID, "pageIndex": 0, "zoom": 1.0});
        let refusal = run_render_page(&state, &request.to_string()).await.err();
        assert_eq!(
            refusal.map(|error| error.message_id),
            Some(DOCUMENT_UNCONFIGURED.to_owned())
        );
    }

    /// Answers the native dialog with a fixed choice (`None` is a cancel).
    struct FakePicker(Option<PathBuf>);

    impl PdfPicker for FakePicker {
        fn pick_pdf(&self, _start_dir: Option<&Path>) -> Option<PathBuf> {
            self.0.clone()
        }
    }

    fn picker(answer: Option<PathBuf>) -> Arc<dyn PdfPicker> {
        Arc::new(FakePicker(answer))
    }

    /// An absolute path to a file named `name` in a fixed directory of the platform.
    fn absolute(name: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"C:\imports\{name}"))
        } else {
            PathBuf::from(format!("/imports/{name}"))
        }
    }

    fn no_selection() -> CommandError {
        CommandError::new("internal_error", "test.no-selection")
    }

    #[tokio::test]
    async fn a_cancelled_dialog_returns_null_and_keeps_the_held_selection()
    -> Result<(), CommandError> {
        let state = configured_state();
        run_choose(&state, picker(Some(absolute("cv.pdf"))), None).await?;
        let held = state.held_selection();
        assert!(held.is_some());
        let cancelled = run_choose(&state, picker(None), None).await;
        assert_eq!(cancelled, Ok(None));
        assert_eq!(state.held_selection(), held);
        Ok(())
    }

    #[tokio::test]
    async fn a_chosen_pdf_is_held_by_token_and_only_its_base_name_is_returned()
    -> Result<(), CommandError> {
        let state = configured_state();
        let chosen = run_choose(&state, picker(Some(absolute("cv.pdf"))), None)
            .await?
            .ok_or_else(no_selection)?;
        assert_eq!(chosen.display_name, "cv.pdf");
        assert_eq!(chosen.selection_token.len(), 36);
        let wire = serde_json::to_string(&chosen).unwrap_or_default();
        assert!(!wire.contains("imports"), "the path crossed IPC: {wire}");
        let held = state.held_selection().ok_or_else(no_selection)?;
        assert_eq!(held.token, chosen.selection_token);
        assert_eq!(held.path, absolute("cv.pdf"));
        Ok(())
    }

    #[tokio::test]
    async fn a_choice_replaces_the_held_selection() -> Result<(), CommandError> {
        let state = configured_state();
        let first = run_choose(&state, picker(Some(absolute("a.pdf"))), None)
            .await?
            .ok_or_else(no_selection)?;
        let second = run_choose(&state, picker(Some(absolute("b.PDF"))), None)
            .await?
            .ok_or_else(no_selection)?;
        assert_ne!(first.selection_token, second.selection_token);
        assert_eq!(second.display_name, "b.PDF");
        let refused = run_preview_selected(
            &state,
            &json!({"selectionToken": first.selection_token}).to_string(),
        )
        .await
        .err();
        assert_eq!(
            refused,
            Some(CommandError::new(
                "validation_error",
                DOCUMENT_SELECTION_INVALID
            ))
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_refused_form_is_refused_with_the_selection_copy_and_drops_the_selection()
    -> Result<(), CommandError> {
        let state = configured_state();
        run_choose(&state, picker(Some(absolute("kept.pdf"))), None).await?;
        let forms = [
            r"\\server\share\cv.pdf",
            r"\\?\C:\imports\cv.pdf",
            r"\\.\C:\cv.pdf",
            "//server/share/cv.pdf",
            "C:cv.pdf",
            "imports/cv.pdf",
            r"\imports\cv.pdf",
        ];
        for form in forms {
            let refusal = run_choose(&state, picker(Some(PathBuf::from(form))), None)
                .await
                .err();
            assert_eq!(
                refusal,
                Some(CommandError::new(
                    "validation_error",
                    DOCUMENT_SELECTION_INVALID
                )),
                "form {form}"
            );
            assert!(state.held_selection().is_none(), "form {form}");
        }
        Ok(())
    }

    #[test]
    fn a_selection_must_be_a_local_absolute_pdf_file() {
        assert!(admitted_path_text(&absolute("cv.pdf")).is_ok());
        assert!(admitted_path_text(&absolute("CV.Pdf")).is_ok());
        for name in ["cv.txt", "cv", "cv.pdf.exe"] {
            assert_eq!(
                admitted_path_text(&absolute(name)).err(),
                Some(CommandError::new(
                    "validation_error",
                    DOCUMENT_SELECTION_INVALID
                )),
                "name {name}"
            );
        }
        assert!(!is_local_absolute(Path::new(r"\\server\share\cv.pdf")));
        assert!(!is_local_absolute(Path::new("cv.pdf")));
    }

    #[tokio::test]
    async fn a_preview_without_a_selection_is_the_none_copy_and_an_import_needs_the_database() {
        let state = configured_state();
        let request = json!({"selectionToken": "00000000-0000-4000-8000-0000000000ab"}).to_string();
        assert_eq!(
            run_preview_selected(&state, &request).await.err(),
            Some(CommandError::new(
                "validation_error",
                DOCUMENT_SELECTION_NONE
            ))
        );
        assert_eq!(
            run_import_selected(&state, &request).await.err(),
            Some(CommandError::new("not_configured", DOCUMENT_UNCONFIGURED))
        );
    }

    #[tokio::test]
    async fn a_token_other_than_the_held_one_is_refused_as_invalid() -> Result<(), CommandError> {
        let state = configured_state();
        run_choose(&state, picker(Some(absolute("cv.pdf"))), None).await?;
        let refusal = run_preview_selected(
            &state,
            &json!({"selectionToken": "not-the-token"}).to_string(),
        )
        .await
        .err();
        assert_eq!(
            refusal,
            Some(CommandError::new(
                "validation_error",
                DOCUMENT_SELECTION_INVALID
            ))
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_request_without_a_token_is_refused_as_invalid() {
        let state = configured_state();
        for request in ["{}", r#"{"selectionToken": 7}"#, "[]"] {
            assert_eq!(
                run_preview_selected(&state, request).await.err(),
                Some(CommandError::new(
                    "validation_error",
                    DOCUMENT_SELECTION_INVALID
                )),
                "request {request}"
            );
        }
    }

    #[tokio::test]
    async fn a_reset_expires_the_token() -> Result<(), CommandError> {
        let state = configured_state();
        let chosen = run_choose(&state, picker(Some(absolute("cv.pdf"))), None)
            .await?
            .ok_or_else(no_selection)?;
        state.release_document_work();
        let refusal = run_preview_selected(
            &state,
            &json!({"selectionToken": chosen.selection_token}).to_string(),
        )
        .await
        .err();
        assert_eq!(
            refusal,
            Some(CommandError::new(
                "validation_error",
                DOCUMENT_SELECTION_NONE
            ))
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_choose_or_a_preview_is_refused_as_busy_while_the_documents_slot_is_held() {
        let state = configured_state();
        let _running = state.begin_document();
        assert_eq!(
            run_choose(&state, picker(Some(absolute("cv.pdf"))), None)
                .await
                .err(),
            Some(CommandError::busy(crate::state::DOCUMENT_BUSY))
        );
        assert_eq!(
            run_preview_selected(&state, r#"{"selectionToken":"x"}"#)
                .await
                .err(),
            Some(CommandError::busy(crate::state::DOCUMENT_BUSY))
        );
    }

    #[test]
    fn the_render_is_refused_as_busy_while_the_documents_slot_is_held() {
        let state = AppState::new(Err(StartupError));
        let running = state.begin_document();
        assert!(running.is_ok());
        assert_eq!(
            state.begin_document().err(),
            Some(CommandError::busy(crate::state::DOCUMENT_BUSY))
        );
    }
}
