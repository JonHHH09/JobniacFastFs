//! Explicit, append-only, checksummed schema migrations for the Jobniac SQLite database.
//!
//! The packaged `V<NNNN>__<description>.sql` files (under `sql/sqlite/`) are applied in version
//! order, each in one `BEGIN IMMEDIATE` transaction together with its history row and the
//! `PRAGMA user_version` mirror. SQLite DDL is transactional, so a failed migration leaves the file
//! as it was. Nothing runs implicitly: [`inspect_schema`] only reads, and [`migrate`] is the sole
//! writer.
//!
//! Fail-closed rules, all checked before any write: an applied migration whose packaged checksum
//! changed, an applied version unknown to this build (a newer database: an older app refuses it
//! instead of damaging it), a non-contiguous history, a `user_version` that does not match the
//! history, and a database that holds tables without Jobniac history.
//!
//! The history table is `schema_migrations`. Concurrent runners (the desktop app and the CLI)
//! serialize on the write lock: each migration re-reads the history after taking it and skips a
//! version another process already applied.
//!
//! Failure reporting: a database error during [`migrate`] is reported with its SQLite result code,
//! as in `migration failed (SQLite code 1); no partial migration was committed`. A database error
//! while [`inspect_schema`] reads the history is reported as `database inspection failed`.

use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::sqlite::pool::open_standalone;

/// `busy_timeout` of the migration connection (other processes may hold the write lock briefly).
const MIGRATION_BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/// `PRAGMA application_id` of a Jobniac database (ASCII `JOBN`). Set by the first migration; a
/// database with another non-zero id belongs to another application and is refused.
pub const APPLICATION_ID: i64 = 0x4A_4F_42_4E;

const HISTORY_EXISTS_SQL: &str = "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations')";
const FOREIGN_OBJECTS_SQL: &str = "SELECT EXISTS (SELECT 1 FROM sqlite_master \
    WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\')";
const APPLIED_SQL: &str = "SELECT version, checksum FROM schema_migrations ORDER BY version";
const INSERT_HISTORY_SQL: &str = "INSERT INTO schema_migrations \
    (version, description, script, checksum, applied_at, execution_ms) \
    VALUES (?1, ?2, ?3, ?4, ?5, ?6)";
const CREATE_HISTORY_TABLE_SQL: &str = "CREATE TABLE schema_migrations (\
    version INTEGER PRIMARY KEY CHECK (version > 0), \
    description TEXT NOT NULL, \
    script TEXT NOT NULL, \
    checksum TEXT NOT NULL CHECK (length(checksum) = 64), \
    applied_at TEXT NOT NULL, \
    execution_ms INTEGER NOT NULL CHECK (execution_ms >= 0)) STRICT";

const CONNECTION_FAILED: &str = "database connection failed";
const INSPECTION_FAILED: &str = "database inspection failed";
const CONTIGUITY_MESSAGE: &str = "packaged migrations must be numbered contiguously from V0001";
const NOT_CONTIGUOUS_MESSAGE: &str = "applied migration history is not contiguous";
const FOREIGN_MESSAGE: &str = "database contains tables without Jobniac migration history";
const FOREIGN_APPLICATION_MESSAGE: &str = "database belongs to another application";
const VERSION_MARKER_MESSAGE: &str = "database version marker does not match the migration history";

/// The packaged migration files, embedded byte for byte. Line endings are normalized on load.
const PACKAGED_FILES: [(&str, &str); 1] = [(
    "V0001__baseline.sql",
    include_str!("../../sql/sqlite/V0001__baseline.sql"),
)];

/// Schema state reported by [`inspect_schema`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaState {
    /// No database is configured.
    Unconfigured,
    /// The database holds no Jobniac history yet (or the file does not exist); `migrate` creates it.
    Empty,
    /// Some packaged migrations are not applied yet.
    Pending,
    /// Every packaged migration is applied and verified.
    Current,
    /// The history holds a version newer than this build knows.
    Ahead,
    /// The database holds tables, an application id or history that this build does not manage.
    Foreign,
}

