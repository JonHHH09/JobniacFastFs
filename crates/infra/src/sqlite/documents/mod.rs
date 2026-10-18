//! SQLite adapter of the document ports.
//!
//! The repository keeps the original bytes private: bytes are selected only inside Rust, only when
//! their size is within the 10 MiB bound, and only after their size and SHA-256 are verified. A new
//! document never adopts an existing identity. The SQL uses `?n` binds and the prefixed table
//! names of the baseline schema.
//!
//! `save_file` inserts the blob (`ON CONFLICT (sha256) DO NOTHING`), then reads it back on a
//! conflict, then inserts the document reference. The `FOR SHARE` read of the PostgreSQL adapter
//! is not needed: every write transaction is `BEGIN IMMEDIATE`, so no other writer can change the
//! verified bytes. Every port method runs in the transaction the use case opened.

mod list;
pub mod rows;

use async_trait::async_trait;
use jobniac_core::documents::{
    DocumentListRepository, DocumentMetadata, DocumentRepository, DocumentSummary, ExtractionRecord,
};
use jobniac_core::errors::AppError;
use jobniac_core::pagination::{Page, PageRequest};
use rusqlite::Connection;
use uuid::Uuid;

use crate::blocking::run_blocking as run_file_phase;
use crate::sqlite::codec::{Id, Instant};
use crate::sqlite::profiles::{
    assert_generated_document_attachable, execute, query_rows, unavailable, with_connection,
};
use crate::sqlite::unit_of_work::SqliteTransaction;

use rows::{
    MAX_PAGE_JSON_BYTES, column, extraction_from_row, instant_for_write, metadata_from_row,
    pages_to_json, validate_extraction_input, validate_file_input, verified_content,
};

/// The upper bound of a stored document, in bytes (`MAX_DOCUMENT_BYTES`, 10 MiB).
const MAX_DOCUMENT_BYTES: i64 = 10 * 1024 * 1024;
/// The bound of a canonical extraction's characters (`MAX_EXTRACTION_CHARACTERS`).
const MAX_EXTRACTION_CHARACTERS: i64 = 250_000;
/// The bound of a canonical extraction's pages (`MAX_EXTRACTION_PAGES`).
const MAX_EXTRACTION_PAGES: i64 = 200;

/// The metadata columns of a document joined to its blob. `length` of a BLOB cast counts bytes,
/// as `octet_length` did.
const METADATA_COLUMNS: &str = "
    file.id, file.blob_id,
    CASE WHEN length(CAST(file.original_file_name AS BLOB)) <= 4096
         THEN file.original_file_name END AS original_file_name,
    CASE WHEN length(CAST(file.media_type AS BLOB)) <= 255
         THEN file.media_type END AS media_type,
    file.created_at, file.updated_at, blob.byte_size, blob.sha256
";

/// The bounded content column, with the size bound at `?1`.
const BOUNDED_CONTENT: &str = "
    CASE WHEN blob.byte_size BETWEEN 1 AND ?1
              AND length(blob.content) = blob.byte_size
         THEN blob.content END AS content
";

/// The extraction columns with the three bounds at `?1` to `?3`.
const EXTRACTION_COLUMNS: &str = "
    id, file_id, character_count, page_count, truncated, created_at,
    CASE WHEN length(CAST(extractor AS BLOB)) <= 255 THEN extractor END AS extractor,
    CASE WHEN length(extracted_text) <= ?1
         THEN extracted_text END AS extracted_text,
    CASE WHEN json_valid(page_projections)
              AND json_type(page_projections) = 'array'
              AND json_array_length(page_projections) <= ?2
              AND length(CAST(page_projections AS BLOB)) <= ?3
         THEN page_projections END AS page_projections
";

