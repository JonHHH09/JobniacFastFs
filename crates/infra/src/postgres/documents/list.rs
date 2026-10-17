//! Keyset listing of stored documents (desktop only).
//!
//! The order is `created_at DESC, id ASC`, which is the order of the existing
//! `idx_document_documents_created_at` index (`created_at DESC, id`); the schema is fixed,
//! so no index is added. The size comes from the blob row by primary key and the page count from
//! the canonical extraction by its unique `file_id` index, so the page is `limit + 1` index-ordered
//! rows with two point lookups each. Like the profile listing, the first page fixes a created-at
//! watermark that later pages reuse, so documents stored after the first page do not shift the
//! listing. Newest-first order already keeps newer rows off later pages; the snapshot watermark is
//! kept for cursor-format consistency with the other lists. The SQL selects no content, hash or
//! extraction text. Names and media types are bounded in SQL as the metadata read does, and a
//! stored row outside its bounds is corrupt data.

use jobniac_core::documents::DocumentSummary;
use jobniac_core::errors::AppError;
use jobniac_core::pagination::{Page, PageRequest, keyset_page};
use jobniac_text::str_rules::strip_text;
use time::OffsetDateTime;
use tokio_postgres::Client;

use crate::postgres::profiles::{db, unavailable};

use super::rows::column;

/// The seven binds are the anchor's snapshot, its created-at (three times), its identifier and the
/// fetch limit; the keyset shape of the profile listing.
const LIST_DOCUMENTS_SQL: &str = r#"
WITH bounds AS (
    SELECT COALESCE($1::timestamptz, CURRENT_TIMESTAMP) AS snapshot_at
)
SELECT file.id, file.created_at, blob.byte_size, extraction.page_count, bounds.snapshot_at,
       CASE WHEN octet_length(file.original_file_name) <= 4096
            THEN file.original_file_name END AS original_file_name,
       CASE WHEN octet_length(file.media_type) <= 255
            THEN file.media_type END AS media_type
FROM document.documents AS file
CROSS JOIN bounds
JOIN document.blobs AS blob ON blob.id = file.blob_id
LEFT JOIN document.pdf_extractions AS extraction ON extraction.file_id = file.id
WHERE file.created_at <= bounds.snapshot_at
  AND file.media_type = 'application/pdf'
  AND (
      $2::timestamptz IS NULL
      OR file.created_at < $3::timestamptz
      OR (
          file.created_at = $4::timestamptz
          AND file.id > $5::uuid
      )
  )
ORDER BY file.created_at DESC, file.id ASC
LIMIT $6
"#;

/// Reads one bounded page of document summaries for `request`.
pub(super) async fn list_documents(
    client: &Client,
    request: &PageRequest<'_>,
) -> Result<Page<DocumentSummary>, AppError> {
    let anchor = request.cursor();
    let snapshot = anchor.map(|cursor| cursor.snapshot_at);
    let anchor_created_at = anchor.map(|cursor| cursor.created_at);
    let anchor_id = anchor.map(|cursor| cursor.id);
    let fetch = i64::try_from(request.limit())
        .map_err(|_| AppError::Internal)?
        .saturating_add(1);
    let rows = db(client
        .query(
            LIST_DOCUMENTS_SQL,
            &[
                &snapshot,
                &anchor_created_at,
                &anchor_created_at,
                &anchor_created_at,
                &anchor_id,
                &fetch,
            ],
        )
        .await)?;
    let mut summaries = Vec::with_capacity(rows.len());
    let mut snapshot_at = None;
    for row in &rows {
        if snapshot_at.is_none() {
            snapshot_at = Some(column::<OffsetDateTime>(row, "snapshot_at")?);
        }
        let original_file_name =
            column::<Option<String>>(row, "original_file_name")?.ok_or_else(unavailable)?;
        let media_type = column::<Option<String>>(row, "media_type")?.ok_or_else(unavailable)?;
        let byte_size: i64 = column(row, "byte_size")?;
        let page_count = column::<Option<i32>>(row, "page_count")?
            .map(|count| u32::try_from(count).map_err(|_| unavailable()))
            .transpose()?;
        if strip_text(&original_file_name).is_empty()
            || strip_text(&media_type).is_empty()
            || byte_size <= 0
        {
            return Err(unavailable());
        }
        summaries.push(DocumentSummary {
            id: column(row, "id")?,
            original_file_name,
            media_type,
            byte_size,
            page_count,
            created_at: column(row, "created_at")?,
        });
    }
    // The first row's watermark, else the anchor's. Without rows and without an anchor no cursor
    // is issued, so the last fallback is never signed.
    let snapshot_at = snapshot_at
        .or(snapshot)
        .unwrap_or_else(OffsetDateTime::now_utc);
    keyset_page(
        request,
        summaries,
        snapshot_at,
        |summary| summary.created_at,
        |summary| summary.id,
    )
}
