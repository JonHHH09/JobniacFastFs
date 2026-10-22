//! The success paths of the generated-root commands through the real `jobniac-cli` binary, against a
//! temporary SQLite database: `setup-generated-root` (first run, then the confirming second
//! run), `cleanup` on an empty queue and with one queued asset, and the refusal of a schema that is
//! not current. The stdout bytes and exit statuses are the ones the binary prints. Every
//! process starts with a cleared environment that receives only the temporary database path and,
//! where the command needs them, the synthetic root values. All data is synthetic, and each root is
//! a temporary directory removed at the end.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use jobniac_core::cleanup::CleanupRepository;
use jobniac_core::clock::{Clock, SystemClock};
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::{AssetIdentity, PreparationOutcome, sha256_hex};
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::generated::root::ensure_private_directory;
use jobniac_infra::sqlite::cleanup::SqliteCleanupRepository;
use jobniac_infra::sqlite::codec::instant_text;
use jobniac_infra::sqlite::migrations::migrate;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use jobniac_testkit::scratch::scratch_dir;
use jobniac_testkit::sqlite::TempDatabase;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

/// The configured identity of every synthetic root in this file.
const ROOT_ID: &str = "6f1c2d3e-4a5b-4c6d-8e7f-900112233445";
/// The prefix of every `setup-generated-root` line.
const PREFIX: &str = "Jobniac setup-generated-root: ";
/// The compact metadata of an empty pass, in the metadata key order.
const EMPTY_METADATA: &str = concat!(
    r#"{"recoveredGenerations":0,"claimed":0,"completed":0,"retryScheduled":0,"#,
    r#""leaseLost":0,"blocked":0,"stateFailures":0,"prunedCompleted":0}"#,
);
/// The metadata of a pass that removed one queued asset.
const ONE_TASK_METADATA: &str = concat!(
    r#"{"recoveredGenerations":0,"claimed":1,"completed":1,"retryScheduled":0,"#,
    r#""leaseLost":0,"blocked":0,"stateFailures":0,"prunedCompleted":0}"#,
);
/// Synthetic PDF bytes for the queued asset.
const PDF: &[u8] = b"%PDF-1.7\n% synthetic generation cli asset\n";

/// A temporary database and a private temporary root, removed when the value is dropped.
struct Scenario {
    database: TempDatabase,
    scratch: tempfile::TempDir,
}

impl Scenario {
    /// The generated root inside the scratch directory, created private.
    fn root(&self) -> Result<PathBuf, Box<dyn Error>> {
        let root = self.scratch.path().join("generated");
        if !ensure_private_directory(&root.to_string_lossy()) {
            return Err("the private test root could not be created".into());
        }
        Ok(root)
    }

    /// Runs `jobniac-cli <mode>` with the database settings and the given extra variables.
    fn run(&self, mode: &str, extra: &[(&str, String)]) -> std::io::Result<Output> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jobniac-cli"));
        command.env_clear().arg(mode);
        // Windows needs SYSTEMROOT for its runtime; without it the child cannot start.
        if let Some(system_root) = std::env::var_os("SYSTEMROOT") {
            command.env("SYSTEMROOT", system_root);
        }
        let (key, value) = self.database.environment_pair();
        command.env(key, value);
        for (key, value) in extra {
            command.env(key, value);
        }
        command.output()
    }
}

/// The environment of a root-bearing command: the root path and its configured identity.
fn root_variables(root: &Path) -> Vec<(&'static str, String)> {
    vec![
        (
            "JOBNIAC_GENERATED_ROOT",
            root.to_string_lossy().into_owned(),
        ),
        ("JOBNIAC_GENERATED_ROOT_ID", ROOT_ID.to_owned()),
    ]
}

/// The cleanup environment: the lifecycle flag, then the root values.
fn cleanup_variables(root: &Path) -> Vec<(&'static str, String)> {
    let mut variables = vec![("JOBNIAC_GENERATED_ENABLED", "1".to_owned())];
    variables.extend(root_variables(root));
    variables
}

/// A scenario with a fresh database file, migrated when `migrated` is set.
fn start(migrated: bool) -> Result<Scenario, Box<dyn Error>> {
    let database = TempDatabase::new()?;
    if migrated {
        migrate(database.path()).map_err(|error| error.0)?;
    }
    // Canonical form: the owned-asset checks refuse a root that differs from its final path, and
    // `%TEMP%` can be an 8.3 alias on Windows.
    let scratch = scratch_dir("jobniac-cli-success-")?;
    Ok(Scenario { database, scratch })
}

