//! Process adapters of the document ports: the import-root reader and the PDF
//! worker client. Both are built once at startup from the environment snapshot and hold no
//! document state.
//!
//! * [`ImportRootReader`] reads `JOBNIAC_DOCUMENT_IMPORT_ROOT` through the safe file layer. Every
//!   refusal becomes a validation error whose `reason` is the caller-visible text.
//! * [`WorkerPdfExtractor`] extracts text through the `pdf-extract` worker with the canonical
//!   limit. It needs a worker supervisor and an absolute `JOBNIAC_PDFIUM_DIR`; without them every
//!   extraction fails with the fixed extraction message.

use std::future::{Future, pending};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::documents::{
    DocumentFileReader, FileSnapshot, MAX_CHARACTERS_LIMIT, MAX_PAGE_COUNT, PageText, PdfExtractor,
    PdfRenderer, PdfText, RENDER_FAILURE_MESSAGE, RenderedPdfPage,
};
use jobniac_core::errors::{AppError, ApplicationError, ErrorCode, validation_error};
use jobniac_infra::files::import_root::{ImportRefusal, ImportRoot, read_picked};
use jobniac_infra::workers::WorkerSupervisor;
use jobniac_infra::workers::pdf::{
    self, EXTRACT_FAILURE_MESSAGE, EXTRACTION_VALIDATION_MESSAGE, EXTRACTOR_VERSION,
    ExtractionOptions, PdfError, PdfExtraction,
};
use tokio::sync::watch;

/// The one file the desktop user picked in the native file dialog and that may be read without an
/// import root. Only Rust code that holds the picker result puts a path in it ([`Self::grant`]);
/// no IPC string and no MCP argument ever does. It holds at most one path, and the desktop state
/// revokes it when the selection is replaced, spent or reset.
#[derive(Debug, Clone, Default)]
pub struct PickedFileGrant {
    path: Arc<Mutex<Option<String>>>,
}

impl PickedFileGrant {
    /// A grant that admits nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Admits exactly `path` (a picker result), replacing any earlier grant.
    pub fn grant(&self, path: &str) {
        *self.path.lock().unwrap_or_else(PoisonError::into_inner) = Some(path.to_owned());
    }

    /// Admits nothing.
    pub fn revoke(&self) {
        *self.path.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn admits(&self, path: &str) -> bool {
        self.path
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_deref()
            == Some(path)
    }
}

/// Reads files from the configured document import root.
///
/// With no root configured, the reader also reads the file in its [`PickedFileGrant`] (the desktop
/// app's native-dialog choice) through the same safe file layer, without the containment rule. With
/// a root configured the root always applies, picked or not.
#[derive(Debug, Clone)]
pub struct ImportRootReader {
    root: ImportRoot,
    root_configured: bool,
    picked: PickedFileGrant,
}

impl ImportRootReader {
    /// A reader for `root`; `None` or an empty value refuses every read as not configured.
    #[must_use]
    pub fn new(root: Option<String>) -> Self {
        Self::with_picked(root, PickedFileGrant::new())
    }

