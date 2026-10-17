//! Shared health use case.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;

use crate::clock::{Clock, format_isoformat_utc};

/// Sanitized error category of one readiness probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCategory {
    None,
    ConnectionUnavailable,
    AuthenticationFailed,
    QueryFailed,
    Timeout,
    Unknown,
}

/// Sanitized result from a single readiness probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthProbeResult {
    successful: bool,
    error_category: ErrorCategory,
}

impl HealthProbeResult {
    pub const fn ready() -> Self {
        Self {
            successful: true,
            error_category: ErrorCategory::None,
        }
    }

    /// A failed probe; `None` is not a failure category and becomes `Unknown`.
    pub const fn failed(category: ErrorCategory) -> Self {
        let error_category = match category {
            ErrorCategory::None => ErrorCategory::Unknown,
            other => other,
        };
        Self {
            successful: false,
            error_category,
        }
    }

    pub const fn successful(&self) -> bool {
        self.successful
    }

    pub const fn error_category(&self) -> ErrorCategory {
        self.error_category
    }
}

/// Outbound port for one sanitized, read-only readiness attempt.
#[async_trait]
pub trait HealthProbe: Send + Sync + std::fmt::Debug {
    async fn probe(&self) -> HealthProbeResult;
}

/// Status of the generated-resume cleanup backlog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CleanupStatus {
    Unknown,
    Degraded,
    Healthy,
}

/// Observed generated-output cleanup health (`generatedResumeCleanup`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReport {
    pub status: CleanupStatus,
    pub pending_count: u64,
    pub processing_count: u64,
    pub expired_completed_count: u64,
    pub oldest_due_age_seconds: u64,
    pub repeated_failure: bool,
}

impl CleanupReport {
    /// The canonical report when no approved cleanup lifecycle is composed.
    pub const fn unknown() -> Self {
        Self {
            status: CleanupStatus::Unknown,
            pending_count: 0,
            processing_count: 0,
            expired_completed_count: 0,
            oldest_due_age_seconds: 0,
            repeated_failure: false,
        }
    }
}

/// Port reporting the observed cleanup backlog. Errors collapse to the unknown report.
#[async_trait]
pub trait CleanupHealthPort: Send + Sync + std::fmt::Debug {
    async fn check(&self) -> Result<CleanupReport, ()>;
}

/// Overall readiness status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HealthStatus {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthMetadata {
    pub checked_at: String,
    pub total_checks: u32,
    pub successful_checks: u32,
    pub failed_checks: u32,
}

/// The stable JSON health envelope shared by every inbound adapter. Field order is part of
/// the wire contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    pub status: HealthStatus,
    pub error_category: ErrorCategory,
    pub metadata: HealthMetadata,
    pub generated_resume_cleanup: CleanupReport,
}

fn health_report(result: HealthProbeResult, clock: &dyn Clock) -> HealthReport {
    let successful = result.successful();
    HealthReport {
        status: if successful {
            HealthStatus::Up
        } else {
            HealthStatus::Down
        },
        error_category: if successful {
            ErrorCategory::None
        } else {
            result.error_category()
        },
        metadata: HealthMetadata {
            checked_at: format_isoformat_utc(clock.now()),
            total_checks: 1,
            successful_checks: u32::from(successful),
            failed_checks: u32::from(!successful),
        },
        generated_resume_cleanup: CleanupReport::unknown(),
    }
}

/// The canonical sanitized response for an adapter-level failure.
pub fn unknown_health_report(clock: &dyn Clock) -> HealthReport {
    health_report(HealthProbeResult::failed(ErrorCategory::Unknown), clock)
}

/// Return the stable health envelope for every inbound adapter.
#[derive(Debug, Clone)]
pub struct HealthService {
    probe: Arc<dyn HealthProbe>,
    cleanup_health: Option<Arc<dyn CleanupHealthPort>>,
    clock: Arc<dyn Clock>,
}

impl HealthService {
    pub fn new(
        probe: Arc<dyn HealthProbe>,
        cleanup_health: Option<Arc<dyn CleanupHealthPort>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            probe,
            cleanup_health,
            clock,
        }
    }

    pub async fn check(&self) -> HealthReport {
        let result = self.probe.probe().await;
        let mut report = health_report(result, self.clock.as_ref());
        report.generated_resume_cleanup = match &self.cleanup_health {
            None => CleanupReport::unknown(),
            Some(port) => port.check().await.unwrap_or(CleanupReport::unknown()),
        };
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FixedClock;
    use time::macros::datetime;

    #[derive(Debug)]
    struct StubProbe(HealthProbeResult);

    #[async_trait]
    impl HealthProbe for StubProbe {
        async fn probe(&self) -> HealthProbeResult {
            self.0
        }
    }

    #[derive(Debug)]
    struct FailingCleanup;

    #[async_trait]
    impl CleanupHealthPort for FailingCleanup {
        async fn check(&self) -> Result<CleanupReport, ()> {
            Err(())
        }
    }

    fn service(
        result: HealthProbeResult,
        cleanup: Option<Arc<dyn CleanupHealthPort>>,
    ) -> HealthService {
        HealthService::new(
            Arc::new(StubProbe(result)),
            cleanup,
            Arc::new(FixedClock::new(datetime!(2026-10-13 12:00:00.25 UTC))),
        )
    }

    #[tokio::test]
    async fn up_report_matches_the_wire_shape() {
        let report = service(HealthProbeResult::ready(), None).check().await;
        let json = serde_json::to_string(&report).unwrap_or_default();
        assert_eq!(
            json,
            concat!(
                r#"{"status":"UP","errorCategory":"NONE","metadata":{"checkedAt":"2026-10-13T12:00:00.250000Z","#,
                r#""totalChecks":1,"successfulChecks":1,"failedChecks":0},"generatedResumeCleanup":"#,
                r#"{"status":"UNKNOWN","pendingCount":0,"processingCount":0,"expiredCompletedCount":0,"#,
                r#""oldestDueAgeSeconds":0,"repeatedFailure":false}}"#
            )
        );
    }

    #[tokio::test]
    async fn down_report_keeps_category_and_failed_cleanup_is_unknown() {
        let report = service(
            HealthProbeResult::failed(ErrorCategory::Timeout),
            Some(Arc::new(FailingCleanup)),
        )
        .check()
        .await;
        assert_eq!(report.status, HealthStatus::Down);
        assert_eq!(report.error_category, ErrorCategory::Timeout);
        assert_eq!(report.metadata.failed_checks, 1);
        assert_eq!(report.generated_resume_cleanup, CleanupReport::unknown());
    }

    #[test]
    fn none_is_not_a_failure_category() {
        assert_eq!(
            HealthProbeResult::failed(ErrorCategory::None).error_category(),
            ErrorCategory::Unknown
        );
    }
}
