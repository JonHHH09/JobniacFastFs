//! The parent-side adapters of the sandboxed fetch workers.
//!
//! [`WorkerLinkFetcher`] implements the core fetch port [`JobLinkContentFetcher`] and
//! [`WorkerBoardTransport`] the Arbeitnow transport of `jobniac_infra::workers::fetch::board`. Both
//! run one child of the supervisor per call: the child is attached to its containment before it
//! reads its request, the request carries only the URL or the page and visa fields, and every
//! failure maps to the fixed classification of the port.
//!
//! The child entry point is `run_from_process` for the two kinds; the `__worker` mode of both
//! binaries dispatches to it (see [`crate::worker`]).
//!
//! Cancellation: [`WorkerLinkFetcher::new`] takes an optional cancellation check. When it is given,
//! a check that reports true before the child starts returns [`JobLinkFetchFailure::Interrupted`]
//! without spawning anything, and a check that turns true while the child runs makes the supervisor
//! kill the child tree and return the same failure. The job-link composition passes `None`, so a
//! link fetch is never interrupted by the desktop or MCP layers; `None` never resolves the
//! cancellation future.

use std::sync::Arc;
use std::time::Duration;

use jobniac_core::job_links::urls::wire_url;
use jobniac_core::job_links::{JobLinkContentFetcher, JobLinkFetchFailure, JobLinkFetchResult};
use jobniac_infra::workers::fetch::board::{
    BoardCategory, BoardFailure, BoardResponse, validate_request,
};
use jobniac_infra::workers::fetch::wire;
use jobniac_infra::workers::{WorkerError, WorkerErrorKind, WorkerKind, WorkerSupervisor};

/// How often a running fetch polls its cancellation check.
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// A cancellation check. It returns true once the caller has cancelled the fetch.
pub type Interrupted = Arc<dyn Fn() -> bool + Send + Sync>;

/// The guarded job-page fetcher: one `fetch-link` child per call.
#[derive(Clone)]
pub struct WorkerLinkFetcher {
    supervisor: Arc<WorkerSupervisor>,
    interrupted: Option<Interrupted>,
}

impl std::fmt::Debug for WorkerLinkFetcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerLinkFetcher")
            .field("interrupted", &self.interrupted.is_some())
            .finish_non_exhaustive()
    }
}

impl WorkerLinkFetcher {
    /// A fetcher that runs its children through `supervisor`, with an optional cancellation check.
    #[must_use]
    pub fn new(supervisor: Arc<WorkerSupervisor>, interrupted: Option<Interrupted>) -> Self {
        Self {
            supervisor,
            interrupted,
        }
    }
}

impl JobLinkContentFetcher for WorkerLinkFetcher {
    /// Fetches one job page. Must be called from a thread without a running Tokio runtime; inside a
    /// runtime it is refused before any child starts.
    fn fetch(&self, retrieval_url: &str) -> Result<JobLinkFetchResult, JobLinkFetchFailure> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(JobLinkFetchFailure::Unavailable);
        }
        wire_url(retrieval_url)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| JobLinkFetchFailure::Unavailable)?;
        let cancel = cancelled(self.interrupted.clone());
        let outcome = runtime.block_on(self.supervisor.run(
            WorkerKind::FetchLink,
            retrieval_url.as_bytes(),
            cancel,
        ));
        match outcome {
            Ok(body) => wire::decode_link(&body).map_err(|_| JobLinkFetchFailure::Unavailable)?,
            Err(error) => Err(link_failure(error)),
        }
    }
}

fn link_failure(error: WorkerError) -> JobLinkFetchFailure {
    match error.kind() {
        WorkerErrorKind::Cancelled => JobLinkFetchFailure::Interrupted,
        _ => JobLinkFetchFailure::Unavailable,
    }
}

/// The Arbeitnow transport: one `fetch-arbeitnow` child per request, with the fixed host.
#[derive(Clone, Debug)]
pub struct WorkerBoardTransport {
    supervisor: Arc<WorkerSupervisor>,
}

