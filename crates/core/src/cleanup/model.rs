//! Owned-asset cleanup contracts: queue constants, claims, outcomes, removal failures and the
//! observed queue snapshot.
//!
//! The identity, the preparation outcome and the claim and retry batch bounds are shared with the
//! generated-asset lifecycle and are re-exported from [`crate::generated_assets`] rather than
//! duplicated here.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::clock::truncate_to_micros;
use crate::generated_assets::AssetIdentity;

/// Delay before a retryable failure is attempted again (one minute).
pub const RETRY_DELAY: Duration = Duration::minutes(1);
/// Age after which a completed task's queue metadata may be pruned.
pub const COMPLETED_RETENTION: Duration = Duration::days(30);
/// Completed tasks removed by one prune statement.
pub const RETENTION_BATCH_SIZE: usize = 1_000;
/// Prune batches one purge run may execute.
pub const RETENTION_MAX_BATCHES: usize = 10;
/// Age of the oldest due task at which health reports `DEGRADED`.
pub const OLDEST_DUE_THRESHOLD: Duration = Duration::minutes(5);
/// Attempts after which a failing task counts as a repeated failure.
pub const REPEATED_FAILURE_ATTEMPTS: u32 = 3;

/// Refusal of a cleanup claim that cannot describe a leased task. Carries no values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cleanup claim is invalid")]
pub struct InvalidCleanupClaim;

/// A leased cleanup task: the immutable identity it removes, its fence and its retirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupClaim {
    task_id: Uuid,
    asset: AssetIdentity,
    lease_token: Uuid,
    attempt_count: u32,
    expires_at: OffsetDateTime,
    retirement_id: Uuid,
}

impl CleanupClaim {
    /// Builds a claim. The attempt count must be positive; the expiry is kept at microsecond
    /// precision because that is the precision the ledger stores.
    ///
    /// # Errors
    ///
    /// Fails with [`InvalidCleanupClaim`] when the attempt count is zero.
    pub fn new(
        task_id: Uuid,
        asset: AssetIdentity,
        lease_token: Uuid,
        attempt_count: u32,
        expires_at: OffsetDateTime,
        retirement_id: Uuid,
    ) -> Result<Self, InvalidCleanupClaim> {
        if attempt_count == 0 {
            return Err(InvalidCleanupClaim);
        }
        Ok(Self {
            task_id,
            asset,
            lease_token,
            attempt_count,
            expires_at: truncate_to_micros(expires_at),
            retirement_id,
        })
    }

    /// The cleanup task identity.
    pub const fn task_id(&self) -> Uuid {
        self.task_id
    }

    /// The immutable asset identity the task removes.
    pub const fn asset(&self) -> &AssetIdentity {
        &self.asset
    }

    /// The fence token of this lease.
    pub const fn lease_token(&self) -> Uuid {
        self.lease_token
    }

    /// The attempt number of this lease (1 for the first attempt).
    pub const fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    /// The instant at which this lease expires (UTC, microseconds).
    pub const fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }

    /// The retirement identity the asset was fenced with.
    pub const fn retirement_id(&self) -> Uuid {
        self.retirement_id
    }
}

/// Result of one cleanup execution attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupOutcome {
    /// The file is gone (or was already absent) and the task and asset are finalized.
    Completed,
    /// A retryable failure was recorded with a backoff.
    RetryScheduled,
    /// The fence no longer matches; nothing was recorded.
    LeaseLost,
    /// A non-retryable failure was recorded; the task waits for an operator.
    Blocked,
}

/// Result of a removal that did not fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalOutcome {
    /// The owned file was removed by this call.
    Removed,
    /// The file was already absent.
    AlreadyAbsent,
}

/// Fixed, safe category of a cleanup failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalFailureCategory {
    /// The file could not be removed or the namespace is busy. Retryable, never permanent.
    FileDeleteFailed,
    /// The file on disk is not the recorded identity; it is preserved.
    IdentityMismatch,
    /// Ownership of the root or the file cannot be proven; nothing is touched.
    OwnershipUnproven,
    /// The platform cannot provide the namespace protocol.
    UnsupportedPlatform,
}

impl RemovalFailureCategory {
    /// The category name, as stored in the ledger.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FileDeleteFailed => "FILE_DELETE_FAILED",
            Self::IdentityMismatch => "IDENTITY_MISMATCH",
            Self::OwnershipUnproven => "OWNERSHIP_UNPROVEN",
            Self::UnsupportedPlatform => "UNSUPPORTED_PLATFORM",
        }
    }

    /// Whether the failure is retried with a backoff rather than blocking the task.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::FileDeleteFailed)
    }
}

/// A removal failure carrying only its fixed category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{}", .category.as_str())]
pub struct RemovalFailure {
    category: RemovalFailureCategory,
}

impl RemovalFailure {
    /// The failure for `category`.
    #[must_use]
    pub const fn new(category: RemovalFailureCategory) -> Self {
        Self { category }
    }

    /// The fixed category.
    #[must_use]
    pub const fn category(&self) -> RemovalFailureCategory {
        self.category
    }
}

/// Observed aggregates of the cleanup queue. Uncertain ownership is carried as a flag, never as
/// healthy synthetic data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupQueueSnapshot {
    /// Pending and blocked tasks.
    pub pending_count: u64,
    /// Tasks currently leased.
    pub processing_count: u64,
    /// Completed tasks older than the retention period.
    pub expired_completed_count: u64,
    /// Due time of the oldest due task, when one is due.
    pub oldest_due_at: Option<OffsetDateTime>,
    /// Whether a non-completed task has reached the repeated-failure attempt threshold.
    pub repeated_failure: bool,
    /// Whether a blocked task or a pre-ledger row leaves ownership unproven.
    pub ownership_uncertain: bool,
}
