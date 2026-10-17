//! The page render of a verified artifact. It renders the cached verified
//! bytes of the viewer, never a second read of the stored row.
//!
//! The caller holds the [`DocumentArtifact`] that [`super::ArtifactService::read_artifact`] verified.
//! This use case renders pages from those bytes through the PDF renderer port, so the metadata shown
//! and the page rendered come from one read, and a later change of the stored row cannot alter them.

use std::sync::Arc;

use crate::documents::model::{MAX_PAGE_COUNT, PAGE_VALIDATION_MESSAGE};
use crate::documents::repository::{PdfRenderer, RenderedPdfPage};
use crate::documents::service::sha256_hex;
use crate::documents::view::{document_validation, page_index_in_range};
use crate::errors::AppError;

use super::model::{DocumentArtifact, integrity_error};

/// Renders pages of verified artifacts through the PDF renderer port.
#[derive(Debug, Clone)]
pub struct ArtifactPageRenderer {
    renderer: Arc<dyn PdfRenderer>,
}

impl ArtifactPageRenderer {
    /// A page renderer over the PDF renderer port (the worker renderer in the composed application).
    #[must_use]
    pub fn new(renderer: Arc<dyn PdfRenderer>) -> Self {
        Self { renderer }
    }

    /// Renders the zero-based `page_index` of `artifact` to PNG. The bytes are checked against the
    /// digest and size they were verified with before the renderer sees them, and the index is
    /// checked against the page count the renderer reports.
    ///
    /// # Errors
    ///
    /// A validation error for an index the document does not have, the internal integrity error when
    /// the held bytes no longer match their digest or size, or the renderer's own failure.
    pub async fn render_page(
        &self,
        artifact: &DocumentArtifact,
        page_index: u32,
    ) -> Result<RenderedPdfPage, AppError> {
        if page_index >= MAX_PAGE_COUNT {
            return Err(page_index_refusal());
        }
        let size_matches =
            u64::try_from(artifact.document.byte_size).ok() == Some(artifact.content.len() as u64);
        if !size_matches || sha256_hex(&artifact.content) != artifact.document.sha256 {
            return Err(integrity_error());
        }
        let rendered = self.renderer.render(&artifact.content, page_index).await?;
        if !page_index_in_range(page_index, rendered.page_count) {
            return Err(page_index_refusal());
        }
        Ok(rendered)
    }
}

/// The refusal of a page index the document does not have (the documents viewer's copy).
fn page_index_refusal() -> AppError {
    document_validation(
        PAGE_VALIDATION_MESSAGE,
        "pageIndex",
        "page index is outside the document",
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use uuid::Uuid;

    use super::*;
    use crate::documents::model::DocumentMetadata;
    use crate::errors::{ApplicationError, ErrorCode};

    /// Synthetic PDF bytes; the fake renderer never parses them.
    const CONTENT: &[u8] = b"%PDF-1.4\nsynthetic\n";

    /// Answers every render with a fixed page count and records the bytes it was given.
    #[derive(Debug)]
    struct FakeRenderer {
        page_count: u32,
        fail: bool,
        seen: Mutex<Vec<(Vec<u8>, u32)>>,
    }

    #[async_trait]
    impl PdfRenderer for FakeRenderer {
        async fn render(
            &self,
            content: &[u8],
            page_index: u32,
        ) -> Result<RenderedPdfPage, AppError> {
            if let Ok(mut seen) = self.seen.lock() {
                seen.push((content.to_vec(), page_index));
            }
            if self.fail {
                return Err(AppError::Application(ApplicationError::with_message(
                    ErrorCode::InternalError,
                    "fixed render failure",
                )));
            }
            Ok(RenderedPdfPage {
                page_count: self.page_count,
                width: 8,
                height: 6,
                png: vec![0x89, b'P', b'N', b'G'],
            })
        }
    }

    fn artifact(content: &[u8]) -> DocumentArtifact {
        DocumentArtifact {
            document: DocumentMetadata {
                id: Uuid::nil(),
                original_file_name: "Example CV.pdf".to_owned(),
                media_type: "application/pdf".to_owned(),
                byte_size: i64::try_from(content.len()).unwrap_or(0),
                sha256: sha256_hex(content),
                created_at: "2026-01-01T00:00:00Z".to_owned(),
                updated_at: "2026-01-01T00:00:00Z".to_owned(),
            },
            content: content.to_vec(),
        }
    }

    fn renderer(page_count: u32, fail: bool) -> (Arc<FakeRenderer>, ArtifactPageRenderer) {
        let fake = Arc::new(FakeRenderer {
            page_count,
            fail,
            seen: Mutex::new(Vec::new()),
        });
        let port: Arc<dyn PdfRenderer> = fake.clone();
        (fake, ArtifactPageRenderer::new(port))
    }

    #[tokio::test]
    async fn a_page_is_rendered_from_the_held_verified_bytes() {
        let (fake, pages) = renderer(3, false);
        let held = artifact(CONTENT);
        let rendered = pages.render_page(&held, 1).await;
        assert!(rendered.is_ok_and(|page| page.page_count == 3));
        let seen = fake
            .seen
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default();
        assert_eq!(seen, vec![(CONTENT.to_vec(), 1)]);
    }

    #[tokio::test]
    async fn bytes_that_no_longer_match_their_digest_are_refused_before_rendering() {
        let (fake, pages) = renderer(1, false);
        let mut tampered = artifact(CONTENT);
        tampered.content = b"%PDF-1.4\nchanged\n".to_vec();
        let refused = pages.render_page(&tampered, 0).await;
        assert!(refused.is_err());
        let mut resized = artifact(CONTENT);
        resized.document.byte_size += 1;
        assert!(pages.render_page(&resized, 0).await.is_err());
        assert!(
            fake.seen
                .lock()
                .map(|seen| seen.is_empty())
                .unwrap_or(false)
        );
    }

    #[tokio::test]
    async fn an_index_outside_the_document_or_the_limit_is_refused() {
        let (_fake, pages) = renderer(2, false);
        let held = artifact(CONTENT);
        assert!(pages.render_page(&held, 2).await.is_err());
        assert!(pages.render_page(&held, MAX_PAGE_COUNT).await.is_err());
        assert!(pages.render_page(&held, 1).await.is_ok());
    }

    #[tokio::test]
    async fn a_renderer_failure_is_passed_through() {
        let (_fake, pages) = renderer(1, true);
        assert!(pages.render_page(&artifact(CONTENT), 0).await.is_err());
    }
}
