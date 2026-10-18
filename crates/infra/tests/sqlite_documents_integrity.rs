//! Stored-document integrity against a disposable SQLite database: tampered blobs and extraction
//! rows, a deduplicated import whose winning blob is corrupt, supplementary characters in the
//! UTF-16 count, and an invalid canonical extraction. Rows are changed only through a raw
//! connection of the disposable database, and every byte is synthetic.

mod sqlite_support;

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::clock::SystemClock;
use jobniac_core::documents::{
    DocumentFileReader, DocumentService, FileSnapshot, PageText, PdfExtractor, PdfText,
};
use jobniac_core::errors::{AppError, error_envelope};
use jobniac_infra::sqlite::documents::SqliteDocumentRepository;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use serde_json::json;
use sqlite_support::{SqliteFixture, TestResult};

type Service = DocumentService<SqliteUnitOfWork, SqliteDocumentRepository>;

/// The unavailable message of a corrupt stored row (the database-unavailable text).
const UNAVAILABLE: &str = "Local database is unavailable";
/// The domain-value message of an invalid canonical extraction.
const INVALID_EXTRACTION: &str = "Canonical PDF extraction must be valid and bounded";

/// A migrated database file and the unit of work over it.
struct Fixture {
    database: SqliteFixture,
    uow: SqliteUnitOfWork,
}

fn start() -> Fixture {
    let database = SqliteFixture::start();
    let uow = database.unit_of_work();
    Fixture { database, uow }
}

/// A reader that returns one fixed synthetic file.
#[derive(Debug)]
struct FixedReader {
    content: Vec<u8>,
}

#[async_trait]
impl DocumentFileReader for FixedReader {
    async fn read(&self, _path: &str) -> Result<FileSnapshot, AppError> {
        Ok(FileSnapshot {
            file_name: "synthetic-cv.pdf".to_owned(),
            content: self.content.clone(),
        })
    }
}

/// An extractor that returns a fixed canonical extraction of one synthetic page.
#[derive(Debug)]
struct FixedExtractor {
    text: &'static str,
    character_count: i64,
}

#[async_trait]
impl PdfExtractor for FixedExtractor {
    fn version(&self) -> &str {
        "integrity-v1"
    }

    async fn extract(&self, _content: &[u8], _file_name: &str) -> Result<PdfText, AppError> {
        Ok(PdfText {
            file_name: "stored.pdf".to_owned(),
            page_count: 1,
            character_count: self.character_count,
            truncated: false,
            text: self.text.to_owned(),
            pages: vec![PageText {
                page_number: 1,
                text: self.text.to_owned(),
            }],
        })
    }
}

fn synthetic_pdf() -> Vec<u8> {
    b"%PDF-1.4\n% synthetic integrity\n".to_vec()
}

fn service(uow: &SqliteUnitOfWork, extractor: FixedExtractor) -> Service {
    DocumentService::new(
        uow.clone(),
        SqliteDocumentRepository,
        Arc::new(FixedReader {
            content: synthetic_pdf(),
        }),
        Arc::new(extractor),
        Arc::new(SystemClock),
    )
}

fn message(error: &AppError) -> String {
    error_envelope(error).message
}

fn count(fixture: &Fixture, table: &str) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    Ok(fixture.database.connection().query_row(
        &format!("SELECT count(*) FROM document_{table}"),
        [],
        |row| row.get(0),
    )?)
}

/// Replaces the bytes of every stored blob without changing its size, so the digest no longer
/// matches while every table constraint still holds.
fn corrupt_blob_bytes(fixture: &Fixture) -> TestResult {
    fixture.database.connection().execute(
        "UPDATE document_blobs SET content = zeroblob(byte_size)",
        [],
    )?;
    Ok(())
}

