//! Running CPU-bound or blocking work from an async task without parking its worker thread.

/// Runs a blocking phase (hashing, PDF rendering, write, fsync) without parking an async worker.
///
/// The phase stays on the calling task, because the database transaction, the advisory locks and
/// the pooled session of that task may be held across it, so it cannot move to `spawn_blocking`
/// (the closure borrows the caller's state). On a multi-thread runtime `block_in_place` hands the
/// worker's other tasks to other threads for the duration; on a current-thread runtime, or outside
/// a runtime, the closure runs in place.
pub fn run_blocking<T>(phase: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(phase)
        }
        _ => phase(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use super::run_blocking;

    /// Waits up to two seconds for `flag`, spinning like a blocking phase would.
    fn spin_until(flag: &AtomicBool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !flag.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        flag.load(Ordering::SeqCst)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_blocking_phase_does_not_park_the_only_worker() {
        let flag = Arc::new(AtomicBool::new(false));
        let sibling = Arc::clone(&flag);
        let handle = tokio::spawn(async move { sibling.store(true, Ordering::SeqCst) });
        // With a single worker the sibling can run during the phase only if the phase yields it.
        assert!(run_blocking(|| spin_until(&flag)));
        handle.await.expect("the sibling task finishes");
    }

    #[tokio::test]
    async fn a_blocking_phase_runs_in_place_on_a_current_thread_runtime() {
        assert_eq!(run_blocking(|| 7), 7);
    }

    #[test]
    fn a_blocking_phase_runs_outside_a_runtime() {
        assert_eq!(run_blocking(|| 7), 7);
    }
}
