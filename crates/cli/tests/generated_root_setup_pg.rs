//! The explicit generated-root setup against a disposable PostgreSQL 18 server: the first run
//! initializes and registers an empty root, the second confirms it without change, and a different
//! configured identity is refused. The server-backed test skips with a notice when
//! `JOBNIAC_TEST_PG_PORT` is unset locally; on CI (`CI` set) a missing port fails it. All data is
//! synthetic, and the root is a temporary directory removed at the end.

use std::error::Error;

use jobniac_core::generated_root_setup::{SetupRefusal, SetupStatus};
use jobniac_infra::generated::root::ensure_private_directory;
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::migrations::migrate;
use jobniac_runtime::generation::{GeneratedRootLocation, compose_root_setup};
use jobniac_testkit::pg::TestServer;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

#[tokio::test]
async fn setup_creates_then_confirms_the_root_and_refuses_a_foreign_identity() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        assert!(
            std::env::var_os("CI").is_none(),
            "JOBNIAC_TEST_PG_PORT must be set in CI so that the PostgreSQL tests run"
        );
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let config = load_database_config(&database.environment())
        .ok_or("the disposable database configuration is invalid")?;
    migrate(&config).await.map_err(|error| error.0)?;

    // The root must be private (owner-only permissions), as the default root is created.
    let root = std::env::temp_dir().join(format!("jobniac-setup-{}", Uuid::new_v4()));
    let root_text = root.to_string_lossy().into_owned();
    if !ensure_private_directory(&root_text) {
        return Err("the private test root could not be created".into());
    }
    let location = GeneratedRootLocation {
        root: root_text,
        is_default: false,
    };
    let outcome = check(&config, &location).await;
    let _ = std::fs::remove_dir_all(&root);
    outcome
}

async fn check(
    config: &jobniac_infra::postgres::config::DatabaseConfig,
    location: &GeneratedRootLocation,
) -> TestResult {
    let created = compose_root_setup(config, location, None)
        .run()
        .await
        .map_err(|refusal| format!("the first run was refused: {}", refusal.text()))?;
    assert_eq!(created.status, SetupStatus::Created);

    let confirmed = compose_root_setup(config, location, Some(created.root_id))
        .run()
        .await
        .map_err(|refusal| format!("the second run was refused: {}", refusal.text()))?;
    assert_eq!(confirmed.status, SetupStatus::AlreadySetUp);
    assert_eq!(confirmed.root_id, created.root_id);

    let foreign = compose_root_setup(config, location, Some(Uuid::new_v4()))
        .run()
        .await;
    assert_eq!(foreign, Err(SetupRefusal::IdentityMismatch));
    Ok(())
}
