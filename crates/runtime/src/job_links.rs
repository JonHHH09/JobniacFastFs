//! Opt-in link ingestion composition.
//!
//! The link use case is composed only when `JOBNIAC_LINK_FETCH_ENABLED` is exactly `1`. The platform
//! gate runs first: the owned child transport is admitted on Linux and Windows only, so an enabled
//! flag elsewhere stops startup with the sanitized [`StartupError`]. Without a database configuration
//! nothing is composed. Construction connects, spawns and fetches nothing.
//!
//! The fetch port of the core use case is asynchronous. The worker fetch of
//! [`crate::worker_fetch::WorkerLinkFetcher`] refuses to run inside a Tokio runtime, so
//! [`BlockingLinkFetcher`] runs it on a plain thread and awaits the result. A `spawn_blocking` thread would inherit the runtime context and
//! be refused, so it is not used here.
//!
//! # Cancellation
//!
//! The composition passes no cancellation check to [`WorkerLinkFetcher`] (`None`). A fetch is
//! therefore bounded only by the supervisor's own wall-clock deadline, never by the caller going
//! away. This is intended, and is pinned by the tests of this module.
//!
//! # Panics
//!
//! The fetch path is panic-free by inspection: [`WorkerLinkFetcher`], the supervisor it calls, the
//! frame decoders and [`jobniac_core::job_links::urls::wire_url`] use no `unwrap`, `expect`,
//! unchecked indexing or unchecked arithmetic on data from outside. The thread that runs a fetch
//! still catches a panic, because a dropped result channel is reported as `UNAVAILABLE`. That catch
//! only works when panics unwind (dev and test profiles). The release profile sets
//! `panic = "abort"` (root `Cargo.toml`), so in a release build a panic on the fetch thread aborts
//! the whole process instead of failing one link. The catch is kept for the unwinding profiles.
//!
//! # Concurrency
//!
//! Each fetch runs on its own thread with its own current-thread runtime, so the number of
//! concurrent fetches is capped by a [`FetchGate`] of [`MAX_CONCURRENT_LINK_FETCHES`] slots. A call
//! that finds the gate full is refused at once with `UNAVAILABLE` and starts nothing.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use jobniac_core::clock::SystemClock;
use jobniac_core::env::Environment;
use jobniac_core::job_links::service::{JobLinkPageFetcher, JobLinkService};
use jobniac_core::job_links::{JobLinkContentFetcher, JobLinkFetchFailure, JobLinkFetchResult};
use jobniac_infra::sqlite::jobs::SqliteJobRepository;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;

use crate::services::StartupError;
use crate::worker_fetch::WorkerLinkFetcher;

/// Opt-in flag of `add_job_from_link`: admitted only as exactly `1`.
pub const LINK_FETCH_ENABLED_VARIABLE: &str = "JOBNIAC_LINK_FETCH_ENABLED";

/// The most link fetches that may run at once. Each one holds a thread and a child process.
pub const MAX_CONCURRENT_LINK_FETCHES: usize = 4;

/// The composed link use case over the SQLite unit of work and the job repository.
pub type JobLinkUseCase = JobLinkService<SqliteUnitOfWork, SqliteJobRepository>;

/// Whether the platform admits the owned child transports: Linux and Windows only.
const fn platform_supported() -> bool {
    cfg!(any(target_os = "linux", target_os = "windows"))
}

/// The platform gate: refuses with the sanitized [`StartupError`] when the platform is not admitted.
///
/// # Errors
///
/// [`StartupError`] when `supported` is false.
fn admit_platform(supported: bool) -> Result<(), StartupError> {
    if supported { Ok(()) } else { Err(StartupError) }
}

