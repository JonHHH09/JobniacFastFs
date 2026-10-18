//! The two document reads and writes of the generated-asset adapters: storing a verified PDF as a
//! blob and a document, and reading it back with its size and digest checked.
//!
//! These are the statements of the PostgreSQL document repository (`save_file_in_transaction` and
//! `find_content`) reduced to what generation needs, so this module does not depend on the document
//! repository port.

use jobniac_core::clock::{format_instant, parse_instant};
use jobniac_core::documents::DocumentMetadata;
use jobniac_core::errors::{AppError, ApplicationError};
use jobniac_core::generated_assets::sha256_hex;
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use super::codec::{id, parse_id, parse_ts, ts};
use crate::sqlite::error::SqliteError;

/// The upper bound of a stored document, in bytes (10 MiB).
const MAX_DOCUMENT_BYTES: i64 = 10 * 1024 * 1024;

fn unavailable() -> AppError {
    ApplicationError::database_unavailable().into()
}

fn stored_instant(text: &str) -> Result<String, AppError> {
    parse_instant(text).map(ts)
}

/// Deduplicates a blob by digest and inserts the document reference. The caller owns the
/// transaction and has validated the bytes against the metadata.
///
/// # Errors
///
/// Fails with the classified database error, or the unavailable error when the stored blob of the
/// digest differs from `content`.
pub fn save_file(
    conn: &Connection,
    metadata: &DocumentMetadata,
    content: &[u8],
) -> Result<(), AppError> {
    let created = stored_instant(&metadata.created_at)?;
    let updated = stored_instant(&metadata.updated_at)?;
    let byte_size = i64::try_from(content.len()).map_err(|_| AppError::Internal)?;
    let inserted: Option<String> = conn
        .query_row(
            "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (sha256) DO NOTHING RETURNING id",
            params![
                id(Uuid::new_v4()),
                metadata.sha256,
                byte_size,
                content,
                created
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(SqliteError::from)?;
    let blob_id = if let Some(blob_id) = inserted {
        blob_id
    } else {
        // A conflict: the stored bytes come from another transaction, so they are verified.
        let (blob_id, stored_size, stored_sha, stored): (String, i64, String, Option<Vec<u8>>) =
            conn.query_row(
                "SELECT id, byte_size, sha256,
                        CASE WHEN byte_size BETWEEN 1 AND ?1 AND length(content) = byte_size
                             THEN content END
                 FROM document_blobs WHERE sha256 = ?2",
                params![MAX_DOCUMENT_BYTES, metadata.sha256],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(SqliteError::from)?
            .ok_or_else(unavailable)?;
        let verified = verified_content(stored, stored_size, &stored_sha)?;
        if verified != content {
            return Err(unavailable());
        }
        blob_id
    };
    conn.execute(
        "INSERT INTO document_documents
             (id, blob_id, original_file_name, media_type, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            id(metadata.id),
            blob_id,
            metadata.original_file_name,
            metadata.media_type,
            created,
            updated
        ],
    )
    .map_err(SqliteError::from)?;
    Ok(())
}

/// Reads the metadata and the verified bytes of one document.
///
/// # Errors
///
/// Fails with the unavailable error when the stored bytes do not match their size or digest.
pub fn find_content(
    conn: &Connection,
    document_id: Uuid,
) -> Result<Option<(DocumentMetadata, Vec<u8>)>, AppError> {
    let row = conn
        .query_row(
            "SELECT file.id, file.original_file_name, file.media_type, file.created_at,
                    file.updated_at, blob.byte_size, blob.sha256,
                    CASE WHEN blob.byte_size BETWEEN 1 AND ?1
                              AND length(blob.content) = blob.byte_size
                         THEN blob.content END
             FROM document_documents file JOIN document_blobs blob ON blob.id = file.blob_id
             WHERE file.id = ?2",
            params![MAX_DOCUMENT_BYTES, id(document_id)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<Vec<u8>>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(SqliteError::from)?;
    let Some((doc_id, file_name, media_type, created, updated, byte_size, sha256, content)) = row
    else {
        return Ok(None);
    };
    let metadata = DocumentMetadata {
        id: parse_id(&doc_id)?,
        original_file_name: file_name,
        media_type,
        byte_size,
        sha256,
        created_at: format_instant(parse_ts(&created)?),
        updated_at: format_instant(parse_ts(&updated)?),
    };
    let content = verified_content(content, metadata.byte_size, &metadata.sha256)?;
    Ok(Some((metadata, content)))
}

/// Verifies bytes read from a blob row against its size and digest.
fn verified_content(
    content: Option<Vec<u8>>,
    byte_size: i64,
    sha256: &str,
) -> Result<Vec<u8>, AppError> {
    let content = content.ok_or_else(unavailable)?;
    if !content.is_empty()
        && i64::try_from(content.len()).ok() == Some(byte_size)
        && sha256_hex(&content) == sha256
    {
        Ok(content)
    } else {
        Err(unavailable())
    }
}
