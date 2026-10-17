//! The persistence port of document artifacts.
//!
//! The adapter bounds the bytes in SQL before hydration, independently of the 10 MiB extraction
//! path, and performs the same integrity checks as [`super::model::validate_artifact`] before it
//! returns. Its failures are classified so the service can answer with the fixed messages
//! without looking at any adapter text.

use async_trait::async_trait;
use uuid::Uuid;

use crate::documents::model::DocumentMetadata;

/// The classified failures of the artifact lookup. No adapter text is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactLookupError {
    /// The database is not configured, unreachable or returned an unusable row.
    Unavailable,
    /// The stored size is above the 1 MiB artifact limit.
    Oversized,
    /// The row failed its integrity checks or the adapter failed.
    Failed,
}

/// Persistence port of the artifact read. Every call takes the transaction handle that the
/// service opens, so no connection is ambient.
#[async_trait]
pub trait ArtifactRepository: Send + Sync {
    /// The open transaction handle that repository calls take.
    type Tx: Send;

    /// The metadata and bytes of one stored document, when the stored size is within the artifact
    /// limit. `Ok(None)` means the document does not exist.
    async fn find_artifact_content(
        &self,
        tx: &mut Self::Tx,
        document_id: Uuid,
    ) -> Result<Option<(DocumentMetadata, Vec<u8>)>, ArtifactLookupError>;
}
