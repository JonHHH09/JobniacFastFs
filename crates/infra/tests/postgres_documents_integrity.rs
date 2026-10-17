//! Stored-document integrity against a disposable PostgreSQL 18 server: tampered blobs and
//! extraction rows, a deduplicated import whose winning blob is corrupt, supplementary characters
//! in the UTF-16 count, and an invalid canonical extraction. The server-backed tests skip with a
//! notice when `JOBNIAC_TEST_PG_PORT` is unset. Rows are changed only through the superuser
//! connection of the disposable database, and every byte is synthetic.

use std::error::Error;
use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::clock::SystemClock;
use jobniac_core::documents::{
    DocumentFileReader, DocumentService, FileSnapshot, PageText, PdfExtractor, PdfText,
};
use jobniac_core::errors::{AppError, error_envelope};
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::documents::PgDocumentRepository;
use jobniac_infra::postgres::migrations::migrate;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use jobniac_testkit::pg::{TestDatabase, TestServer};
use serde_json::json;

type TestResult = Result<(), Box<dyn Error>>;
type Service = DocumentService<PgUnitOfWork, PgDocumentRepository>;

/// The unavailable message of a corrupt stored row (the database-unavailable text).
const UNAVAILABLE: &str = "Local database is unavailable";
/// The domain-value message of an invalid canonical extraction.
const INVALID_EXTRACTION: &str = "Canonical PDF extraction must be valid and bounded";

/// A migrated disposable database and the unit of work over it.
struct Fixture {
    database: TestDatabase,
    uow: PgUnitOfWork,
}

async fn start() -> Result<Option<Fixture>, Box<dyn Error>> {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(None);
    };
    let database = server.fresh_database().await;
    let config = load_database_config(&database.environment())
        .ok_or("the disposable database configuration is invalid")?;
    migrate(&config).await.map_err(|error| error.0)?;
    let uow = PgUnitOfWork::new(PgPool::new(Some(config)));
    Ok(Some(Fixture { database, uow }))
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

fn service(uow: &PgUnitOfWork, extractor: FixedExtractor) -> Service {
    DocumentService::new(
        uow.clone(),
        PgDocumentRepository,
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

async fn count(fixture: &Fixture, table: &str) -> Result<i64, Box<dyn Error>> {
    let client = fixture.database.admin_client().await;
    let row = client
        .query_one(
            &format!("SELECT count(*)::bigint FROM document.{table}"),
            &[],
        )
        .await?;
    Ok(row.try_get::<_, i64>(0)?)
}

/// Changes the first byte of every stored blob without changing its size, so the digest no longer
/// matches while every table constraint still holds.
async fn corrupt_blob_bytes(fixture: &Fixture) -> Result<(), Box<dyn Error>> {
    let client = fixture.database.admin_client().await;
    client
        .execute(
            "UPDATE document.blobs SET content = overlay(content placing '\\x00'::bytea from 1 for 1)",
            &[],
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn a_blob_with_altered_bytes_reads_as_unavailable() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let extractor = FixedExtractor {
        text: "Synthetic text",
        character_count: 14,
    };
    let svc = service(&fixture.uow, extractor);
    let stored = svc
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    corrupt_blob_bytes(&fixture).await?;

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
    let Some(fixture) = start().await? else {
        return Ok(());
    };
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
    // The blob's own CHECK keeps byte_size equal to octet_length(content), so both change together
    // and the digest recorded at import no longer matches the bytes.
    let client = fixture.database.admin_client().await;
    client
        .execute(
            "UPDATE document.blobs SET content = content || '\\x41'::bytea, \
             byte_size = byte_size + 1",
            &[],
        )
        .await?;
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
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let svc = service(
        &fixture.uow,
        FixedExtractor {
            text: "Synthetic text",
            character_count: 14,
        },
    );
    svc.store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    corrupt_blob_bytes(&fixture).await?;

    let refused = svc
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await;
    assert!(
        refused.is_err(),
        "the winner's bytes must equal the input before a second identity is written"
    );
    assert_eq!(
        count(&fixture, "documents").await?,
        1,
        "the refused import leaves no new document row"
    );
    assert_eq!(count(&fixture, "blobs").await?, 1);
    Ok(())
}

#[tokio::test]
async fn a_corrupt_cached_extraction_reads_as_unavailable() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
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
    assert_eq!(count(&fixture, "pdf_extractions").await?, 1);

    // An array of the right shape whose element carries an extra key is corrupt data.
    let client = fixture.database.admin_client().await;
    client
        .execute(
            "UPDATE document.pdf_extractions SET page_projections = \
             '[{\"pageNumber\": 1, \"text\": \"a\", \"extra\": 1}]'::jsonb",
            &[],
        )
        .await?;
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
    let Some(fixture) = start().await? else {
        return Ok(());
    };
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
    let client = fixture.database.admin_client().await;
    let row = client
        .query_one(
            "SELECT character_count FROM document.pdf_extractions LIMIT 1",
            &[],
        )
        .await?;
    assert_eq!(row.try_get::<_, i32>(0)?, 4);
    Ok(())
}

#[tokio::test]
async fn an_invalid_canonical_extraction_is_refused_and_nothing_is_written() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
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
    assert_eq!(count(&fixture, "pdf_extractions").await?, 0);
    Ok(())
}
