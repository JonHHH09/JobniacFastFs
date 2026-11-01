//! Application state managed by Tauri: the composed services or the sanitized startup failure,
//! and the single-flight flag of the health check.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use jobniac_core::arbeitnow::scan::ScanRequest;
use jobniac_core::artifacts::DocumentArtifact;
use jobniac_runtime::app_settings::{AppSettings, SettingsSaveError};
use jobniac_runtime::arbeitnow::ArbeitnowServices;
use jobniac_runtime::desktop_features::FeatureStates;
use jobniac_runtime::job_links::JobLinkUseCase;
use jobniac_runtime::mcp_server::{McpSupervisor, ProcessSpawner};
use jobniac_runtime::services::{Services, StartupError};
use serde::Serialize;
use uuid::Uuid;

use crate::error::CommandError;
use crate::page_image::PageRasterStore;
use crate::settings_commands::SETTINGS_BUSY;

/// The window event that tells React a close was refused.
pub const CLOSE_REFUSED_EVENT: &str = "shell:close-refused";

/// Message id of a busy Arbeitnow operation (scan, next page or import). React disables the controls
/// while the workspace is busy, so the refusal has no copy.
pub const ARBEITNOW_BUSY: &str = "arbeitnow.busy";

/// Message id of a close refused while an Arbeitnow operation runs.
pub const ARBEITNOW_CLOSE: &str = "arbeitnow.close";

/// One row of the held Arbeitnow scan. The candidate token stays here; the screen only ever sees the
/// row id. `imported` is set by a successful import and makes a second import of the row a refusal.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct HeldRow {
    /// The opaque, random row id handed to React.
    pub(crate) row_id: String,
    /// The signed candidate token of the row. Never serialized.
    pub(crate) token: String,
    /// Whether the row was imported already.
    pub(crate) imported: bool,
}

impl fmt::Debug for HeldRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeldRow")
            .field("row_id", &self.row_id)
            .field("token", &"<redacted>")
            .field("imported", &self.imported)
            .finish()
    }
}

/// The most rows one held Arbeitnow scan keeps: 5 pages of 100 jobs, the largest scan the service
/// admits (`maxPages` 5, `limit` 100). A next page is not loaded once this many rows are held, and
/// the last page gets only the room left (the page request `limit` is reduced to fit).
pub(crate) const MAX_HELD_ROWS: usize = 500;

/// The Arbeitnow session: the rows of the last successful scan (with the next page appended), the
/// filters of that scan and the cursor of the next page. The cursor and the tokens never leave Rust.
/// A new scan replaces the whole table, so the row ids of an earlier scan are refused afterwards.
#[derive(Default)]
pub(crate) struct ArbeitnowSession {
    rows: Vec<HeldRow>,
    filters: Option<ScanRequest>,
    cursor: Option<String>,
}

impl fmt::Debug for ArbeitnowSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArbeitnowSession")
            .field("rows", &self.rows.len())
            .field("cursor", &self.cursor.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

impl ArbeitnowSession {
    /// Replaces the table with the rows of a successful scan, and keeps its filters and cursor.
    pub(crate) fn replace(
        &mut self,
        rows: Vec<HeldRow>,
        filters: ScanRequest,
        cursor: Option<String>,
    ) {
        self.rows = rows;
        self.filters = Some(filters);
        self.cursor = cursor;
    }

    /// Appends the rows of a successful next page and replaces the cursor.
    pub(crate) fn append(&mut self, rows: Vec<HeldRow>, cursor: Option<String>) {
        self.rows.extend(rows);
        self.cursor = cursor;
    }

    /// Whether a next page can be loaded: a cursor is held and the table is below
    /// [`MAX_HELD_ROWS`]. The table is bounded.
    pub(crate) fn has_next(&self) -> bool {
        self.cursor.is_some() && self.rows.len() < MAX_HELD_ROWS
    }

    /// The filters, the cursor and the number of rows the next page may add; `None` when no page can
    /// follow ([`ArbeitnowSession::has_next`] is false).
    pub(crate) fn continuation(&self) -> Option<(ScanRequest, String, usize)> {
        if !self.has_next() {
            return None;
        }
        let filters = self.filters.clone()?;
        let cursor = self.cursor.clone()?;
        Some((filters, cursor, MAX_HELD_ROWS - self.rows.len()))
    }

    /// The candidate token of a row that was not imported yet; `None` for an unknown, stale or
    /// already imported row id.
    pub(crate) fn unimported_token(&self, row_id: &str) -> Option<String> {
        self.rows
            .iter()
            .find(|row| row.row_id == row_id && !row.imported)
            .map(|row| row.token.clone())
    }

    /// Marks a row imported; the row id cannot be used for another import.
    pub(crate) fn mark_imported(&mut self, row_id: &str) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.row_id == row_id) {
            row.imported = true;
        }
    }

    /// Drops every row, token, filter and cursor (shutdown).
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Message id of a close refused while the review workspace is busy or its draft is unsaved.
pub const REVIEW_CLOSE: &str = "reviews.close";

/// Message id of a close refused while the matching workspace is busy.
pub const MATCH_CLOSE: &str = "matching.close";

/// Message id of a close refused while the documents workspace is busy.
pub const DOCUMENT_CLOSE: &str = "documents.close";

/// Message id of a close refused while the profile-from-PDF workspace is busy.
pub const PROFILE_PDF_CLOSE: &str = "profilePdf.close";

/// Message id of a close refused while a generation runs.
pub const GENERATION_CLOSE: &str = "generation.close";

/// Message id for a generation request refused because another one is running. One guard covers the
/// whole generation workspace (copy `generation.busy`, the busy state's accessible name).
pub const GENERATION_BUSY: &str = "generation.busy";

/// Message id for a document request (metadata, stored preview, reset) refused because another one is
/// running. One guard covers the whole documents workspace.
pub const DOCUMENT_BUSY: &str = "documents.busy";

/// Message id for a profile-from-PDF request (ingest, confirm, lookup) refused because another one is
/// running. One guard covers the whole workspace.
pub const PROFILE_PDF_BUSY: &str = "profilePdf.busy";

/// Message id of an ingestion refused while a staged replacement waits for its confirmation.
pub const PROFILE_PDF_CONFIRM_REQUIRED: &str = "profilePdf.confirm.required";

