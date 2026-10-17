//! Explicit, append-only, checksummed schema migrations for the Jobniac PostgreSQL database.
//!
//! The packaged `V<NNNN>__<description>.sql` files are applied in version order, each in one
//! transaction together with its history row, under a session advisory lock on a dedicated
//! connection. Nothing runs implicitly: [`inspect_schema`] only reads, and [`migrate`] is the sole
//! writer.
//!
//! Fail-closed rules, all checked before any write: an applied migration whose packaged checksum
//! changed, an applied version unknown to this build, a non-contiguous history, and a database
//! that holds Flyway job-engine schemas or Jobniac-managed schemas without Jobniac history.
//!
//! Failure reporting:
//! - a database error during [`migrate`] is reported with its SQLSTATE, as in
//!   `migration failed (SQLSTATE 42P01); no partial migration was committed`; failures that are not
//!   database errors are labelled `OperationalError`;
//! - a database error while [`inspect_schema`] reads the history is reported as
//!   `database inspection failed` instead of escaping as a raw driver error.

use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio_postgres::{Client, NoTls};

use crate::postgres::config::DatabaseConfig;
use crate::postgres::session;
use crate::postgres::sqlstate;

const HISTORY_EXISTS_SQL: &str = "SELECT to_regclass('jobniac.schema_migrations') IS NOT NULL";
const FLYWAY_HISTORY_SQL: &str = "SELECT to_regclass('profile.flyway_schema_history') IS NOT NULL";
const MANAGED_SCHEMAS_SQL: &str =
    "SELECT nspname FROM pg_catalog.pg_namespace WHERE nspname = ANY($1) ORDER BY nspname";
const APPLIED_SQL: &str = "SELECT version, description, script, checksum \
    FROM jobniac.schema_migrations ORDER BY version";
const INSERT_HISTORY_SQL: &str = "INSERT INTO jobniac.schema_migrations \
    (version, description, script, checksum, execution_ms) VALUES ($1, $2, $3, $4, $5)";
const CREATE_HISTORY_SCHEMA_SQL: &str = "CREATE SCHEMA jobniac";
const CREATE_HISTORY_TABLE_SQL: &str = concat!(
    "CREATE TABLE jobniac.schema_migrations (",
    " version integer PRIMARY KEY CHECK (version > 0),",
    " description text NOT NULL,",
    " script text NOT NULL,",
    " checksum char(64) NOT NULL,",
    " applied_at timestamptz NOT NULL DEFAULT clock_timestamp(),",
    " execution_ms integer NOT NULL CHECK (execution_ms >= 0))",
);
const LOCK_SQL: &str = "SELECT pg_advisory_lock($1)";
const UNLOCK_SQL: &str = "SELECT pg_advisory_unlock($1)";

/// Schemas owned by Jobniac. Their presence without history means the database is not ours.
const MANAGED_SCHEMAS: [&str; 6] = [
    "cover_letter",
    "document",
    "job_schema",
    "match",
    "profile",
    "resume",
];

/// Advisory lock key of the migration runner (ASCII `JOBNIA`).
const ADVISORY_LOCK_KEY: i64 = 0x4A_4F_42_4E_49_41;

const MIGRATION_OPTIONS: &str =
    "-c search_path=pg_catalog -c statement_timeout=300000 -c lock_timeout=30000";
/// Inspection uses the migration session settings plus a read-only default.
const INSPECTION_OPTIONS: &str = "-c search_path=pg_catalog -c statement_timeout=300000 \
    -c lock_timeout=30000 -c default_transaction_read_only=on";
const MIGRATION_APPLICATION_NAME: &str = "jobniac-migrate";

const CONNECTION_FAILED: &str = "database connection failed";
const INSPECTION_FAILED: &str = "database inspection failed";
const CONTIGUITY_MESSAGE: &str = "packaged migrations must be numbered contiguously from V0001";
const NOT_CONTIGUOUS_MESSAGE: &str = "applied migration history is not contiguous";