/// Read-only schema status for startup diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaStatus {
    /// Classification of the database.
    pub state: SchemaState,
    /// The highest applied version in the history, when there is one.
    pub current_version: Option<u32>,
    /// The highest version this build packages.
    pub latest_version: u32,
    /// Packaged versions that are not applied.
    pub pending: Vec<u32>,
    /// A fixed, sanitized explanation.
    pub detail: String,
}

/// One packaged migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    /// The version number from the file name.
    pub version: u32,
    /// The description from the file name.
    pub description: String,
    /// The file name, for example `V0001__baseline.sql`.
    pub script: String,
    /// The SQL text with `\r\n` replaced by `\n`.
    pub sql: String,
}

impl Migration {
    /// Lowercase hex SHA-256 of the SQL after replacing `\r\n` with `\n`.
    ///
    /// This is the checksum stored in the migration ledger.
    pub fn checksum(&self) -> String {
        let normalized = normalize_line_endings(&self.sql);
        hex::encode(Sha256::digest(normalized.as_bytes()))
    }
}

/// A sanitized, fail-closed migration refusal. It never contains SQL text, paths or row values.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct MigrationError(pub String);

/// One history row. Only what verification needs is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedRow {
    /// The applied version.
    pub version: i64,
    /// The checksum recorded when it was applied.
    pub checksum: String,
}

/// Load and validate the packaged migrations, contiguous from `V0001`.
///
/// # Errors
///
/// [`MigrationError`] when the packaged versions are not numbered contiguously from 1.
pub fn packaged_migrations() -> Result<Vec<Migration>, MigrationError> {
    let mut migrations = Vec::with_capacity(PACKAGED_FILES.len());
    for (script, sql) in PACKAGED_FILES {
        let Some((version, description)) = parse_file_name(script) else {
            continue;
        };
        migrations.push(Migration {
            version,
            description: description.to_owned(),
            script: script.to_owned(),
            sql: normalize_line_endings(sql),
        });
    }
    migrations.sort_by_key(|migration| migration.version);
    let contiguous = migrations
        .iter()
        .zip(1_u32..)
        .all(|(migration, expected)| migration.version == expected);
    if !contiguous {
        return Err(MigrationError(CONTIGUITY_MESSAGE.to_owned()));
    }
    Ok(migrations)
}

/// Read-only schema state. Never creates or alters anything.
///
/// `None` yields [`SchemaState::Unconfigured`] without opening a file; a path that does not exist
/// yields [`SchemaState::Empty`] and is not created.
///
/// # Errors
///
/// [`MigrationError`] when the file cannot be read as a database.
pub fn inspect_schema(path: Option<&Path>) -> Result<SchemaStatus, MigrationError> {
    let migrations = packaged_migrations()?;
    let latest = latest_version(&migrations);
    let Some(path) = path else {
        return Ok(status(
            SchemaState::Unconfigured,
            None,
            latest,
            Vec::new(),
            "database is not configured",
        ));
    };
    if !path.exists() {
        return Ok(classify(&Snapshot::empty(), &migrations));
    }
    let connection = open_standalone(path, true, MIGRATION_BUSY_TIMEOUT)
        .map_err(|_| MigrationError(CONNECTION_FAILED.to_owned()))?;
    let observed =
        snapshot(&connection).map_err(|_| MigrationError(INSPECTION_FAILED.to_owned()))?;
    Ok(classify(&observed, &migrations))
}

/// Apply the pending packaged migrations to the database at `path`, creating the file when it does
/// not exist. The parent directory must exist.
///
/// Returns the versions applied by this call, in order.
///
/// # Errors
///
/// A fail-closed [`MigrationError`] (see the module documentation), or the sanitized failure of a
/// migration, which leaves earlier migrations committed and the failing one rolled back.
pub fn migrate(path: &Path) -> Result<Vec<u32>, MigrationError> {
    let mut connection = open_standalone(path, false, MIGRATION_BUSY_TIMEOUT)
        .map_err(|_| MigrationError(CONNECTION_FAILED.to_owned()))?;
    migrate_connection(&mut connection)
}

