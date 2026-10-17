//! The stored-page render use case against in-memory ports: page index limits, the stored-bytes
//! checks, the page count the worker reports, and the render failure without a renderer.
//! All data is synthetic.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jobniac_core::clock::FixedClock;
use jobniac_core::documents::{
    DocumentFileReader, DocumentMetadata, DocumentRepository, DocumentService, ExtractionRecord,
    FileSnapshot, MAX_PAGE_COUNT, PdfExtractor, PdfRenderer, PdfText, RENDER_FAILURE_MESSAGE,
    RenderedPdfPage, sha256_hex,
};
use jobniac_core::errors::{AppError, ErrorCode, error_envelope};
use jobniac_core::profiles::repository::UnitOfWork;
use jobniac_core::transactions::TransactionOptions;
use time::macros::datetime;
use uuid::Uuid;

/// A unit of work whose transaction handle is the unit type.
#[derive(Debug, Clone, Copy)]
struct FakeUow;

impl UnitOfWork for FakeUow {
    type Tx = ();

    async fn transaction<T, F>(&self, _options: TransactionOptions, body: F) -> Result<T, AppError>
    where
        F: AsyncFnOnce(&mut ()) -> Result<T, AppError>,
    {
        let mut tx = ();
        body(&mut tx).await
    }
}

/// One stored file: its metadata and its bytes.
type StoredFile = (DocumentMetadata, Vec<u8>);

/// An in-memory repository holding the stored files; clones share the state.
#[derive(Debug, Clone, Default)]
struct MemRepo {
    files: Arc<Mutex<HashMap<Uuid, StoredFile>>>,
    content_reads: Arc<AtomicUsize>,
}

#[async_trait]
impl DocumentRepository for MemRepo {
    type Tx = ();

    async fn save_file(
        &self,
        _tx: &mut (),
        metadata: &DocumentMetadata,
        content: &[u8],
    ) -> Result<DocumentMetadata, AppError> {
        if let Ok(mut files) = self.files.lock() {
            files.insert(metadata.id, (metadata.clone(), content.to_vec()));
        }
        Ok(metadata.clone())
    }

    async fn find_metadata(
        &self,
        _tx: &mut (),
        document_id: Uuid,
    ) -> Result<Option<DocumentMetadata>, AppError> {
        Ok(self
            .files
            .lock()
            .ok()
            .and_then(|files| files.get(&document_id).map(|(meta, _)| meta.clone())))
    }

    async fn find_content(
        &self,
        _tx: &mut (),
        document_id: Uuid,
    ) -> Result<Option<(DocumentMetadata, Vec<u8>)>, AppError> {
        self.content_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .files
            .lock()
            .ok()
            .and_then(|files| files.get(&document_id).cloned()))
    }

    async fn find_extraction(
        &self,
        _tx: &mut (),
        _document_id: Uuid,
    ) -> Result<Option<ExtractionRecord>, AppError> {
        Ok(None)
    }

    async fn save_extraction(
        &self,
        _tx: &mut (),
        record: &ExtractionRecord,
    ) -> Result<ExtractionRecord, AppError> {
        Ok(record.clone())
    }
}

#[derive(Debug)]
struct UnusedReader;

#[async_trait]
impl DocumentFileReader for UnusedReader {
    async fn read(&self, _path: &str) -> Result<FileSnapshot, AppError> {
        Err(AppError::Internal)
    }
}

#[derive(Debug)]
struct UnusedExtractor;

#[async_trait]
impl PdfExtractor for UnusedExtractor {
    fn version(&self) -> &str {
        "test"
    }

    async fn extract(&self, _content: &[u8], _file_name: &str) -> Result<PdfText, AppError> {
        Err(AppError::Internal)
    }
}

/// A renderer that records the index it was asked for and reports a fixed page count.
#[derive(Debug)]
struct RecordingRenderer {
    page_count: u32,
    requested: Mutex<Vec<u32>>,
}

#[async_trait]
impl PdfRenderer for RecordingRenderer {
    async fn render(&self, content: &[u8], page_index: u32) -> Result<RenderedPdfPage, AppError> {
        if let Ok(mut requested) = self.requested.lock() {
            requested.push(page_index);
        }
        Ok(RenderedPdfPage {
            page_count: self.page_count,
            width: 10,
            height: 20,
            png: content.to_vec(),
        })
    }
}

type Service = DocumentService<FakeUow, MemRepo>;

fn service(repo: &MemRepo, renderer: Option<Arc<RecordingRenderer>>) -> Service {
    let base = DocumentService::new(
        FakeUow,
        repo.clone(),
        Arc::new(UnusedReader),
        Arc::new(UnusedExtractor),
        Arc::new(FixedClock::new(datetime!(2026-01-02 03:04:05 UTC))),
    );
    match renderer {
        Some(renderer) => base.with_renderer(renderer),
        None => base,
    }
}

fn pdf_bytes() -> Vec<u8> {
    b"%PDF-1.4\n% synthetic test document\n".to_vec()
}

/// Stores `content` under a fresh identifier with the metadata the storage would write.
fn store(repo: &MemRepo, content: &[u8], media_type: &str) -> Uuid {
    let id = Uuid::new_v4();
    let metadata = DocumentMetadata {
        id,
        original_file_name: "synthetic.pdf".to_owned(),
        media_type: media_type.to_owned(),
        byte_size: i64::try_from(content.len()).unwrap_or(i64::MAX),
        sha256: sha256_hex(content),
        created_at: "2026-01-02T03:04:05Z".to_owned(),
        updated_at: "2026-01-02T03:04:05Z".to_owned(),
    };
    if let Ok(mut files) = repo.files.lock() {
        files.insert(id, (metadata, content.to_vec()));
    }
    id
}

