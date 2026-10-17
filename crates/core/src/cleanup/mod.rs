//! Owned-asset cleanup: the durable queue contracts, the bounded executor and the observed backlog
//! health.
//!
//! The executor and the health probe depend only on ports. The PostgreSQL ledger and the owned
//! file namespace implement them in `jobniac_infra`. Entity-deletion cleanup is not here.

pub mod executor;
pub mod health;
pub mod lifecycle;
pub mod model;
pub mod qualification;

pub use lifecycle::{GenerationLifecycle, GenerationLifecycleSummary};

pub use executor::{
    CleanupExecutor, CleanupRepository, CleanupRunSummary, CleanupUnavailable, OwnedAssetRemover,
};
pub use health::{CleanupHealthRepository, CleanupHealthService, QualifiedCleanupHealth, classify};
pub use model::{
    COMPLETED_RETENTION, CleanupClaim, CleanupOutcome, CleanupQueueSnapshot, InvalidCleanupClaim,
    OLDEST_DUE_THRESHOLD, REPEATED_FAILURE_ATTEMPTS, RETENTION_BATCH_SIZE, RETENTION_MAX_BATCHES,
    RETRY_DELAY, RemovalFailure, RemovalFailureCategory, RemovalOutcome,
};
pub use qualification::{
    GenerationRootFiles, GenerationRootQualification, GenerationRootRepository, GenerationRootState,
};

pub use crate::generated_assets::{CLAIM_LEASE, PreparationOutcome, RETRY_BATCH_SIZE};
