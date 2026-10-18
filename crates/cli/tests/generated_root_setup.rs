//! The explicit generated-root setup against a temporary SQLite database: the first run initializes
//! and registers an empty root, the second confirms it without change, and a different configured
//! identity is refused. All data is synthetic, and the root is a temporary directory removed at the
//! end.

use std::error::Error;

use jobniac_core::generated_root_setup::{SetupRefusal, SetupStatus};
use jobniac_infra::generated::root::ensure_private_directory;
use jobniac_infra::sqlite::migrations::migrate;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_runtime::generation::{GeneratedRootLocation, compose_root_setup};
use jobniac_testkit::scratch::scratch_dir;
use jobniac_testkit::sqlite::TempDatabase;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

#[tokio::test]
async fn setup_creates_then_confirms_the_root_and_refuses_a_foreign_identity() -> TestResult {
    let database = TempDatabase::new()?;
    migrate(database.path()).map_err(|error| error.0)?;
    let pool = SqlitePool::open(database.path())?;

    // The root must be private (owner-only permissions), as the default root is created.
    // The scratch folder is in canonical form (the owned-root checks refuse a root that differs from
    // its final path, and `%TEMP%` can be an 8.3 alias on Windows) and is removed when it drops.
    let scratch = scratch_dir("jobniac-setup-")?;
    let root_text = scratch
        .path()
        .join("generated")
        .to_string_lossy()
        .into_owned();
    if !ensure_private_directory(&root_text) {
        return Err("the private test root could not be created".into());
    }
    let location = GeneratedRootLocation {
        root: root_text,
        is_default: false,
    };
    check(&pool, &location).await
}

async fn check(pool: &SqlitePool, location: &GeneratedRootLocation) -> TestResult {
    let created = compose_root_setup(pool, location, None)
        .run()
        .await
        .map_err(|refusal| format!("the first run was refused: {}", refusal.text()))?;
    assert_eq!(created.status, SetupStatus::Created);

    let confirmed = compose_root_setup(pool, location, Some(created.root_id))
        .run()
        .await
        .map_err(|refusal| format!("the second run was refused: {}", refusal.text()))?;
    assert_eq!(confirmed.status, SetupStatus::AlreadySetUp);
    assert_eq!(confirmed.root_id, created.root_id);

    let foreign = compose_root_setup(pool, location, Some(Uuid::new_v4()))
        .run()
        .await;
    assert_eq!(foreign, Err(SetupRefusal::IdentityMismatch));
    Ok(())
}
