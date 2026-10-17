//! Cleanup health classification and the qualified probe: UNKNOWN for uncertain ownership or an
//! unqualified lifecycle, DEGRADED and HEALTHY exactly at the documented thresholds. Pure, in-memory.

use std::error::Error;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jobniac_core::cleanup::{
    CleanupHealthRepository, CleanupHealthService, CleanupQueueSnapshot, GenerationRootFiles,
    GenerationRootQualification, GenerationRootRepository, GenerationRootState,
    QualifiedCleanupHealth, RemovalFailure, RemovalFailureCategory, classify,
};
use jobniac_core::clock::{Clock, FixedClock};
use jobniac_core::errors::AppError;
use jobniac_core::health::{CleanupHealthPort, CleanupReport, CleanupStatus};
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const NOW: OffsetDateTime = datetime!(2026-10-15 12:00:00 UTC);

fn healthy_snapshot() -> CleanupQueueSnapshot {
    CleanupQueueSnapshot {
        pending_count: 0,
        processing_count: 0,
        expired_completed_count: 0,
        oldest_due_at: None,
        repeated_failure: false,
        ownership_uncertain: false,
    }
}

#[test]
fn an_empty_queue_is_healthy_with_zero_age() {
    let report = classify(&healthy_snapshot(), NOW);
    assert_eq!(report.status, CleanupStatus::Healthy);
    assert_eq!(report.oldest_due_age_seconds, 0);
}

#[test]
fn the_five_minute_threshold_is_inclusive_for_degraded() {
    let mut snapshot = healthy_snapshot();
    snapshot.oldest_due_at =
        Some(NOW - time::Duration::seconds(299) - time::Duration::milliseconds(999));
    let just_under = classify(&snapshot, NOW);
    assert_eq!(just_under.status, CleanupStatus::Healthy);
    assert_eq!(just_under.oldest_due_age_seconds, 299);

    snapshot.oldest_due_at = Some(NOW - time::Duration::seconds(300));
    let at = classify(&snapshot, NOW);
    assert_eq!(at.status, CleanupStatus::Degraded);
    assert_eq!(at.oldest_due_age_seconds, 300);
}

#[test]
fn a_future_due_time_has_zero_age() {
    let mut snapshot = healthy_snapshot();
    snapshot.oldest_due_at = Some(NOW + time::Duration::hours(1));
    assert_eq!(classify(&snapshot, NOW).oldest_due_age_seconds, 0);
}

#[test]
fn repeated_failures_and_expired_completed_tasks_are_degraded() {
    let mut repeated = healthy_snapshot();
    repeated.repeated_failure = true;
    assert_eq!(classify(&repeated, NOW).status, CleanupStatus::Degraded);

    let mut expired = healthy_snapshot();
    expired.expired_completed_count = 1;
    let report = classify(&expired, NOW);
    assert_eq!(report.status, CleanupStatus::Degraded);
    assert_eq!(report.expired_completed_count, 1);
}

#[test]
fn uncertain_ownership_is_unknown_and_keeps_the_observed_counts() {
    let mut snapshot = healthy_snapshot();
    snapshot.ownership_uncertain = true;
    snapshot.pending_count = 4;
    snapshot.processing_count = 2;
    snapshot.repeated_failure = true;
    let report = classify(&snapshot, NOW);
    assert_eq!(report.status, CleanupStatus::Unknown);
    assert_eq!(report.pending_count, 4);
    assert_eq!(report.processing_count, 2);
    assert!(report.repeated_failure);
}

/// Repository returning one snapshot and recording the thresholds it was asked for.
#[derive(Debug)]
struct SnapshotRepository {
    snapshot: Option<CleanupQueueSnapshot>,
    seen: Mutex<Option<(OffsetDateTime, u32, OffsetDateTime)>>,
}

#[async_trait]
impl CleanupHealthRepository for SnapshotRepository {
    async fn read_queue_snapshot(
        &self,
        now: OffsetDateTime,
        attempts: u32,
        completed_before: OffsetDateTime,
    ) -> Result<CleanupQueueSnapshot, AppError> {
        if let Ok(mut seen) = self.seen.lock() {
            *seen = Some((now, attempts, completed_before));
        }
        self.snapshot.ok_or(AppError::Internal)
    }
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(FixedClock::new(NOW))
}