/// A profile replacement staged by a confirmation step. Its inputs stay in Rust; React holds only the
/// opaque `token`. It is consumed by a confirm or a cancel with the same token, and dropped on close.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingReplacement {
    /// The opaque token React sends back.
    pub token: String,
    /// The stored document the replacement is built from.
    pub document_id: Uuid,
    /// The profile to replace.
    pub existing_profile_id: Uuid,
    /// The revision the replacement is checked against.
    pub expected_revision: i64,
    /// The character budget of the extraction, when one was given.
    pub max_characters: Option<i64>,
}

impl fmt::Debug for PendingReplacement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The token authorises replacing a stored profile: it never appears in debug output.
        f.debug_struct("PendingReplacement")
            .field("token", &"<redacted>")
            .field("document_id", &self.document_id)
            .field("existing_profile_id", &self.existing_profile_id)
            .field("expected_revision", &self.expected_revision)
            .field("max_characters", &self.max_characters)
            .finish()
    }
}

/// The PDF chosen in the native dialog. The path stays here; React holds
/// only the opaque `token`. One selection is held at a time: a choose replaces it, and reset, a
/// successful import, the window close and shutdown drop it.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingSelection {
    /// The unguessable token React sends back with an import or a preview.
    pub token: String,
    /// The absolute path of the chosen file. Never sent over IPC.
    pub path: PathBuf,
    /// The base name of the chosen file, the only part of the path React may show.
    pub display_name: String,
}

impl fmt::Debug for PendingSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The token and the local path never appear in debug output; the display name may.
        f.debug_struct("PendingSelection")
            .field("token", &"<redacted>")
            .field("path", &"<redacted>")
            .field("display_name", &self.display_name)
            .finish()
    }
}

/// Message id of a close refused while an artifact preview, page turn or export runs.
pub const ARTIFACT_CLOSE: &str = "artifacts.close";

/// Message id for an artifact request (preview, page, export) refused because another one is running.
/// One guard covers the whole artifacts workspace (copy `artifacts.busy`, the busy accessible name).
pub const ARTIFACT_BUSY: &str = "artifacts.busy";

/// The stored PDF the artifact viewer shows: the verified artifact the preview read, and the page count
/// the worker reported for it. Page turns render from these held bytes (the digest verified at preview),
/// so the stored row is not read again. Nothing here is sent over IPC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactSession {
    /// The verified artifact: its metadata and its bytes, shared with the renders.
    pub artifact: Arc<DocumentArtifact>,
    /// The page count the worker reported for the document.
    pub page_count: u32,
}

impl ArtifactSession {
    /// The stored document the viewer shows.
    #[must_use]
    pub fn document_id(&self) -> Uuid {
        self.artifact.document.id
    }
}

/// Message id for a health check refused because another one is running.
pub const HEALTH_BUSY: &str = "health.busy";

/// Message id for a profile-list request refused because another one is running.
pub const PROFILE_LIST_BUSY: &str = "profiles.busy";

/// Message id for a profile-workspace request (get, save, search, personal) refused because another
/// one is running. The profile search shares this guard.
pub const PROFILE_BUSY: &str = "profile.busy";

/// Message id for a job-workspace request (add, get, list, search, update) refused because another
/// one is running. One guard covers the whole job workspace.
pub const JOB_BUSY: &str = "job.busy";

/// Message id for a matching-workspace request (pair, page, history, report) refused because another
/// one is running. One guard covers the whole matching workspace.
pub const MATCH_BUSY: &str = "matching.busy";

/// Message id for an advisory-review request (submit, read, lists, acknowledge, link) refused because
/// another one is running. One guard covers the whole review workspace.
pub const REVIEW_BUSY: &str = "reviews.busy";

/// Message id of a close refused while a job operation (including a delete) is busy.
pub const JOB_CLOSE: &str = "job.busy.close";

/// Message id of a close refused while a profile operation (including a delete) is busy.
pub const PROFILE_CLOSE: &str = "profile.busy.close";

/// Message id of a close refused while the job editor holds unsaved edits (reason `refuse_unsaved`). The job has no confirmation dialog.
pub const JOB_UNSAVED_CLOSE: &str = "job.validation.unsaved";

/// Message id of the profile discard confirmation that a close with unsaved profile or personal
/// edits opens.
pub const PROFILE_CONFIRM_CLOSE: &str = "profile.confirm.close";

/// How long a delete confirmation token stays usable after Rust issues it.
pub const DELETE_TOKEN_TTL: Duration = Duration::from_secs(300);

/// The entity kind a delete confirmation is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteKind {
    /// A profile hard delete.
    Profile,
    /// A job hard delete.
    Job,
}

/// The one delete confirmation Rust has issued and not yet consumed. It names one entity of one kind.
/// A later request replaces it; its first use, a mismatch or its expiry drops it.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingDeletion {
    token: String,
    kind: DeleteKind,
    entity_id: Uuid,
    issued_at: Instant,
}

impl fmt::Debug for PendingDeletion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The token is the confirmation secret: it never appears in debug output.
        f.debug_struct("PendingDeletion")
            .field("token", &"<redacted>")
            .field("kind", &self.kind)
            .field("entity_id", &self.entity_id)
            .field("issued_at", &self.issued_at)
            .finish()
    }
}

/// The state every command reads. Built once at startup.
#[derive(Debug)]
pub struct AppState {
    services: Result<Services, StartupError>,
    health_in_flight: AtomicBool,
    profile_list_in_flight: AtomicBool,
    profile_in_flight: AtomicBool,
    job_in_flight: AtomicBool,
    match_in_flight: AtomicBool,
    review_in_flight: AtomicBool,
    review_draft_dirty: AtomicBool,
    job_draft_dirty: AtomicBool,
    profile_draft_dirty: AtomicBool,
    /// Set by `shell_request_close` after the profile discard is confirmed; the next gate run takes
    /// it and bypasses the profile confirmation only.
    confirmed_discard_close: AtomicBool,
    document_in_flight: AtomicBool,
    document_list_in_flight: AtomicBool,
    generation_in_flight: AtomicBool,
    artifact_in_flight: AtomicBool,
    profile_pdf_in_flight: AtomicBool,
    profile_pdf_ready: AtomicBool,
    pending_replacement: Mutex<Option<PendingReplacement>>,
    page_rasters: Mutex<PageRasterStore>,
    artifact_rasters: Mutex<PageRasterStore>,
    selection: Mutex<Option<PendingSelection>>,
    artifact_session: Mutex<Option<ArtifactSession>>,
    pending_deletion: Mutex<Option<PendingDeletion>>,
    /// The opt-in link capture (`job_add_link`), composed only when `JOBNIAC_LINK_FETCH_ENABLED` is
    /// exactly `1` and a database is configured. It runs under the job slot (`begin_job`).
    job_links: Option<Arc<JobLinkUseCase>>,
    /// The opt-in Arbeitnow scan and import, composed only with the exact flag, valid keys and a
    /// database. `None` hides the screen.
    arbeitnow: Option<Arc<ArbeitnowServices>>,
    arbeitnow_in_flight: AtomicBool,
    arbeitnow_session: Mutex<ArbeitnowSession>,
    settings_in_flight: AtomicBool,
    /// The persisted settings (the Settings screen); the file lives in `settings_dir`.
    settings: Mutex<AppSettings>,
    /// The app config directory that holds `settings.json`, or `None` when the platform gave none
    /// (a save then fails and the settings stay in memory only). The path never crosses IPC.
    settings_dir: Option<PathBuf>,
    /// Which optional features the environment forces and which are composed in this run.
    feature_states: FeatureStates,
    /// The supervisor of the `jobniac-cli http` child (the MCP server switch).
    mcp: Arc<McpSupervisor>,
}