/// Composes the link use case from the snapshot `env` and the open database.
///
/// The snapshot is the only input: the process environment is never read here. The flag is checked
/// before the platform and the database, so a flag other than `1` composes nothing on any host.
///
/// # Errors
///
/// [`StartupError`] when the flag is exactly `1` on an unsupported platform, even without a database.
pub fn compose_job_links(
    env: &Environment,
    database: Option<&SqlitePool>,
) -> Result<Option<Arc<JobLinkUseCase>>, StartupError> {
    if !env.flag_enabled(LINK_FETCH_ENABLED_VARIABLE) {
        return Ok(None);
    }
    admit_platform(platform_supported())?;
    let Some(database) = database else {
        return Ok(None);
    };
    let uow = SqliteUnitOfWork::new(database.clone());
    // No cancellation check is passed (`None`): see the module docs.
    let inner = crate::diagnostics::current_process_supervisor().map(|supervisor| {
        Arc::new(WorkerLinkFetcher::new(supervisor, None))
            as Arc<dyn JobLinkContentFetcher + Send + Sync>
    });
    Ok(Some(Arc::new(JobLinkService::new(
        uow,
        SqliteJobRepository,
        Arc::new(BlockingLinkFetcher::new(inner)),
        Arc::new(SystemClock),
    ))))
}

/// The worker fetch as an asynchronous port, with a cap on concurrent fetches.
///
/// Without an inner fetcher every fetch is `UNAVAILABLE`, which is what a failed child start
/// reports. A fetch that finds the gate full is `UNAVAILABLE` too, and starts no thread.
pub struct BlockingLinkFetcher {
    inner: Option<Arc<dyn JobLinkContentFetcher + Send + Sync>>,
    gate: Arc<FetchGate>,
}

impl BlockingLinkFetcher {
    /// A fetcher over `inner` that admits at most [`MAX_CONCURRENT_LINK_FETCHES`] fetches at once.
    #[must_use]
    pub fn new(inner: Option<Arc<dyn JobLinkContentFetcher + Send + Sync>>) -> Self {
        Self::with_limit(inner, MAX_CONCURRENT_LINK_FETCHES)
    }

    /// A fetcher over `inner` that admits at most `limit` fetches at once.
    fn with_limit(
        inner: Option<Arc<dyn JobLinkContentFetcher + Send + Sync>>,
        limit: usize,
    ) -> Self {
        Self {
            inner,
            gate: Arc::new(FetchGate::new(limit)),
        }
    }
}

impl std::fmt::Debug for BlockingLinkFetcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BlockingLinkFetcher")
            .field("configured", &self.inner.is_some())
            .field("limit", &self.gate.limit)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl JobLinkPageFetcher for BlockingLinkFetcher {
    async fn fetch(&self, retrieval_url: &str) -> Result<JobLinkFetchResult, JobLinkFetchFailure> {
        let Some(inner) = self.inner.clone() else {
            return Err(JobLinkFetchFailure::Unavailable);
        };
        let Some(slot) = self.gate.try_enter() else {
            return Err(JobLinkFetchFailure::Unavailable);
        };
        let url = retrieval_url.to_owned();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let spawned = std::thread::Builder::new().spawn(move || {
            let outcome = inner.fetch(&url);
            // The slot is released before the result is sent, so a caller that has the result can
            // start its next fetch at once. A panic drops the slot while unwinding.
            drop(slot);
            // A closed receiver means the caller went away; the result is then not needed.
            let _ = sender.send(outcome);
        });
        if spawned.is_err() {
            // The closure, with its slot and sender, was dropped by the failed spawn.
            return Err(JobLinkFetchFailure::Unavailable);
        }
        // A panicked thread drops the sender: the fetch is then reported as unavailable.
        receiver
            .await
            .unwrap_or(Err(JobLinkFetchFailure::Unavailable))
    }
}

/// A counting gate over the link fetches in flight. Slots are taken without blocking.
#[derive(Debug)]
struct FetchGate {
    active: AtomicUsize,
    limit: usize,
}