/// The packaged migration files, embedded byte for byte. Line endings are normalized on load.
const PACKAGED_FILES: [(&str, &str); 6] = [
    (
        "V0001__baseline_from_spring_v29.sql",
        include_str!("../../sql/V0001__baseline_from_spring_v29.sql"),
    ),
    (
        "V0002__cutover_ledger.sql",
        include_str!("../../sql/V0002__cutover_ledger.sql"),
    ),
    (
        "V0003__resume_section_order_unique.sql",
        include_str!("../../sql/V0003__resume_section_order_unique.sql"),
    ),
    (
        "V0004__ascii_search_postings.sql",
        include_str!("../../sql/V0004__ascii_search_postings.sql"),
    ),
    (
        "V0005__owned_generated_asset_cleanup.sql",
        include_str!("../../sql/V0005__owned_generated_asset_cleanup.sql"),
    ),
    (
        "V0006__generated_asset_intents.sql",
        include_str!("../../sql/V0006__generated_asset_intents.sql"),
    ),
];

/// Schema state reported by [`inspect_schema`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaState {
    /// No database is configured.
    Unconfigured,
    /// The database holds no Jobniac history yet; `migrate` creates it.
    Empty,
    /// Some packaged migrations are not applied yet.
    Pending,
    /// Every packaged migration is applied and verified.
    Current,
    /// The history holds a version newer than this build knows.
    Ahead,
    /// The database holds schemas or history that this build does not manage.
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
    /// The file name, for example `V0001__baseline_from_spring_v29.sql`.
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

/// A sanitized, fail-closed migration refusal. It never contains SQL text, DSNs or row values.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct MigrationError(pub String);

/// Load and validate the packaged migrations, contiguous from `V0001`.
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
/// `None` yields [`SchemaState::Unconfigured`] without a connection.
pub async fn inspect_schema(
    config: Option<&DatabaseConfig>,
) -> Result<SchemaStatus, MigrationError> {
    let migrations = packaged_migrations()?;
    let Some(config) = config else {
        return Ok(status(
            SchemaState::Unconfigured,
            None,
            latest_version(&migrations),
            Vec::new(),
            "database is not configured",
        ));
    };
    let client = connect(config, INSPECTION_OPTIONS).await?;
    let observed = snapshot(&client)
        .await
        .map_err(|_| MigrationError(INSPECTION_FAILED.to_owned()))?;
    Ok(classify(&observed, &migrations))
}

/// Apply the pending packaged migrations under a session advisory lock.
///
/// Returns the versions applied by this call, in order. Each migration and its history row run in
/// one transaction; a failure rolls that migration back and leaves earlier ones committed.
pub async fn migrate(config: &DatabaseConfig) -> Result<Vec<u32>, MigrationError> {
    let migrations = packaged_migrations()?;
    let mut client = connect(config, MIGRATION_OPTIONS).await?;
    client
        .execute(LOCK_SQL, &[&ADVISORY_LOCK_KEY])
        .await
        .map_err(|error| failure(&error))?;
    let outcome = apply_pending(&mut client, &migrations).await;
    // Release the lock explicitly. The session lock also ends with the connection.
    let _ = client.execute(UNLOCK_SQL, &[&ADVISORY_LOCK_KEY]).await;
    outcome
}

/// Open a dedicated session. Connection errors never expose libpq text.
async fn connect(config: &DatabaseConfig, options: &str) -> Result<Client, MigrationError> {
    // A libpq service file can redirect the session, so the guard of the repositories
    // applies here too, before any connection attempt.
    if config.libpq_service_set() {
        return Err(MigrationError(CONNECTION_FAILED.to_owned()));
    }
    let pg = config.connect_config(
        session::CONNECT_TIMEOUT,
        options,
        Some(MIGRATION_APPLICATION_NAME),
    );
    // tokio-postgres applies its connect timeout to the TCP socket only. The whole connection
    // (startup and authentication included) is bounded here, as libpq's connect_timeout is.
    match tokio::time::timeout(session::CONNECT_TIMEOUT, pg.connect(NoTls)).await {
        Ok(Ok((client, connection))) => {
            // The connection task drives the socket. Its result is discarded so no error text is kept.
            tokio::spawn(async move {
                let _ = connection.await;
            });
            Ok(client)
        }
        Ok(Err(_)) | Err(_) => Err(MigrationError(CONNECTION_FAILED.to_owned())),
    }
}