#[tokio::test]
async fn a_blob_with_altered_bytes_reads_as_unavailable() -> TestResult {
    let fixture = start();
    let extractor = FixedExtractor {
        text: "Synthetic text",
        character_count: 14,
    };
    let svc = service(&fixture.uow, extractor);
    let stored = svc
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    corrupt_blob_bytes(&fixture)?;

    let error = svc
        .extract_stored_pdf_text(&json!({"documentId": stored.id.to_string()}))
        .await
        .err()
        .ok_or("a corrupt blob must not be extracted")?;
    assert_eq!(message(&error), UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn a_blob_with_extra_bytes_reads_as_unavailable() -> TestResult {
    let fixture = start();
    let svc = service(
        &fixture.uow,
        FixedExtractor {
            text: "Synthetic text",
            character_count: 14,
        },
    );
    let stored = svc
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    // The blob's own CHECK keeps byte_size equal to length(content), so both change together
    // and the digest recorded at import no longer matches the bytes.
    fixture.database.connection().execute(
        "UPDATE document_blobs SET content = CAST(content || 'A' AS BLOB), \
         byte_size = byte_size + 1",
        [],
    )?;
    let error = svc
        .extract_stored_pdf_text(&json!({"documentId": stored.id.to_string()}))
        .await
        .err()
        .ok_or("a size mismatch must not be extracted")?;
    assert_eq!(message(&error), UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn a_dedupe_onto_a_corrupt_winning_blob_is_refused_and_rolls_back() -> TestResult {
    let fixture = start();
    let svc = service(
        &fixture.uow,
        FixedExtractor {
            text: "Synthetic text",
            character_count: 14,
        },
    );
    svc.store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    corrupt_blob_bytes(&fixture)?;

    let refused = svc
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await;
    assert!(
        refused.is_err(),
        "the winner's bytes must equal the input before a second identity is written"
    );
    assert_eq!(
        count(&fixture, "documents")?,
        1,
        "the refused import leaves no new document row"
    );
    assert_eq!(count(&fixture, "blobs")?, 1);
    Ok(())
}

#[tokio::test]
async fn a_corrupt_cached_extraction_reads_as_unavailable() -> TestResult {
    let fixture = start();
    let svc = service(
        &fixture.uow,
        FixedExtractor {
            text: "Synthetic text",
            character_count: 14,
        },
    );
    let stored = svc
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    let persist = json!({"documentId": stored.id.to_string(), "persistExtraction": true});
    svc.extract_stored_pdf_text(&persist).await?;
    assert_eq!(count(&fixture, "pdf_extractions")?, 1);

    // An array of the right shape whose element carries an extra key is corrupt data.
    fixture.database.connection().execute(
        "UPDATE document_pdf_extractions SET page_projections = \
         '[{\"pageNumber\": 1, \"text\": \"a\", \"extra\": 1}]'",
        [],
    )?;
    let error = svc
        .extract_stored_pdf_text(&persist)
        .await
        .err()
        .ok_or("a corrupt cached extraction must not be served")?;
    assert_eq!(message(&error), UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn supplementary_characters_persist_with_their_utf16_count() -> TestResult {
    let fixture = start();
    // "A", one emoji (a surrogate pair) and "B": four UTF-16 units, three code points.
    let text = "A\u{1F600}B";
    let svc = service(
        &fixture.uow,
        FixedExtractor {
            text,
            character_count: 4,
        },
    );
    let stored = svc
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    let persist = json!({"documentId": stored.id.to_string(), "persistExtraction": true});
    let miss = svc.extract_stored_pdf_text(&persist).await?;
    assert_eq!(miss.extraction.character_count, 4);

    let hit = svc.extract_stored_pdf_text(&persist).await?;
    assert_eq!(hit.extraction.text, text);
    assert_eq!(hit.extraction.character_count, 4);
    let stored: i64 = fixture.database.connection().query_row(
        "SELECT character_count FROM document_pdf_extractions LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(stored, 4);
    Ok(())
}

#[tokio::test]
async fn an_invalid_canonical_extraction_is_refused_and_nothing_is_written() -> TestResult {
    let fixture = start();
    // The count of three UTF-16 units is claimed as one: the record is not a valid extraction.
    let svc = service(
        &fixture.uow,
        FixedExtractor {
            text: "abc",
            character_count: 1,
        },
    );
    let stored = svc
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    let persist = json!({"documentId": stored.id.to_string(), "persistExtraction": true});
    let error = svc
        .extract_stored_pdf_text(&persist)
        .await
        .err()
        .ok_or("an invalid extraction must be refused")?;
    assert_eq!(message(&error), INVALID_EXTRACTION);
    assert_eq!(count(&fixture, "pdf_extractions")?, 0);
    Ok(())
}
