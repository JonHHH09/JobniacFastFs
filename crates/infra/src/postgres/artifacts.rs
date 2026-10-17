//! PostgreSQL adapter of the document artifact read.
//!
//! The SQL bounds the bytes at the 1 MiB artifact limit before hydration: the content column is
//! NULL above the limit, and a declared size above the limit is classified as oversize without
//! reading the content. Metadata and bytes are then checked against the core rules. Adapter text
//! never leaves this module: every failure becomes an [`ArtifactLookupError`].

use async_trait::async_trait;
use jobniac_core::artifacts::{
    ArtifactLookupError, ArtifactRepository, DocumentArtifact, validate_artifact,
};
use jobniac_core::documents::DocumentMetadata;
use jobniac_core::errors::{AppError, ErrorCode};
use uuid::Uuid;

use super::documents::rows::{column, metadata_from_row};
use super::unit_of_work::PgTransaction;

/// The artifact read: the document, its blob and the bounded content, by identifier.
const FIND_ARTIFACT_SQL: &str = "SELECT
    file.id, file.blob_id,
    CASE WHEN octet_length(file.original_file_name) <= 4096
         THEN file.original_file_name END AS original_file_name,
    CASE WHEN octet_length(file.media_type) <= 255
         THEN file.media_type END AS media_type,
    file.created_at, file.updated_at, blob.byte_size, blob.sha256,
    CASE WHEN blob.byte_size BETWEEN 1 AND $1
              AND octet_length(blob.content) = blob.byte_size
         THEN blob.content END AS content
FROM document.documents file
JOIN document.blobs blob ON blob.id = file.blob_id
WHERE file.id = $2";

/// The bound of the artifact read in SQL, as the bind value (`MAX_ARTIFACT_BYTES`).
const ARTIFACT_LIMIT: i64 = 1024 * 1024;

/// The PostgreSQL artifact repository. It holds no state; every call runs in the caller's
/// transaction.
#[derive(Debug, Clone, Copy, Default)]
pub struct PgArtifactRepository;

#[async_trait]
impl ArtifactRepository for PgArtifactRepository {
    type Tx = PgTransaction;

    async fn find_artifact_content(
        &self,
        tx: &mut PgTransaction,
        document_id: Uuid,
    ) -> Result<Option<(DocumentMetadata, Vec<u8>)>, ArtifactLookupError> {
        let rows = tx
            .client()
            .query(FIND_ARTIFACT_SQL, &[&ARTIFACT_LIMIT, &document_id])
            .await
            .map_err(|_| ArtifactLookupError::Unavailable)?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let byte_size: i64 =
            column(row, "byte_size").map_err(|_| ArtifactLookupError::Unavailable)?;
        if byte_size > ARTIFACT_LIMIT {
            return Err(ArtifactLookupError::Oversized);
        }
        let metadata = metadata_from_row(row).map_err(|_| ArtifactLookupError::Unavailable)?;
        let content: Option<Vec<u8>> =
            column(row, "content").map_err(|_| ArtifactLookupError::Unavailable)?;
        let checked = validate_artifact(DocumentArtifact {
            document: metadata,
            content: content.unwrap_or_default(),
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