impl FetchGate {
    const fn new(limit: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            limit,
        }
    }

    /// Takes a slot, or `None` when all `limit` slots are in use.
    fn try_enter(self: &Arc<Self>) -> Option<FetchSlot> {
        let mut current = self.active.load(Ordering::Acquire);
        loop {
            if current >= self.limit {
                return None;
            }
            match self.active.compare_exchange_weak(
                current,
                current.saturating_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(FetchSlot {
                        gate: Arc::clone(self),
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }
}

/// One slot of a [`FetchGate`]; releasing it on drop makes the slot available again.
#[derive(Debug)]
struct FetchSlot {
    gate: Arc<FetchGate>,
}

impl Drop for FetchSlot {
    fn drop(&mut self) {
        self.gate.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Condvar, Mutex, PoisonError};
    use std::time::Duration;

    use super::*;

    const URL: &str = "https://93.184.216.34/jobs/1";

    /// A fetcher whose calls wait until the test opens the gate, and count the calls that entered.
    struct BlockingFake {
        open: Arc<(Mutex<bool>, Condvar)>,
        entered: Arc<AtomicUsize>,
    }

    impl JobLinkContentFetcher for BlockingFake {
        fn fetch(&self, _: &str) -> Result<JobLinkFetchResult, JobLinkFetchFailure> {
            self.entered.fetch_add(1, Ordering::SeqCst);
            let (lock, ready) = &*self.open;
            let mut open = lock.lock().unwrap_or_else(PoisonError::into_inner);
            while !*open {
                open = ready.wait(open).unwrap_or_else(PoisonError::into_inner);
            }
            Ok(result())
        }
    }

    /// Panics on every call, as a bug in the fetch thread would.
    struct PanickingFake;

    impl JobLinkContentFetcher for PanickingFake {
        fn fetch(&self, _: &str) -> Result<JobLinkFetchResult, JobLinkFetchFailure> {
            panic!("synthetic fetch panic");
        }
    }

    /// Returns the given failure at once.
    struct FailingFake(JobLinkFetchFailure);

    impl JobLinkContentFetcher for FailingFake {
        fn fetch(&self, _: &str) -> Result<JobLinkFetchResult, JobLinkFetchFailure> {
            Err(self.0)
        }
    }

    fn result() -> JobLinkFetchResult {
        JobLinkFetchResult {
            title: Some("Example role".to_owned()),
            description: None,
            http_status: 200,
        }
    }

    fn database() -> Result<(tempfile::TempDir, SqlitePool), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let pool = SqlitePool::open(root.path().join("jobniac.sqlite3"))?;
        Ok((root, pool))
    }

    #[test]
    fn the_platform_gate_fails_closed_with_a_sanitized_error() {
        assert!(admit_platform(true).is_ok());
        let refused = admit_platform(false);
        assert!(refused.is_err());
        let message = refused
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(!message.is_empty());
    }

    #[test]
    fn disabled_flags_compose_nothing_even_with_a_database()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_root, pool) = database()?;
        for value in ["0", "", "true", "01", "yes", " 1"] {
            let env = Environment::from_pairs([(LINK_FETCH_ENABLED_VARIABLE, value)]);
            assert!(compose_job_links(&env, Some(&pool))?.is_none(), "{value:?}");
        }
        Ok(())
    }

    #[test]
    fn enabled_without_database_stays_disabled() -> Result<(), StartupError> {
        let env = Environment::from_pairs([(LINK_FETCH_ENABLED_VARIABLE, "1")]);
        if platform_supported() {
            assert!(compose_job_links(&env, None)?.is_none());
        }
        Ok(())
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn enabled_construction_connects_spawns_and_fetches_nothing()
    -> Result<(), Box<dyn std::error::Error>> {
        // The supervisor only records the current executable, so construction returns a service
        // without a reader connection, a child or a fetch.
        let (_root, pool) = database()?;
        let env = Environment::from_pairs([(LINK_FETCH_ENABLED_VARIABLE, "1")]);
        let service = compose_job_links(&env, Some(&pool))?.ok_or(StartupError)?;
        assert_eq!(pool.idle_readers(), 0);
        assert!(!format!("{service:?}").is_empty());
        Ok(())
    }

    #[test]
    fn explicit_environment_mapping_is_honoured() -> Result<(), Box<dyn std::error::Error>> {
        // Only the snapshot counts: the flag and the database come from `env`, never from the
        // process environment, so an empty snapshot composes nothing.
        let empty = Environment::from_pairs(Vec::<(&str, &str)>::new());
        assert!(compose_job_links(&empty, None)?.is_none());
        let flag_only = Environment::from_pairs([(LINK_FETCH_ENABLED_VARIABLE, "1")]);
        assert!(compose_job_links(&flag_only, None)?.is_none());
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            let (_root, pool) = database()?;
            let env = Environment::from_pairs([(LINK_FETCH_ENABLED_VARIABLE, "1")]);
            assert!(compose_job_links(&env, Some(&pool))?.is_some());
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_panicking_fetch_is_reported_as_unavailable_under_the_unwinding_profile() {
        let fetcher = BlockingLinkFetcher::with_limit(Some(Arc::new(PanickingFake)), 1);
        assert_eq!(
            fetcher.fetch(URL).await,
            Err(JobLinkFetchFailure::Unavailable)
        );
        assert_eq!(fetcher.gate.active.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_missing_inner_fetcher_is_unavailable_and_takes_no_slot() {
        let fetcher = BlockingLinkFetcher::with_limit(None, 1);
        assert_eq!(
            fetcher.fetch(URL).await,
            Err(JobLinkFetchFailure::Unavailable)
        );
        assert_eq!(fetcher.gate.active.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_early_failure_passes_through_and_frees_its_slot() {
        let fetcher = BlockingLinkFetcher::with_limit(
            Some(Arc::new(FailingFake(JobLinkFetchFailure::Interrupted))),
            1,
        );
        assert_eq!(
            fetcher.fetch(URL).await,
            Err(JobLinkFetchFailure::Interrupted)
        );
        assert_eq!(fetcher.gate.active.load(Ordering::SeqCst), 0);
        assert_eq!(
            fetcher.fetch(URL).await,
            Err(JobLinkFetchFailure::Interrupted),
            "the freed slot admits the next fetch"
        );
    }

    #[tokio::test]
    async fn the_fifth_concurrent_fetch_is_refused_at_once_and_slots_are_freed() {
        let open = Arc::new((Mutex::new(false), Condvar::new()));
        let entered = Arc::new(AtomicUsize::new(0));
        let inner = BlockingFake {
            open: Arc::clone(&open),
            entered: Arc::clone(&entered),
        };
        let fetcher = Arc::new(BlockingLinkFetcher::new(Some(Arc::new(inner))));
        assert_eq!(fetcher.gate.limit, MAX_CONCURRENT_LINK_FETCHES);

        let mut calls = Vec::new();
        for _ in 0..MAX_CONCURRENT_LINK_FETCHES {
            let fetcher = Arc::clone(&fetcher);
            calls.push(tokio::spawn(async move { fetcher.fetch(URL).await }));
        }
        while entered.load(Ordering::SeqCst) < MAX_CONCURRENT_LINK_FETCHES {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        let fifth = tokio::time::timeout(Duration::from_secs(5), fetcher.fetch(URL)).await;
        assert_eq!(fifth, Ok(Err(JobLinkFetchFailure::Unavailable)));
        assert_eq!(entered.load(Ordering::SeqCst), MAX_CONCURRENT_LINK_FETCHES);

        {
            let (lock, ready) = &*open;
            *lock.lock().unwrap_or_else(PoisonError::into_inner) = true;
            ready.notify_all();
        }
        for call in calls {
            let outcome = call.await;
            assert!(matches!(outcome, Ok(Ok(_))), "{outcome:?}");
        }
        assert_eq!(fetcher.gate.active.load(Ordering::SeqCst), 0);
        assert!(
            fetcher.fetch(URL).await.is_ok(),
            "freed slots admit fetches again"
        );
    }

    #[test]
    fn the_gate_frees_a_slot_on_drop_and_never_exceeds_its_limit() {
        let gate = Arc::new(FetchGate::new(2));
        let first = gate.try_enter();
        let second = gate.try_enter();
        assert!(first.is_some() && second.is_some());
        assert!(gate.try_enter().is_none());
        drop(first);
        assert!(gate.try_enter().is_some());
    }
}
