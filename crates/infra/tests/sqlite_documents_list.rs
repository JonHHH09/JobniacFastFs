//! The read-only document listing against a disposable SQLite database.
//!
//! The tests store synthetic PDFs through the real document use cases, pin their creation instants
//! through a raw connection, and page the listing through the signed cursor. They cover the
//! newest-first order with an identifier tie-break, the page size, the page count of a persisted
//! extraction, the first-page watermark, the empty store and the unconfigured database.

mod sqlite_support;

use std::error::Error;
use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::clock::SystemClock;
use jobniac_core::documents::{
    DocumentFileReader, DocumentListService, DocumentService, FileSnapshot, PageText, PdfExtractor,
    PdfText,
};
use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::pagination::{CursorCodec, CursorKey};
use jobniac_infra::sqlite::documents::SqliteDocumentRepository;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use serde_json::json;
use sqlite_support::SqliteFixture;
use time::{Duration, macros::datetime};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type Lister = DocumentListService<SqliteUnitOfWork, SqliteDocumentRepository>;

const SECRET: &str = "document-list-secret-0123456789abcdef";

struct Fixture {
    database: SqliteFixture,
    uow: SqliteUnitOfWork,
}

fn start() -> Fixture {
    let database = SqliteFixture::start();
    let uow = database.unit_of_work();
    Fixture { database, uow }
}

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

#[derive(Debug)]
struct TwoPageExtractor;

#[async_trait]
impl PdfExtractor for TwoPageExtractor {
    fn version(&self) -> &str {
        "v1"
    }

    async fn extract(&self, _content: &[u8], _file_name: &str) -> Result<PdfText, AppError> {
        Ok(PdfText {
            file_name: "stored.pdf".to_owned(),
            page_count: 2,
            character_count: 8,
            truncated: false,
            text: "one\n\ntwo".to_owned(),
            pages: vec![
                PageText {
                    page_number: 1,
                    text: "one".to_owned(),
                },
                PageText {
                    page_number: 2,
                    text: "two".to_owned(),
                },
            ],
        })
    }
}

fn storing(
    uow: &SqliteUnitOfWork,
    name: &str,
    marker: &str,
) -> DocumentService<SqliteUnitOfWork, SqliteDocumentRepository> {
    DocumentService::new(
        uow.clone(),
        SqliteDocumentRepository,
        Arc::new(FixedReader {
            file_name: name.to_owned(),
            content: format!("%PDF-1.4\n% synthetic {marker}\n").into_bytes(),
        }),
        Arc::new(TwoPageExtractor),
        Arc::new(SystemClock),
    )
}

fn lister(uow: &SqliteUnitOfWork) -> Result<Lister, Box<dyn Error + Send + Sync>> {
    Ok(DocumentListService::new(
        uow.clone(),
        SqliteDocumentRepository,
        CursorCodec::new(CursorKey::from_secret(SECRET)?),
    ))
}

async fn store(fixture: &Fixture, name: &str) -> Result<Uuid, Box<dyn Error + Send + Sync>> {
    let svc = storing(&fixture.uow, name, name);
    Ok(svc
        .store_document_file(&json!({"path": "synthetic.pdf"}))
        .await?
        .id)
}

/// Pins the creation (and update) instant of one document, in whole seconds after a fixed epoch.
fn set_created(fixture: &Fixture, id: Uuid, seconds: i64) -> TestResult {
    let at = datetime!(2026-01-01 00:00 UTC) + Duration::seconds(seconds);
    let stamp = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000000Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    );
    fixture.database.connection().execute(
        "UPDATE document_documents SET created_at = ?2, updated_at = ?2 WHERE id = ?1",
        rusqlite::params![id.to_string(), stamp],
    )?;
    Ok(())
}

