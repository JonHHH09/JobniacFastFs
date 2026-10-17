//! Cleanup executor: enablement, claim batches, outcome counting, the identity guard handed to the
//! repository, retention pruning and the claim and failure value rules. Pure, with in-memory ports.

use std::error::Error;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jobniac_core::cleanup::{
    CleanupClaim, CleanupExecutor, CleanupHealthRepository, CleanupOutcome, CleanupRepository,
    CleanupRunSummary, CleanupUnavailable, OwnedAssetRemover, RETENTION_BATCH_SIZE, RemovalFailure,
    RemovalFailureCategory, RemovalOutcome,
};
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::{
    AssetIdentity, PreparationOutcome, RETRY_BATCH_SIZE, sha256_hex,
};
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const NOW: OffsetDateTime = datetime!(2026-10-15 12:00:00 UTC);

fn identity(root: Uuid) -> Result<AssetIdentity, Box<dyn Error>> {
    let content = b"%PDF-1.7\n% synthetic cleanup asset\n";
    Ok(AssetIdentity::new(
        Uuid::new_v4(),
        root,
        &sha256_hex(content),
        u64::try_from(content.len())?,
    )?)
}

fn claim(asset: AssetIdentity, attempt: u32) -> Result<CleanupClaim, Box<dyn Error>> {
    Ok(CleanupClaim::new(
        Uuid::new_v4(),
        asset,
        Uuid::new_v4(),
        attempt,
        NOW,
        Uuid::new_v4(),
    )?)
}

/// Records every removal call and returns a scripted outcome.
#[derive(Debug, Default)]
struct RecordingRemover {
    calls: Mutex<Vec<Uuid>>,
}

impl OwnedAssetRemover for RecordingRemover {
    fn remove_owned(&self, identity: &AssetIdentity) -> Result<RemovalOutcome, RemovalFailure> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(identity.asset_id());
        }
        Ok(RemovalOutcome::Removed)
    }
}

/// Scripted repository: claims are returned as given, and each execution either calls the remover
/// with `target` (the identity the repository would reconstruct) or fails.
#[derive(Debug)]
struct ScriptedRepository {
    claims: Vec<CleanupClaim>,
    claim_error: bool,
    execute_error: bool,
    target: Option<AssetIdentity>,
    outcome: CleanupOutcome,
    executed: Mutex<usize>,
    prune_results: Mutex<Vec<usize>>,
    prune_calls: Mutex<usize>,
    guard_result: Mutex<Option<RemovalFailure>>,
}

impl ScriptedRepository {
    fn new(claims: Vec<CleanupClaim>) -> Self {
        Self {
            claims,
            claim_error: false,
            execute_error: false,
            target: None,
            outcome: CleanupOutcome::Completed,
            executed: Mutex::new(0),
            prune_results: Mutex::new(Vec::new()),
            prune_calls: Mutex::new(0),
            guard_result: Mutex::new(None),
        }
    }
}

#[async_trait]
impl CleanupHealthRepository for ScriptedRepository {
    async fn read_queue_snapshot(
        &self,
        _now: OffsetDateTime,
        _attempts: u32,
        _completed_before: OffsetDateTime,
    ) -> Result<jobniac_core::cleanup::CleanupQueueSnapshot, AppError> {
        Err(AppError::Internal)
    }
}

#[async_trait]
impl CleanupRepository for ScriptedRepository {
    async fn prepare_cleanup(&self, _asset_id: Uuid) -> Result<PreparationOutcome, AppError> {
        Ok(PreparationOutcome::Prepared)
    }

    async fn claim_due(&self, _root: Uuid, _limit: usize) -> Result<Vec<CleanupClaim>, AppError> {
        if self.claim_error {
            return Err(AppError::Internal);
        }
        Ok(self.claims.clone())
    }

    async fn renew_claim(&self, _claim: &CleanupClaim) -> Result<Option<CleanupClaim>, AppError> {
        Ok(None)
    }