/// What the database holds, read before any verification.
#[derive(Debug)]
enum Snapshot {
    /// No Jobniac history table; `foreign` is true when Flyway or managed schemas exist.
    Absent { foreign: bool },
    /// The history rows in version order.
    Present(Vec<AppliedRow>),
}

/// One history row. Only what verification needs is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AppliedRow {
    version: i64,
    checksum: String,
}

/// A failure inside the migration body: a refusal, or a database error to classify.
#[derive(Debug)]
enum Step {
    Refused(MigrationError),
    Database(tokio_postgres::Error),
}

impl From<MigrationError> for Step {
    fn from(error: MigrationError) -> Self {
        Self::Refused(error)
    }
}

impl From<tokio_postgres::Error> for Step {
    fn from(error: tokio_postgres::Error) -> Self {
        Self::Database(error)
    }
}

async fn apply_pending(
    client: &mut Client,
    migrations: &[Migration],
) -> Result<Vec<u32>, MigrationError> {
    match apply_steps(client, migrations).await {
        Ok(applied) => Ok(applied),
        Err(Step::Refused(error)) => Err(error),
        Err(Step::Database(error)) => Err(failure(&error)),
    }
}

async fn apply_steps(client: &mut Client, migrations: &[Migration]) -> Result<Vec<u32>, Step> {
    if !history_exists(client).await? {
        if flyway_history_present(client).await? {
            return Err(Step::Refused(MigrationError(
                "database contains a legacy job-engine-spring schema; \
                 use the importer rehearsal against a fresh Jobniac database"
                    .to_owned(),
            )));
        }
        if managed_schemas_present(client).await? {
            return Err(Step::Refused(MigrationError(
                "database contains Jobniac-managed schema names without Jobniac history".to_owned(),
            )));
        }
        create_history(client).await?;
    }
    let applied = applied_rows(client).await?;
    verify(&applied, migrations)?;
    let mut applied_now = Vec::new();
    for migration in migrations {
        let done = applied
            .iter()
            .any(|row| row.version == i64::from(migration.version));
        if done {
            continue;
        }
        let elapsed_ms = apply_one(client, migration).await?;
        tracing::info!(
            version = migration.version,
            execution_ms = elapsed_ms,
            "applied schema migration"
        );
        applied_now.push(migration.version);
    }
    Ok(applied_now)
}

/// Create the history schema and table in one transaction.
async fn create_history(client: &mut Client) -> Result<(), tokio_postgres::Error> {
    let transaction = client.transaction().await?;
    transaction.batch_execute(CREATE_HISTORY_SCHEMA_SQL).await?;
    transaction.batch_execute(CREATE_HISTORY_TABLE_SQL).await?;
    transaction.commit().await
}

/// Apply one migration and record it in the same transaction. Returns the execution time.
async fn apply_one(
    client: &mut Client,
    migration: &Migration,
) -> Result<i32, tokio_postgres::Error> {
    let checksum = migration.checksum();
    let version = i32::try_from(migration.version).unwrap_or(i32::MAX);
    let transaction = client.transaction().await?;
    let started = Instant::now();
    transaction.batch_execute(&migration.sql).await?;
    let elapsed_ms = elapsed_millis(started.elapsed());
    transaction
        .execute(
            INSERT_HISTORY_SQL,
            &[
                &version,
                &migration.description,
                &migration.script,
                &checksum,
                &elapsed_ms,
            ],
        )
        .await?;
    transaction.commit().await?;
    Ok(elapsed_ms)
}

async fn snapshot(client: &Client) -> Result<Snapshot, tokio_postgres::Error> {
    if !history_exists(client).await? {
        let foreign =
            flyway_history_present(client).await? || managed_schemas_present(client).await?;
        return Ok(Snapshot::Absent { foreign });
    }
    Ok(Snapshot::Present(applied_rows(client).await?))
}

async fn history_exists(client: &Client) -> Result<bool, tokio_postgres::Error> {
    let row = client.query_one(HISTORY_EXISTS_SQL, &[]).await?;
    row.try_get(0)
}

async fn flyway_history_present(client: &Client) -> Result<bool, tokio_postgres::Error> {
    let row = client.query_one(FLYWAY_HISTORY_SQL, &[]).await?;
    row.try_get(0)
}

