//! Read-only keyset listing of stored documents (desktop only).
//!
//! The listing has no MCP tool: it feeds the desktop pickers that
//! otherwise need a pasted document identifier. Documents come most recent first, ties broken by
//! identifier, so the order is the one of the `idx_document_documents_created_at` index. The cursor
//! is bound to the `documents` scope and the `all` filter and signed by the shared cursor codec,
//! so a cursor from another listing is refused. The page size comes from the codec (default 20,
//! at most 100).

use time::OffsetDateTime;
use uuid::Uuid;

use crate::errors::AppError;
use crate::pagination::{CursorCodec, Page};
use crate::profiles::repository::{READ_ONLY, UnitOfWork};

use super::repository::DocumentListRepository;
use super::view::safe_basename;

/// The scope that binds a document-list cursor.
pub const DOCUMENT_LIST_SCOPE: &str = "documents";
/// The filter identity of the unfiltered listing.
pub const DOCUMENT_LIST_FILTER: &str = "all";

/// One stored document as the list shows it. Bytes, hashes and extraction text stay in the
/// database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSummary {
    /// The document identifier.
    pub id: Uuid,
    /// The base name of the original file (the repository returns it as stored; the service reduces
    /// it to a safe base name).
    pub original_file_name: String,
    /// The media type of the document.
    pub media_type: String,
    /// The byte size of the content, from the blob row.
    pub byte_size: i64,
    /// The page count of the canonical extraction, when one is stored.
    pub page_count: Option<u32>,
    /// The creation instant, which orders the listing.
    pub created_at: OffsetDateTime,
}

/// One page of the listing: summaries and the cursor of the next page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentListPage {
    /// The page of summaries, most recent first.
    pub documents: Vec<DocumentSummary>,
    /// The cursor of the next page, or `None` on the last page.
    pub next_cursor: Option<String>,
}

/// Lists stored documents through a repository and a cursor codec.
#[derive(Debug)]
pub struct DocumentListService<U, R> {
    uow: U,
    repo: R,
    codec: CursorCodec,
}

impl<U, R> DocumentListService<U, R>
where
    U: UnitOfWork,
    R: DocumentListRepository<Tx = U::Tx>,
{
    /// A service over a unit of work, a repository and the cursor codec of this process.
    pub fn new(uow: U, repo: R, codec: CursorCodec) -> Self {
        Self { uow, repo, codec }
    }

    /// One page of stored documents, most recent first.
    ///
    /// # Errors
    ///
    /// The cursor error (`invalid_cursor`) for a bad or foreign cursor, the limit error for a limit
    /// outside 1 to 100, or a repository error.
    pub async fn list_documents(
        &self,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<DocumentListPage, AppError> {
        let request =
            self.codec
                .page_request(limit, cursor, DOCUMENT_LIST_SCOPE, DOCUMENT_LIST_FILTER)?;
        let Page { items, next_cursor } = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo.list_documents(tx, &request).await
            })
            .await?;
        Ok(DocumentListPage {
            documents: items
                .into_iter()
                .map(|summary| DocumentSummary {
                    original_file_name: safe_basename(&summary.original_file_name),
                    ..summary
                })
                .collect(),
            next_cursor,
        })
    }
}