#[tokio::test]
async fn the_service_reads_with_the_documented_thresholds() -> TestResult {
    let repository = Arc::new(SnapshotRepository {
        snapshot: Some(healthy_snapshot()),
        seen: Mutex::new(None),
    });
    let service = CleanupHealthService::new(
        Some(Arc::clone(&repository) as Arc<dyn CleanupHealthRepository>),
        clock(),
        true,
    );
    let report = service.check().await.map_err(|()| "port failed")?;
    assert_eq!(report.status, CleanupStatus::Healthy);
    let seen = repository
        .seen
        .lock()
        .map_err(|_| "lock")?
        .ok_or("not read")?;
    assert_eq!(seen, (NOW, 3, NOW - time::Duration::days(30)));
    Ok(())
}

#[tokio::test]
async fn the_service_is_unknown_without_a_lifecycle_or_on_a_read_failure() -> TestResult {
    let repository = Arc::new(SnapshotRepository {
        snapshot: Some(healthy_snapshot()),
        seen: Mutex::new(None),
    });
    let not_available = CleanupHealthService::new(
        Some(Arc::clone(&repository) as Arc<dyn CleanupHealthRepository>),
        clock(),
        false,
    );
    assert_eq!(not_available.check().await, Ok(CleanupReport::unknown()));
    assert!(repository.seen.lock().map_err(|_| "lock")?.is_none());

    let failing = Arc::new(SnapshotRepository {
        snapshot: None,
        seen: Mutex::new(None),
    });
    let read_failure = CleanupHealthService::new(
        Some(failing as Arc<dyn CleanupHealthRepository>),
        clock(),
        true,
    );
    assert_eq!(read_failure.check().await, Ok(CleanupReport::unknown()));
    Ok(())
}

#[derive(Debug)]
struct RootRepository {
    state: Option<GenerationRootState>,
}

#[async_trait]
impl GenerationRootRepository for RootRepository {
    async fn read_generation_root_state(
        &self,
        _root: Uuid,
        _now: OffsetDateTime,
    ) -> Result<GenerationRootState, AppError> {
        self.state.ok_or(AppError::Internal)
    }
}

#[derive(Debug)]
struct Files {
    qualifies: bool,
}

impl GenerationRootFiles for Files {
    fn qualify_owned_root(&self) -> Result<(), RemovalFailure> {
        if self.qualifies {
            Ok(())
        } else {
            Err(RemovalFailure::new(
                RemovalFailureCategory::OwnershipUnproven,
            ))
        }
    }
}

fn qualified_health(
    state: Option<GenerationRootState>,
    qualifies: bool,
    enabled: bool,
) -> QualifiedCleanupHealth {
    let root = Uuid::new_v4();
    let qualification = GenerationRootQualification::new(
        Some(Arc::new(RootRepository { state }) as Arc<dyn GenerationRootRepository>),
        Some(Arc::new(Files { qualifies }) as Arc<dyn GenerationRootFiles>),
        Some(root),
        enabled,
    );
    let repository = Arc::new(SnapshotRepository {
        snapshot: Some(healthy_snapshot()),
        seen: Mutex::new(None),
    });
    QualifiedCleanupHealth::new(
        Arc::new(qualification),
        repository as Arc<dyn CleanupHealthRepository>,
        clock(),
    )
}

const REGISTERED: GenerationRootState = GenerationRootState {
    registered: true,
    expired_pending: false,
};

#[tokio::test]
async fn an_unqualified_lifecycle_reports_unknown() -> TestResult {
    let disabled = qualified_health(Some(REGISTERED), true, false);
    assert_eq!(disabled.check().await, Ok(CleanupReport::unknown()));

    let unregistered = qualified_health(
        Some(GenerationRootState {
            registered: false,
            expired_pending: false,
        }),
        true,
        true,
    );
    assert_eq!(unregistered.check().await, Ok(CleanupReport::unknown()));

    let unproven = qualified_health(Some(REGISTERED), false, true);
    assert_eq!(unproven.check().await, Ok(CleanupReport::unknown()));
    Ok(())
}

#[tokio::test]
async fn a_qualified_lifecycle_reports_the_backlog() -> TestResult {
    let report = qualified_health(Some(REGISTERED), true, true)
        .check()
        .await
        .map_err(|()| "port failed")?;
    assert_eq!(report.status, CleanupStatus::Healthy);
    Ok(())
}

#[tokio::test]
async fn expired_pending_generations_force_unknown_but_keep_the_counts() -> TestResult {
    let report = qualified_health(
        Some(GenerationRootState {
            registered: true,
            expired_pending: true,
        }),
        true,
        true,
    )
    .check()
    .await
    .map_err(|()| "port failed")?;
    assert_eq!(report.status, CleanupStatus::Unknown);
    assert_eq!(report.pending_count, 0);
    Ok(())
}

#[test]
fn the_fixed_clock_is_usable_for_the_probe_tests() {
    assert_eq!(FixedClock::new(NOW).now(), NOW);
}