    async fn execute_claim(
        &self,
        _claim: &CleanupClaim,
        remover: &dyn OwnedAssetRemover,
    ) -> Result<CleanupOutcome, AppError> {
        if self.execute_error {
            return Err(AppError::Internal);
        }
        if let Ok(mut executed) = self.executed.lock() {
            *executed += 1;
        }
        if let Some(target) = &self.target {
            let result = remover.remove_owned(target);
            if let (Err(failure), Ok(mut slot)) = (result, self.guard_result.lock()) {
                *slot = Some(failure);
            }
        }
        Ok(self.outcome)
    }

    async fn prune_completed(&self, _limit: usize) -> Result<usize, AppError> {
        if let Ok(mut calls) = self.prune_calls.lock() {
            *calls += 1;
        }
        let mut results = self.prune_results.lock().map_err(|_| AppError::Internal)?;
        if results.is_empty() {
            return Ok(RETENTION_BATCH_SIZE);
        }
        Ok(results.remove(0))
    }
}

fn enabled(repository: Arc<ScriptedRepository>, root: Uuid) -> CleanupExecutor {
    let remover: Arc<dyn OwnedAssetRemover> = Arc::new(RecordingRemover::default());
    CleanupExecutor::new(Some(repository), Some(remover), Some(root), true)
}

#[tokio::test]
async fn disabled_or_unconfigured_executors_refuse_every_run() -> TestResult {
    let root = Uuid::new_v4();
    let repository: Arc<dyn CleanupRepository> = Arc::new(ScriptedRepository::new(Vec::new()));
    let remover: Arc<dyn OwnedAssetRemover> = Arc::new(RecordingRemover::default());
    let refusal = CleanupUnavailable("Cleanup lifecycle is not enabled");

    let disabled = CleanupExecutor::new(
        Some(Arc::clone(&repository)),
        Some(Arc::clone(&remover)),
        Some(root),
        false,
    );
    assert_eq!(disabled.run_due_tasks().await, Err(refusal.clone()));
    assert_eq!(disabled.purge_completed_tasks().await, Err(refusal.clone()));

    let no_root = CleanupExecutor::new(Some(repository), Some(remover), None, true);
    assert_eq!(no_root.run_due_tasks().await, Err(refusal));
    Ok(())
}

#[tokio::test]
async fn run_counts_each_outcome_and_skips_foreign_roots() -> TestResult {
    let root = Uuid::new_v4();
    let own = identity(root)?;
    let foreign = identity(Uuid::new_v4())?;
    let claims = vec![
        claim(own.clone(), 1)?,
        claim(own.clone(), 1)?,
        claim(foreign, 1)?,
    ];
    let mut repository = ScriptedRepository::new(claims);
    repository.outcome = CleanupOutcome::RetryScheduled;
    let repository = Arc::new(repository);
    let executor = enabled(Arc::clone(&repository), root);

    let summary = executor.run_due_tasks().await?;

    assert_eq!(
        summary,
        CleanupRunSummary {
            claimed: 3,
            retry_scheduled: 2,
            state_failures: 1,
            ..CleanupRunSummary::default()
        }
    );
    assert_eq!(*repository.executed.lock().map_err(|_| "lock")?, 2);
    Ok(())
}

#[tokio::test]
async fn an_execution_failure_is_a_state_failure_and_a_claim_failure_is_unavailable() -> TestResult
{
    let root = Uuid::new_v4();
    let mut failing = ScriptedRepository::new(vec![claim(identity(root)?, 1)?]);
    failing.execute_error = true;
    let summary = enabled(Arc::new(failing), root).run_due_tasks().await?;
    assert_eq!(summary.state_failures, 1);
    assert_eq!(summary.claimed, 1);

    let mut unclaimable = ScriptedRepository::new(Vec::new());
    unclaimable.claim_error = true;
    assert_eq!(
        enabled(Arc::new(unclaimable), root).run_due_tasks().await,
        Err(CleanupUnavailable("Cleanup state is unavailable"))
    );
    Ok(())
}

