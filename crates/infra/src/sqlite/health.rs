//! Sanitized, read-only SQLite readiness probe.
//!
//! The probe checks the bundled engine version, runs `SELECT 1` and `PRAGMA quick_check` on a
//! short-lived read-only connection. It never returns or logs error text; it reports only an
//! [`ErrorCategory`].

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use jobniac_core::health::{ErrorCategory, HealthProbe, HealthProbeResult};
use rusqlite::Connection;

use crate::sqlite::error::SqliteError;
use crate::sqlite::pool::open_standalone;

/// The oldest SQLite the schema supports (3.46.0): STRICT tables, `RAISE` in triggers, `unixepoch`
/// and the JSON functions the baseline uses. The bundled engine is newer; the check guards against
/// a build that links a system library.
pub const MIN_SQLITE_VERSION_NUMBER: i32 = 3_046_000;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// Whether the linked engine is at least [`MIN_SQLITE_VERSION_NUMBER`].
///
/// # Errors
///
/// [`SqliteError::Unavailable`] when it is older.
pub fn check_engine_version() -> Result<(), SqliteError> {
    if rusqlite::version_number() >= MIN_SQLITE_VERSION_NUMBER {
        Ok(())
    } else {
        Err(SqliteError::Unavailable)
    }
}

/// Run `PRAGMA quick_check` and accept only the single answer `ok`.
///
/// The failure never carries the engine's report, which can name tables and pages.
///
/// # Errors
///
/// [`SqliteError::Unavailable`] when the report is not exactly `ok` (the file is damaged), or the
/// classified engine error when the pragma itself fails.
pub fn quick_check(connection: &Connection) -> Result<(), SqliteError> {
    let mut statement = connection.prepare("PRAGMA quick_check")?;
    let rows: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    if check_quick_check_report(&rows) {
        Ok(())
    } else {
        Err(SqliteError::Unavailable)
    }
}

/// Whether a `quick_check` report is the healthy one.
fn check_quick_check_report(rows: &[String]) -> bool {
    matches!(rows, [only] if only == "ok")
}

/// Readiness probe for the configured local database file.
///
/// Without a configured file the probe reports `ConnectionUnavailable` and opens nothing.
#[derive(Debug)]
pub struct SqliteHealthProbe {
    path: Option<PathBuf>,
}

impl SqliteHealthProbe {
    /// A probe for the database at `path`; `None` when the database is not configured.
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }
}

#[async_trait]
impl HealthProbe for SqliteHealthProbe {
    async fn probe(&self) -> HealthProbeResult {
        let Some(path) = self.path.clone() else {
            return HealthProbeResult::failed(ErrorCategory::ConnectionUnavailable);
        };
        let task = tokio::task::spawn_blocking(move || probe_blocking(&path));
        match tokio::time::timeout(PROBE_TIMEOUT, task).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => HealthProbeResult::failed(ErrorCategory::Unknown),
            Err(_elapsed) => HealthProbeResult::failed(ErrorCategory::Timeout),
        }
    }
}

fn probe_blocking(path: &std::path::Path) -> HealthProbeResult {
    match run_probe(path) {
        Ok(()) => HealthProbeResult::ready(),
        Err(category) => HealthProbeResult::failed(category),
    }
}

fn run_probe(path: &std::path::Path) -> Result<(), ErrorCategory> {
    let connection =
        open_standalone(path, true, PROBE_BUSY_TIMEOUT).map_err(|error| categorize(&error))?;
    if check_engine_version().is_err() {
        return Err(ErrorCategory::QueryFailed);
    }
    let one: i64 = connection
        .query_row("SELECT 1", [], |row| row.get(0))
        .map_err(|error| categorize(&SqliteError::from(error)))?;
    if one != 1 {
        return Err(ErrorCategory::QueryFailed);
    }
    quick_check(&connection).map_err(|error| categorize(&error))
}

/// The category of a failure, without keeping its text: a lock wait that ran out is a timeout, an
/// open, I/O or damage failure is an unavailable connection, anything else a query failure.
fn categorize(error: &SqliteError) -> ErrorCategory {
    match error {
        SqliteError::Busy => ErrorCategory::Timeout,
        SqliteError::NotConfigured | SqliteError::Unavailable => {
            ErrorCategory::ConnectionUnavailable
        }
        _ => ErrorCategory::QueryFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_engine_meets_the_minimum_version() {
        assert_eq!(check_engine_version(), Ok(()));
        assert!(rusqlite::version_number() >= MIN_SQLITE_VERSION_NUMBER);
    }

    #[test]
    fn only_a_single_ok_row_is_a_healthy_report() {
        assert!(check_quick_check_report(&["ok".to_owned()]));
        assert!(!check_quick_check_report(&[]));
        assert!(!check_quick_check_report(&[
            "row 3 missing from index x".to_owned()
        ]));
        assert!(!check_quick_check_report(&[
            "ok".to_owned(),
            "ok".to_owned()
        ]));
    }

    #[test]
    fn failures_follow_the_categories() {
        let cases = [
            (SqliteError::Busy, ErrorCategory::Timeout),
            (
                SqliteError::Unavailable,
                ErrorCategory::ConnectionUnavailable,
            ),
            (
                SqliteError::NotConfigured,
                ErrorCategory::ConnectionUnavailable,
            ),
            (SqliteError::Internal, ErrorCategory::QueryFailed),
        ];
        for (error, expected) in cases {
            assert_eq!(categorize(&error), expected, "{error:?}");
        }
    }

    #[test]
    fn quick_check_passes_on_a_fresh_database() -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1)")?;
        assert_eq!(quick_check(&connection), Ok(()));
        Ok(())
    }

    #[tokio::test]
    async fn an_unconfigured_probe_reports_connection_unavailable() {
        let result = SqliteHealthProbe::new(None).probe().await;
        assert_eq!(
            result,
            HealthProbeResult::failed(ErrorCategory::ConnectionUnavailable)
        );
    }

    #[tokio::test]
    async fn a_missing_file_is_connection_unavailable_and_is_not_created()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("missing.sqlite3");
        let result = SqliteHealthProbe::new(Some(path.clone())).probe().await;
        assert_eq!(
            result,
            HealthProbeResult::failed(ErrorCategory::ConnectionUnavailable)
        );
        assert!(!path.exists(), "the probe never creates the database");
        Ok(())
    }

    #[tokio::test]
    async fn a_healthy_database_is_ready_and_a_garbage_file_is_not()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("ok.sqlite3");
        let pool = crate::sqlite::pool::SqlitePool::open(&path)?;
        let result = SqliteHealthProbe::new(Some(path)).probe().await;
        assert_eq!(result, HealthProbeResult::ready());
        drop(pool);

        let garbage = directory.path().join("garbage.sqlite3");
        std::fs::write(&garbage, vec![0x42_u8; 4096])?;
        let result = SqliteHealthProbe::new(Some(garbage)).probe().await;
        assert!(!result.successful());
        Ok(())
    }
}