impl WorkerBoardTransport {
    /// A transport that runs its children through `supervisor`.
    #[must_use]
    pub fn new(supervisor: Arc<WorkerSupervisor>) -> Self {
        Self { supervisor }
    }

    /// Fetches one page of the board. Same thread restriction as [`WorkerLinkFetcher`].
    ///
    /// # Errors
    ///
    /// Returns the board failure of the first refused or failed step. The request is validated
    /// before any child starts.
    pub fn fetch(&self, page: i64, visa: Option<bool>) -> Result<BoardResponse, BoardFailure> {
        let page = validate_request(page)?;
        let transport = BoardFailure {
            category: BoardCategory::Transport,
            retryable: true,
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(transport);
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| transport)?;
        let request = wire::encode_board_request(page, visa);
        let outcome = runtime.block_on(self.supervisor.run(
            WorkerKind::FetchArbeitnow,
            &request,
            std::future::pending::<()>(),
        ));
        let body = outcome.map_err(|_| transport)?;
        wire::decode_board(&body).map_err(|_| transport)?
    }
}

/// Resolves when the check reports a cancellation; never otherwise.
async fn cancelled(interrupted: Option<Interrupted>) {
    let Some(check) = interrupted else {
        std::future::pending::<()>().await;
        return;
    };
    while !check() {
        tokio::time::sleep(CANCEL_POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use jobniac_core::job_links::JobLinkContentFetcher;
    use jobniac_core::job_links::JobLinkFetchFailure;
    use jobniac_infra::workers::WorkerSupervisor;
    use jobniac_infra::workers::fetch::board::{BoardCategory, BoardFailure};

    use super::{Interrupted, WorkerBoardTransport, WorkerLinkFetcher};

    /// A supervisor whose program does not exist. Every test here either refuses before a child is
    /// created or cancels before it starts, so the program is never run.
    fn unused_supervisor() -> Arc<WorkerSupervisor> {
        Arc::new(WorkerSupervisor::new(
            PathBuf::from("jobniac-no-such-worker-program"),
            Vec::new(),
        ))
    }

    fn transport_failure() -> BoardFailure {
        BoardFailure {
            category: BoardCategory::Transport,
            retryable: true,
        }
    }

    #[tokio::test]
    async fn link_fetch_inside_a_running_loop_is_refused_before_any_child() {
        let fetcher = WorkerLinkFetcher::new(unused_supervisor(), None);
        assert_eq!(
            fetcher.fetch("https://8.8.8.8/").err(),
            Some(JobLinkFetchFailure::Unavailable)
        );
    }

    #[tokio::test]
    async fn board_fetch_inside_a_running_loop_is_refused_before_any_child() {
        let transport = WorkerBoardTransport::new(unused_supervisor());
        assert_eq!(transport.fetch(1, None).err(), Some(transport_failure()));
    }

    #[test]
    fn link_fetch_rejects_a_bad_url_before_any_child() {
        let fetcher = WorkerLinkFetcher::new(unused_supervisor(), None);
        assert_eq!(
            fetcher.fetch("ftp://example.com/").err(),
            Some(JobLinkFetchFailure::InvalidUrl)
        );
    }

    #[test]
    fn cancellation_reported_from_the_start_is_interrupted_before_any_child() {
        // A check that is already true makes the supervisor return Cancelled before it starts the
        // program, so the missing program is never reached; the fetch maps that to Interrupted.
        let interrupted: Interrupted = Arc::new(|| true);
        let fetcher = WorkerLinkFetcher::new(unused_supervisor(), Some(interrupted));
        assert_eq!(
            fetcher.fetch("https://8.8.8.8/").err(),
            Some(JobLinkFetchFailure::Interrupted)
        );
    }

    #[test]
    fn a_missing_program_is_unavailable_not_interrupted() {
        let fetcher = WorkerLinkFetcher::new(unused_supervisor(), None);
        assert_eq!(
            fetcher.fetch("https://8.8.8.8/").err(),
            Some(JobLinkFetchFailure::Unavailable)
        );
    }
}