#[tokio::test]
async fn the_guard_refuses_an_identity_other_than_the_claimed_one() -> TestResult {
    let root = Uuid::new_v4();
    let claimed = identity(root)?;
    let substituted = identity(root)?;
    let mut repository = ScriptedRepository::new(vec![claim(claimed, 1)?]);
    repository.target = Some(substituted);
    let repository = Arc::new(repository);
    let remover = Arc::new(RecordingRemover::default());
    let executor = CleanupExecutor::new(
        Some(Arc::clone(&repository) as Arc<dyn CleanupRepository>),
        Some(Arc::clone(&remover) as Arc<dyn OwnedAssetRemover>),
        Some(root),
        true,
    );

    executor.run_due_tasks().await?;

    let refused = repository
        .guard_result
        .lock()
        .map_err(|_| "lock")?
        .ok_or("the guard must refuse a substituted identity")?;
    assert_eq!(refused.category(), RemovalFailureCategory::IdentityMismatch);
    assert!(remover.calls.lock().map_err(|_| "lock")?.is_empty());
    Ok(())
}

#[tokio::test]
async fn the_claimed_identity_reaches_the_remover_once() -> TestResult {
    let root = Uuid::new_v4();
    let claimed = identity(root)?;
    let expected_id = claimed.asset_id();
    let mut repository = ScriptedRepository::new(vec![claim(claimed.clone(), 1)?]);
    repository.target = Some(claimed);
    let repository = Arc::new(repository);
    let remover = Arc::new(RecordingRemover::default());
    let executor = CleanupExecutor::new(
        Some(Arc::clone(&repository) as Arc<dyn CleanupRepository>),
        Some(Arc::clone(&remover) as Arc<dyn OwnedAssetRemover>),
        Some(root),
        true,
    );

    executor.run_due_tasks().await?;

    assert!(
        repository
            .guard_result
            .lock()
            .map_err(|_| "lock")?
            .is_none()
    );
    assert_eq!(
        *remover.calls.lock().map_err(|_| "lock")?,
        vec![expected_id]
    );
    Ok(())
}

#[tokio::test]
async fn purge_repeats_full_batches_and_stops_at_the_bound() -> TestResult {
    let root = Uuid::new_v4();
    let repository = Arc::new(ScriptedRepository::new(Vec::new()));
    let executor = enabled(Arc::clone(&repository), root);
    assert_eq!(
        executor.purge_completed_tasks().await,
        Ok(10 * RETENTION_BATCH_SIZE)
    );
    assert_eq!(*repository.prune_calls.lock().map_err(|_| "lock")?, 10);

    let mut partial = ScriptedRepository::new(Vec::new());
    partial.prune_results = Mutex::new(vec![RETENTION_BATCH_SIZE, RETENTION_BATCH_SIZE, 5]);
    let partial = Arc::new(partial);
    let total = enabled(Arc::clone(&partial), root)
        .purge_completed_tasks()
        .await?;
    assert_eq!(total, 2 * RETENTION_BATCH_SIZE + 5);
    assert_eq!(*partial.prune_calls.lock().map_err(|_| "lock")?, 3);
    Ok(())
}

#[test]
fn claims_refuse_a_zero_attempt_and_keep_microsecond_expiry() -> TestResult {
    let root = Uuid::new_v4();
    assert!(
        CleanupClaim::new(
            Uuid::new_v4(),
            identity(root)?,
            Uuid::new_v4(),
            0,
            NOW,
            Uuid::new_v4()
        )
        .is_err()
    );
    let precise = NOW + time::Duration::nanoseconds(123_456_789);
    let claim = CleanupClaim::new(
        Uuid::new_v4(),
        identity(root)?,
        Uuid::new_v4(),
        1,
        precise,
        Uuid::new_v4(),
    )?;
    assert_eq!(
        claim.expires_at(),
        NOW + time::Duration::nanoseconds(123_456_000)
    );
    Ok(())
}

#[test]
fn only_file_delete_failures_are_retryable_and_names_are_fixed() {
    assert!(RemovalFailureCategory::FileDeleteFailed.is_retryable());
    assert!(!RemovalFailureCategory::IdentityMismatch.is_retryable());
    assert!(!RemovalFailureCategory::OwnershipUnproven.is_retryable());
    assert!(!RemovalFailureCategory::UnsupportedPlatform.is_retryable());
    assert_eq!(
        RemovalFailureCategory::FileDeleteFailed.as_str(),
        "FILE_DELETE_FAILED"
    );
    assert_eq!(RETRY_BATCH_SIZE, 25);
}
