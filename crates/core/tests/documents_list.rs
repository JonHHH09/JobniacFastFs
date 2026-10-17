//! The read-only document listing against an in-memory repository.

use std::error::Error;

use async_trait::async_trait;
use jobniac_core::documents::{
    DOCUMENT_LIST_FILTER, DOCUMENT_LIST_SCOPE, DocumentListRepository, DocumentListService,
    DocumentSummary,
};
use jobniac_core::errors::{AppError, ErrorCode, error_envelope};
use jobniac_core::pagination::{CursorCodec, CursorKey, Page, PageRequest, keyset_page};
use jobniac_core::profiles::repository::UnitOfWork;
use jobniac_core::transactions::TransactionOptions;
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const SECRET: &str = "document-list-secret-0123456789abcdef";
const SNAPSHOT: OffsetDateTime = datetime!(2026-10-17 12:00:00 UTC);

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

/// Holds the documents in storage order and pages them by the keyset rule.
#[derive(Debug)]
struct MemoryDocuments(Vec<DocumentSummary>);

#[async_trait]
impl DocumentListRepository for MemoryDocuments {
    type Tx = ();

    async fn list_documents(
        &self,
        _tx: &mut (),
        request: &PageRequest<'_>,
    ) -> Result<Page<DocumentSummary>, AppError> {
        let mut ordered = self.0.clone();
        ordered.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
        let after = |summary: &DocumentSummary| match request.cursor() {
            None => true,
            Some(cursor) => {
                summary.created_at < cursor.created_at
                    || (summary.created_at == cursor.created_at && summary.id > cursor.id)
            }
        };
        let rows: Vec<_> = ordered
            .into_iter()
            .filter(after)
            .take(request.limit() + 1)
            .collect();
        keyset_page(
            request,
            rows,
            SNAPSHOT,
            |summary| summary.created_at,
            |summary| summary.id,
        )
    }
}

fn summary(index: u128, name: &str, created_at: OffsetDateTime) -> DocumentSummary {
    DocumentSummary {
        id: Uuid::from_u128(index),
        original_file_name: name.to_owned(),
        media_type: "application/pdf".to_owned(),
        byte_size: 1024,
        page_count: Some(2),
        created_at,
    }
}

fn service(
    documents: Vec<DocumentSummary>,
) -> Result<DocumentListService<FakeUow, MemoryDocuments>, Box<dyn Error>> {
    let key = CursorKey::from_secret(SECRET)?;
    Ok(DocumentListService::new(
        FakeUow,
        MemoryDocuments(documents),
        CursorCodec::new(key),
    ))
}

#[tokio::test]
async fn documents_come_newest_first_and_page_through_the_signed_cursor() -> TestResult {
    let svc = service(vec![
        summary(1, "old.pdf", datetime!(2026-10-01 08:00:00 UTC)),
        summary(2, "new.pdf", datetime!(2026-10-03 08:00:00 UTC)),
        summary(3, "mid.pdf", datetime!(2026-10-02 08:00:00 UTC)),
    ])?;
    let first = svc.list_documents(Some(2), None).await?;
    let names: Vec<_> = first
        .documents
        .iter()
        .map(|d| d.original_file_name.as_str())
        .collect();
    assert_eq!(names, ["new.pdf", "mid.pdf"]);
    let cursor = first.next_cursor.ok_or("a second page was expected")?;
    let second = svc.list_documents(Some(2), Some(&cursor)).await?;
    assert_eq!(second.documents.len(), 1);
    assert_eq!(second.documents[0].original_file_name, "old.pdf");
    assert_eq!(second.next_cursor, None);
    Ok(())
}

#[tokio::test]
async fn equal_instants_are_ordered_by_identifier_across_pages() -> TestResult {
    let at = datetime!(2026-10-02 08:00:00 UTC);
    let svc = service(vec![
        summary(3, "c.pdf", at),
        summary(1, "a.pdf", at),
        summary(2, "b.pdf", at),
    ])?;
    let first = svc.list_documents(Some(1), None).await?;
    let cursor = first.next_cursor.clone().ok_or("cursor")?;
    let second = svc.list_documents(Some(1), Some(&cursor)).await?;
    let third_cursor = second.next_cursor.clone().ok_or("cursor")?;
    let third = svc.list_documents(Some(1), Some(&third_cursor)).await?;
    let names: Vec<_> = [first, second, third]
        .iter()
        .map(|page| page.documents[0].original_file_name.clone())
        .collect();
    assert_eq!(names, ["a.pdf", "b.pdf", "c.pdf"]);
    Ok(())
}

#[tokio::test]
async fn the_name_is_reduced_to_a_safe_base_name() -> TestResult {
    let svc = service(vec![summary(
        1,
        "C:\\Users\\a\\cv.pdf",
        datetime!(2026-10-02 08:00:00 UTC),
    )])?;
    let page = svc.list_documents(None, None).await?;
    assert_eq!(page.documents[0].original_file_name, "cv.pdf");
    Ok(())
}

#[tokio::test]
async fn an_empty_store_is_an_empty_page_without_a_cursor() -> TestResult {
    let page = service(Vec::new())?.list_documents(None, None).await?;
    assert!(page.documents.is_empty());
    assert_eq!(page.next_cursor, None);
    Ok(())
}

#[tokio::test]
async fn a_bad_foreign_or_unsigned_cursor_and_a_bad_limit_are_refused() -> TestResult {
    let svc = service(vec![
        summary(1, "a.pdf", datetime!(2026-10-01 08:00:00 UTC)),
        summary(2, "b.pdf", datetime!(2026-10-02 08:00:00 UTC)),
    ])?;
    let refusal = svc.list_documents(None, Some("garbage")).await.err();
    assert_eq!(
        refusal.map(|error| error_envelope(&error).code),
        Some(ErrorCode::ValidationError)
    );
    // A cursor signed with the right key but for another listing is refused.
    let other = CursorCodec::new(CursorKey::from_secret(SECRET)?).next_cursor(
        "profiles",
        "all",
        SNAPSHOT,
        SNAPSHOT,
        Uuid::from_u128(1),
    )?;
    assert!(svc.list_documents(None, Some(&other)).await.is_err());
    let own = CursorCodec::new(CursorKey::from_secret(SECRET)?).next_cursor(
        DOCUMENT_LIST_SCOPE,
        "not-the-filter",
        SNAPSHOT,
        SNAPSHOT,
        Uuid::from_u128(1),
    )?;
    assert!(svc.list_documents(None, Some(&own)).await.is_err());
    assert_eq!(DOCUMENT_LIST_FILTER, "all");
    for limit in [0, 101, -1] {
        assert!(svc.list_documents(Some(limit), None).await.is_err());
    }
    Ok(())
}