    /// A reader for `root` that also admits the file of `picked` while no root is configured.
    #[must_use]
    pub fn with_picked(root: Option<String>, picked: PickedFileGrant) -> Self {
        Self {
            root_configured: root.as_deref().is_some_and(|text| !text.is_empty()),
            root: ImportRoot::new(root),
            picked,
        }
    }
}

#[async_trait]
impl DocumentFileReader for ImportRootReader {
    async fn read(&self, path: &str) -> Result<FileSnapshot, AppError> {
        // The read is blocking file IO (up to the import limit): it runs on the blocking pool so
        // that no async worker thread waits on the disk.
        let root = self.root.clone();
        let picked = !self.root_configured && self.picked.admits(path);
        let path = path.to_owned();
        let outcome = tokio::task::spawn_blocking(move || {
            if picked {
                read_picked(&path)
            } else {
                root.read(&path)
            }
        })
        .await
        .map_err(|_| {
            AppError::from(ApplicationError::with_message(
                ErrorCode::InternalError,
                "Document import failed",
            ))
        })?;
        match outcome {
            Ok(file) => Ok(FileSnapshot {
                file_name: file.file_name,
                content: file.content,
            }),
            Err(refusal) => Err(refusal_error(refusal)),
        }
    }
}

/// The validation error of an import refusal. The service replaces the message with the request's
/// own message and keeps only the `reason` detail.
fn refusal_error(refusal: ImportRefusal) -> AppError {
    AppError::from(validation_error(
        EXTRACTION_VALIDATION_MESSAGE,
        Some("path"),
        Some(refusal.reason()),
    ))
}

/// The cancellation of the in-flight PDF worker calls of one process.
///
/// Every worker call made through [`WorkerPdfExtractor`] or [`WorkerPdfRenderer`] that shares one
/// value takes a signal when it starts. [`WorkerCancellation::cancel_in_flight`] resolves the
/// signals taken before it, so the supervisor kills the child tree and the call fails as cancelled.
/// A call that starts after the cancellation is not affected.
#[derive(Debug, Clone)]
pub struct WorkerCancellation {
    generation: Arc<watch::Sender<u64>>,
}

impl Default for WorkerCancellation {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerCancellation {
    /// A cancellation with no call in flight.
    #[must_use]
    pub fn new() -> Self {
        let (generation, _) = watch::channel(0_u64);
        Self {
            generation: Arc::new(generation),
        }
    }

    /// Cancels every worker call that is in flight now (window reset, shutdown).
    pub fn cancel_in_flight(&self) {
        self.generation
            .send_modify(|value| *value = value.wrapping_add(1));
    }

    /// The signal of one worker call: it resolves when [`Self::cancel_in_flight`] runs after the
    /// call takes it. The signal never resolves on its own.
    pub fn signal(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut receiver = self.generation.subscribe();
        let start = *receiver.borrow_and_update();
        async move {
            loop {
                if receiver.changed().await.is_err() {
                    // The sender is gone only when the process is being torn down; the call then
                    // stays uncancelled rather than cancelling by accident.
                    pending::<()>().await;
                }
                if *receiver.borrow_and_update() != start {
                    return;
                }
            }
        }
    }
}

/// Extracts canonical PDF text through the PDF worker.
#[derive(Debug)]
pub struct WorkerPdfExtractor {
    supervisor: Option<Arc<WorkerSupervisor>>,
    pdfium_dir: Option<PathBuf>,
    cancellation: WorkerCancellation,
}

impl WorkerPdfExtractor {
    /// An extractor over an optional supervisor and an optional PDFium directory, with its own
    /// cancellation. Use [`Self::shared`] to share the supervisor and the cancellation with the
    /// renderer.
    #[must_use]
    pub fn new(supervisor: Option<WorkerSupervisor>, pdfium_dir: Option<PathBuf>) -> Self {
        Self::shared(
            supervisor.map(Arc::new),
            pdfium_dir,
            WorkerCancellation::new(),
        )
    }

    /// An extractor over a supervisor shared with other worker adapters, cancelled through
    /// `cancellation`. Sharing the supervisor keeps the per-kind concurrency bound.
    #[must_use]
    pub fn shared(
        supervisor: Option<Arc<WorkerSupervisor>>,
        pdfium_dir: Option<PathBuf>,
        cancellation: WorkerCancellation,
    ) -> Self {
        Self {
            supervisor,
            pdfium_dir,
            cancellation,
        }
    }
}

#[async_trait]
impl PdfExtractor for WorkerPdfExtractor {
    fn version(&self) -> &str {
        EXTRACTOR_VERSION
    }

