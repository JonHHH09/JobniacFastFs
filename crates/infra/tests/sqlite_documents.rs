//! Stored documents and the extraction cache against a disposable SQLite database.
//!
//! The tests drive the real document use cases through the SQLite repository, with in-memory file
//! and extractor fakes. They cover SHA-256 deduplication, concurrent imports, metadata reads, the
//! cache hit, miss and recompute paths, and the unconfigured database. All data is synthetic.

mod sqlite_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use jobniac_core::clock::SystemClock;
use jobniac_core::documents::{
    DocumentFileReader, DocumentService, FileSnapshot, PageText, PdfExtractor, PdfText,
};
use jobniac_core::errors::{AppError, ErrorCode, error_envelope, validation_error};
use jobniac_infra::sqlite::documents::SqliteDocumentRepository;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use serde_json::json;
use sqlite_support::{SqliteFixture, TestResult};
use uuid::Uuid;

type Service = DocumentService<SqliteUnitOfWork, SqliteDocumentRepository>;

/// A migrated database file and the unit of work over it.
struct Fixture {
    database: SqliteFixture,
    uow: SqliteUnitOfWork,
}

/// A fresh database migrated to the packaged schema.
fn start() -> Fixture {
    let database = SqliteFixture::start();
    let uow = database.unit_of_work();
    Fixture { database, uow }
}

/// A reader that returns one fixed synthetic file.
#[derive(Debug)]
struct FixedReader {
    file_name: String,
    content: Vec<u8>,
}

#[async_trait]
impl DocumentFileReader for FixedReader {
    async fn read(&self, _path: &str) -> Result<FileSnapshot, AppError> {
        Ok(FileSnapshot {
            file_name: self.file_name.clone(),
            content: self.content.clone(),
        })
    }
}

/// An extractor that counts its calls and returns a fixed two-page synthetic text.
#[derive(Debug)]
struct CountingExtractor {
    version: &'static str,
    calls: AtomicUsize,
}