/// A workspace that refuses the window to close. React shows the copy for `message_id`; the reason
/// is `busy` or `refuse_unsaved`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseRefusal {
    /// The refusing workspace: `review` or `match`.
    pub workspace: &'static str,
    /// Why the close is refused: `busy`, `refuse_unsaved` or `confirm_discard`.
    pub reason: &'static str,
    /// The copy id of the refusal.
    pub message_id: &'static str,
}

impl AppState {
    /// Wrap the result of the composition root.
    pub fn new(services: Result<Services, StartupError>) -> Self {
        Self {
            services,
            health_in_flight: AtomicBool::new(false),
            profile_list_in_flight: AtomicBool::new(false),
            profile_in_flight: AtomicBool::new(false),
            job_in_flight: AtomicBool::new(false),
            match_in_flight: AtomicBool::new(false),
            review_in_flight: AtomicBool::new(false),
            review_draft_dirty: AtomicBool::new(false),
            job_draft_dirty: AtomicBool::new(false),
            profile_draft_dirty: AtomicBool::new(false),
            confirmed_discard_close: AtomicBool::new(false),
            document_in_flight: AtomicBool::new(false),
            document_list_in_flight: AtomicBool::new(false),
            generation_in_flight: AtomicBool::new(false),
            artifact_in_flight: AtomicBool::new(false),
            profile_pdf_in_flight: AtomicBool::new(false),
            profile_pdf_ready: AtomicBool::new(false),
            pending_replacement: Mutex::new(None),
            page_rasters: Mutex::new(PageRasterStore::default()),
            artifact_rasters: Mutex::new(PageRasterStore::default()),
            selection: Mutex::new(None),
            artifact_session: Mutex::new(None),
            pending_deletion: Mutex::new(None),
            job_links: None,
            arbeitnow: None,
            arbeitnow_in_flight: AtomicBool::new(false),
            arbeitnow_session: Mutex::new(ArbeitnowSession::default()),
            settings_in_flight: AtomicBool::new(false),
            settings: Mutex::new(AppSettings::default()),
            settings_dir: None,
            feature_states: FeatureStates::new(),
            // No sidecar and no inherited environment: the composition root replaces it.
            mcp: Arc::new(McpSupervisor::new(
                Arc::new(ProcessSpawner::with_sidecar(None)),
                Vec::new(),
            )),
        }
    }

    /// Attaches the loaded settings and the directory they are saved to. The composition root calls
    /// this once, before the state is managed.
    #[must_use]
    pub fn with_settings(mut self, directory: Option<PathBuf>, settings: AppSettings) -> Self {
        self.settings_dir = directory;
        self.settings = Mutex::new(settings);
        self
    }

    /// Attaches what each optional feature came to at startup. The composition root calls this once,
    /// before the state is managed.
    #[must_use]
    pub fn with_feature_states(mut self, feature_states: FeatureStates) -> Self {
        self.feature_states = feature_states;
        self
    }

    /// What each optional feature came to at startup.
    pub fn feature_states(&self) -> FeatureStates {
        self.feature_states
    }

    /// Attaches the MCP server supervisor. The composition root calls this once, before the state is
    /// managed.
    #[must_use]
    pub fn with_mcp_supervisor(mut self, mcp: Arc<McpSupervisor>) -> Self {
        self.mcp = mcp;
        self
    }

    /// The current settings.
    pub fn app_settings(&self) -> AppSettings {
        *lock(&self.settings)
    }

    /// Saves `settings` to the settings file and, only when that succeeded, makes them current.
    pub(crate) fn save_settings(&self, settings: AppSettings) -> Result<(), SettingsSaveError> {
        let directory = self.settings_dir.as_deref().ok_or(SettingsSaveError)?;
        settings.save(directory)?;
        *lock(&self.settings) = settings;
        Ok(())
    }

    /// The MCP server supervisor. Cloned handles let a blocking thread run a start or a stop.
    pub fn mcp_supervisor(&self) -> Arc<McpSupervisor> {
        Arc::clone(&self.mcp)
    }

