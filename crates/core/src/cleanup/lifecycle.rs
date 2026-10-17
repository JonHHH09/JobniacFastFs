//! The bounded cleanup pass of the generated root.
//!
//! One pass proves the root is qualified, recovers expired classified generations, runs the due
//! cleanup tasks and purges the completed ones. Any failure is the same fixed refusal, and the
//! summary carries only counts.

use std::sync::Arc;

use crate::clock::Clock;
use crate::generated_assets::{GeneratedAssetService, RETRY_BATCH_SIZE};

use super::executor::{CleanupExecutor, CleanupRunSummary, CleanupUnavailable};
use super::qualification::GenerationRootQualification;

/// The fixed refusal of a failed pass.
pub const LIFECYCLE_UNAVAILABLE: &str = "Generation lifecycle is unavailable";

/// The counts of one pass. The recovered identifiers are
/// reported as a count only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GenerationLifecycleSummary {
    /// The number of expired classified generations recovered.
    pub recovered_generations: usize,
    /// The counts of the due cleanup tasks.
    pub cleanup: CleanupRunSummary,
    /// The number of completed tasks purged.
    pub pruned_completed: usize,
}

impl GenerationLifecycleSummary {
    /// The metadata keys and counts in their fixed order.
    #[must_use]
    pub const fn metadata(&self) -> [(&'static str, usize); 8] {
        [
            ("recoveredGenerations", self.recovered_generations),
            ("claimed", self.cleanup.claimed),
            ("completed", self.cleanup.completed),
            ("retryScheduled", self.cleanup.retry_scheduled),
            ("leaseLost", self.cleanup.lease_lost),
            ("blocked", self.cleanup.blocked),
            ("stateFailures", self.cleanup.state_failures),
            ("prunedCompleted", self.pruned_completed),
        ]
    }

    /// Whether the pass reports a failure exit (state failures or blocked).
    #[must_use]
    pub const fn has_failures(&self) -> bool {
        self.cleanup.state_failures > 0 || self.cleanup.blocked > 0
    }
}

/// The bounded cleanup pass over one configured root.
pub struct GenerationLifecycle {
    qualification: Arc<GenerationRootQualification>,
    assets: Arc<GeneratedAssetService>,
    executor: Arc<CleanupExecutor>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for GenerationLifecycle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GenerationLifecycle")
            .finish_non_exhaustive()
    }
}

impl GenerationLifecycle {
    /// Builds the pass over the shared qualification, asset lifecycle and executor.
    #[must_use]
    pub fn new(
        qualification: Arc<GenerationRootQualification>,
        assets: Arc<GeneratedAssetService>,
        executor: Arc<CleanupExecutor>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            qualification,
            assets,
            executor,
            clock,
        }
    }

    /// Runs one bounded pass.
    ///
    /// # Errors
    ///
    /// [`CleanupUnavailable`] with [`LIFECYCLE_UNAVAILABLE`] when the root is not qualified or any
    /// step is refused. Nothing is reported beyond the fixed text.
    pub async fn run_cleanup(&self) -> Result<GenerationLifecycleSummary, CleanupUnavailable> {
        let unavailable = || CleanupUnavailable(LIFECYCLE_UNAVAILABLE);
        self.qualification
            .inspect(self.clock.now())
            .await
            .map_err(|_| unavailable())?;
        let recovered = self
            .assets
            .recover_expired_generations()
            .await
            .map_err(|_| unavailable())?;
        if recovered.len() > RETRY_BATCH_SIZE {
            return Err(unavailable());
        }
        let cleanup = self
            .executor
            .run_due_tasks()
            .await
            .map_err(|_| unavailable())?;
        let pruned_completed = self
            .executor
            .purge_completed_tasks()
            .await
            .map_err(|_| unavailable())?;
        Ok(GenerationLifecycleSummary {
            recovered_generations: recovered.len(),
            cleanup,
            pruned_completed,
        })
    }
}
