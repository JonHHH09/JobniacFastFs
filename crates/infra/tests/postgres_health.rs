//! Readiness probe tests against a disposable PostgreSQL 18 server.
//!
//! The server-backed tests need `JOBNIAC_TEST_PG_PORT` and are skipped with a notice when it is
//! unset. The unconfigured case needs no server.

use std::error::Error;

use jobniac_core::env::Environment;
use jobniac_core::health::{ErrorCategory, HealthProbe, HealthProbeResult};
use jobniac_infra::postgres::config::{DatabaseConfig, load_database_config};
use jobniac_infra::postgres::health::PostgresHealthProbe;
use jobniac_testkit::pg::TestServer;

type TestResult = Result<(), Box<dyn Error>>;

fn config_for(environment: &Environment) -> Result<DatabaseConfig, Box<dyn Error>> {
    load_database_config(environment)
        .ok_or_else(|| "test environment is not a valid JOBNIAC_DB_* set".into())
}

/// The same loopback target as `environment`, with a different password.
fn with_password(environment: &Environment, password: &str) -> Result<Environment, Box<dyn Error>> {
    let mut pairs = Vec::new();
    for key in [
        "JOBNIAC_DB_HOST",
        "JOBNIAC_DB_PORT",
        "JOBNIAC_DB_NAME",
        "JOBNIAC_DB_USER",
    ] {
        let value = environment
            .get(key)
            .ok_or("test environment is missing a JOBNIAC_DB_* value")?;
        pairs.push((key.to_owned(), value.to_owned()));
    }
    pairs.push(("JOBNIAC_DB_PASSWORD".to_owned(), password.to_owned()));
    Ok(Environment::from_pairs(pairs))
}

#[tokio::test]
async fn probe_reports_ready_on_postgresql_18() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let probe = PostgresHealthProbe::new(Some(config_for(&database.environment())?));
    assert_eq!(probe.probe().await, HealthProbeResult::ready());
    Ok(())
}

#[tokio::test]
async fn wrong_password_is_an_authentication_failure() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let environment = with_password(&database.environment(), "synthetic-wrong-password")?;
    let probe = PostgresHealthProbe::new(Some(config_for(&environment)?));
    let result = probe.probe().await;
    assert!(!result.successful());
    assert_eq!(result.error_category(), ErrorCategory::AuthenticationFailed);
    Ok(())
}

#[tokio::test]
async fn unconfigured_probe_reports_connection_unavailable() {
    let probe = PostgresHealthProbe::new(None);
    assert_eq!(
        probe.probe().await,
        HealthProbeResult::failed(ErrorCategory::ConnectionUnavailable)
    );
}