/// The stdout of a successful command, as text. Any stderr is a failure of the scenario.
fn success_stdout(output: &Output) -> Result<String, Box<dyn Error>> {
    if output.status.code() != Some(0) {
        return Err(format!(
            "exit {:?}, stderr {:?}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    assert!(output.stderr.is_empty(), "a success prints no stderr");
    Ok(String::from_utf8(output.stdout.clone())?)
}

#[tokio::test]
async fn setup_registers_the_root_once_and_then_confirms_it() -> TestResult {
    let scenario = start(true)?;
    let root = scenario.root()?;
    let variables = root_variables(&root);

    let first = scenario.run("setup-generated-root", &variables)?;
    assert_eq!(
        success_stdout(&first)?,
        format!("Generated root {ROOT_ID}: created and registered.\n")
    );

    let second = scenario.run("setup-generated-root", &variables)?;
    assert_eq!(
        success_stdout(&second)?,
        format!("Generated root {ROOT_ID}: already set up.\n")
    );
    Ok(())
}

#[tokio::test]
async fn cleanup_on_an_empty_queue_prints_zero_metadata() -> TestResult {
    let scenario = start(true)?;
    let root = scenario.root()?;
    let setup = scenario.run("setup-generated-root", &root_variables(&root))?;
    success_stdout(&setup)?;

    let output = scenario.run("cleanup", &cleanup_variables(&root))?;
    assert_eq!(success_stdout(&output)?, format!("{EMPTY_METADATA}\n"));
    Ok(())
}

#[tokio::test]
async fn cleanup_removes_one_queued_asset_and_reports_it() -> TestResult {
    let scenario = start(true)?;
    let root = scenario.root()?;
    let setup = scenario.run("setup-generated-root", &root_variables(&root))?;
    success_stdout(&setup)?;

    let root_id = Uuid::parse_str(ROOT_ID)?;
    let identity = queue_one_asset(&scenario, root_id).await?;
    let owned = root.join(identity.relative_path());
    if let Some(parent) = owned.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&owned, PDF)?;
    // The file layer proves ownership by refusing any group or other permission bit, so the
    // fixture is private exactly as a published asset is (a default umask would give 0644 and
    // block the task as OWNERSHIP_UNPROVEN).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&owned, std::fs::Permissions::from_mode(0o600))?;
    }

    let output = scenario.run("cleanup", &cleanup_variables(&root))?;
    assert_eq!(success_stdout(&output)?, format!("{ONE_TASK_METADATA}\n"));
    assert!(!owned.exists(), "the queued asset is removed by the pass");
    Ok(())
}

#[tokio::test]
async fn setup_refuses_a_schema_that_is_not_current_and_creates_nothing() -> TestResult {
    let scenario = start(false)?;
    let root = scenario.root()?;

    let output = scenario.run("setup-generated-root", &root_variables(&root))?;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "a refusal prints no status");
    assert_eq!(
        String::from_utf8(output.stderr)?,
        format!("{PREFIX}the schema is not current; run `jobniac migrate`.\n")
    );
    assert_eq!(std::fs::read_dir(&root)?.count(), 0, "no marker is written");
    Ok(())
}

/// Registers the root row and seeds one RESERVED asset under it, then queues it for cleanup
/// through the repository, as the application does. Returns the asset identity.
async fn queue_one_asset(scenario: &Scenario, root: Uuid) -> Result<AssetIdentity, Box<dyn Error>> {
    let uow = SqliteUnitOfWork::new(SqlitePool::open(scenario.database.path())?);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let asset_id = Uuid::new_v4();
    let identity = AssetIdentity::new(asset_id, root, &sha256_hex(PDF), u64::try_from(PDF.len())?)?;
    let relative = identity.relative_path().to_owned();
    let digest = identity.sha256().to_owned();
    let size = PDF.len();
    let now = instant_text(clock.now());
    uow.transaction(TransactionOptions::default(), async |tx| {
        tx.run(|connection| {
            connection.execute(
                "INSERT INTO jobniac_generated_asset_roots (id, created_at) VALUES (?1, ?2) \
                 ON CONFLICT (id) DO NOTHING",
                [root.to_string(), now.clone()],
            )?;
            connection.execute(
                &format!(
                    "INSERT INTO jobniac_generated_assets (asset_id, root_id, relative_path, \
                     sha256, size_bytes, document_id, lifecycle, created_at, \
                     generation_intent_required) \
                     VALUES (?1, ?2, ?3, ?4, {size}, NULL, 'RESERVED', ?5, 0)"
                ),
                [
                    asset_id.to_string(),
                    root.to_string(),
                    relative.clone(),
                    digest.clone(),
                    now.clone(),
                ],
            )?;
            Ok(())
        })
        .map_err(|_| AppError::Internal)
    })
    .await?;

    let repository = SqliteCleanupRepository::new(uow, clock);
    if repository.prepare_cleanup(asset_id).await? != PreparationOutcome::Prepared {
        return Err("the seeded asset was not queued for cleanup".into());
    }
    Ok(identity)
}