    /// Take the settings slot (single flight, refused rather than queued). A server toggle and a
    /// port change hold it; it is not a window close refusal.
    pub(crate) fn begin_settings(&self) -> Result<FlightGuard<'_>, CommandError> {
        FlightGuard::begin(&self.settings_in_flight, SETTINGS_BUSY)
    }

    /// Attaches the composed link capture (or `None` when it is not composed). The composition root
    /// calls this once, before the state is managed.
    #[must_use]
    pub fn with_job_links(mut self, job_links: Option<Arc<JobLinkUseCase>>) -> Self {
        self.job_links = job_links;
        self
    }

    /// The composed link capture, or `None` when the runtime did not compose it.
    pub fn job_links(&self) -> Option<&Arc<JobLinkUseCase>> {
        self.job_links.as_ref()
    }

    /// Attaches the composed Arbeitnow services (or `None` when they are not composed). The
    /// composition root calls this once, before the state is managed.
    #[must_use]
    pub fn with_arbeitnow(mut self, arbeitnow: Option<ArbeitnowServices>) -> Self {
        self.arbeitnow = arbeitnow.map(Arc::new);
        self
    }

    /// The composed Arbeitnow services, or `None` when the runtime did not compose them.
    pub fn arbeitnow(&self) -> Option<&Arc<ArbeitnowServices>> {
        self.arbeitnow.as_ref()
    }

    /// The Arbeitnow session: the held rows, filters and cursor. The lock is held only briefly.
    pub(crate) fn arbeitnow_session(&self) -> MutexGuard<'_, ArbeitnowSession> {
        lock(&self.arbeitnow_session)
    }

    /// Drops the Arbeitnow rows, tokens and cursor (window close and application shutdown).
    pub fn clear_arbeitnow_session(&self) {
        self.arbeitnow_session().clear();
    }

    /// Holds the stored PDF the artifact viewer shows, replacing any earlier one.
    pub fn set_artifact_session(&self, session: ArtifactSession) {
        *lock(&self.artifact_session) = Some(session);
    }

    /// The stored PDF the artifact viewer shows, or `None` when nothing is loaded. The clone shares the
    /// verified bytes; it does not copy them.
    #[must_use]
    pub fn artifact_session(&self) -> Option<ArtifactSession> {
        lock(&self.artifact_session).clone()
    }

    /// Drops the artifact viewer state: its resident raster and the held session. Used when a preview
    /// or a page load fails (the viewer is cleared), on a new preview, and on
    /// window close and shutdown. It never touches the documents viewer's raster.
    pub fn clear_artifact_view(&self) {
        self.artifact_rasters().clear();
        *lock(&self.artifact_session) = None;
    }

    /// Records whether the profile-from-PDF screen shows a result (the state a cancelled replacement
    /// returns to: `ready`, or `idle` when nothing was shown).
    pub fn set_profile_pdf_ready(&self, ready: bool) {
        self.profile_pdf_ready.store(ready, Ordering::Release);
    }

    /// Whether the profile-from-PDF screen shows a result.
    #[must_use]
    pub fn profile_pdf_ready(&self) -> bool {
        self.profile_pdf_ready.load(Ordering::Acquire)
    }

    /// Holds a replacement that waits for its confirmation. Replaces any earlier staged one.
    pub fn stage_replacement(&self, pending: PendingReplacement) {
        *lock(&self.pending_replacement) = Some(pending);
    }

    /// Whether a replacement waits for its confirmation.
    #[must_use]
    pub fn has_pending_replacement(&self) -> bool {
        lock(&self.pending_replacement).is_some()
    }

    /// Takes the staged replacement when `token` names it. A different token leaves it in place.
    pub fn take_replacement(&self, token: &str) -> Option<PendingReplacement> {
        let mut slot = lock(&self.pending_replacement);
        match slot.as_ref() {
            Some(pending) if pending.token == token => slot.take(),
            _ => None,
        }
    }

    /// The page raster of the documents viewer (the `jobniac-page` scheme, documents paths). The lock is
    /// held only for one lookup or replacement.
    pub fn page_rasters(&self) -> MutexGuard<'_, PageRasterStore> {
        lock(&self.page_rasters)
    }

    /// The page raster of the artifacts viewer (the `jobniac-page` scheme, `artifact/` paths). Each viewer
    /// has its own store, so one viewer's preview never replaces the other's page.
    pub fn artifact_rasters(&self) -> MutexGuard<'_, PageRasterStore> {
        lock(&self.artifact_rasters)
    }

    /// Drops the documents viewer's resident page raster, cancels every PDF worker call in flight and
    /// drops the held document selection (documents reset, window close, application shutdown). The
    /// raster is dropped even when the services did not compose, so a stale page is never served after
    /// the release. The artifact viewer is not touched: see [`Self::clear_artifact_view`].
    pub fn release_document_work(&self) {
        if let Ok(services) = &self.services {
            services.documents.cancellation.cancel_in_flight();
        }
        self.page_rasters().clear();
        self.clear_selection();
    }

    /// Replaces the held document selection (a choose has picked a file, or refused its shape).
    ///
    /// The picked path is also granted to the document reader (no import root needed for it); the
    /// grant follows the selection and is the only way a path reaches the reader unrooted.
    pub fn stage_selection(&self, selection: PendingSelection) {
        // The grant changes under the selection lock, so a racing release can never leave a grant
        // without its selection (or the reverse).
        let mut held = lock(&self.selection);
        if let Ok(services) = &self.services {
            match selection.path.to_str() {
                Some(path) => services.documents.picked_file.grant(path),
                None => services.documents.picked_file.revoke(),
            }
        }
        *held = Some(selection);
    }

    /// The held document selection, cloned. The slot keeps it until it is replaced or cleared.
    #[must_use]
    pub fn held_selection(&self) -> Option<PendingSelection> {
        lock(&self.selection).clone()
    }

    /// Drops the held document selection.
    ///
    /// The grant is revoked under the same lock that guards the selection, so both change together.
    pub fn clear_selection(&self) {
        let mut held = lock(&self.selection);
        if let Ok(services) = &self.services {
            services.documents.picked_file.revoke();
        }
        *held = None;
    }

    /// Issues the confirmation token of one delete request, replacing any earlier one. The token is a
    /// random identifier that names no path and carries no secret.
    pub(crate) fn issue_deletion_token(
        &self,
        kind: DeleteKind,
        entity_id: Uuid,
        now: Instant,
    ) -> String {
        let token = Uuid::new_v4().to_string();
        *lock(&self.pending_deletion) = Some(PendingDeletion {
            token: token.clone(),
            kind,
            entity_id,
            issued_at: now,
        });
        token
    }

    /// Consumes the pending confirmation. The result is true only when one exists and matches the kind,
    /// the entity and the token, and it has not expired. The slot is emptied in every case, so a refused
    /// token cannot be tried again.
    pub(crate) fn take_deletion_token(
        &self,
        kind: DeleteKind,
        entity_id: Uuid,
        token: &str,
        now: Instant,
    ) -> bool {
        let Some(pending) = lock(&self.pending_deletion).take() else {
            return false;
        };
        pending.kind == kind
            && pending.entity_id == entity_id
            && pending.token == token
            && now.saturating_duration_since(pending.issued_at) <= DELETE_TOKEN_TTL
    }

    /// Records whether the review draft holds unsaved edits. This is a report of React's draft, not a
    /// workspace operation, so it is not guarded by the single-flight slot.
    pub fn set_review_draft_dirty(&self, dirty: bool) {
        self.review_draft_dirty.store(dirty, Ordering::Release);
    }

    /// Records whether the job editor holds unsaved edits (see `set_review_draft_dirty`).
    pub fn set_job_draft_dirty(&self, dirty: bool) {
        self.job_draft_dirty.store(dirty, Ordering::Release);
    }

    /// Records whether the profile editor holds unsaved profile or personal edits.
    pub fn set_profile_draft_dirty(&self, dirty: bool) {
        self.profile_draft_dirty.store(dirty, Ordering::Release);
    }

    /// Forgets what the previous page told Rust about its drafts. The three dirty flags and the
    /// confirmed-close mark are pure reports of React state; a freshly loaded page holds no draft
    /// and reports nothing until one changes, so a reload (F5, a dev full reload) would otherwise
    /// leave a stale `true` that refuses the window close for a draft that no longer exists. The
    /// in-flight slots are not touched: their guards release them.
    pub fn reset_webview_reports(&self) {
        self.review_draft_dirty.store(false, Ordering::Release);
        self.job_draft_dirty.store(false, Ordering::Release);
        self.profile_draft_dirty.store(false, Ordering::Release);
        self.confirmed_discard_close.store(false, Ordering::Release);
    }

    /// Marks the next close as confirmed by the user (the profile discard was chosen).
    pub fn request_confirmed_close(&self) {
        self.confirmed_discard_close.store(true, Ordering::Release);
    }

    /// Takes the confirmed-close mark, so one close consumes it.
    pub fn take_confirmed_close(&self) -> bool {
        self.confirmed_discard_close.swap(false, Ordering::AcqRel)
    }

    /// The first workspace that refuses the window to close, or `None` when the window may close,
    /// with no confirmed discard.
    pub fn close_refusal(&self) -> Option<CloseRefusal> {
        self.close_refusal_with(false)
    }

    /// The first workspace that refuses the window to close. `confirmed_discard` is true when the
    /// user chose to discard the profile edits in the confirmation: it bypasses the profile
    /// `confirm_discard` refusal only. Busy refusals and the unsaved job and review drafts still
    /// refuse. The order is the gate order.
    pub fn close_refusal_with(&self, confirmed_discard: bool) -> Option<CloseRefusal> {
        // Arbeitnow comes first in the gate order.
        if self.arbeitnow_in_flight.load(Ordering::Acquire) {
            return Some(CloseRefusal {
                workspace: "arbeitnow",
                reason: "busy",
                message_id: ARBEITNOW_CLOSE,
            });
        }
        // A running page render holds the documents slot, so it refuses the close as busy.
        if self.document_in_flight.load(Ordering::Acquire) {
            return Some(CloseRefusal {
                workspace: "document",
                reason: "busy",
                message_id: DOCUMENT_CLOSE,
            });
        }
        // Artifacts follow documents and generation follows artifacts, before the profile-from-PDF
        // workspace.
        if self.artifact_in_flight.load(Ordering::Acquire) {
            return Some(CloseRefusal {
                workspace: "artifact",
                reason: "busy",
                message_id: ARTIFACT_CLOSE,
            });
        }
        if self.generation_in_flight.load(Ordering::Acquire) {
            return Some(CloseRefusal {
                workspace: "generation",
                reason: "busy",
                message_id: GENERATION_CLOSE,
            });
        }
        if self.profile_pdf_in_flight.load(Ordering::Acquire) {
            return Some(CloseRefusal {
                workspace: "profilePdf",
                reason: "busy",
                message_id: PROFILE_PDF_CLOSE,
            });
        }
        if self.review_in_flight.load(Ordering::Acquire) {
            return Some(CloseRefusal {
                workspace: "review",
                reason: "busy",
                message_id: REVIEW_CLOSE,
            });
        }
        if self.review_draft_dirty.load(Ordering::Acquire) {
            return Some(CloseRefusal {
                workspace: "review",
                reason: "refuse_unsaved",
                message_id: REVIEW_CLOSE,
            });
        }
        if self.match_in_flight.load(Ordering::Acquire) {
            return Some(CloseRefusal {
                workspace: "match",
                reason: "busy",
                message_id: MATCH_CLOSE,
            });
        }
        // Jobs and profiles come last in the gate order. Their deletes hold these guards.
        // Order 8 (job): busy, then unsaved edits. Order 9 (profile): busy, then the discard confirmation.
        if self.job_in_flight.load(Ordering::Acquire) {
            return Some(CloseRefusal {
                workspace: "job",
                reason: "busy",
                message_id: JOB_CLOSE,
            });
        }
        if self.job_draft_dirty.load(Ordering::Acquire) {
            return Some(CloseRefusal {
                workspace: "job",
                reason: "refuse_unsaved",
                message_id: JOB_UNSAVED_CLOSE,
            });
        }
        if self.profile_in_flight.load(Ordering::Acquire) {
            return Some(CloseRefusal {
                workspace: "profile",
                reason: "busy",
                message_id: PROFILE_CLOSE,
            });
        }
        if !confirmed_discard && self.profile_draft_dirty.load(Ordering::Acquire) {
            return Some(CloseRefusal {
                workspace: "profile",
                reason: "confirm_discard",
                message_id: PROFILE_CONFIRM_CLOSE,
            });
        }
        None
    }

    /// Take the Arbeitnow slot (single flight, refused rather than queued). A scan, a next page and an
    /// import all hold it.
    pub(crate) fn begin_arbeitnow(&self) -> Result<FlightGuard<'_>, CommandError> {
        FlightGuard::begin(&self.arbeitnow_in_flight, ARBEITNOW_BUSY)
    }

    /// Take the documents-workspace slot (single flight, refused rather than queued).
    pub(crate) fn begin_document(&self) -> Result<FlightGuard<'_>, CommandError> {
        FlightGuard::begin(&self.document_in_flight, DOCUMENT_BUSY)
    }

    /// Take the document-list slot (single flight, refused rather than queued). The listing is
    /// read-only and has a slot of its own, so a picker can list while the documents workspace
    /// imports, previews or holds the native dialog.
    pub(crate) fn begin_document_list(&self) -> Result<FlightGuard<'_>, CommandError> {
        FlightGuard::begin(&self.document_list_in_flight, DOCUMENT_BUSY)
    }

    /// Take the artifacts-workspace slot (single flight, refused rather than queued).
    pub(crate) fn begin_artifact(&self) -> Result<FlightGuard<'_>, CommandError> {
        FlightGuard::begin(&self.artifact_in_flight, ARTIFACT_BUSY)
    }

    /// Take the generation-workspace slot (single flight, refused rather than queued).
    pub(crate) fn begin_generation(&self) -> Result<FlightGuard<'_>, CommandError> {
        FlightGuard::begin(&self.generation_in_flight, GENERATION_BUSY)
    }

    /// Take the profile-from-PDF slot (single flight, refused rather than queued).
    pub(crate) fn begin_profile_pdf(&self) -> Result<FlightGuard<'_>, CommandError> {
        FlightGuard::begin(&self.profile_pdf_in_flight, PROFILE_PDF_BUSY)
    }

    /// Take the job-workspace slot (single flight, refused rather than queued).
    pub(crate) fn begin_job(&self) -> Result<FlightGuard<'_>, CommandError> {
        FlightGuard::begin(&self.job_in_flight, JOB_BUSY)
    }

    /// Take the matching-workspace slot (single flight, refused rather than queued).
    pub(crate) fn begin_matching(&self) -> Result<FlightGuard<'_>, CommandError> {
        FlightGuard::begin(&self.match_in_flight, MATCH_BUSY)
    }

    /// Take the advisory-review slot (single flight, refused rather than queued).
    pub(crate) fn begin_review(&self) -> Result<FlightGuard<'_>, CommandError> {
        FlightGuard::begin(&self.review_in_flight, REVIEW_BUSY)
    }

    /// Take the profile-list slot (single flight, refused rather than queued).
    pub(crate) fn begin_profile_list(&self) -> Result<FlightGuard<'_>, CommandError> {
        FlightGuard::begin(&self.profile_list_in_flight, PROFILE_LIST_BUSY)
    }

    /// Take the profile-workspace slot (single flight, refused rather than queued).
    pub(crate) fn begin_profile(&self) -> Result<FlightGuard<'_>, CommandError> {
        FlightGuard::begin(&self.profile_in_flight, PROFILE_BUSY)
    }

    /// The composed services, or the startup failure as a command error.
    pub fn services(&self) -> Result<&Services, CommandError> {
        self.services
            .as_ref()
            .map_err(|_| CommandError::startup_failed())
    }

    /// Take the health-check slot. A second request while the first runs is refused, not queued.
    pub(crate) fn begin_health_check(&self) -> Result<HealthCheckGuard<'_>, CommandError> {
        self.health_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| HealthCheckGuard(&self.health_in_flight))
            .map_err(|_| CommandError::busy(HEALTH_BUSY))
    }
}

