//! Persistence port of the matching use cases.
//!
//! Unlike the job and profile ports, the matching port takes no unit-of-work handle. The
//! repository opens its own connection for each call, so each port method is one atomic unit: a
//! pair analysis, a batch read, one report write, one read or one page. The adapter owns the
//! transactions, the lock order and the retry policy described by [`retries_serialization_failure`].

use async_trait::async_trait;
use uuid::Uuid;

use crate::errors::AppError;
use crate::pagination::{Page, PageRequest};

use super::graph::{JobGraph, ProfileGraph};
use super::report::{MatchReport, MatchView};

/// The scorer the repository applies to a profile and a job inside one atomic pair analysis.
pub type PairScorer<'a> =
    dyn Fn(&ProfileGraph, &JobGraph) -> Result<MatchReport, AppError> + Sync + 'a;

/// Attempts a snapshot (repeatable-read) read makes before it gives up on a serialization failure.
pub const SNAPSHOT_ATTEMPTS: u32 = 3;

/// Whether a snapshot operation that failed with SQLSTATE 40001 on attempt `attempt` (zero-based)
/// starts again. Only snapshot operations retry, and only while attempts remain. Any other failure is final.
pub const fn retries_serialization_failure(attempt: u32, snapshot: bool) -> bool {
    snapshot && attempt < SNAPSHOT_ATTEMPTS - 1
}

/// Storage of the immutable match history.
///
/// Implementations report a failure they cannot recover from as an [`AppError`] whose message is
/// safe to show. A snapshot operation that hits a serialization failure is retried by the adapter
/// under [`retries_serialization_failure`]; when the attempts run out it is reported as database
/// unavailable, and the use cases never see the raw database error.
#[async_trait]
pub trait MatchingRepository: Send + Sync {
    /// Loads the profile and job, scores them with `scorer` and stores the report, all in one
    /// atomic unit. A report already stored for the same identity is returned instead of a new row.
    async fn analyze_pair(
        &self,
        profile_id: Uuid,
        job_id: Uuid,
        scorer: &PairScorer<'_>,
    ) -> Result<MatchReport, AppError>;

    /// The profile and one page of its jobs, read from one snapshot. The page is bounded by the
    /// request's limit and keyset cursor.
    async fn load_batch(
        &self,
        profile_id: Uuid,
        request: &PageRequest<'_>,
    ) -> Result<(ProfileGraph, Page<JobGraph>), AppError>;

    /// Stores a report, or returns the stored report with the same identity. Reports are insert-only.
    async fn save_report(&self, report: MatchReport) -> Result<MatchReport, AppError>;

    /// The stored report with `report_id` and its staleness, or `None` when it does not exist.
    async fn get_report(&self, report_id: Uuid) -> Result<Option<MatchView>, AppError>;

    /// One page of stored reports, filtered by profile and by job when given, newest first.
    async fn list_reports(
        &self,
        profile_id: Option<Uuid>,
        job_id: Option<Uuid>,
        request: &PageRequest<'_>,
    ) -> Result<Page<MatchView>, AppError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_snapshot_operations_retry_and_only_twice() {
        assert!(retries_serialization_failure(0, true));
        assert!(retries_serialization_failure(1, true));
        assert!(!retries_serialization_failure(2, true));
        assert!(!retries_serialization_failure(0, false));
        assert_eq!(SNAPSHOT_ATTEMPTS, 3);
    }
}
