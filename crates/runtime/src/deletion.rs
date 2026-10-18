//! Deletion composition.
//!
//! The deletion use cases are admitted only when `JOBNIAC_DELETION_ENABLED` is exactly `1` and a
//! database is configured. Both come from the one environment snapshot the
//! composition root takes, so the flag and the database always describe the same process. Without
//! either condition nothing is composed and the tools stay unregistered.

use std::sync::Arc;

use jobniac_core::clock::SystemClock;
use jobniac_core::deletion::{
    JobDeletionRepository, JobDeletionService, ProfileDeletionRepository, ProfileDeletionService,
};
use jobniac_core::env::Environment;
use jobniac_infra::sqlite::deletion::SqliteEntityDeletionRepository;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;

/// Opt-in flag of the profile and job deletion tools: admitted only as exactly `1`.
pub const DELETION_ENABLED_VARIABLE: &str = "JOBNIAC_DELETION_ENABLED";

/// The composed deletion use cases. Each is `None` unless the flag and the database are present.
#[derive(Debug, Clone, Default)]
pub struct DeletionServices {
    /// The profile hard-delete use case behind `delete_profile`.
    pub profile: Option<Arc<ProfileDeletionService>>,
    /// The job hard-delete use case behind `delete_job`.
    pub job: Option<Arc<JobDeletionService>>,
}

/// Composes the deletion use cases from the snapshot `env` and the open database.
#[must_use]
pub fn compose_deletion(env: &Environment, database: Option<&SqlitePool>) -> DeletionServices {
    let Some(database) = database else {
        return DeletionServices::default();
    };
    if !env.flag_enabled(DELETION_ENABLED_VARIABLE) {
        return DeletionServices::default();
    }
    let uow = SqliteUnitOfWork::new(database.clone());
    let repository = Arc::new(SqliteEntityDeletionRepository::new(
        uow,
        Arc::new(SystemClock),
    ));
    let profile_repository: Arc<dyn ProfileDeletionRepository> = repository.clone();
    let job_repository: Arc<dyn JobDeletionRepository> = repository;
    DeletionServices {
        profile: Some(Arc::new(ProfileDeletionService::new(profile_repository))),
        job: Some(Arc::new(JobDeletionService::new(job_repository))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> (tempfile::TempDir, SqlitePool) {
        let root = tempfile::tempdir().unwrap_or_else(|_| panic!("a temporary directory"));
        let pool = SqlitePool::open(root.path().join("jobniac.sqlite3"))
            .unwrap_or_else(|_| panic!("the temporary database must open"));
        (root, pool)
    }

    #[test]
    fn only_the_exact_flag_with_a_database_composes_both_services() {
        let (_root, config) = database();
        let on = Environment::from_pairs([(DELETION_ENABLED_VARIABLE, "1")]);
        let composed = compose_deletion(&on, Some(&config));
        assert!(composed.profile.is_some() && composed.job.is_some());
        for value in ["true", "yes", " 1", "01", "0", ""] {
            let env = Environment::from_pairs([(DELETION_ENABLED_VARIABLE, value)]);
            let composed = compose_deletion(&env, Some(&config));
            assert!(
                composed.profile.is_none() && composed.job.is_none(),
                "{value:?}"
            );
        }
    }

    #[test]
    fn the_flag_without_a_database_composes_nothing() {
        let on = Environment::from_pairs([(DELETION_ENABLED_VARIABLE, "1")]);
        let composed = compose_deletion(&on, None);
        assert!(composed.profile.is_none() && composed.job.is_none());
    }
}