/// [`migrate`] on an open read-write connection (the tests use it on in-memory databases).
///
/// # Errors
///
/// As [`migrate`].
pub fn migrate_connection(connection: &mut Connection) -> Result<Vec<u32>, MigrationError> {
    let migrations = packaged_migrations()?;
    match apply_steps(connection, &migrations) {
        Ok(applied) => Ok(applied),
        Err(Step::Refused(error)) => Err(error),
        Err(Step::Database(error)) => Err(failure(&error)),
    }
}

/// What the database holds, read before any verification.
#[derive(Debug)]
enum Snapshot {
    /// No Jobniac history table; `foreign` is true when other tables exist or the application id
    /// is not ours.
    Absent { foreign: bool },
    /// The history rows in version order and the `user_version` marker.
    Present {
        rows: Vec<AppliedRow>,
        user_version: i64,
    },
}

impl Snapshot {
    fn empty() -> Self {
        Self::Absent { foreign: false }
    }
}

/// A failure inside the migration body: a refusal, or a database error to report.
#[derive(Debug)]
enum Step {
    Refused(MigrationError),
    Database(rusqlite::Error),
}

impl From<MigrationError> for Step {
    fn from(error: MigrationError) -> Self {
        Self::Refused(error)
    }
}

impl From<rusqlite::Error> for Step {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

fn apply_steps(connection: &mut Connection, migrations: &[Migration]) -> Result<Vec<u32>, Step> {
    bootstrap(connection)?;
    let mut applied_now = Vec::new();
    for migration in migrations {
        if let Some(elapsed_ms) = apply_one(connection, migrations, migration)? {
            tracing::info!(
                version = migration.version,
                execution_ms = elapsed_ms,
                "applied schema migration"
            );
            applied_now.push(migration.version);
        }
    }
    Ok(applied_now)
}

/// Create the history table when the database is empty, refusing a foreign database. One
/// transaction under the write lock, so two runners cannot both create it.
fn bootstrap(connection: &mut Connection) -> Result<(), Step> {
    connection.execute_batch("BEGIN IMMEDIATE")?;
    let outcome = bootstrap_locked(connection);
    match &outcome {
        Ok(()) => connection.execute_batch("COMMIT")?,
        Err(_) => {
            let _ = connection.execute_batch("ROLLBACK");
        }
    }
    outcome
}

fn bootstrap_locked(connection: &Connection) -> Result<(), Step> {
    if history_exists(connection)? {
        return Ok(());
    }
    if foreign_objects_exist(connection)? {
        return Err(Step::Refused(MigrationError(FOREIGN_MESSAGE.to_owned())));
    }
    check_application_id(connection)?;
    connection.execute_batch(CREATE_HISTORY_TABLE_SQL)?;
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    Ok(())
}

/// Apply one migration and record it, in one transaction. Returns the execution time, or `None`
/// when another runner already applied it. The history is re-verified under the write lock.
fn apply_one(
    connection: &mut Connection,
    migrations: &[Migration],
    migration: &Migration,
) -> Result<Option<i64>, Step> {
    connection.execute_batch("BEGIN IMMEDIATE")?;
    let outcome = apply_locked(connection, migrations, migration);
    match &outcome {
        Ok(_) => connection.execute_batch("COMMIT")?,
        Err(_) => {
            let _ = connection.execute_batch("ROLLBACK");
        }
    }
    outcome
}

fn apply_locked(
    connection: &Connection,
    migrations: &[Migration],
    migration: &Migration,
) -> Result<Option<i64>, Step> {
    let snapshot = snapshot(connection)?;
    let Snapshot::Present { rows, user_version } = snapshot else {
        return Err(Step::Refused(MigrationError(FOREIGN_MESSAGE.to_owned())));
    };
    verify(&rows, migrations)?;
    verify_marker(&rows, user_version)?;
    if rows
        .iter()
        .any(|row| row.version == i64::from(migration.version))
    {
        return Ok(None);
    }
    let started = Instant::now();
    connection.execute_batch(&migration.sql)?;
    let elapsed_ms = elapsed_millis(started.elapsed());
    connection.execute(
        INSERT_HISTORY_SQL,
        rusqlite::params![
            i64::from(migration.version),
            migration.description,
            migration.script,
            migration.checksum(),
            now_rfc3339(),
            elapsed_ms,
        ],
    )?;
    connection.pragma_update(None, "user_version", i64::from(migration.version))?;
    Ok(Some(elapsed_ms))
}

fn snapshot(connection: &Connection) -> Result<Snapshot, rusqlite::Error> {
    if !history_exists(connection)? {
        let foreign = foreign_objects_exist(connection)? || !application_id_is_ours(connection)?;
        return Ok(Snapshot::Absent { foreign });
    }
    Ok(Snapshot::Present {
        rows: applied_rows(connection)?,
        user_version: connection.pragma_query_value(None, "user_version", |row| row.get(0))?,
    })
}

fn history_exists(connection: &Connection) -> Result<bool, rusqlite::Error> {
    connection.query_row(HISTORY_EXISTS_SQL, [], |row| row.get(0))
}

fn foreign_objects_exist(connection: &Connection) -> Result<bool, rusqlite::Error> {
    connection.query_row(FOREIGN_OBJECTS_SQL, [], |row| row.get(0))
}

/// An application id of zero (never set) or ours.
fn application_id_is_ours(connection: &Connection) -> Result<bool, rusqlite::Error> {
    let id: i64 = connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    Ok(id == 0 || id == APPLICATION_ID)
}

fn check_application_id(connection: &Connection) -> Result<(), Step> {
    if application_id_is_ours(connection)? {
        Ok(())
    } else {
        Err(Step::Refused(MigrationError(
            FOREIGN_APPLICATION_MESSAGE.to_owned(),
        )))
    }
}

fn applied_rows(connection: &Connection) -> Result<Vec<AppliedRow>, rusqlite::Error> {
    let mut statement = connection.prepare(APPLIED_SQL)?;
    let rows = statement.query_map([], |row| {
        Ok(AppliedRow {
            version: row.get(0)?,
            checksum: row.get(1)?,
        })
    })?;
    rows.collect()
}

/// Check applied rows against the packaged migrations, in version order.
///
/// # Errors
///
/// A refusal for an applied version unknown to this build (a newer database), a checksum that
/// differs from the packaged file, or a history with gaps.
pub fn verify(applied: &[AppliedRow], migrations: &[Migration]) -> Result<(), MigrationError> {
    for row in applied {
        let packaged = migrations
            .iter()
            .find(|migration| i64::from(migration.version) == row.version)
            .ok_or_else(|| {
                MigrationError(format!(
                    "database has migration V{:04} unknown to this Jobniac build",
                    row.version
                ))
            })?;
        if packaged.checksum() != row.checksum {
            return Err(MigrationError(format!(
                "checksum mismatch for applied migration V{:04}",
                row.version
            )));
        }
    }
    let contiguous = applied
        .iter()
        .zip(1_i64..)
        .all(|(row, expected)| row.version == expected);
    if contiguous {
        Ok(())
    } else {
        Err(MigrationError(NOT_CONTIGUOUS_MESSAGE.to_owned()))
    }
}

/// `PRAGMA user_version` mirrors the highest applied version.
fn verify_marker(applied: &[AppliedRow], user_version: i64) -> Result<(), MigrationError> {
    let latest = applied.last().map_or(0, |row| row.version);
    if user_version == latest {
        Ok(())
    } else {
        Err(MigrationError(VERSION_MARKER_MESSAGE.to_owned()))
    }
}

/// Classify a snapshot. An unknown applied version is `Ahead`; any other violation is `Foreign`.
fn classify(snapshot: &Snapshot, migrations: &[Migration]) -> SchemaStatus {
    let latest = latest_version(migrations);
    match snapshot {
        Snapshot::Absent { foreign: true } => status(
            SchemaState::Foreign,
            None,
            latest,
            Vec::new(),
            "database holds tables not managed by Jobniac",
        ),
        Snapshot::Absent { foreign: false } => status(
            SchemaState::Empty,
            None,
            latest,
            migrations
                .iter()
                .map(|migration| migration.version)
                .collect(),
            "database is not initialized",
        ),
        Snapshot::Present { rows, user_version } => {
            let current = rows.last().and_then(|row| u32::try_from(row.version).ok());
            let checked =
                verify(rows, migrations).and_then(|()| verify_marker(rows, *user_version));
            if let Err(error) = checked {
                let state = if error.0.contains("unknown") {
                    SchemaState::Ahead
                } else {
                    SchemaState::Foreign
                };
                return status(state, current, latest, Vec::new(), error.0);
            }
            let pending: Vec<u32> = migrations
                .iter()
                .filter(|migration| {
                    !rows
                        .iter()
                        .any(|row| row.version == i64::from(migration.version))
                })
                .map(|migration| migration.version)
                .collect();
            if pending.is_empty() {
                status(
                    SchemaState::Current,
                    current,
                    latest,
                    Vec::new(),
                    "schema is current",
                )
            } else {
                status(
                    SchemaState::Pending,
                    current,
                    latest,
                    pending,
                    "migrations are pending",
                )
            }
        }
    }
}

fn status(
    state: SchemaState,
    current_version: Option<u32>,
    latest_version: u32,
    pending: Vec<u32>,
    detail: impl Into<String>,
) -> SchemaStatus {
    SchemaStatus {
        state,
        current_version,
        latest_version,
        pending,
        detail: detail.into(),
    }
}

fn latest_version(migrations: &[Migration]) -> u32 {
    migrations.last().map_or(0, |migration| migration.version)
}

/// The sanitized failure for a database error. The result code is kept; the message text is not.
fn failure(error: &rusqlite::Error) -> MigrationError {
    let label = match error {
        rusqlite::Error::SqliteFailure(failure, _) => {
            format!("SQLite code {}", failure.extended_code & 0xff)
        }
        _ => "OperationalError".to_owned(),
    };
    MigrationError(format!(
        "migration failed ({label}); no partial migration was committed"
    ))
}

/// Milliseconds rounded to the nearest whole number, for the timing.
fn elapsed_millis(elapsed: Duration) -> i64 {
    let millis = (elapsed.as_micros() + 500) / 1_000;
    i64::try_from(millis).unwrap_or(i64::MAX)
}

/// The current time as RFC 3339 UTC (the history is audit data, not compared as time).
fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// Parse `V<4 digits>__<[a-z0-9_]+>.sql`. Returns the version and the description.
fn parse_file_name(name: &str) -> Option<(u32, &str)> {
    let rest = name.strip_prefix('V')?;
    let digits = rest.get(..4)?;
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let version = digits.parse().ok()?;
    let description = rest.get(4..)?.strip_prefix("__")?.strip_suffix(".sql")?;
    let valid_description = !description.is_empty()
        && description
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    valid_description.then_some((version, description))
}

/// The highest applied version in the history of an open database, or `None` without history.
/// Used by the tests and by the startup diagnostics.
///
/// # Errors
///
/// The driver error of the read.
pub fn applied_version(connection: &Connection) -> Result<Option<i64>, rusqlite::Error> {
    if !history_exists(connection)? {
        return Ok(None);
    }
    connection.query_row("SELECT max(version) FROM schema_migrations", [], |row| {
        row.get::<_, Option<i64>>(0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected checksums of the packaged files.
    const PACKAGED_CHECKSUMS: [(u32, &str); 1] = [(1, "baseline")];

    fn migration(sql: &str) -> Migration {
        Migration {
            version: 1,
            description: "synthetic".to_owned(),
            script: "V0001__synthetic.sql".to_owned(),
            sql: sql.to_owned(),
        }
    }

    fn good_rows(migrations: &[Migration]) -> Vec<AppliedRow> {
        migrations
            .iter()
            .map(|m| AppliedRow {
                version: i64::from(m.version),
                checksum: m.checksum(),
            })
            .collect()
    }

    fn temp_database() -> Result<(tempfile::TempDir, std::path::PathBuf), std::io::Error> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("migrate.sqlite3");
        Ok((directory, path))
    }

    #[test]
    fn packaged_migrations_are_contiguous_and_named() -> Result<(), MigrationError> {
        let migrations = packaged_migrations()?;
        assert_eq!(migrations.len(), PACKAGED_CHECKSUMS.len());
        for (migration, (version, description)) in migrations.iter().zip(PACKAGED_CHECKSUMS) {
            assert_eq!(migration.version, version);
            assert_eq!(migration.description, description);
            assert_eq!(migration.checksum().len(), 64);
            assert!(!migration.sql.contains('\r'));
        }
        Ok(())
    }

    #[test]
    fn checksum_ignores_crlf_line_endings() {
        let lf = migration("CREATE TABLE a (\n id integer\n);\n");
        let crlf = migration("CREATE TABLE a (\r\n id integer\r\n);\r\n");
        assert_eq!(lf.checksum(), crlf.checksum());
        assert_eq!(lf.checksum().len(), 64);
    }

    #[test]
    fn file_names_follow_the_version_pattern() {
        assert_eq!(
            parse_file_name("V0001__baseline.sql"),
            Some((1, "baseline"))
        );
        assert_eq!(parse_file_name("V0042__a.sql"), Some((42, "a")));
        for name in [
            "V001__a.sql",
            "V0001_a.sql",
            "V0001__.sql",
            "V0001__A.sql",
            "V0001__a-b.sql",
            "V0001__a.sql.bak",
            "v0001__a.sql",
            "V000x__a.sql",
            "V0001__a.txt",
        ] {
            assert_eq!(parse_file_name(name), None, "{name} should be rejected");
        }
    }

    #[test]
    fn applied_history_is_verified_before_any_write() -> Result<(), MigrationError> {
        let migrations = packaged_migrations()?;
        let good = good_rows(&migrations);
        assert_eq!(verify(&good, &migrations), Ok(()));

        let mut tampered = good.clone();
        tampered[0].checksum = "0".repeat(64);
        assert_eq!(
            verify(&tampered, &migrations),
            Err(MigrationError(
                "checksum mismatch for applied migration V0001".to_owned()
            ))
        );

        let mut ahead = good.clone();
        ahead.push(AppliedRow {
            version: 7,
            checksum: "a".repeat(64),
        });
        assert_eq!(
            verify(&ahead, &migrations),
            Err(MigrationError(
                "database has migration V0007 unknown to this Jobniac build".to_owned()
            ))
        );

        let gap = vec![
            good[0].clone(),
            AppliedRow {
                version: 3,
                checksum: "a".repeat(64),
            },
        ];
        // V0003 is unknown to this build, which is reported before the gap.
        assert!(verify(&gap, &migrations).is_err());
        Ok(())
    }

    #[test]
    fn a_history_with_a_gap_is_not_contiguous() {
        let migrations = [
            Migration {
                version: 1,
                description: "a".to_owned(),
                script: "V0001__a.sql".to_owned(),
                sql: "SELECT 1".to_owned(),
            },
            Migration {
                version: 2,
                description: "b".to_owned(),
                script: "V0002__b.sql".to_owned(),
                sql: "SELECT 2".to_owned(),
            },
            Migration {
                version: 3,
                description: "c".to_owned(),
                script: "V0003__c.sql".to_owned(),
                sql: "SELECT 3".to_owned(),
            },
        ];
        let rows = good_rows(&migrations);
        let gap = vec![rows[0].clone(), rows[2].clone()];
        assert_eq!(
            verify(&gap, &migrations),
            Err(MigrationError(NOT_CONTIGUOUS_MESSAGE.to_owned()))
        );
    }

    #[test]
    fn snapshots_classify_to_the_schema_states() -> Result<(), MigrationError> {
        let migrations = packaged_migrations()?;
        let rows = good_rows(&migrations);

        let empty = classify(&Snapshot::empty(), &migrations);
        assert_eq!(empty.state, SchemaState::Empty);
        assert_eq!(empty.pending, vec![1]);
        assert_eq!(empty.current_version, None);

        let foreign = classify(&Snapshot::Absent { foreign: true }, &migrations);
        assert_eq!(foreign.state, SchemaState::Foreign);
        assert!(foreign.pending.is_empty());

        let current = classify(
            &Snapshot::Present {
                rows: rows.clone(),
                user_version: 1,
            },
            &migrations,
        );
        assert_eq!(current.state, SchemaState::Current);
        assert_eq!(current.current_version, Some(1));
        assert_eq!(current.latest_version, 1);

        let mismatch = classify(
            &Snapshot::Present {
                rows: rows.clone(),
                user_version: 0,
            },
            &migrations,
        );
        assert_eq!(mismatch.state, SchemaState::Foreign);

        let mut future = rows;
        future.push(AppliedRow {
            version: 2,
            checksum: "b".repeat(64),
        });
        let ahead = classify(
            &Snapshot::Present {
                rows: future,
                user_version: 2,
            },
            &migrations,
        );
        assert_eq!(ahead.state, SchemaState::Ahead);
        assert_eq!(ahead.current_version, Some(2));
        assert!(ahead.pending.is_empty());
        Ok(())
    }

    #[test]
    fn inspection_without_configuration_is_unconfigured() -> Result<(), MigrationError> {
        let status = inspect_schema(None)?;
        assert_eq!(status.state, SchemaState::Unconfigured);
        assert_eq!(status.latest_version, 1);
        assert_eq!(status.detail, "database is not configured");
        Ok(())
    }

    #[test]
    fn a_missing_file_is_empty_and_inspection_does_not_create_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_directory, path) = temp_database()?;
        let status = inspect_schema(Some(&path))?;
        assert_eq!(status.state, SchemaState::Empty);
        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn a_fresh_database_is_migrated_and_the_rerun_is_idempotent()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_directory, path) = temp_database()?;
        assert_eq!(migrate(&path)?, vec![1]);

        let connection = Connection::open(&path)?;
        let user_version: i64 =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let application_id: i64 =
            connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
        assert_eq!(user_version, 1, "user_version mirrors the history");
        assert_eq!(application_id, APPLICATION_ID);
        let (checksum, script): (String, String) = connection.query_row(
            "SELECT checksum, script FROM schema_migrations WHERE version = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(checksum, packaged_migrations()?[0].checksum());
        assert_eq!(script, "V0001__baseline.sql");
        assert_eq!(applied_version(&connection)?, Some(1));
        drop(connection);

        assert_eq!(
            migrate(&path)?,
            Vec::<u32>::new(),
            "nothing is applied twice"
        );
        let status = inspect_schema(Some(&path))?;
        assert_eq!(status.state, SchemaState::Current);
        assert_eq!(status.current_version, Some(1));
        Ok(())
    }

    #[test]
    fn a_newer_database_is_refused_without_a_write() -> Result<(), Box<dyn std::error::Error>> {
        let (_directory, path) = temp_database()?;
        migrate(&path)?;
        {
            let connection = Connection::open(&path)?;
            connection.execute(
                "INSERT INTO schema_migrations \
                 (version, description, script, checksum, applied_at, execution_ms) \
                 VALUES (2, 'future', 'V0002__future.sql', ?1, 'x', 0)",
                [&"c".repeat(64)],
            )?;
            connection.pragma_update(None, "user_version", 2)?;
        }
        let refused = migrate(&path);
        assert_eq!(
            refused,
            Err(MigrationError(
                "database has migration V0002 unknown to this Jobniac build".to_owned()
            ))
        );
        let status = inspect_schema(Some(&path))?;
        assert_eq!(status.state, SchemaState::Ahead);
        assert_eq!(status.current_version, Some(2));
        // The refusal wrote nothing: V0001 is still the only baseline and the marker is as left.
        let connection = Connection::open(&path)?;
        let count: i64 =
            connection.query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))?;
        assert_eq!(count, 2);
        Ok(())
    }

    #[test]
    fn a_tampered_checksum_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let (_directory, path) = temp_database()?;
        migrate(&path)?;
        {
            let connection = Connection::open(&path)?;
            connection.execute(
                "UPDATE schema_migrations SET checksum = ?1 WHERE version = 1",
                [&"0".repeat(64)],
            )?;
        }
        assert_eq!(
            migrate(&path),
            Err(MigrationError(
                "checksum mismatch for applied migration V0001".to_owned()
            ))
        );
        assert_eq!(inspect_schema(Some(&path))?.state, SchemaState::Foreign);
        Ok(())
    }

    #[test]
    fn a_missing_history_row_leaves_a_gap_that_is_refused() -> Result<(), Box<dyn std::error::Error>>
    {
        let (_directory, path) = temp_database()?;
        migrate(&path)?;
        {
            let connection = Connection::open(&path)?;
            connection.execute("DELETE FROM schema_migrations", [])?;
        }
        // The marker no longer matches the (now empty) history.
        assert_eq!(
            migrate(&path),
            Err(MigrationError(VERSION_MARKER_MESSAGE.to_owned()))
        );
        Ok(())
    }

    #[test]
    fn a_database_with_foreign_tables_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let (_directory, path) = temp_database()?;
        {
            let connection = Connection::open(&path)?;
            connection.execute_batch("CREATE TABLE other_app (x INTEGER)")?;
        }
        assert_eq!(
            migrate(&path),
            Err(MigrationError(FOREIGN_MESSAGE.to_owned()))
        );
        assert_eq!(inspect_schema(Some(&path))?.state, SchemaState::Foreign);
        let connection = Connection::open(&path)?;
        assert_eq!(
            applied_version(&connection)?,
            None,
            "no history was created"
        );
        Ok(())
    }

    #[test]
    fn a_database_of_another_application_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let (_directory, path) = temp_database()?;
        {
            let connection = Connection::open(&path)?;
            connection.pragma_update(None, "application_id", 0x1234_5678_i64)?;
        }
        assert_eq!(
            migrate(&path),
            Err(MigrationError(FOREIGN_APPLICATION_MESSAGE.to_owned()))
        );
        Ok(())
    }

    #[test]
    fn a_failing_migration_rolls_back_with_its_history_row()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut connection = Connection::open_in_memory()?;
        let broken = [Migration {
            version: 1,
            description: "broken".to_owned(),
            script: "V0001__broken.sql".to_owned(),
            sql: "CREATE TABLE half (x INTEGER); INSERT INTO missing VALUES (1);".to_owned(),
        }];
        let outcome = apply_steps(&mut connection, &broken);
        assert!(matches!(outcome, Err(Step::Database(_))));
        let half: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'half'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(half, 0, "DDL is transactional: no partial migration");
        assert_eq!(applied_version(&connection)?, None);
        let rows: i64 =
            connection.query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))?;
        assert_eq!(rows, 0);
        assert_eq!(
            failure(&rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
                Some("no such table: missing".to_owned()),
            )),
            MigrationError(
                "migration failed (SQLite code 1); no partial migration was committed".to_owned()
            )
        );
        Ok(())
    }
}