    async fn extract(&self, content: &[u8], file_name: &str) -> Result<PdfText, AppError> {
        let (Some(supervisor), Some(pdfium_dir)) = (&self.supervisor, &self.pdfium_dir) else {
            return Err(extraction_failure());
        };
        let options = ExtractionOptions {
            max_characters: MAX_CHARACTERS_LIMIT,
            include_pages: true,
        };
        let extracted = pdf::extract_text(
            supervisor,
            pdfium_dir,
            content,
            file_name,
            options,
            self.cancellation.signal(),
        )
        .await
        .map_err(pdf_error)?;
        pdf_text(extracted)
    }
}

/// Renders one page of PDF bytes to PNG through the PDF worker.
#[derive(Debug)]
pub struct WorkerPdfRenderer {
    supervisor: Option<Arc<WorkerSupervisor>>,
    pdfium_dir: Option<PathBuf>,
    cancellation: WorkerCancellation,
}

impl WorkerPdfRenderer {
    /// A renderer over an optional supervisor shared with the extractor and an optional PDFium
    /// directory, cancelled through `cancellation`.
    #[must_use]
    pub const fn new(
        supervisor: Option<Arc<WorkerSupervisor>>,
        pdfium_dir: Option<PathBuf>,
        cancellation: WorkerCancellation,
    ) -> Self {
        Self {
            supervisor,
            pdfium_dir,
            cancellation,
        }
    }
}

#[async_trait]
impl PdfRenderer for WorkerPdfRenderer {
    async fn render(&self, content: &[u8], page_index: u32) -> Result<RenderedPdfPage, AppError> {
        let (Some(supervisor), Some(pdfium_dir)) = (&self.supervisor, &self.pdfium_dir) else {
            return Err(render_failure());
        };
        let rendered = pdf::render_page(
            supervisor,
            pdfium_dir,
            content,
            page_index,
            self.cancellation.signal(),
        )
        .await
        .map_err(|_| render_failure())?;
        Ok(RenderedPdfPage {
            page_count: rendered.page_count,
            width: rendered.width,
            height: rendered.height,
            png: rendered.png,
        })
    }
}

/// The render failure of a page that cannot be shown: the fixed preview message, on the
/// `documentId` field. Every worker failure, cancellation included, maps here.
fn render_failure() -> AppError {
    AppError::from(validation_error(
        RENDER_FAILURE_MESSAGE,
        Some("documentId"),
        Some(RENDER_FAILURE_MESSAGE),
    ))
}

/// The validation error of a failed extraction: the fixed message on the
/// `path` field.
fn extraction_failure() -> AppError {
    AppError::from(validation_error(
        EXTRACT_FAILURE_MESSAGE,
        Some("path"),
        Some(EXTRACT_FAILURE_MESSAGE),
    ))
}

/// Maps a worker outcome to the document error.
fn pdf_error(error: PdfError) -> AppError {
    match error {
        PdfError::Validation { field, reason } => AppError::from(validation_error(
            EXTRACTION_VALIDATION_MESSAGE,
            Some(field),
            Some(&reason),
        )),
        PdfError::PageLimit => AppError::from(validation_error(
            EXTRACTION_VALIDATION_MESSAGE,
            Some("path"),
            Some(&format!("PDF page count exceeds limit of {MAX_PAGE_COUNT}")),
        )),
        PdfError::Extraction | PdfError::Render | PdfError::Cancelled { .. } => {
            extraction_failure()
        }
    }
}

/// Converts the worker's extraction into the core value. Sizes beyond the core range cannot occur
/// after the worker's own limits; they fail closed as an extraction failure.
fn pdf_text(extracted: PdfExtraction) -> Result<PdfText, AppError> {
    let character_count =
        i64::try_from(extracted.character_count).map_err(|_| extraction_failure())?;
    Ok(PdfText {
        file_name: extracted.file_name,
        page_count: extracted.page_count,
        character_count,
        truncated: extracted.truncated,
        text: extracted.text,
        pages: extracted
            .pages
            .into_iter()
            .map(|page| PageText {
                page_number: page.page_number,
                text: page.text,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use jobniac_core::errors::error_envelope;
    use jobniac_infra::workers::pdf::PdfPage;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn an_unconfigured_import_root_refuses_with_the_expected_reason() {
        let reader = ImportRootReader::new(None);
        let error = reader.read(r"C:\imports\cv.pdf").await.err();
        let reason = error
            .as_ref()
            .and_then(|error| error_envelope(error).details.get("reason").cloned());
        assert_eq!(
            reason.as_deref(),
            Some("document import root is not configured")
        );
        let empty = ImportRootReader::new(Some(String::new()))
            .read("x.pdf")
            .await
            .err();
        assert_eq!(
            empty
                .as_ref()
                .and_then(|error| error_envelope(error).details.get("reason").cloned())
                .as_deref(),
            Some("document import root is not configured")
        );
    }

    #[tokio::test]
    async fn an_extractor_without_a_worker_or_pdfium_fails_with_the_fixed_message() {
        let extractor = WorkerPdfExtractor::new(None, None);
        let error = extractor.extract(b"%PDF-1.4\n", "cv.pdf").await.err();
        let envelope = error.as_ref().map(error_envelope);
        assert_eq!(
            envelope
                .as_ref()
                .map(|value| value.message.clone())
                .as_deref(),
            Some(EXTRACT_FAILURE_MESSAGE)
        );
        assert_eq!(
            envelope
                .as_ref()
                .and_then(|value| value.details.get("field").cloned())
                .as_deref(),
            Some("path")
        );
    }

    #[test]
    fn worker_page_limits_and_failures_map_to_the_expected_envelopes() {
        let limit = error_envelope(&pdf_error(PdfError::PageLimit));
        assert_eq!(limit.message, EXTRACTION_VALIDATION_MESSAGE);
        assert_eq!(
            limit.details.get("reason").map(String::as_str),
            Some("PDF page count exceeds limit of 200")
        );
        let failure = error_envelope(&pdf_error(PdfError::Extraction));
        assert_eq!(failure.message, EXTRACT_FAILURE_MESSAGE);
        assert_eq!(failure.code.code(), "validation_error");
    }

    #[tokio::test]
    async fn a_cancellation_resolves_only_the_signals_taken_before_it() {
        let cancellation = WorkerCancellation::new();
        let before = cancellation.signal();
        cancellation.cancel_in_flight();
        let resolved = tokio::time::timeout(Duration::from_secs(1), before).await;
        assert!(
            resolved.is_ok(),
            "a signal taken before the cancel resolves"
        );
        let after = cancellation.signal();
        let pending_still = tokio::time::timeout(Duration::from_millis(50), after).await;
        assert!(pending_still.is_err(), "a later call is not cancelled");
    }

    #[tokio::test]
    async fn an_unconfigured_renderer_refuses_with_the_fixed_render_message() {
        let renderer = WorkerPdfRenderer::new(None, None, WorkerCancellation::new());
        let error = renderer.render(b"%PDF-1.4\n", 0).await.err();
        let envelope = error.as_ref().map(error_envelope);
        assert_eq!(
            envelope
                .as_ref()
                .map(|value| value.message.clone())
                .as_deref(),
            Some(RENDER_FAILURE_MESSAGE)
        );
        assert_eq!(
            envelope
                .as_ref()
                .and_then(|value| value.details.get("field").cloned())
                .as_deref(),
            Some("documentId")
        );
    }

    #[test]
    fn worker_text_keeps_the_canonical_counts_and_pages() {
        let converted = pdf_text(PdfExtraction {
            file_name: "cv.pdf".to_owned(),
            page_count: 1,
            character_count: 5,
            truncated: false,
            text: "Alpha".to_owned(),
            pages: vec![PdfPage {
                page_number: 1,
                text: "Alpha".to_owned(),
            }],
        });
        let text = converted.ok();
        assert_eq!(text.as_ref().map(|value| value.character_count), Some(5));
        assert_eq!(text.as_ref().map(|value| value.pages.len()), Some(1));
    }
}
