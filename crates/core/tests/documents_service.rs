//! Document use cases against in-memory ports.

use std::collections::HashMap;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jobniac_core::clock::FixedClock;
use jobniac_core::documents::{
    DocumentFileReader, DocumentMetadata, DocumentRepository, DocumentService, ExtractionRecord,
    FileSnapshot, PageText, PdfExtractor, PdfText, sha256_hex,
};
use jobniac_core::errors::{AppError, ErrorCode, error_envelope, validation_error};
use jobniac_core::profiles::repository::UnitOfWork;
use jobniac_core::transactions::TransactionOptions;
use serde_json::{Value, json};
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

/// The shared state of the in-memory repository.
#[derive(Debug, Default)]
struct State {
    files: HashMap<Uuid, (DocumentMetadata, Vec<u8>)>,
    extractions: HashMap<Uuid, ExtractionRecord>,
}

/// An in-memory repository. Clones share one state.
#[derive(Debug, Clone, Default)]
struct MemRepo {
    state: Arc<Mutex<State>>,
}

impl MemRepo {
    fn with_state<T>(&self, body: impl FnOnce(&mut State) -> T) -> Result<T, AppError> {
        let mut guard = self.state.lock().map_err(|_| AppError::Internal)?;
        Ok(body(&mut guard))
    }
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
        let stored = metadata.clone();
        self.with_state(|state| {
            state
                .files
                .insert(metadata.id, (stored.clone(), content.to_vec()));
        })?;
        Ok(metadata.clone())
    }

    async fn find_metadata(
        &self,
        _tx: &mut (),
        document_id: Uuid,
    ) -> Result<Option<DocumentMetadata>, AppError> {
        self.with_state(|state| state.files.get(&document_id).map(|(meta, _)| meta.clone()))
    }

    async fn find_content(
        &self,
        _tx: &mut (),
        document_id: Uuid,
    ) -> Result<Option<(DocumentMetadata, Vec<u8>)>, AppError> {
        self.with_state(|state| state.files.get(&document_id).cloned())
    }

    async fn find_extraction(
        &self,
        _tx: &mut (),
        document_id: Uuid,
    ) -> Result<Option<ExtractionRecord>, AppError> {
        self.with_state(|state| state.extractions.get(&document_id).cloned())
    }

    async fn save_extraction(
        &self,
        _tx: &mut (),
        record: &ExtractionRecord,
    ) -> Result<ExtractionRecord, AppError> {
        self.with_state(|state| {
            let id = state
                .extractions
                .get(&record.file_id)
                .map_or(record.id, |old| old.id);
            let mut saved = record.clone();
            saved.id = id;
            state.extractions.insert(record.file_id, saved.clone());
            saved
        })
    }
}

/// A reader that returns one fixed outcome: a snapshot, or the reason of a refusal.
#[derive(Debug)]
struct FixedReader {
    outcome: Result<FileSnapshot, &'static str>,
}

#[async_trait]
impl DocumentFileReader for FixedReader {
    async fn read(&self, _path: &str) -> Result<FileSnapshot, AppError> {
        match &self.outcome {
            Ok(snapshot) => Ok(snapshot.clone()),
            Err(reason) => Err(AppError::from(validation_error(
                "refused",
                Some("path"),
                Some(reason),
            ))),
        }
    }
}

/// An extractor that counts its calls and returns a fixed two-page text.
#[derive(Debug)]
struct CountingExtractor {
    version: &'static str,
    calls: AtomicUsize,
    names: Mutex<Vec<String>>,
}