/// Inserts a blob or keeps the existing row with the same digest (`ON CONFLICT DO NOTHING`).
const INSERT_BLOB_SQL: &str = "
        INSERT INTO document_blobs (id, sha256, byte_size, content, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT (sha256) DO NOTHING
        RETURNING id
        ";

/// Reads the winning blob of a digest with its bounded content. The size bound is at `?1`, the
/// digest at `?2`.
fn select_blob_sql() -> String {
    format!(
        "SELECT blob.id, blob.byte_size, blob.sha256, {BOUNDED_CONTENT} \
         FROM document_blobs blob WHERE blob.sha256 = ?2"
    )
}

/// Inserts the document reference to a verified blob.
const INSERT_DOCUMENT_SQL: &str = "
        INSERT INTO document_documents
            (id, blob_id, original_file_name, media_type, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ";

/// Refreshes the canonical extraction of a document, keeping its identifier and references.
const SAVE_EXTRACTION_SQL: &str = "
                INSERT INTO document_pdf_extractions
                    (id, file_id, extractor, character_count, page_count, truncated,
                     extracted_text, page_projections, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT (file_id) DO UPDATE SET
                    extractor = excluded.extractor,
                    character_count = excluded.character_count,
                    page_count = excluded.page_count,
                    truncated = excluded.truncated,
                    extracted_text = excluded.extracted_text,
                    page_projections = excluded.page_projections,
                    created_at = excluded.created_at
                RETURNING id, file_id, extractor, character_count, page_count, truncated,
                          extracted_text, page_projections, created_at
                ";

/// The SQLite repository of stored documents and extraction records. It holds no state: each
/// call runs on the transaction handle that the use case passes in.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteDocumentRepository;

/// Composes the metadata SQL of one document, with the given where clause.
fn metadata_sql(where_clause: &str) -> String {
    format!(
        "SELECT {METADATA_COLUMNS} FROM document_documents file \
         JOIN document_blobs blob ON blob.id = file.blob_id WHERE {where_clause}"
    )
}

#[async_trait]
impl DocumentRepository for SqliteDocumentRepository {
    type Tx = SqliteTransaction;

    async fn save_file(
        &self,
        tx: &mut SqliteTransaction,
        metadata: &DocumentMetadata,
        content: &[u8],
    ) -> Result<DocumentMetadata, AppError> {
        with_connection(tx, |conn| save_file_in_transaction(conn, metadata, content))
    }

    async fn find_metadata(
        &self,
        tx: &mut SqliteTransaction,
        document_id: Uuid,
    ) -> Result<Option<DocumentMetadata>, AppError> {
        with_connection(tx, |conn| find_metadata(conn, document_id))
    }

    async fn find_content(
        &self,
        tx: &mut SqliteTransaction,
        document_id: Uuid,
    ) -> Result<Option<(DocumentMetadata, Vec<u8>)>, AppError> {
        with_connection(tx, |conn| find_content_in_transaction(conn, document_id))
    }

    async fn find_extraction(
        &self,
        tx: &mut SqliteTransaction,
        document_id: Uuid,
    ) -> Result<Option<ExtractionRecord>, AppError> {
        with_connection(tx, |conn| {
            let mut found = query_rows(
                conn,
                &format!(
                    "SELECT {EXTRACTION_COLUMNS} FROM document_pdf_extractions WHERE file_id = ?4"
                ),
                &[
                    &MAX_EXTRACTION_CHARACTERS,
                    &MAX_EXTRACTION_PAGES,
                    &i64::from(MAX_PAGE_JSON_BYTES),
                    &Id(document_id),
                ],
                extraction_from_row,
            )?;
            Ok(found.pop())
        })
    }

    async fn save_extraction(
        &self,
        tx: &mut SqliteTransaction,
        record: &ExtractionRecord,
    ) -> Result<ExtractionRecord, AppError> {
        with_connection(tx, |conn| save_extraction_in_transaction(conn, record))
    }
}

#[async_trait]
impl DocumentListRepository for SqliteDocumentRepository {
    type Tx = SqliteTransaction;

    async fn list_documents(
        &self,
        tx: &mut SqliteTransaction,
        request: &PageRequest<'_>,
    ) -> Result<Page<DocumentSummary>, AppError> {
        with_connection(tx, |conn| list::list_documents(conn, request))
    }
}

/// Deduplicates a verified blob and inserts the document reference. The caller owns the
/// transaction.
pub fn save_file_in_transaction(
    conn: &Connection,
    metadata: &DocumentMetadata,
    content: &[u8],
) -> Result<DocumentMetadata, AppError> {
    run_file_phase(|| validate_file_input(metadata, content))?;
    let created = instant_for_write(&metadata.created_at)?;
    let updated = instant_for_write(&metadata.updated_at)?;
    let byte_size = i64::try_from(content.len()).map_err(|_| AppError::Internal)?;
    // A returned row means this statement wrote the blob, whose bytes were just validated against
    // the metadata, so reading them back would only transfer and hash 10 MiB a second time. Only
    // a conflict, where the stored bytes come from an earlier transaction, is read and verified.
    let inserted = query_rows(
        conn,
        INSERT_BLOB_SQL,
        &[
            &Id(Uuid::new_v4()),
            &metadata.sha256,
            &byte_size,
            &content,
            &Instant(created),
        ],
        |row| Ok(column::<Id>(row, "id")?.0),
    )?;
    let blob_id: Uuid = if let Some(blob_id) = inserted.first() {
        *blob_id
    } else {
        let blobs = query_rows(
            conn,
            &select_blob_sql(),
            &[&MAX_DOCUMENT_BYTES, &metadata.sha256],
            |blob| {
                let stored_size: i64 = column(blob, "byte_size")?;
                let stored_sha: String = column(blob, "sha256")?;
                let stored = column(blob, "content")?;
                Ok((column::<Id>(blob, "id")?.0, stored_size, stored_sha, stored))
            },
        )?;
        let (blob_id, stored_size, stored_sha, stored) =
            blobs.into_iter().next().ok_or_else(unavailable)?;
        let verified = run_file_phase(|| verified_content(stored, stored_size, &stored_sha))?;
        if verified != content {
            return Err(unavailable());
        }
        blob_id
    };
    execute(
        conn,
        INSERT_DOCUMENT_SQL,
        &[
            &Id(metadata.id),
            &Id(blob_id),
            &metadata.original_file_name,
            &metadata.media_type,
            &Instant(created),
            &Instant(updated),
        ],
    )?;
    find_metadata(conn, metadata.id)?.ok_or_else(unavailable)
}

/// Reads the metadata and the verified bytes of one document. The caller owns the transaction.
pub fn find_content_in_transaction(
    conn: &Connection,
    document_id: Uuid,
) -> Result<Option<(DocumentMetadata, Vec<u8>)>, AppError> {
    let mut found = query_rows(
        conn,
        &format!(
            "SELECT {METADATA_COLUMNS}, {BOUNDED_CONTENT} FROM document_documents file \
             JOIN document_blobs blob ON blob.id = file.blob_id WHERE file.id = ?2"
        ),
        &[&MAX_DOCUMENT_BYTES, &Id(document_id)],
        |row| {
            let metadata = metadata_from_row(row)?;
            let raw = column(row, "content")?;
            Ok((metadata, raw))
        },
    )?;
    let Some((metadata, raw)) = found.pop() else {
        return Ok(None);
    };
    let content = run_file_phase(|| verified_content(raw, metadata.byte_size, &metadata.sha256))?;
    Ok(Some((metadata, content)))
}

/// Reads the metadata of one document.
fn find_metadata(
    conn: &Connection,
    document_id: Uuid,
) -> Result<Option<DocumentMetadata>, AppError> {
    let mut found = query_rows(
        conn,
        &metadata_sql("file.id = ?1"),
        &[&Id(document_id)],
        metadata_from_row,
    )?;
    Ok(found.pop())
}

/// Writes the canonical extraction of a document, upserting on the document identifier. The caller
/// owns the transaction.
pub fn save_extraction_in_transaction(
    conn: &Connection,
    record: &ExtractionRecord,
) -> Result<ExtractionRecord, AppError> {
    validate_extraction_input(record)?;
    let character_count = record.character_count;
    let page_count = i64::from(record.page_count);
    let created = instant_for_write(&record.created_at)?;
    let projections = pages_to_json(&record.pages).to_string();
    // The PostgreSQL guard trigger on the extraction's file: an owned generated document is
    // attachable only while its asset is ACTIVE (rule 14 of `RULES_TO_PORT.md`).
    assert_generated_document_attachable(
        conn,
        record.file_id,
        "owned_pdf_extraction_reference_guard",
    )?;
    let mut saved = query_rows(
        conn,
        SAVE_EXTRACTION_SQL,
        &[
            &Id(record.id),
            &Id(record.file_id),
            &record.extractor,
            &character_count,
            &page_count,
            &record.truncated,
            &record.extracted_text,
            &projections,
            &Instant(created),
        ],
        extraction_from_row,
    )?;
    saved.pop().ok_or_else(unavailable)
}