async fn managed_schemas_present(client: &Client) -> Result<bool, tokio_postgres::Error> {
    let names: Vec<&str> = MANAGED_SCHEMAS.to_vec();
    let rows = client.query(MANAGED_SCHEMAS_SQL, &[&names]).await?;
    Ok(!rows.is_empty())
}

async fn applied_rows(client: &Client) -> Result<Vec<AppliedRow>, tokio_postgres::Error> {
    let rows = client.query(APPLIED_SQL, &[]).await?;
    let mut applied = Vec::with_capacity(rows.len());
    for row in &rows {
        applied.push(AppliedRow {
            version: i64::from(row.try_get::<_, i32>(0)?),
            checksum: row.try_get(3)?,
        });
    }
    Ok(applied)
}

/// Check applied rows against the packaged migrations, in version order.
fn verify(applied: &[AppliedRow], migrations: &[Migration]) -> Result<(), MigrationError> {
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

/// Classify a snapshot. An unknown applied version is `Ahead`; any other violation is `Foreign`.
fn classify(snapshot: &Snapshot, migrations: &[Migration]) -> SchemaStatus {
    let latest = latest_version(migrations);
    match snapshot {
        Snapshot::Absent { foreign: true } => status(
            SchemaState::Foreign,
            None,
            latest,
            Vec::new(),
            "database holds schemas not managed by Jobniac",
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
        Snapshot::Present(applied) => {
            let current = applied
                .last()
                .and_then(|row| u32::try_from(row.version).ok());
            if let Err(error) = verify(applied, migrations) {
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
                    !applied
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

/// The sanitized failure for a database error. The SQLSTATE is kept; the message text is not.
fn failure(error: &tokio_postgres::Error) -> MigrationError {
    let label = match sqlstate::code(error) {
        Some(code) => format!("SQLSTATE {code}"),
        None => "OperationalError".to_owned(),
    };
    MigrationError(format!(
        "migration failed ({label}); no partial migration was committed"
    ))
}

/// Milliseconds rounded to the nearest whole number, for the timing.
fn elapsed_millis(elapsed: Duration) -> i32 {
    let millis = (elapsed.as_micros() + 500) / 1_000;
    i32::try_from(millis).unwrap_or(i32::MAX)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected checksums of the packaged files.
    const PACKAGED_CHECKSUMS: [(u32, &str, &str); 6] = [
        (
            1,
            "baseline_from_spring_v29",
            "77ed4396ae96b03172b66c3dbd1352e1d53f9ce65f98df1b25adf65d90e4b844",
        ),
        (
            2,
            "cutover_ledger",
            "3e24e96db674782a83c61734eacca6829880cde1eec20e78bab1d0493a6af909",
        ),
        (
            3,
            "resume_section_order_unique",
            "972aae00257820da4a206321787e4ef8deb948a99254365bfc2b22482530ffee",
        ),
        (
            4,
            "ascii_search_postings",
            "91f5d791e6d281d6084885b118cccb8c735626be1445c2f1fc068ab473cc5edf",
        ),
        (
            5,
            "owned_generated_asset_cleanup",
            "afebabf2edbc0a88a78d52b4f75e6df3ee21d7e4070e10e60933b45865906b81",
        ),
        (
            6,
            "generated_asset_intents",
            "0d6432012faf1a9fa7f637b04811b85de2449907ed2b349481480377a8a1d13b",
        ),
    ];

    fn migration(sql: &str) -> Migration {
        Migration {
            version: 1,
            description: "synthetic".to_owned(),
            script: "V0001__synthetic.sql".to_owned(),
            sql: sql.to_owned(),
        }
    }

    fn fixture_migrations() -> Result<Vec<Migration>, MigrationError> {
        packaged_migrations()
    }

    #[test]
    fn packaged_migrations_match_the_ledger_checksums() -> Result<(), MigrationError> {
        let migrations = packaged_migrations()?;
        assert_eq!(migrations.len(), PACKAGED_CHECKSUMS.len());
        for (migration, (version, description, checksum)) in
            migrations.iter().zip(PACKAGED_CHECKSUMS)
        {
            assert_eq!(migration.version, version);
            assert_eq!(migration.description, description);
            assert_eq!(migration.checksum(), checksum, "V{version:04}");
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
            parse_file_name("V0001__baseline_from_spring_v29.sql"),
            Some((1, "baseline_from_spring_v29"))
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
        let migrations = fixture_migrations()?;
        let good: Vec<AppliedRow> = migrations
            .iter()
            .map(|m| AppliedRow {
                version: i64::from(m.version),
                checksum: m.checksum(),
            })
            .collect();
        assert_eq!(verify(&good, &migrations), Ok(()));

        let mut tampered = good.clone();
        tampered[2].checksum = "0".repeat(64);
        assert_eq!(
            verify(&tampered, &migrations),
            Err(MigrationError(
                "checksum mismatch for applied migration V0003".to_owned()
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

        let gap = vec![good[0].clone(), good[2].clone()];
        assert_eq!(
            verify(&gap, &migrations),
            Err(MigrationError(NOT_CONTIGUOUS_MESSAGE.to_owned()))
        );
        Ok(())
    }

    #[test]
    fn snapshots_classify_to_the_schema_states() -> Result<(), MigrationError> {
        let migrations = fixture_migrations()?;
        let rows: Vec<AppliedRow> = migrations
            .iter()
            .map(|m| AppliedRow {
                version: i64::from(m.version),
                checksum: m.checksum(),
            })
            .collect();

        let empty = classify(&Snapshot::Absent { foreign: false }, &migrations);
        assert_eq!(empty.state, SchemaState::Empty);
        assert_eq!(empty.pending, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(empty.current_version, None);

        let foreign = classify(&Snapshot::Absent { foreign: true }, &migrations);
        assert_eq!(foreign.state, SchemaState::Foreign);
        assert!(foreign.pending.is_empty());

        let current = classify(&Snapshot::Present(rows.clone()), &migrations);
        assert_eq!(current.state, SchemaState::Current);
        assert_eq!(current.current_version, Some(6));
        assert_eq!(current.latest_version, 6);

        let partial = classify(&Snapshot::Present(rows[..4].to_vec()), &migrations);
        assert_eq!(partial.state, SchemaState::Pending);
        assert_eq!(partial.pending, vec![5, 6]);

        let mut future = rows;
        future.push(AppliedRow {
            version: 7,
            checksum: "b".repeat(64),
        });
        let ahead = classify(&Snapshot::Present(future), &migrations);
        assert_eq!(ahead.state, SchemaState::Ahead);
        assert_eq!(ahead.current_version, Some(7));
        assert!(ahead.pending.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn inspection_without_configuration_is_unconfigured() -> Result<(), MigrationError> {
        let status = inspect_schema(None).await?;
        assert_eq!(status.state, SchemaState::Unconfigured);
        assert_eq!(status.latest_version, 6);
        assert_eq!(status.detail, "database is not configured");
        Ok(())
    }

    /// With `PGSERVICE` set, `migrate` and `inspect_schema` refuse before connecting. A
    /// loopback listener records whether any connection arrived.
    #[tokio::test]
    async fn migrate_and_inspect_refuse_to_connect_while_libpq_service_is_set()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::postgres::config::load_database_config;
        use jobniac_core::env::Environment;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port().to_string();
        let config = load_database_config(&Environment::from_pairs([
            ("JOBNIAC_DB_HOST", "127.0.0.1"),
            ("JOBNIAC_DB_PORT", port.as_str()),
            ("JOBNIAC_DB_NAME", "jobniac_service"),
            ("JOBNIAC_DB_USER", "app_user"),
            ("JOBNIAC_DB_PASSWORD", "synthetic-password"),
            ("PGSERVICE", "synthetic-service"),
        ]))
        .ok_or("a configuration with PGSERVICE set must still validate")?;

        let refused = MigrationError(CONNECTION_FAILED.to_owned());
        assert_eq!(migrate(&config).await, Err(refused.clone()));
        assert_eq!(
            inspect_schema(Some(&config)).await.map(|_| ()),
            Err(refused)
        );
        match listener.accept() {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Ok(_) => Err("a connection was attempted while PGSERVICE was set".into()),
            Err(error) => Err(error.into()),
        }
    }
}
