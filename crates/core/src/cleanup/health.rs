//! Cleanup backlog health.
//!
//! Classification is pure: `UNKNOWN` while ownership is uncertain, `DEGRADED` for a repeated
//! failure, expired completed tasks or an oldest due task at or past the threshold, otherwise
//! `HEALTHY`. A composition that cannot prove the lifecycle (generation disabled, root unset or
//! unqualified, or expired pending generations) reports `UNKNOWN` with the observed counts.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;

use super::model::{
    COMPLETED_RETENTION, CleanupQueueSnapshot, OLDEST_DUE_THRESHOLD, REPEATED_FAILURE_ATTEMPTS,
};
use super::qualification::GenerationRootQualification;
use crate::clock::Clock;
use crate::errors::AppError;
use crate::health::{CleanupHealthPort, CleanupReport, CleanupStatus};

/// Read-only aggregate port of the cleanup queue. Implementations never claim, create, retry or
/// delete a task.
#[async_trait]
pub trait CleanupHealthRepository: Send + Sync + fmt::Debug {
    /// Reads the queue aggregates as of `now`. Tasks completed before `completed_before` count as
    /// expired; a task is repeated-failing at `repeated_failure_attempts` attempts or more.
    async fn read_queue_snapshot(
        &self,
        now: OffsetDateTime,
        repeated_failure_attempts: u32,
        completed_before: OffsetDateTime,
    ) -> Result<CleanupQueueSnapshot, AppError>;
}

/// Classifies one snapshot observed at `now`.
#[must_use]
pub fn classify(snapshot: &CleanupQueueSnapshot, now: OffsetDateTime) -> CleanupReport {
    let age = snapshot.oldest_due_at.map_or(time::Duration::ZERO, |due| {
        std::cmp::max(time::Duration::ZERO, now - due)
    });
    let status = if snapshot.ownership_uncertain {
        CleanupStatus::Unknown
    } else if snapshot.repeated_failure
        || snapshot.expired_completed_count > 0
        || age >= OLDEST_DUE_THRESHOLD
    {
        CleanupStatus::Degraded
    } else {
        CleanupStatus::Healthy
    };
    CleanupReport {
        status,
        pending_count: snapshot.pending_count,
        processing_count: snapshot.processing_count,
        expired_completed_count: snapshot.expired_completed_count,
        oldest_due_age_seconds: u64::try_from(age.whole_seconds()).unwrap_or(0),
        repeated_failure: snapshot.repeated_failure,
    }
}

/// Reads and classifies the backlog as of `now`. Any read failure is the unknown report.
pub async fn report_at(
    repository: &dyn CleanupHealthRepository,
    now: OffsetDateTime,
) -> CleanupReport {
    match repository
        .read_queue_snapshot(now, REPEATED_FAILURE_ATTEMPTS, now - COMPLETED_RETENTION)
        .await
    {
        Ok(snapshot) => classify(&snapshot, now),
        Err(_) => CleanupReport::unknown(),
    }
}

/// Cleanup health for a composition whose lifecycle may not be available.
#[derive(Clone)]
pub struct CleanupHealthService {
    repository: Option<Arc<dyn CleanupHealthRepository>>,
    clock: Arc<dyn Clock>,
    lifecycle_available: bool,
}

impl fmt::Debug for CleanupHealthService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CleanupHealthService")
            .field("lifecycle_available", &self.lifecycle_available)
            .finish_non_exhaustive()
    }
}

impl CleanupHealthService {
    /// Builds the service. Without `lifecycle_available` and a repository, it reports unknown.
    pub fn new(
        repository: Option<Arc<dyn CleanupHealthRepository>>,
        clock: Arc<dyn Clock>,
        lifecycle_available: bool,
    ) -> Self {
        Self {
            repository,
            clock,
            lifecycle_available,
        }
    }
}

#[async_trait]
impl CleanupHealthPort for CleanupHealthService {
    async fn check(&self) -> Result<CleanupReport, ()> {
        match (&self.repository, self.lifecycle_available) {
            (Some(repository), true) => Ok(report_at(repository.as_ref(), self.clock.now()).await),
            _ => Ok(CleanupReport::unknown()),
        }
    }
}

/// Cleanup health gated by the explicit generation root qualification.
/// The lifecycle must be qualified, and pending generations that
/// expired without recovery force `UNKNOWN`.
#[derive(Clone)]
pub struct QualifiedCleanupHealth {
    qualification: Arc<GenerationRootQualification>,
    repository: Arc<dyn CleanupHealthRepository>,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for QualifiedCleanupHealth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QualifiedCleanupHealth")
            .finish_non_exhaustive()
    }
}

impl QualifiedCleanupHealth {
    /// Builds the qualified probe over `qualification` and the cleanup aggregates.
    pub fn new(
        qualification: Arc<GenerationRootQualification>,
        repository: Arc<dyn CleanupHealthRepository>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            qualification,
            repository,
            clock,
        }
    }
}

#[async_trait]
impl CleanupHealthPort for QualifiedCleanupHealth {
    async fn check(&self) -> Result<CleanupReport, ()> {
        let now = self.clock.now();
        let Ok(state) = self.qualification.inspect(now).await else {
            return Ok(CleanupReport::unknown());
        };
        let mut report = report_at(self.repository.as_ref(), now).await;
        if state.expired_pending {
            report.status = CleanupStatus::Unknown;
        }
        Ok(report)
    }
}
