//! Adapters that run CPU-bound core work in a blocking context (audit findings H089 and H019).
//!
//! The core is runtime-free, so it calls its renderer and digest ports from async code. These
//! adapters implement the ports over the core implementations and run each call through
//! [`jobniac_infra::blocking::run_blocking`], so a German PDF render or a SHA-256 over up to 10 MiB
//! no longer parks an async worker. Behaviour and results are unchanged.

use jobniac_core::documents::{ContentHasher, sha256_hex};
use jobniac_core::errors::ApplicationError;
use jobniac_core::german::content::{GermanCoverLetterContent, StructuredResumeContent};
use jobniac_core::german::generation::{GermanPdfRenderer, PdfGermanRenderer, RenderedGermanPdf};
use jobniac_infra::blocking::run_blocking;

/// The German PDF renderer of the core, run in a blocking context.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockingGermanRenderer {
    inner: PdfGermanRenderer,
}

impl GermanPdfRenderer for BlockingGermanRenderer {
    fn render_resume(
        &self,
        content: &StructuredResumeContent,
    ) -> Result<RenderedGermanPdf, ApplicationError> {
        run_blocking(|| self.inner.render_resume(content))
    }

    fn render_cover_letter(
        &self,
        content: &GermanCoverLetterContent,
    ) -> Result<RenderedGermanPdf, ApplicationError> {
        run_blocking(|| self.inner.render_cover_letter(content))
    }
}

/// The SHA-256 digest of document content, computed in a blocking context.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockingHasher;

impl ContentHasher for BlockingHasher {
    fn sha256_hex(&self, content: &[u8]) -> String {
        run_blocking(|| sha256_hex(content))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use super::*;

    /// A digest of 16 MiB takes long enough that a sibling task on the only worker would be
    /// starved for the whole time if the hash ran on the worker.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn hashing_does_not_starve_a_sibling_task() {
        let content = vec![7_u8; 16 * 1024 * 1024];
        let ran = Arc::new(AtomicBool::new(false));
        let sibling = Arc::clone(&ran);
        let handle = tokio::spawn(async move { sibling.store(true, Ordering::SeqCst) });
        let flag = Arc::clone(&ran);
        // The hash runs on this task; the sibling must have run by the time it returns, which
        // needs the worker to have been handed over. A second thread watches the flag.
        let watcher = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !flag.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            flag.load(Ordering::SeqCst)
        });
        let digest = BlockingHasher.sha256_hex(&content);
        assert_eq!(digest, sha256_hex(&content));
        assert!(watcher.join().expect("watcher"), "the sibling task ran");
        handle.await.expect("the sibling task finishes");
    }

    #[test]
    fn the_blocking_digest_matches_the_inline_digest() {
        assert_eq!(BlockingHasher.sha256_hex(b"abc"), sha256_hex(b"abc"));
    }
}