#[tokio::test]
async fn the_listing_is_newest_first_with_an_identifier_tie_break_across_pages() -> TestResult {
    let fixture = start();
    let oldest = store(&fixture, "oldest.pdf").await?;
    let tie_a = store(&fixture, "tie-a.pdf").await?;
    let tie_b = store(&fixture, "tie-b.pdf").await?;
    let newer = store(&fixture, "newer.pdf").await?;
    let newest = store(&fixture, "newest.pdf").await?;
    set_created(&fixture, oldest, 10)?;
    set_created(&fixture, tie_a, 20)?;
    set_created(&fixture, tie_b, 20)?;
    set_created(&fixture, newer, 30)?;
    set_created(&fixture, newest, 40)?;
    let mut tie = [tie_a, tie_b];
    tie.sort();
    let expected = vec![newest, newer, tie[0], tie[1], oldest];

    let svc = lister(&fixture.uow)?;
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = svc.list_documents(Some(2), cursor.as_deref()).await?;
        assert!(page.documents.len() <= 2);
        seen.extend(page.documents.iter().map(|document| document.id));
        pages += 1;
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, expected);
    assert_eq!(pages, 3);
    Ok(())
}

#[tokio::test]
async fn a_summary_carries_the_safe_name_size_media_type_and_the_extraction_page_count()
-> TestResult {
    let fixture = start();
    let plain = store(&fixture, "plain.pdf").await?;
    let extracted = {
        let svc = storing(&fixture.uow, "dir/with-pages.pdf", "pages");
        let stored = svc
            .store_document_file(&json!({"path": "synthetic.pdf"}))
            .await?;
        svc.extract_stored_pdf_text(
            &json!({"documentId": stored.id.to_string(), "persistExtraction": true}),
        )
        .await?;
        stored
    };
    set_created(&fixture, plain, 10)?;
    set_created(&fixture, extracted.id, 20)?;

    let page = lister(&fixture.uow)?.list_documents(None, None).await?;
    assert_eq!(page.documents.len(), 2);
    let first = &page.documents[0];
    assert_eq!(first.id, extracted.id);
    assert_eq!(first.original_file_name, "with-pages.pdf");
    assert_eq!(first.media_type, "application/pdf");
    assert_eq!(first.byte_size, extracted.byte_size);
    assert_eq!(first.page_count, Some(2));
    let second = &page.documents[1];
    assert_eq!(second.id, plain);
    assert_eq!(second.page_count, None);
    assert_eq!(page.next_cursor, None);
    Ok(())
}

// Newest-first order already keeps newer rows off later pages; the snapshot watermark is kept so
// the cursor format matches the other lists.
#[tokio::test]
async fn a_document_stored_after_the_first_page_never_appears_on_later_pages() -> TestResult {
    let fixture = start();
    let mut ids = Vec::new();
    for (index, name) in ["a.pdf", "b.pdf", "c.pdf"].into_iter().enumerate() {
        let id = store(&fixture, name).await?;
        set_created(&fixture, id, 10 * (index as i64 + 1))?;
        ids.push(id);
    }
    let svc = lister(&fixture.uow)?;
    let first = svc.list_documents(Some(1), None).await?;
    let late = store(&fixture, "late.pdf").await?;
    let cursor = first
        .next_cursor
        .clone()
        .ok_or("a second page was expected")?;
    let mut seen: Vec<Uuid> = first.documents.iter().map(|d| d.id).collect();
    let mut next = Some(cursor);
    while let Some(cursor) = next {
        let page = svc.list_documents(Some(1), Some(&cursor)).await?;
        seen.extend(page.documents.iter().map(|d| d.id));
        next = page.next_cursor;
    }
    assert!(
        !seen.contains(&late),
        "the later document leaked into the listing"
    );
    assert_eq!(seen, vec![ids[2], ids[1], ids[0]]);
    Ok(())
}

#[tokio::test]
async fn an_empty_store_lists_nothing_and_a_foreign_cursor_is_refused() -> TestResult {
    let fixture = start();
    let svc = lister(&fixture.uow)?;
    let page = svc.list_documents(None, None).await?;
    assert!(page.documents.is_empty());
    assert_eq!(page.next_cursor, None);
    assert!(
        svc.list_documents(None, Some("not-a-cursor"))
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn the_unconfigured_database_fails_with_the_unavailable_message() -> TestResult {
    let uow = SqliteUnitOfWork::new(SqlitePool::unconfigured());
    let message = lister(&uow)?
        .list_documents(None, None)
        .await
        .err()
        .map(|error| error_envelope(&error).message);
    assert_eq!(message.as_deref(), Some("Local database is not configured"));
    Ok(())
}