/// Appends one byte to the stored bytes of `id`, so that they no longer match the stored digest.
fn tamper(repo: &MemRepo, id: Uuid) {
    let _ = repo.files.lock().map(|mut files| {
        if let Some(stored) = files.get_mut(&id) {
            stored.1.push(b' ');
        }
    });
}

fn renderer(page_count: u32) -> Arc<RecordingRenderer> {
    Arc::new(RecordingRenderer {
        page_count,
        requested: Mutex::new(Vec::new()),
    })
}

fn envelope_of(error: &AppError) -> jobniac_core::errors::ErrorEnvelope {
    error_envelope(error)
}

#[tokio::test]
async fn a_page_renders_with_the_worker_result_and_the_stored_bytes() -> Result<(), AppError> {
    let repo = MemRepo::default();
    let content = pdf_bytes();
    let id = store(&repo, &content, "application/pdf");
    let worker = renderer(3);
    let rendered = service(&repo, Some(Arc::clone(&worker)))
        .render_stored_page(id, 2)
        .await?;
    assert_eq!(rendered.page_count, 3);
    assert_eq!((rendered.width, rendered.height), (10, 20));
    assert_eq!(rendered.png, content);
    assert_eq!(
        worker
            .requested
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default(),
        vec![2]
    );
    Ok(())
}

#[tokio::test]
async fn an_index_at_or_above_the_page_limit_is_refused_before_the_document_is_read() {
    let repo = MemRepo::default();
    let worker = renderer(1);
    let error = service(&repo, Some(Arc::clone(&worker)))
        .render_stored_page(Uuid::new_v4(), MAX_PAGE_COUNT)
        .await
        .err();
    let envelope = error.as_ref().map(envelope_of);
    assert_eq!(
        envelope.as_ref().map(|value| value.code),
        Some(ErrorCode::ValidationError)
    );
    assert_eq!(
        envelope
            .as_ref()
            .and_then(|value| value.details.get("field").cloned())
            .as_deref(),
        Some("pageIndex")
    );
    assert_eq!(worker.requested.lock().map(|v| v.len()).unwrap_or(1), 0);
}

#[tokio::test]
async fn an_index_the_worker_says_is_past_the_last_page_is_a_validation_error() {
    let repo = MemRepo::default();
    let id = store(&repo, &pdf_bytes(), "application/pdf");
    let error = service(&repo, Some(renderer(2)))
        .render_stored_page(id, 2)
        .await
        .err();
    assert_eq!(
        error.as_ref().map(|value| envelope_of(value).code),
        Some(ErrorCode::ValidationError)
    );
}

#[tokio::test]
async fn a_missing_document_is_not_found() {
    let repo = MemRepo::default();
    let error = service(&repo, Some(renderer(1)))
        .render_stored_page(Uuid::new_v4(), 0)
        .await
        .err();
    assert_eq!(
        error.as_ref().map(|value| envelope_of(value).code),
        Some(ErrorCode::NotFound)
    );
}

#[tokio::test]
async fn a_stored_file_that_is_not_a_pdf_media_type_is_refused() {
    let repo = MemRepo::default();
    let id = store(&repo, &pdf_bytes(), "text/plain");
    let error = service(&repo, Some(renderer(1)))
        .render_stored_page(id, 0)
        .await
        .err();
    assert_eq!(
        error.as_ref().map(|value| envelope_of(value).code),
        Some(ErrorCode::ValidationError)
    );
}

#[tokio::test]
async fn bytes_that_do_not_match_the_stored_digest_fail_the_integrity_check() {
    let repo = MemRepo::default();
    let id = store(&repo, &pdf_bytes(), "application/pdf");
    tamper(&repo, id);
    let worker = renderer(1);
    let error = service(&repo, Some(Arc::clone(&worker)))
        .render_stored_page(id, 0)
        .await
        .err();
    assert_eq!(
        error.as_ref().map(|value| envelope_of(value).code),
        Some(ErrorCode::InternalError)
    );
    assert_eq!(worker.requested.lock().map(|v| v.len()).unwrap_or(1), 0);
}

#[tokio::test]
async fn without_a_renderer_every_render_is_the_fixed_failure() {
    let repo = MemRepo::default();
    let id = store(&repo, &pdf_bytes(), "application/pdf");
    let error = service(&repo, None).render_stored_page(id, 0).await.err();
    let envelope = error.as_ref().map(envelope_of);
    assert_eq!(
        envelope
            .as_ref()
            .map(|value| value.message.clone())
            .as_deref(),
        Some(RENDER_FAILURE_MESSAGE)
    );
}

#[test]
fn a_page_index_must_be_below_a_count_of_one_to_two_hundred() {
    use jobniac_core::documents::page_index_in_range;
    assert!(page_index_in_range(0, 1));
    assert!(page_index_in_range(199, 200));
    assert!(!page_index_in_range(1, 1));
    assert!(!page_index_in_range(0, 0));
    assert!(!page_index_in_range(0, 201));
}

#[tokio::test]
async fn a_file_over_the_preview_limit_is_refused_before_its_content_is_read() {
    let repo = MemRepo::default();
    let mut content = pdf_bytes();
    content.resize(1024 * 1024 + 1, b' ');
    let id = store(&repo, &content, "application/pdf");
    let worker = renderer(1);
    let error = service(&repo, Some(Arc::clone(&worker)))
        .render_stored_page(id, 0)
        .await
        .err();
    assert_eq!(
        error.as_ref().map(|value| envelope_of(value).message),
        Some(RENDER_FAILURE_MESSAGE.to_owned())
    );
    assert_eq!(repo.content_reads.load(Ordering::SeqCst), 0);
    assert_eq!(worker.requested.lock().map(|v| v.len()).unwrap_or(1), 0);
}
