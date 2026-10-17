//! PostgreSQL adapter of the document ports.
//!
//! The repository keeps the original bytes private: bytes are selected only inside Rust, only when
//! their size is within the 10 MiB bound, and only after their size and SHA-256 are verified. A new
//! document never adopts an existing identity. The SQL uses `$n` binds; the schema is qualified
//! because the session pins `search_path` to `pg_catalog`.
//!
//! Lock order: `save_file` inserts the blob (`ON CONFLICT (sha256) DO NOTHING`), then reads it
//! with `FOR SHARE OF blob` so a concurrent content update cannot change verified bytes, then
//! inserts the document reference. Every port method runs in the transaction the use case opened.

mod list;
pub mod rows;

use async_trait::async_trait;
use jobniac_core::documents::{
    DocumentListRepository, DocumentMetadata, DocumentRepository, DocumentSummary, ExtractionRecord,
};
use jobniac_core::errors::AppError;
use jobniac_core::pagination::{Page, PageRequest};
use tokio_postgres::Client;
use uuid::Uuid;

use crate::postgres::generated::writer::run_file_phase;
use crate::postgres::profiles::{db, unavailable};
use crate::postgres::unit_of_work::PgTransaction;

use rows::{
    MAX_PAGE_JSON_BYTES, column, extraction_from_row, instant_for_write, metadata_from_row,
    pages_to_json, validate_extraction_input, validate_file_input, verified_content,
};

/// The upper bound of a stored document, in bytes (`MAX_DOCUMENT_BYTES`, 10 MiB).
const MAX_DOCUMENT_BYTES: i64 = 10 * 1024 * 1024;
/// The bound of a canonical extraction's characters (`MAX_EXTRACTION_CHARACTERS`).
const MAX_EXTRACTION_CHARACTERS: i32 = 250_000;
/// The bound of a canonical extraction's pages (`MAX_EXTRACTION_PAGES`).
const MAX_EXTRACTION_PAGES: i32 = 200;

/// The metadata columns of a document joined to its blob.
const METADATA_COLUMNS: &str = "
    file.id, file.blob_id,
    CASE WHEN octet_length(file.original_file_name) <= 4096
         THEN file.original_file_name END AS original_file_name,
    CASE WHEN octet_length(file.media_type) <= 255
         THEN file.media_type END AS media_type,
    file.created_at, file.updated_at, blob.byte_size, blob.sha256
";

/// The bounded content column, with the size bound at `$1`.
const BOUNDED_CONTENT: &str = "
    CASE WHEN blob.byte_size BETWEEN 1 AND $1
              AND octet_length(blob.content) = blob.byte_size
         THEN blob.content END AS content
";

/// The extraction columns with the three bounds at `$1` to `$3`.
const EXTRACTION_COLUMNS: &str = "
    id, file_id, character_count, page_count, truncated, created_at,
    CASE WHEN octet_length(extractor) <= 255 THEN extractor END AS extractor,
    CASE WHEN char_length(extracted_text) <= $1
         THEN extracted_text END AS extracted_text,
    CASE WHEN jsonb_typeof(page_projections) = 'array'
              AND jsonb_array_length(page_projections) <= $2
              AND octet_length(page_projections::text) <= $3
         THEN page_projections END AS page_projections
";