/// Locks a state slot. A poisoned lock still holds a consistent value (every writer replaces the whole
/// value), so the guard is taken from the poison error rather than failing the request.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Releases the health-check slot when dropped, including on early return.
#[derive(Debug)]
pub(crate) struct HealthCheckGuard<'a>(&'a AtomicBool);

impl Drop for HealthCheckGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// A single-flight slot of one workspace. Dropping it releases the slot.
#[derive(Debug)]
pub(crate) struct FlightGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> FlightGuard<'a> {
    /// Claims the flag, or refuses with the busy message id when it is already held.
    fn begin(flag: &'a AtomicBool, busy_message_id: &str) -> Result<Self, CommandError> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self { flag })
            .map_err(|_| CommandError::busy(busy_message_id))
    }
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_drops_the_held_document_selection() {
        let state = AppState::new(Err(StartupError));
        state.stage_selection(PendingSelection {
            token: "t".to_owned(),
            path: PathBuf::from("cv.pdf"),
            display_name: "cv.pdf".to_owned(),
        });
        assert!(state.held_selection().is_some());
        state.release_document_work();
        assert_eq!(state.held_selection(), None);
    }

    #[test]
    fn startup_failure_is_reported_as_the_fixed_command_error() {
        let state = AppState::new(Err(StartupError));
        assert_eq!(state.services().err(), Some(CommandError::startup_failed()));
    }

    #[test]
    fn health_check_is_single_flight_and_the_slot_is_released_on_drop() {
        let state = AppState::new(Err(StartupError));
        let first = state.begin_health_check();
        assert!(first.is_ok());
        assert_eq!(
            state.begin_health_check().err(),
            Some(CommandError::busy(HEALTH_BUSY))
        );
        drop(first);
        assert!(state.begin_health_check().is_ok());
    }

    #[test]
    fn matching_and_review_have_their_own_single_flight_slots() {
        let state = AppState::new(Err(StartupError));
        let matching = state.begin_matching();
        assert!(matching.is_ok());
        assert_eq!(
            state.begin_matching().err(),
            Some(CommandError::busy(MATCH_BUSY))
        );
        // The review workspace is not blocked by a running comparison (separate workspaces).
        let review = state.begin_review();
        assert!(review.is_ok());
        assert_eq!(
            state.begin_review().err(),
            Some(CommandError::busy(REVIEW_BUSY))
        );
        drop(matching);
        assert!(state.begin_matching().is_ok());
    }

    #[test]
    fn the_window_may_close_when_no_workspace_is_busy_and_the_draft_is_clean() {
        let state = AppState::new(Err(StartupError));
        assert_eq!(state.close_refusal(), None);
    }

    #[test]
    fn a_dirty_review_draft_refuses_the_close_until_it_is_clean() {
        let state = AppState::new(Err(StartupError));
        state.set_review_draft_dirty(true);
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "review",
                reason: "refuse_unsaved",
                message_id: REVIEW_CLOSE,
            })
        );
        state.set_review_draft_dirty(false);
        assert_eq!(state.close_refusal(), None);
    }

    #[test]
    fn a_busy_review_is_reported_before_a_dirty_draft_and_before_matching() {
        let state = AppState::new(Err(StartupError));
        let matching = state.begin_matching();
        state.set_review_draft_dirty(true);
        let review = state.begin_review();
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "review",
                reason: "busy",
                message_id: REVIEW_CLOSE,
            })
        );
        drop(review);
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "review",
                reason: "refuse_unsaved",
                message_id: REVIEW_CLOSE,
            })
        );
        state.set_review_draft_dirty(false);
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "match",
                reason: "busy",
                message_id: MATCH_CLOSE,
            })
        );
        drop(matching);
        assert_eq!(state.close_refusal(), None);
    }

    #[test]
    fn documents_and_profile_pdf_refuse_the_close_before_reviews_and_matching() {
        let state = AppState::new(Err(StartupError));
        let matching = state.begin_matching();
        let profile_pdf = state.begin_profile_pdf();
        let document = state.begin_document();
        assert!(matching.is_ok() && profile_pdf.is_ok() && document.is_ok());
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "document",
                reason: "busy",
                message_id: DOCUMENT_CLOSE,
            })
        );
        drop(document);
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "profilePdf",
                reason: "busy",
                message_id: PROFILE_PDF_CLOSE,
            })
        );
        drop(profile_pdf);
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "match",
                reason: "busy",
                message_id: MATCH_CLOSE,
            })
        );
    }

    #[test]
    fn a_running_generation_refuses_the_close_after_documents_and_before_profile_pdf() {
        let state = AppState::new(Err(StartupError));
        let generation = state.begin_generation();
        let profile_pdf = state.begin_profile_pdf();
        assert!(generation.is_ok() && profile_pdf.is_ok());
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "generation",
                reason: "busy",
                message_id: GENERATION_CLOSE,
            })
        );
        let document = state.begin_document();
        assert!(document.is_ok());
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "document",
                reason: "busy",
                message_id: DOCUMENT_CLOSE,
            })
        );
        drop(document);
        drop(generation);
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "profilePdf",
                reason: "busy",
                message_id: PROFILE_PDF_CLOSE,
            })
        );
    }

    #[test]
    fn a_second_generation_is_refused_as_busy_and_the_slot_is_released() {
        let state = AppState::new(Err(StartupError));
        let first = state.begin_generation();
        assert!(first.is_ok());
        assert_eq!(
            state.begin_generation().err(),
            Some(CommandError::busy(GENERATION_BUSY))
        );
        drop(first);
        assert!(state.begin_generation().is_ok());
    }

    #[test]
    fn a_staged_replacement_is_taken_only_with_its_own_token() {
        let state = AppState::new(Err(StartupError));
        state.stage_replacement(PendingReplacement {
            token: "staged".to_owned(),
            document_id: Uuid::nil(),
            existing_profile_id: Uuid::nil(),
            expected_revision: 4,
            max_characters: Some(5000),
        });
        assert!(state.take_replacement("other").is_none());
        assert!(state.has_pending_replacement());
        let taken = state.take_replacement("staged");
        assert_eq!(taken.map(|pending| pending.expected_revision), Some(4));
        assert!(!state.has_pending_replacement());
        assert!(!state.profile_pdf_ready());
        state.set_profile_pdf_ready(true);
        assert!(state.profile_pdf_ready());
    }

    #[test]
    fn a_running_page_render_refuses_the_close_as_a_busy_document_workspace() {
        let state = AppState::new(Err(StartupError));
        let render = state.begin_document();
        assert!(render.is_ok());
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "document",
                reason: "busy",
                message_id: DOCUMENT_CLOSE,
            })
        );
        drop(render);
        assert_eq!(state.close_refusal(), None);
    }

    #[test]
    fn a_running_artifact_request_refuses_the_close_after_documents_and_before_generation() {
        let state = AppState::new(Err(StartupError));
        let artifact = state.begin_artifact();
        assert!(artifact.is_ok());
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "artifact",
                reason: "busy",
                message_id: ARTIFACT_CLOSE,
            })
        );
        let document = state.begin_document();
        assert!(document.is_ok());
        assert_eq!(
            state.close_refusal().map(|refusal| refusal.workspace),
            Some("document")
        );
        drop(document);
        drop(artifact);
        assert_eq!(state.close_refusal(), None);
    }

    #[test]
    fn a_second_artifact_request_is_refused_as_busy_and_the_slot_is_released() {
        let state = AppState::new(Err(StartupError));
        let first = state.begin_artifact();
        assert!(first.is_ok());
        assert_eq!(
            state.begin_artifact().err(),
            Some(CommandError::busy(ARTIFACT_BUSY))
        );
        drop(first);
        assert!(state.begin_artifact().is_ok());
    }

    #[test]
    fn the_artifact_session_is_cleared_with_its_raster_on_a_failed_load_and_on_window_close() {
        let state = AppState::new(Err(StartupError));
        let session = ArtifactSession {
            artifact: Arc::new(DocumentArtifact {
                document: jobniac_core::documents::model::DocumentMetadata {
                    id: Uuid::nil(),
                    original_file_name: "Example.pdf".to_owned(),
                    media_type: "application/pdf".to_owned(),
                    byte_size: 1,
                    sha256: "0".repeat(64),
                    created_at: "2026-01-01T00:00:00Z".to_owned(),
                    updated_at: "2026-01-01T00:00:00Z".to_owned(),
                },
                content: vec![0x25],
            }),
            page_count: 3,
        };
        state.set_artifact_session(session.clone());
        assert_eq!(state.artifact_session(), Some(session.clone()));
        assert!(state.artifact_rasters().replace(vec![0x89], 1, 1).is_some());
        state.clear_artifact_view();
        assert_eq!(state.artifact_session(), None);
        assert!(state.artifact_rasters().current().is_none());

        state.set_artifact_session(session);
        assert!(state.artifact_rasters().replace(vec![0x89], 1, 1).is_some());
        state.release_document_work();
        assert!(state.artifact_session().is_some());
        assert!(state.artifact_rasters().current().is_some());
        state.clear_artifact_view();
        assert!(state.artifact_session().is_none());
    }

    #[test]
    fn a_document_release_leaves_the_artifact_page_loadable_and_the_reverse() {
        let state = AppState::new(Err(StartupError));
        let document = state.page_rasters().replace(vec![0x89, b'D'], 1, 1);
        let artifact = state.artifact_rasters().replace(vec![0x89, b'A'], 1, 1);
        let (Some(document), Some(artifact)) = (document, artifact) else {
            panic!("both rasters are replaced");
        };
        assert_ne!(document.handle, artifact.handle);
        state.release_document_work();
        assert!(state.page_rasters().current().is_none());
        assert_eq!(state.artifact_rasters().current(), Some(&artifact));
        // Another document preview replaces only the documents raster.
        assert!(state.page_rasters().replace(vec![0x89], 1, 1).is_some());
        assert_eq!(state.artifact_rasters().current(), Some(&artifact));
        // Clearing the artifact view leaves the documents raster in place.
        let documents_now = state.page_rasters().current().cloned();
        state.clear_artifact_view();
        assert!(state.artifact_rasters().current().is_none());
        assert_eq!(state.page_rasters().current().cloned(), documents_now);
    }

    #[test]
    fn releasing_the_document_work_drops_the_raster_even_without_services() {
        let state = AppState::new(Err(StartupError));
        let first = state.page_rasters().replace(vec![0x89, b'P'], 1, 1);
        assert!(first.is_some());
        assert!(state.page_rasters().current().is_some());
        state.release_document_work();
        assert!(state.page_rasters().current().is_none());
        // Releasing an empty store is a no-op.
        state.release_document_work();
        assert!(state.page_rasters().current().is_none());
    }

    #[test]
    fn a_close_refusal_serializes_with_the_event_field_names() -> Result<(), serde_json::Error> {
        let json = serde_json::to_value(CloseRefusal {
            workspace: "match",
            reason: "busy",
            message_id: MATCH_CLOSE,
        })?;
        assert_eq!(json["workspace"], "match");
        assert_eq!(json["messageId"], "matching.close");
        Ok(())
    }

    #[test]
    fn the_pending_deletion_debug_text_never_shows_the_token() {
        let state = AppState::new(Err(StartupError));
        let token = state.issue_deletion_token(DeleteKind::Profile, Uuid::nil(), Instant::now());
        let pending: Option<PendingDeletion> = lock(&state.pending_deletion).clone();
        let text = format!("{pending:?}");
        assert!(text.contains("PendingDeletion"));
        assert!(text.contains("<redacted>"));
        assert!(!text.contains(&token));
    }

    #[test]
    fn the_pending_replacement_and_selection_debug_text_never_shows_the_token_or_path() {
        let replacement = PendingReplacement {
            token: "replacement-tok".to_owned(),
            document_id: Uuid::nil(),
            existing_profile_id: Uuid::nil(),
            expected_revision: 1,
            max_characters: None,
        };
        let text = format!("{replacement:?}");
        assert!(text.contains("PendingReplacement") && text.contains("<redacted>"));
        assert!(!text.contains("replacement-tok"));
        let selection = PendingSelection {
            token: "selection-tok".to_owned(),
            path: PathBuf::from("/private/place/cv.pdf"),
            display_name: "cv.pdf".to_owned(),
        };
        let text = format!("{selection:?}");
        assert!(text.contains("cv.pdf"));
        assert!(!text.contains("selection-tok") && !text.contains("/private/place"));
    }

    #[test]
    fn a_reload_resets_the_reported_dirty_flags_but_not_the_slots() -> Result<(), CommandError> {
        let state = AppState::new(Err(StartupError));
        state.set_review_draft_dirty(true);
        state.set_job_draft_dirty(true);
        state.set_profile_draft_dirty(true);
        state.request_confirmed_close();
        let _job = state.begin_job()?;
        assert!(state.close_refusal().is_some());
        state.reset_webview_reports();
        // Only the running job still refuses; no draft report survives the reload.
        assert_eq!(
            state.close_refusal().map(|refusal| refusal.reason),
            Some("busy")
        );
        drop(_job);
        assert_eq!(state.close_refusal(), None);
        assert!(!state.take_confirmed_close());
        Ok(())
    }

    fn refusal(
        workspace: &'static str,
        reason: &'static str,
        message_id: &'static str,
    ) -> Option<CloseRefusal> {
        Some(CloseRefusal {
            workspace,
            reason,
            message_id,
        })
    }

    #[test]
    fn an_unsaved_job_refuses_with_the_job_copy_and_a_dirty_profile_asks_for_the_discard() {
        let state = AppState::new(Err(StartupError));
        state.set_job_draft_dirty(true);
        assert_eq!(
            state.close_refusal(),
            refusal("job", "refuse_unsaved", "job.validation.unsaved")
        );
        state.set_job_draft_dirty(false);
        state.set_profile_draft_dirty(true);
        assert_eq!(
            state.close_refusal(),
            refusal("profile", "confirm_discard", "profile.confirm.close")
        );
        state.set_profile_draft_dirty(false);
        assert_eq!(state.close_refusal(), None);
    }

    #[test]
    fn the_job_gate_order_is_busy_then_unsaved_then_the_profile_busy_then_the_profile_confirmation()
    {
        let state = AppState::new(Err(StartupError));
        state.set_profile_draft_dirty(true);
        state.set_job_draft_dirty(true);
        state.profile_in_flight.store(true, Ordering::Release);
        // A dirty job is reported before a busy profile: order 8 precedes order 9.
        assert_eq!(
            state.close_refusal(),
            refusal("job", "refuse_unsaved", "job.validation.unsaved")
        );
        state.set_job_draft_dirty(false);
        // The busy profile is reported before its own discard confirmation.
        assert_eq!(
            state.close_refusal(),
            refusal("profile", "busy", "profile.busy.close")
        );
        state.profile_in_flight.store(false, Ordering::Release);
        assert_eq!(
            state.close_refusal(),
            refusal("profile", "confirm_discard", "profile.confirm.close")
        );
    }

    #[test]
    fn a_busy_job_is_reported_before_its_unsaved_edits_and_after_a_dirty_review() {
        let state = AppState::new(Err(StartupError));
        state.job_in_flight.store(true, Ordering::Release);
        state.set_job_draft_dirty(true);
        assert_eq!(
            state.close_refusal(),
            refusal("job", "busy", "job.busy.close")
        );
        state.job_in_flight.store(false, Ordering::Release);
        state.set_review_draft_dirty(true);
        assert_eq!(
            state.close_refusal(),
            refusal("review", "refuse_unsaved", "reviews.close")
        );
    }

    #[test]
    fn a_confirmed_discard_bypasses_only_the_profile_confirmation() {
        let state = AppState::new(Err(StartupError));
        state.set_profile_draft_dirty(true);
        assert_eq!(state.close_refusal_with(true), None);

        state.set_job_draft_dirty(true);
        assert_eq!(
            state.close_refusal_with(true),
            refusal("job", "refuse_unsaved", "job.validation.unsaved")
        );
        state.set_job_draft_dirty(false);

        state.profile_in_flight.store(true, Ordering::Release);
        assert_eq!(
            state.close_refusal_with(true),
            refusal("profile", "busy", "profile.busy.close")
        );
        state.profile_in_flight.store(false, Ordering::Release);

        state.set_review_draft_dirty(true);
        assert_eq!(
            state.close_refusal_with(true),
            refusal("review", "refuse_unsaved", "reviews.close")
        );
    }

    #[test]
    fn a_confirmed_discard_mark_is_taken_once_by_a_close() {
        let state = AppState::new(Err(StartupError));
        assert!(!state.take_confirmed_close());
        state.request_confirmed_close();
        assert!(state.take_confirmed_close());
        assert!(!state.take_confirmed_close());
    }
}