impl CountingExtractor {
    fn new(version: &'static str) -> Arc<Self> {
        Arc::new(Self {
            version,
            calls: AtomicUsize::new(0),
            names: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn names(&self) -> Vec<String> {
        self.names
            .lock()
            .map(|names| names.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl PdfExtractor for CountingExtractor {
    fn version(&self) -> &str {
        self.version
    }

    async fn extract(&self, _content: &[u8], file_name: &str) -> Result<PdfText, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut names) = self.names.lock() {
            names.push(file_name.to_owned());
        }
        let text = "Alpha\n\nBeta".to_owned();
        Ok(PdfText {
            file_name: "stored.pdf".to_owned(),
            page_count: 2,
            character_count: i64::try_from(text.encode_utf16().count()).unwrap_or(i64::MAX),
            truncated: false,
            text,
            pages: vec![
                PageText {
                    page_number: 1,
                    text: "Alpha".to_owned(),
                },
                PageText {
                    page_number: 2,
                    text: "Beta".to_owned(),
                },
            ],
        })
    }
}

type Service = DocumentService<FakeUow, MemRepo>;

fn service(repo: &MemRepo, reader: FixedReader, extractor: &Arc<CountingExtractor>) -> Service {
    DocumentService::new(
        FakeUow,
        repo.clone(),
        Arc::new(reader),
        Arc::clone(extractor) as Arc<dyn PdfExtractor>,
        Arc::new(FixedClock::new(datetime!(2026-01-02 03:04:05 UTC))),
    )
}

fn pdf_bytes() -> Vec<u8> {
    b"%PDF-1.4\n% synthetic test document\n".to_vec()
}

fn readable(name: &str) -> FixedReader {
    FixedReader {
        outcome: Ok(FileSnapshot {
            file_name: name.to_owned(),
            content: pdf_bytes(),
        }),
    }
}

fn refused(reason: &'static str) -> FixedReader {
    FixedReader {
        outcome: Err(reason),
    }
}

/// The `reason` detail of an error envelope, if any.
fn reason_of(error: &AppError) -> Option<String> {
    error_envelope(error).details.get("reason").cloned()
}

fn code_of(error: &AppError) -> ErrorCode {
    error_envelope(error).code
}

#[tokio::test]
async fn store_keeps_the_safe_name_the_size_and_the_digest() -> Result<(), AppError> {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&repo, readable(r"C:\imports\cv.pdf"), &extractor);
    let stored = svc
        .store_document_file(&json!({"path": r"C:\imports\cv.pdf"}))
        .await?;
    assert_eq!(stored.original_file_name, "cv.pdf");
    assert_eq!(stored.media_type, "application/pdf");
    assert_eq!(
        stored.byte_size,
        i64::try_from(pdf_bytes().len()).unwrap_or(0)
    );
    assert_eq!(stored.sha256, sha256_hex(&pdf_bytes()));
    assert_eq!(stored.created_at, "2026-01-02T03:04:05Z");
    assert_eq!(stored.updated_at, stored.created_at);
    let read_back = svc.get_document_metadata(&stored.id.to_string()).await?;
    assert_eq!(read_back, stored);
    Ok(())
}

#[tokio::test]
async fn store_maps_a_relative_marker_name_to_the_fallback_name() -> Result<(), AppError> {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&repo, readable("a/.."), &extractor);
    let stored = svc.store_document_file(&json!({"path": "a/.."})).await?;
    assert_eq!(stored.original_file_name, "stored.pdf");
    Ok(())
}

#[tokio::test]
async fn extract_passes_the_fallback_name_for_a_relative_marker_name() -> Result<(), AppError> {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&repo, readable("a/.."), &extractor);
    svc.extract_pdf_text(&json!({"path": "a/.."})).await?;
    assert_eq!(extractor.names(), vec!["stored.pdf".to_owned()]);
    Ok(())
}

#[tokio::test]
async fn store_validates_the_media_type_before_reading_the_file() {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&repo, refused("never read"), &extractor);
    let error = svc
        .store_document_file(&json!({"path": "x.pdf", "mediaType": "text/plain"}))
        .await
        .err();
    let error = error.as_ref();
    assert_eq!(
        error.map(|e| e.to_string()).as_deref(),
        Some("Invalid document storage request")
    );
    assert_eq!(
        reason_of(error.unwrap_or(&AppError::Internal)).as_deref(),
        Some("must be application/pdf")
    );
}

#[tokio::test]
async fn store_accepts_a_blank_media_type_as_pdf() -> Result<(), AppError> {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&repo, readable("cv.pdf"), &extractor);
    let stored = svc
        .store_document_file(&json!({"path": "cv.pdf", "mediaType": "   "}))
        .await?;
    assert_eq!(stored.media_type, "application/pdf");
    Ok(())
}

#[tokio::test]
async fn store_shortens_the_size_reason_and_keeps_the_refusal_text() {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(
        &repo,
        refused("PDF file size exceeds limit of 10485760 bytes"),
        &extractor,
    );
    let error = svc
        .store_document_file(&json!({"path": "big.pdf"}))
        .await
        .err();
    let reason = error.as_ref().and_then(reason_of);
    assert_eq!(
        reason.as_deref(),
        Some("file size exceeds limit of 10485760 bytes")
    );

    let svc = service(&repo, refused("file was not found"), &extractor);
    let error = svc
        .store_document_file(&json!({"path": "missing.pdf"}))
        .await
        .err();
    assert_eq!(
        error.as_ref().and_then(reason_of).as_deref(),
        Some("file was not found")
    );
}