/// Inserts a blob or keeps the existing row with the same digest (`ON CONFLICT DO NOTHING`).
const INSERT_BLOB_SQL: &str = "
        INSERT INTO document.blobs (id, sha256, byte_size, content, created_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (sha256) DO NOTHING
        RETURNING id
        ";

/// Reads the winning blob of a digest with its bounded content, locking it for the rest of the
/// transaction (`FOR SHARE OF blob`). The size bound is at `$1`, the digest at `$2`.
fn select_blob_sql() -> String {
    format!(
        "SELECT blob.id, blob.byte_size, blob.sha256, {BOUNDED_CONTENT} \
         FROM document.blobs blob WHERE blob.sha256 = $2 FOR SHARE OF blob"
    )
}

/// Inserts the document reference to a verified blob.
const INSERT_DOCUMENT_SQL: &str = "
        INSERT INTO document.documents
            (id, blob_id, original_file_name, media_type, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        ";

/// Refreshes the canonical extraction of a document, keeping its identifier and references.
const SAVE_EXTRACTION_SQL: &str = "
                INSERT INTO document.pdf_extractions
                    (id, file_id, extractor, character_count, page_count, truncated,
                     extracted_text, page_projections, created_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                ON CONFLICT (file_id) DO UPDATE SET
                    extractor = EXCLUDED.extractor,
                    character_count = EXCLUDED.character_count,
                    page_count = EXCLUDED.page_count,
                    truncated = EXCLUDED.truncated,
                    extracted_text = EXCLUDED.extracted_text,
                    page_projections = EXCLUDED.page_projections,
                    created_at = EXCLUDED.created_at
                RETURNING id, file_id, extractor, character_count, page_count, truncated,
                          extracted_text, page_projections, created_at
                ";

/// The PostgreSQL repository of stored documents and extraction records. It holds no state: each
/// call runs on the transaction handle that the use case passes in.
#[derive(Debug, Clone, Copy, Default)]
pub struct PgDocumentRepository;

/// Composes the metadata SQL of one document, with the given where clause.
fn metadata_sql(where_clause: &str) -> String {
    format!(
        "SELECT {METADATA_COLUMNS} FROM document.documents file \
         JOIN document.blobs blob ON blob.id = file.blob_id WHERE {where_clause}"
    )
}

#[async_trait]
impl DocumentRepository for PgDocumentRepository {
    type Tx = PgTransaction;

    async fn save_file(
        &self,
        tx: &mut PgTransaction,
        metadata: &DocumentMetadata,
        content: &[u8],
    ) -> Result<DocumentMetadata, AppError> {
        save_file_in_transaction(tx.client(), metadata, content).await
    }

    async fn find_metadata(
        &self,
        tx: &mut PgTransaction,
        document_id: Uuid,
    ) -> Result<Option<DocumentMetadata>, AppError> {
        find_metadata(tx.client(), document_id).await
    }

    async fn find_content(
        &self,
        tx: &mut PgTransaction,
        document_id: Uuid,
    ) -> Result<Option<(DocumentMetadata, Vec<u8>)>, AppError> {
        let rows = db(tx
            .client()
            .query(
                &format!(
                    "SELECT {METADATA_COLUMNS}, {BOUNDED_CONTENT} FROM document.documents file \
                     JOIN document.blobs blob ON blob.id = file.blob_id WHERE file.id = $2"
                ),
                &[&MAX_DOCUMENT_BYTES, &document_id],
            )
            .await)?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let metadata = metadata_from_row(row)?;
        let raw = column(row, "content")?;
        let content =
            run_file_phase(|| verified_content(raw, metadata.byte_size, &metadata.sha256))?;
        Ok(Some((metadata, content)))
    }

    async fn find_extraction(
        &self,
        tx: &mut PgTransaction,
        document_id: Uuid,
    ) -> Result<Option<ExtractionRecord>, AppError> {
        let rows = db(tx
            .client()
            .query(
                &format!(
                    "SELECT {EXTRACTION_COLUMNS} FROM document.pdf_extractions WHERE file_id = $4"
                ),
                &[
                    &MAX_EXTRACTION_CHARACTERS,
                    &MAX_EXTRACTION_PAGES,
                    &MAX_PAGE_JSON_BYTES,
                    &document_id,
                ],
            )
            .await)?;
        rows.first().map(extraction_from_row).transpose()
    }

    async fn save_extraction(
        &self,
        tx: &mut PgTransaction,
        record: &ExtractionRecord,
    ) -> Result<ExtractionRecord, AppError> {
        save_extraction_in_transaction(tx.client(), record).await
    }
}

#[async_trait]
impl DocumentListRepository for PgDocumentRepository {
    type Tx = PgTransaction;

    async fn list_documents(
        &self,
        tx: &mut PgTransaction,
        request: &PageRequest<'_>,
    ) -> Result<Page<DocumentSummary>, AppError> {
        list::list_documents(tx.client(), request).await
    }
}

/// Deduplicates a verified blob and inserts the document reference. The caller owns the
/// transaction.
pub async fn save_file_in_transaction(
    client: &Client,
    metadata: &DocumentMetadata,
    content: &[u8],
) -> Result<DocumentMetadata, AppError> {
    run_file_phase(|| validate_file_input(metadata, content))?;
    let created = instant_for_write(&metadata.created_at)?;
    let updated = instant_for_write(&metadata.updated_at)?;
    let byte_size = i64::try_from(content.len()).map_err(|_| AppError::Internal)?;
    // A returned row means this statement wrote the blob, whose bytes were just validated against
    // the metadata, so reading them back would only transfer and hash 10 MiB a second time. Only
    // a conflict, where the stored bytes come from another transaction, is read and verified.
    let inserted = db(client
        .query(
            INSERT_BLOB_SQL,
            &[
                &Uuid::new_v4(),
                &metadata.sha256,
                &byte_size,
                &content,
                &created,
            ],
        )
        .await)?;
    let blob_id: Uuid = if let Some(row) = inserted.first() {
        column(row, "id")?
    } else {
        let blobs = db(client
            .query(&select_blob_sql(), &[&MAX_DOCUMENT_BYTES, &metadata.sha256])
            .await)?;
        let blob = blobs.first().ok_or_else(unavailable)?;
        let stored_size: i64 = column(blob, "byte_size")?;
        let stored_sha: String = column(blob, "sha256")?;
        let stored = column(blob, "content")?;
        let verified = run_file_phase(|| verified_content(stored, stored_size, &stored_sha))?;
        if verified != content {
            return Err(unavailable());
        }
        column(blob, "id")?
    };
    db(client
        .execute(
            INSERT_DOCUMENT_SQL,
            &[
                &metadata.id,
                &blob_id,
                &metadata.original_file_name,
                &metadata.media_type,
                &created,
                &updated,
            ],
        )
        .await)?;
    find_metadata(client, metadata.id)
        .await?
        .ok_or_else(unavailable)
}

/// Reads the metadata of one document.
async fn find_metadata(
    client: &Client,
    document_id: Uuid,
) -> Result<Option<DocumentMetadata>, AppError> {
    let rows = db(client
        .query(&metadata_sql("file.id = $1"), &[&document_id])
        .await)?;
    rows.first().map(metadata_from_row).transpose()
}

/// Writes the canonical extraction of a document, upserting on the document identifier. The caller
/// owns the transaction.
pub async fn save_extraction_in_transaction(
    client: &Client,
    record: &ExtractionRecord,
) -> Result<ExtractionRecord, AppError> {
    validate_extraction_input(record)?;
    let character_count = i32::try_from(record.character_count).map_err(|_| AppError::Internal)?;
    let page_count = i32::try_from(record.page_count).map_err(|_| AppError::Internal)?;
    let created = instant_for_write(&record.created_at)?;
    let projections = pages_to_json(&record.pages);
    let rows = db(client
        .query(
            SAVE_EXTRACTION_SQL,
            &[
                &record.id,
                &record.file_id,
                &record.extractor,
                &character_count,
                &page_count,
                &record.truncated,
                &record.extracted_text,
                &projections,
                &created,
            ],
        )
        .await)?;
    let row = rows.first().ok_or_else(unavailable)?;
    extraction_from_row(row)
}
