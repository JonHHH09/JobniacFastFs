//! Migration ledger tests against a disposable PostgreSQL 18 server.
//!
//! The fixture `tests/golden/migrations/ledger.json` is the recorded ledger of the packaged
//! migrations. Two directions are checked:
//!
//! - a database migrated by Rust leaves exactly the recorded ledger (version, description, script
//!   and checksum of every row);
//! - a database carrying the recorded ledger is `Current` for Rust, so the recorded checksums are
//!   accepted.
//!
//! The tests need `JOBNIAC_TEST_PG_PORT` and skip with a notice when it is unset. Each test works
//! on its own fresh database.
//!
//! Not covered here: the advisory-lock contention path.

use std::error::Error;
use std::path::PathBuf;

use jobniac_core::env::Environment;
use jobniac_infra::postgres::config::{DatabaseConfig, load_database_config};
use jobniac_infra::postgres::migrations::{SchemaState, inspect_schema, migrate};
use jobniac_testkit::pg::TestServer;
use tokio_postgres::Client;

type TestResult = Result<(), Box<dyn Error>>;

/// The packaged versions, in order.
const ALL_VERSIONS: [u32; 6] = [1, 2, 3, 4, 5, 6];

/// One row of the history table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LedgerRow {
    version: i32,
    description: String,
    script: String,
    checksum: String,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/migrations/ledger.json")
}

/// The recorded ledger rows, read from the fixture.
fn recorded_ledger() -> Result<Vec<LedgerRow>, Box<dyn Error>> {
    let text = std::fs::read_to_string(fixture_path())?;
    let document: serde_json::Value = serde_json::from_str(&text)?;
    let rows = document["rows"]
        .as_array()
        .ok_or("the ledger fixture has no rows array")?;
    rows.iter()
        .map(|row| -> Result<LedgerRow, Box<dyn Error>> {
            let version = row["version"]
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .ok_or("the ledger fixture has a bad version")?;
            Ok(LedgerRow {
                version,
                description: text_field(row, "description")?,
                script: text_field(row, "script")?,
                checksum: text_field(row, "checksum")?,
            })
        })
        .collect()
}

fn text_field(row: &serde_json::Value, key: &str) -> Result<String, Box<dyn Error>> {
    row[key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("the ledger fixture has no {key}").into())
}

/// The history table as it is now, in version order.
async fn read_ledger(client: &Client) -> Result<Vec<LedgerRow>, Box<dyn Error>> {
    let rows = client
        .query(
            "SELECT version, description, script, checksum \
             FROM jobniac.schema_migrations ORDER BY version",
            &[],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| LedgerRow {
            version: row.get(0),
            description: row.get(1),
            script: row.get(2),
            checksum: row.get(3),
        })
        .collect())
}

/// The validated configuration of a fresh database's environment.
fn config_for(environment: &Environment) -> Result<DatabaseConfig, Box<dyn Error>> {
    load_database_config(environment)
        .ok_or_else(|| "test environment is not a valid JOBNIAC_DB_* set".into())
}

#[tokio::test]
async fn rust_migration_leaves_the_recorded_ledger() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let config = config_for(&database.environment())?;

    assert_eq!(migrate(&config).await?, ALL_VERSIONS.to_vec());

    let admin = database.admin_client().await;
    assert_eq!(read_ledger(&admin).await?, recorded_ledger()?);
    assert_eq!(
        inspect_schema(Some(&config)).await?.state,
        SchemaState::Current
    );
    Ok(())
}

#[tokio::test]
async fn recorded_ledger_is_current_for_rust() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let config = config_for(&database.environment())?;

    // The history table as the recorded ledger has it (same DDL, same column order).
    let admin = database.admin_client().await;
    admin
        .batch_execute(
            "CREATE SCHEMA jobniac; \
             CREATE TABLE jobniac.schema_migrations ( \
                 version integer PRIMARY KEY CHECK (version > 0), \
                 description text NOT NULL, \
                 script text NOT NULL, \
                 checksum char(64) NOT NULL, \
                 applied_at timestamptz NOT NULL DEFAULT clock_timestamp(), \
                 execution_ms integer NOT NULL CHECK (execution_ms >= 0))",
        )
        .await?;
    for row in recorded_ledger()? {
        admin
            .execute(
                "INSERT INTO jobniac.schema_migrations \
                 (version, description, script, checksum, execution_ms) \
                 VALUES ($1, $2, $3, $4, 0)",
                &[&row.version, &row.description, &row.script, &row.checksum],
            )
            .await?;
    }

    let status = inspect_schema(Some(&config)).await?;
    assert_eq!(status.state, SchemaState::Current, "{}", status.detail);
    assert_eq!(status.current_version, Some(6));
    assert!(status.pending.is_empty());
    assert!(migrate(&config).await?.is_empty());
    Ok(())
}