#[tokio::test]
async fn store_rejects_empty_and_non_pdf_content_with_the_storage_message() {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let empty = FixedReader {
        outcome: Ok(FileSnapshot {
            file_name: "empty.pdf".to_owned(),
            content: Vec::new(),
        }),
    };
    let svc = service(&repo, empty, &extractor);
    let error = svc
        .store_document_file(&json!({"path": "empty.pdf"}))
        .await
        .err();
    assert_eq!(
        error.as_ref().and_then(reason_of).as_deref(),
        Some("file must not be empty")
    );

    let text = FixedReader {
        outcome: Ok(FileSnapshot {
            file_name: "notes.txt".to_owned(),
            content: b"plain text".to_vec(),
        }),
    };
    let svc = service(&repo, text, &extractor);
    let error = svc
        .store_document_file(&json!({"path": "notes.txt"}))
        .await
        .err();
    assert_eq!(
        error.as_ref().and_then(reason_of).as_deref(),
        Some("file must be a PDF document")
    );
}

#[tokio::test]
async fn metadata_rejects_a_malformed_identifier_and_reports_a_missing_document() {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&repo, readable("cv.pdf"), &extractor);
    let malformed = svc.get_document_metadata("not-a-uuid").await.err();
    assert_eq!(
        malformed.as_ref().map(code_of),
        Some(ErrorCode::ValidationError)
    );
    assert_eq!(
        malformed.as_ref().and_then(reason_of).as_deref(),
        Some("must be a valid UUID")
    );

    let missing = svc
        .get_document_metadata(&Uuid::nil().to_string())
        .await
        .err();
    assert_eq!(missing.as_ref().map(code_of), Some(ErrorCode::NotFound));
    assert_eq!(
        missing.as_ref().map(|e| e.to_string()).as_deref(),
        Some("Stored document was not found")
    );
}

#[tokio::test]
async fn import_extraction_applies_the_display_budget_without_storing() -> Result<(), AppError> {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&repo, readable("cv.pdf"), &extractor);
    let view = svc
        .extract_pdf_text(&json!({"path": "cv.pdf", "maxCharacters": 3}))
        .await?;
    assert_eq!(view.text, "Alp");
    assert!(view.truncated);
    assert_eq!(view.character_count, 11);
    assert_eq!(view.pages.len(), 1);
    assert_eq!(view.pages[0].text, "Alp");
    let no_pages = svc
        .extract_pdf_text(&json!({"path": "cv.pdf", "includePages": false}))
        .await?;
    assert!(no_pages.pages.is_empty());
    assert!(!no_pages.truncated);
    assert_eq!(extractor.calls(), 2);
    Ok(())
}

#[tokio::test]
async fn a_path_of_unicode_whitespace_only_is_blank() {
    // U+001C to U+001F count as whitespace here, although `str::trim` keeps them.
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&repo, readable("cv.pdf"), &extractor);
    for path in ["\u{1f}", "\u{1c}", " \u{1d}\t\u{1e}"] {
        let error = svc.extract_pdf_text(&json!({"path": path})).await.err();
        assert_eq!(
            error.as_ref().and_then(reason_of).as_deref(),
            Some("must not be blank"),
            "path {path:?}"
        );
    }
    assert_eq!(extractor.calls(), 0);
}

#[tokio::test]
async fn extraction_options_are_checked_in_the_documented_order() {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&repo, readable("cv.pdf"), &extractor);
    let cases = [
        (
            json!({"path": "cv.pdf", "maxCharacters": 0}),
            "must be greater than zero",
        ),
        (
            json!({"path": "cv.pdf", "maxCharacters": 250_001}),
            "must be less than or equal to 250000",
        ),
        (
            json!({"path": "cv.pdf", "maxCharacters": "5"}),
            "must be an integer",
        ),
        (
            json!({"path": "cv.pdf", "maxCharacters": 5, "includePages": "yes"}),
            "must be a boolean",
        ),
    ];
    for (request, reason) in cases {
        let error = svc.extract_pdf_text(&request).await.err();
        assert_eq!(error.as_ref().and_then(reason_of).as_deref(), Some(reason));
    }
    assert_eq!(
        extractor.calls(),
        0,
        "no extraction runs for a rejected request"
    );
}