impl CountingExtractor {
    fn new(version: &'static str) -> Arc<Self> {
        Arc::new(Self {
            version,
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl PdfExtractor for CountingExtractor {
    fn version(&self) -> &str {
        self.version
    }

    async fn extract(&self, _content: &[u8], _file_name: &str) -> Result<PdfText, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = "Synthetic page one\n\nSynthetic page two".to_owned();
        Ok(PdfText {
            file_name: "stored.pdf".to_owned(),
            page_count: 2,
            character_count: i64::try_from(text.encode_utf16().count()).unwrap_or(i64::MAX),
            truncated: false,
            text,
            pages: vec![
                PageText {
                    page_number: 1,
                    text: "Synthetic page one".to_owned(),
                },
                PageText {
                    page_number: 2,
                    text: "Synthetic page two".to_owned(),
                },
            ],
        })
    }
}

/// The synthetic PDF used by the import tests.
fn synthetic_pdf(marker: &str) -> Vec<u8> {
    format!("%PDF-1.4\n% synthetic {marker}\n").into_bytes()
}

fn service(
    uow: &SqliteUnitOfWork,
    content: Vec<u8>,
    extractor: &Arc<CountingExtractor>,
) -> Service {
    DocumentService::new(
        uow.clone(),
        SqliteDocumentRepository,
        Arc::new(FixedReader {
            file_name: "synthetic-cv.pdf".to_owned(),
            content,
        }),
        Arc::clone(extractor) as Arc<dyn PdfExtractor>,
        Arc::new(SystemClock),
    )
}

/// The count of one table of the document schema, read through a raw connection.
fn count(fixture: &Fixture, table: &str) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    Ok(fixture.database.connection().query_row(
        &format!("SELECT count(*) FROM document_{table}"),
        [],
        |row| row.get(0),
    )?)
}

#[tokio::test]
async fn the_unconfigured_database_fails_every_document_call_with_the_unavailable_message()
-> TestResult {
    let uow = SqliteUnitOfWork::new(SqlitePool::unconfigured());
    let extractor = CountingExtractor::new("v1");
    let svc = service(&uow, synthetic_pdf("unconfigured"), &extractor);
    let stored = svc
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await
        .err()
        .map(|error| error_envelope(&error).message);
    assert_eq!(stored.as_deref(), Some("Local database is not configured"));
    let metadata = svc
        .get_document_metadata(&Uuid::nil().to_string())
        .await
        .err()
        .map(|error| error_envelope(&error).message);
    assert_eq!(
        metadata.as_deref(),
        Some("Local database is not configured")
    );
    assert_eq!(extractor.calls(), 0);
    Ok(())
}

#[tokio::test]
async fn two_imports_of_the_same_bytes_share_one_blob_and_keep_two_identities() -> TestResult {
    let fixture = start();
    let extractor = CountingExtractor::new("v1");
    let content = synthetic_pdf("dedupe");
    let svc = service(&fixture.uow, content.clone(), &extractor);
    let first = svc
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    let second = svc
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    assert_ne!(
        first.id, second.id,
        "a new import never adopts an existing identity"
    );
    assert_eq!(first.sha256, second.sha256);
    assert_eq!(first.byte_size, i64::try_from(content.len())?);
    assert_eq!(count(&fixture, "blobs")?, 1);
    assert_eq!(count(&fixture, "documents")?, 2);

    let read = svc.get_document_metadata(&second.id.to_string()).await?;
    assert_eq!(read, second);
    assert_eq!(read.original_file_name, "synthetic-cv.pdf");
    Ok(())
}

#[tokio::test]
async fn concurrent_imports_of_the_same_bytes_both_succeed_with_one_blob() -> TestResult {
    let fixture = start();
    let extractor = CountingExtractor::new("v1");
    let content = synthetic_pdf("concurrent");
    let left = service(&fixture.uow, content.clone(), &extractor);
    let right = service(&fixture.uow, content, &extractor);
    let request = json!({"path": "synthetic-cv.pdf"});
    let (a, b) = tokio::join!(
        left.store_document_file(&request),
        right.store_document_file(&request),
    );
    let (a, b) = (a?, b?);
    assert_ne!(a.id, b.id);
    assert_eq!(a.sha256, b.sha256);
    assert_eq!(count(&fixture, "blobs")?, 1);
    assert_eq!(count(&fixture, "documents")?, 2);
    Ok(())
}

#[tokio::test]
async fn a_missing_document_is_not_found_and_a_malformed_identifier_is_rejected() -> TestResult {
    let fixture = start();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&fixture.uow, synthetic_pdf("missing"), &extractor);
    let missing = svc
        .get_document_metadata(&Uuid::nil().to_string())
        .await
        .err()
        .map(|error| error_envelope(&error).code);
    assert_eq!(missing, Some(ErrorCode::NotFound));
    let malformed = svc
        .get_document_metadata("nope")
        .await
        .err()
        .map(|error| error_envelope(&error).code);
    assert_eq!(malformed, Some(ErrorCode::ValidationError));
    Ok(())
}

#[tokio::test]
async fn the_extraction_cache_misses_then_hits_then_recomputes_on_a_new_extractor() -> TestResult {
    let fixture = start();
    let old = CountingExtractor::new("v1");
    let writer = service(&fixture.uow, synthetic_pdf("cache"), &old);
    let stored = writer
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    let document_id = stored.id.to_string();
    let persist = json!({"documentId": document_id, "persistExtraction": true});

    let miss = writer.extract_stored_pdf_text(&persist).await?;
    let extraction_id = miss
        .extraction_id
        .ok_or("a persisted extraction has an identifier")?;
    assert_eq!(old.calls(), 1);
    assert_eq!(count(&fixture, "pdf_extractions")?, 1);

    let hit = writer.extract_stored_pdf_text(&persist).await?;
    assert_eq!(hit.extraction_id, Some(extraction_id));
    assert_eq!(
        old.calls(),
        1,
        "a matching cached extraction is served from the table"
    );
    assert_eq!(hit.extraction.text, miss.extraction.text);
    assert_eq!(hit.extraction.pages.len(), 2);

    let new = CountingExtractor::new("v2");
    let reader = service(&fixture.uow, synthetic_pdf("cache"), &new);
    let recomputed = reader.extract_stored_pdf_text(&persist).await?;
    assert_eq!(
        new.calls(),
        1,
        "a different extractor version is recomputed"
    );
    assert_eq!(
        recomputed.extraction_id,
        Some(extraction_id),
        "the refresh keeps the identifier"
    );
    assert_eq!(count(&fixture, "pdf_extractions")?, 1);

    let settled = reader.extract_stored_pdf_text(&persist).await?;
    assert_eq!(
        new.calls(),
        1,
        "the refreshed record matches the new version"
    );
    assert_eq!(settled.extraction_id, Some(extraction_id));
    Ok(())
}

#[tokio::test]
async fn a_non_persisted_stored_extraction_leaves_no_record() -> TestResult {
    let fixture = start();
    let extractor = CountingExtractor::new("v1");
    let svc = service(&fixture.uow, synthetic_pdf("transient"), &extractor);
    let stored = svc
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    let result = svc
        .extract_stored_pdf_text(&json!({"documentId": stored.id.to_string()}))
        .await?;
    assert_eq!(result.extraction_id, None);
    assert_eq!(count(&fixture, "pdf_extractions")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_refused_import_root_is_reported_before_any_database_call() -> TestResult {
    let uow = SqliteUnitOfWork::new(SqlitePool::unconfigured());
    let extractor = CountingExtractor::new("v1");
    let svc = DocumentService::new(
        uow.clone(),
        SqliteDocumentRepository,
        Arc::new(RefusingReader),
        Arc::clone(&extractor) as Arc<dyn PdfExtractor>,
        Arc::new(SystemClock),
    );
    let error = svc
        .store_document_file(&json!({"path": "cv.pdf"}))
        .await
        .err();
    let reason = error
        .as_ref()
        .and_then(|error| error_envelope(error).details.get("reason").cloned());
    assert_eq!(
        reason.as_deref(),
        Some("document import root is not configured")
    );
    assert_eq!(extractor.calls(), 0);
    Ok(())
}

/// A reader that refuses every path as an unconfigured import root.
#[derive(Debug)]
struct RefusingReader;

#[async_trait]
impl DocumentFileReader for RefusingReader {
    async fn read(&self, _path: &str) -> Result<FileSnapshot, AppError> {
        Err(AppError::from(validation_error(
            "refused",
            Some("path"),
            Some("document import root is not configured"),
        )))
    }
}
