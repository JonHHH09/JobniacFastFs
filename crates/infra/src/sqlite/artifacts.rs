//! SQLite adapter of the document artifact read.
//!
//! The SQL bounds the bytes at the 1 MiB artifact limit before hydration: the content column is
//! NULL above the limit, and a declared size above the limit is classified as oversize without
//! reading the content. Metadata and bytes are then checked against the core rules. Adapter text
//! never leaves this module: every failure becomes an [`ArtifactLookupError`].

use async_trait::async_trait;
use jobniac_core::artifacts::{
    ArtifactLookupError, ArtifactRepository, DocumentArtifact, validate_artifact,
};
use jobniac_core::clock::{format_instant, parse_instant};
use jobniac_core::documents::DocumentMetadata;
use jobniac_core::errors::{AppError, ErrorCode};
use rusqlite::OptionalExtension;
use uuid::Uuid;

use super::unit_of_work::SqliteTransaction;

/// The artifact read: the document, its blob and the bounded content, by identifier. Name and
/// media type are NULL above their bounds (`length` of a cast blob counts bytes).
const FIND_ARTIFACT_SQL: &str = "SELECT
    file.id, file.blob_id,
    CASE WHEN length(CAST(file.original_file_name AS BLOB)) <= 4096
         THEN file.original_file_name END AS original_file_name,
    CASE WHEN length(CAST(file.media_type AS BLOB)) <= 255
         THEN file.media_type END AS media_type,
    file.created_at, file.updated_at, blob.byte_size, blob.sha256,
    CASE WHEN blob.byte_size BETWEEN 1 AND ?1
              AND length(blob.content) = blob.byte_size
         THEN blob.content END AS content
FROM document_documents file
JOIN document_blobs blob ON blob.id = file.blob_id
WHERE file.id = ?2";

/// The bound of the artifact read in SQL, as the bind value (`MAX_ARTIFACT_BYTES`).
const ARTIFACT_LIMIT: i64 = 1024 * 1024;

/// The stored row, before it is checked.
struct ArtifactRow {
    id: String,
    original_file_name: Option<String>,
    media_type: Option<String>,
    created_at: String,
    updated_at: String,
    byte_size: i64,
    sha256: String,
    content: Option<Vec<u8>>,
}

/// The SQLite artifact repository. It holds no state; every call runs in the caller's
/// transaction.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteArtifactRepository;

#[async_trait]
impl ArtifactRepository for SqliteArtifactRepository {
    type Tx = SqliteTransaction;

    async fn find_artifact_content(
        &self,
        tx: &mut SqliteTransaction,
        document_id: Uuid,
    ) -> Result<Option<(DocumentMetadata, Vec<u8>)>, ArtifactLookupError> {
        let id = document_id.to_string();
        let row = tx
            .run(|conn| {
                Ok(conn
                    .query_row(
                        FIND_ARTIFACT_SQL,
                        rusqlite::params![ARTIFACT_LIMIT, id],
                        |row| {
                            Ok(ArtifactRow {
                                id: row.get("id")?,
                                original_file_name: row.get("original_file_name")?,
                                media_type: row.get("media_type")?,
                                created_at: row.get("created_at")?,
                                updated_at: row.get("updated_at")?,
                                byte_size: row.get("byte_size")?,
                                sha256: row.get("sha256")?,
                                content: row.get("content")?,
                            })
                        },
                    )
                    .optional()?)
            })
            .map_err(|_| ArtifactLookupError::Unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        if row.byte_size > ARTIFACT_LIMIT {
            return Err(ArtifactLookupError::Oversized);
        }
        let metadata = metadata_from_row(&row).ok_or(ArtifactLookupError::Unavailable)?;
        let checked = validate_artifact(DocumentArtifact {
            document: metadata,
            content: row.content.unwrap_or_default(),
        })
        .map_err(|error| match error {
            AppError::Application(application)
                if application.code() == ErrorCode::ValidationError =>
            {
                ArtifactLookupError::Oversized
            }
            _ => ArtifactLookupError::Failed,
        })?;
        if checked.document.id != document_id {
            return Err(ArtifactLookupError::Unavailable);
        }
        Ok(Some((checked.document, checked.content)))
    }
}

/// The wire metadata of a stored row; `None` for a NULL (over-bound) name or media type, or an
/// unreadable identifier or instant. The core validation re-checks the remaining bounds.
fn metadata_from_row(row: &ArtifactRow) -> Option<DocumentMetadata> {
    let created = parse_instant(&row.created_at).ok()?;
    let updated = parse_instant(&row.updated_at).ok()?;
    Some(DocumentMetadata {
        id: Uuid::parse_str(&row.id).ok()?,
        original_file_name: row.original_file_name.clone()?,
        media_type: row.media_type.clone()?,
        byte_size: row.byte_size,
        sha256: row.sha256.clone(),
        created_at: format_instant(created),
        updated_at: format_instant(updated),
    })
}