#[tokio::test]
async fn stored_extraction_caches_when_the_version_matches() -> Result<(), AppError> {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&repo, readable("cv.pdf"), &extractor);
    let stored = svc.store_document_file(&json!({"path": "cv.pdf"})).await?;
    let document_id = stored.id.to_string();

    let first = svc
        .extract_stored_pdf_text(&json!({"documentId": document_id, "persistExtraction": true}))
        .await?;
    let extraction_id = first.extraction_id.ok_or(AppError::Internal)?;
    assert_eq!(extractor.calls(), 1);

    let second = svc
        .extract_stored_pdf_text(&json!({"documentId": document_id, "persistExtraction": true}))
        .await?;
    assert_eq!(second.extraction_id, Some(extraction_id));
    assert_eq!(
        extractor.calls(),
        1,
        "a matching cached extraction is not recomputed"
    );

    let not_persisted = svc
        .extract_stored_pdf_text(&json!({"documentId": document_id}))
        .await?;
    assert_eq!(not_persisted.extraction_id, None);
    assert_eq!(
        extractor.calls(),
        2,
        "without persistExtraction nothing is read from the cache"
    );
    Ok(())
}

#[tokio::test]
async fn a_different_extractor_version_recomputes_and_keeps_the_identifier() -> Result<(), AppError>
{
    let repo = MemRepo::default();
    let old = CountingExtractor::new("v1");
    let writer = service(&repo, readable("cv.pdf"), &old);
    let stored = writer
        .store_document_file(&json!({"path": "cv.pdf"}))
        .await?;
    let document_id = stored.id.to_string();
    let first = writer
        .extract_stored_pdf_text(&json!({"documentId": document_id, "persistExtraction": true}))
        .await?;
    let extraction_id = first.extraction_id;

    let new = CountingExtractor::new("v2");
    let reader = service(&repo, readable("cv.pdf"), &new);
    let refreshed = reader
        .extract_stored_pdf_text(&json!({"documentId": document_id, "persistExtraction": true}))
        .await?;
    assert_eq!(new.calls(), 1, "the other version is recomputed");
    assert_eq!(
        refreshed.extraction_id, extraction_id,
        "the refresh keeps the identifier"
    );

    let again = reader
        .extract_stored_pdf_text(&json!({"documentId": document_id, "persistExtraction": true}))
        .await?;
    assert_eq!(
        new.calls(),
        1,
        "the refreshed record now matches the version"
    );
    assert_eq!(again.extraction_id, extraction_id);
    Ok(())
}

#[tokio::test]
async fn stored_extraction_verifies_the_bytes_before_any_cache_use() -> Result<(), AppError> {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&repo, readable("cv.pdf"), &extractor);
    let stored = svc.store_document_file(&json!({"path": "cv.pdf"})).await?;
    repo.with_state(|state| {
        if let Some(entry) = state.files.get_mut(&stored.id) {
            entry.1 = b"%PDF-1.4\n% tampered\n".to_vec();
        }
    })?;
    let error = svc
        .extract_stored_pdf_text(
            &json!({"documentId": stored.id.to_string(), "persistExtraction": true}),
        )
        .await
        .err();
    assert_eq!(error.as_ref().map(code_of), Some(ErrorCode::InternalError));
    assert_eq!(
        error.as_ref().map(|e| e.to_string()).as_deref(),
        Some("Stored document integrity check failed")
    );
    assert_eq!(extractor.calls(), 0);
    Ok(())
}

#[tokio::test]
async fn stored_extraction_of_a_missing_document_is_not_found() {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&repo, readable("cv.pdf"), &extractor);
    let error = svc
        .extract_stored_pdf_text(&json!({"documentId": Uuid::nil().to_string()}))
        .await
        .err();
    assert_eq!(error.as_ref().map(code_of), Some(ErrorCode::NotFound));
}

#[tokio::test]
async fn a_stored_document_identifier_must_be_a_string() {
    let repo = MemRepo::default();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&repo, readable("cv.pdf"), &extractor);
    let error = svc
        .extract_stored_pdf_text(&json!({"documentId": 7}))
        .await
        .err();
    assert_eq!(
        error.as_ref().and_then(reason_of).as_deref(),
        Some("must be a valid UUID")
    );
    let request: Value = json!(null);
    let error = svc.store_document_file(&request).await.err();
    assert_eq!(
        error.as_ref().and_then(reason_of).as_deref(),
        Some("must not be null")
    );
}
