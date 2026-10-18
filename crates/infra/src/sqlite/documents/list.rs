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

use jobniac_core::clock::truncate_to_micros;
use jobniac_core::documents::DocumentSummary;
use jobniac_core::errors::AppError;
use jobniac_core::pagination::{Page, PageRequest, keyset_page};
use jobniac_text::str_rules::strip_text;
use rusqlite::Connection;
use time::OffsetDateTime;

use crate::sqlite::profiles::codec::{Id, Instant};
use crate::sqlite::profiles::{query_rows, unavailable};

use super::rows::column;

/// The four binds are the snapshot watermark, the anchor's created-at, its identifier and the
/// fetch limit; the keyset shape of the profile listing. The anchor binds are NULL on the first
/// page.
const LIST_DOCUMENTS_SQL: &str = r#"
SELECT file.id, file.created_at, blob.byte_size, extraction.page_count,
       CASE WHEN length(CAST(file.original_file_name AS BLOB)) <= 4096
            THEN file.original_file_name END AS original_file_name,
       CASE WHEN length(CAST(file.media_type AS BLOB)) <= 255
            THEN file.media_type END AS media_type
FROM document_documents AS file
JOIN document_blobs AS blob ON blob.id = file.blob_id
LEFT JOIN document_pdf_extractions AS extraction ON extraction.file_id = file.id
WHERE file.created_at <= ?1
  AND file.media_type = 'application/pdf'
  AND (
      ?2 IS NULL
      OR file.created_at < ?2
      OR (
          file.created_at = ?2
          AND file.id > ?3
      )
  )
ORDER BY file.created_at DESC, file.id ASC
LIMIT ?4
"#;

/// Reads one bounded page of document summaries for `request`.
pub(super) fn list_documents(
    conn: &Connection,
    request: &PageRequest<'_>,
) -> Result<Page<DocumentSummary>, AppError> {
    let anchor = request.cursor();
    // The first page fixes the watermark at the current instant (the old `CURRENT_TIMESTAMP`).
    let snapshot_at = anchor.map_or_else(
        || truncate_to_micros(OffsetDateTime::now_utc()),
        |cursor| cursor.snapshot_at,
    );
    let anchor_created_at = anchor.map(|cursor| Instant(cursor.created_at));
    let anchor_id = anchor.map(|cursor| Id(cursor.id));
    let fetch = i64::try_from(request.limit())
        .map_err(|_| AppError::Internal)?
        .saturating_add(1);
    let summaries = query_rows(
        conn,
        LIST_DOCUMENTS_SQL,
        &[
            &Instant(snapshot_at),
            &anchor_created_at,
            &anchor_id,
            &fetch,
        ],
        |row| {
            let original_file_name =
                column::<Option<String>>(row, "original_file_name")?.ok_or_else(unavailable)?;
            let media_type =
                column::<Option<String>>(row, "media_type")?.ok_or_else(unavailable)?;
            let byte_size: i64 = column(row, "byte_size")?;
            let page_count = column::<Option<i64>>(row, "page_count")?
                .map(|count| u32::try_from(count).map_err(|_| unavailable()))
                .transpose()?;
            if strip_text(&original_file_name).is_empty()
                || strip_text(&media_type).is_empty()
                || byte_size <= 0
            {
                return Err(unavailable());
            }
            Ok(DocumentSummary {
                id: column::<Id>(row, "id")?.0,
                original_file_name,
                media_type,
                byte_size,
                page_count,
                created_at: column::<Instant>(row, "created_at")?.0,
            })
        },
    )?;
    keyset_page(
        request,
        summaries,
        snapshot_at,
        |summary| summary.created_at,
        |summary| summary.id,
    )
}
