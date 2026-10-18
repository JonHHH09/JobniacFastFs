//! The mode handlers of the console binary.
//!
//! Each handler runs one command: `migrate`, `cleanup`, `setup-generated-root` and `mcp` (STDIO) live here, and `http`
//! (the loopback listener) lives in `http_mode`. The desktop UI is the separate `jobniac` binary.

use std::future::Future;
use std::io;
use std::process::ExitCode;
use std::sync::Arc;

use jobniac_core::cleanup::GenerationLifecycleSummary;
use jobniac_core::generated_root::RootPlatform;
use jobniac_core::generated_root_setup::SetupResult;
use jobniac_infra::sqlite::location::create_database_directory;
use jobniac_infra::sqlite::migrations::{
    SchemaState, inspect_schema, migrate, packaged_migrations,
};
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use jobniac_mcp::{McpServices, serve_stdio};
use jobniac_runtime::database::database_path;
use jobniac_runtime::generation::{
    GeneratedRootLocation, compose_core, compose_root_setup, configured_root_identity,
    load_generated_root_location, platform_name,
};
use jobniac_runtime::job_links::compose_job_links;
use jobniac_runtime::pdfium_location::{cli_bundle_roots, resolve_pdfium_dir};
use jobniac_runtime::services::{StartupError, compose_with_pdfium};
use tracing_subscriber::filter::{FilterExt, LevelFilter, filter_fn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, fmt};
use uuid::Uuid;

use crate::arbeitnow_ports;
use crate::artifact_ports;
use crate::console::{stderr_line, stdout_line};
use crate::deletion_ports;
use crate::document_ports;
use crate::generation_ports;
use crate::job_link_ports;
use crate::job_ports;
use crate::match_ports;
use crate::profile_ports;

/// The sanitized failure for any unexpected error. It never names the setting or the cause.
const STARTUP_FAILURE: &str = "Jobniac could not start. Check local configuration.";
/// Message for `migrate` when no database location can be resolved.
const MIGRATE_NOT_CONFIGURED: &str =
    "Jobniac migrate: database location is not available; check JOBNIAC_DB_PATH.";
/// Message for `migrate` when the database directory cannot be created.
const MIGRATE_DIRECTORY_FAILED: &str = "Jobniac migrate: database directory could not be created.";
/// Prefix of every `setup-generated-root` refusal.
const SETUP_PREFIX: &str = "Jobniac setup-generated-root: ";
/// Setup refusal without a usable database.
const SETUP_NOT_CONFIGURED: &str = "database is not available; check JOBNIAC_DB_PATH.";
/// Setup refusal without an admitted generated root.
const SETUP_ROOT_INVALID: &str = "the generated root path is not configured or invalid.";
/// Setup refusal of a malformed `JOBNIAC_GENERATED_ROOT_ID`.
const SETUP_IDENTITY_INVALID: &str = "JOBNIAC_GENERATED_ROOT_ID is invalid.";
/// Setup refusal of a schema that is not current.
const SETUP_SCHEMA_NOT_CURRENT: &str = "the schema is not current; run `jobniac migrate`.";
/// The fixed failure of a cleanup pass that cannot run.
const CLEANUP_UNAVAILABLE: &str =
    "Jobniac cleanup: configured generation lifecycle is unavailable.";

/// Exit status for an interrupted run (128 + SIGINT). Ctrl+C
/// prints nothing in any mode.
const INTERRUPTED: u8 = 130;

/// Apply pending schema migrations: the only command that writes the schema.
pub fn run_migrate() -> ExitCode {
    let env = jobniac_runtime::process_environment();
    let Some(path) = database_path(&env) else {
        stderr_line(MIGRATE_NOT_CONFIGURED);
        return ExitCode::FAILURE;
    };
    if create_database_directory(&path).is_err() {
        stderr_line(MIGRATE_DIRECTORY_FAILED);
        return ExitCode::FAILURE;
    }
    // The migration is one blocking SQLite transaction per version: an interruption rolls back the
    // uncommitted work, so no interrupt handling is needed here.
    let result = migrate(&path);
    match result {
        Ok(applied) if applied.is_empty() => match packaged_migrations() {
            Ok(migrations) => {
                stdout_line(&format!(
                    "Schema is current (V{:04}); nothing to apply.",
                    migrations.len()
                ));
                ExitCode::SUCCESS
            }
            Err(error) => {
                stderr_line(&format!("Jobniac migrate: {error}"));
                ExitCode::FAILURE
            }
        },
        Ok(applied) => {
            let names: Vec<String> = applied
                .iter()
                .map(|version| format!("V{version:04}"))
                .collect();
            stdout_line(&format!("Applied migrations: {}", names.join(", ")));
            ExitCode::SUCCESS
        }
        Err(error) => {
            stderr_line(&format!("Jobniac migrate: {error}"));
            ExitCode::FAILURE
        }
    }
}

/// The PDFium directory of this console binary: `JOBNIAC_PDFIUM_DIR` when set, else the library in
/// the bundle folders beside the executable. `None` leaves the PDF
/// operations failing with their fixed messages, as before.
pub(crate) fn cli_pdfium_dir(env: &jobniac_core::env::Environment) -> Option<std::path::PathBuf> {
    let roots = std::env::current_exe()
        .map(|executable| cli_bundle_roots(&executable))
        .unwrap_or_default();
    resolve_pdfium_dir(env, &roots, |path| path.is_file()).ok()
}

/// Compose the MCP tool services from the process environment. Shared by `mcp` (STDIO) and `http`
/// (loopback HTTP), so both transports serve the same catalog.
pub(crate) fn compose_mcp_services(
    env: &jobniac_core::env::Environment,
) -> Result<McpServices, StartupError> {
    let services = compose_with_pdfium(env, cli_pdfium_dir(env))?;
    let job_links = compose_job_links(env, services.database.as_ref())?;
    let ports = profile_ports::from_services(&services.profiles);
    let jobs = job_ports::from_services(&services.jobs);
    let matching = match_ports::from_services(&services.matching);
    let documents = document_ports::from_services(&services.documents, &services.profile_pdf);
    let generation_service = generation_ports::from_service(&services.generation.facade);
    Ok(McpServices {
        health: services.health,
        profile_list: ports.list,
        profile_details: ports.details,
        profile_search: ports.search,
        job_service: jobs.service,
        job_search: jobs.search,
        matching_service: matching.matching,
        review_service: matching.reviews,
        document_service: documents.documents,
        profile_pdf_service: documents.profile_pdf,
        generation_service: Some(generation_service),
        document_artifact_service: artifact_ports::from_services(&services.artifacts),
        deletion: deletion_ports::from_services(&services.deletion),
        job_link_service: job_link_ports::from_service(job_links.as_ref()),
        arbeitnow: arbeitnow_ports::from_environment(env, services.database.as_ref())?,
    })
}

/// Serve the diagnostic MCP server over STDIO until stdin closes or Ctrl+C is pressed.
pub fn run_mcp() -> ExitCode {
    let env = jobniac_runtime::process_environment();
    let mcp = match compose_mcp_services(&env) {
        Ok(mcp) => mcp,
        Err(error) => {
            stderr_line(&error.to_string());
            return ExitCode::FAILURE;
        }
    };
    init_tracing();
    let Some(runtime) = build_runtime() else {
        return unexpected_failure();
    };
    match run_until_interrupt(runtime, serve_stdio(mcp), interrupt()) {
        None => ExitCode::from(INTERRUPTED),
        Some(Ok(())) => ExitCode::SUCCESS,
        Some(Err(_)) => unexpected_failure(),
    }
}

/// Run one bounded generated-root cleanup pass and print its counts.
///
/// The counts are printed even when the pass reports failures, which exit 1.
pub fn run_cleanup() -> ExitCode {
    let env = jobniac_runtime::process_environment();
    // Cleanup never creates or migrates the database: a missing file is the unavailable case.
    let pool = database_path(&env)
        .filter(|path| path.is_file())
        .and_then(|path| SqlitePool::open(path).ok());
    let uow = SqliteUnitOfWork::new(pool.unwrap_or_else(SqlitePool::unconfigured));
    let lifecycle = Arc::clone(&compose_core(&env, &uow).lifecycle);
    let Some(runtime) = build_runtime() else {
        return unexpected_failure();
    };
    match run_until_interrupt(
        runtime,
        async move { lifecycle.run_cleanup().await },
        interrupt(),
    ) {
        None => ExitCode::from(INTERRUPTED),
        Some(Ok(summary)) => {
            stdout_line(&metadata_json(&summary));
            if summary.has_failures() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Some(Err(_)) => {
            stderr_line(CLEANUP_UNAVAILABLE);
            ExitCode::FAILURE
        }
    }
}

/// The compact JSON object of a cleanup pass, keys in the metadata order (no spaces).
fn metadata_json(summary: &GenerationLifecycleSummary) -> String {
    let fields: Vec<String> = summary
        .metadata()
        .iter()
        .map(|(key, value)| format!("\"{key}\":{value}"))
        .collect();
    format!("{{{}}}", fields.join(","))
}

/// Initialize and register the owned generated root.
///
/// Every refusal prints the setup prefix and a fixed text to stderr and exits 1. Nothing is
/// repaired, and no path is ever printed.
pub fn run_setup_generated_root() -> ExitCode {
    let env = jobniac_runtime::process_environment();
    let Some(database) = database_path(&env) else {
        return setup_refused(SETUP_NOT_CONFIGURED);
    };
    let platform = RootPlatform::from_platform_name(platform_name());
    let Some(location) = load_generated_root_location(&env, platform) else {
        return setup_refused(SETUP_ROOT_INVALID);
    };
    let Ok(configured) = configured_root_identity(&env) else {
        return setup_refused(SETUP_IDENTITY_INVALID);
    };
    let Some(runtime) = build_runtime() else {
        return unexpected_failure();
    };
    let outcome = run_until_interrupt(
        runtime,
        setup_once(database, location, configured),
        interrupt(),
    );
    match outcome {
        None => ExitCode::from(INTERRUPTED),
        Some(Ok(result)) => {
            stdout_line(&format!(
                "Generated root {}: {}.",
                result.root_id,
                result.status.text()
            ));
            ExitCode::SUCCESS
        }
        Some(Err(text)) => setup_refused(&text),
    }
}

/// The schema check and the setup of one root. The error is the text after the prefix.
async fn setup_once(
    database: std::path::PathBuf,
    location: GeneratedRootLocation,
    configured: Option<Uuid>,
) -> Result<SetupResult, String> {
    match inspect_schema(Some(&database)) {
        Ok(status) if status.state == SchemaState::Current => {}
        Ok(_) => return Err(SETUP_SCHEMA_NOT_CURRENT.to_owned()),
        Err(error) => return Err(error.to_string()),
    }
    let pool = SqlitePool::open(&database).map_err(|_| SETUP_NOT_CONFIGURED.to_owned())?;
    let setup = compose_root_setup(&pool, &location, configured);
    setup
        .run()
        .await
        .map_err(|refusal| format!("{}.", refusal.text()))
}

/// Prints one setup refusal with the command prefix and exits 1.
fn setup_refused(text: &str) -> ExitCode {
    stderr_line(&format!("{SETUP_PREFIX}{text}"));
    ExitCode::FAILURE
}

/// Drives `work` until it finishes or `stop` resolves, then ends the runtime.
///
/// Returns `None` when `stop` resolved first. The runtime is shut down without waiting for its
/// blocking tasks: the stdin read behind the MCP transport cannot be cancelled, and dropping the
/// runtime would wait for it, so an interrupted server would never exit.
fn run_until_interrupt<F, S>(
    runtime: tokio::runtime::Runtime,
    work: F,
    stop: S,
) -> Option<F::Output>
where
    F: Future,
    S: Future<Output = ()>,
{
    let outcome = runtime.block_on(async {
        tokio::select! {
            output = work => Some(output),
            _ = stop => None,
        }
    });
    runtime.shutdown_background();
    outcome
}

/// Whether `target` is a module path of the MCP SDK (`rmcp`, `rmcp::service`, `rmcp_macros`, ...).
fn is_sdk_target(target: &str) -> bool {
    target.starts_with("rmcp")
}

/// Whether `target` is `hyper` or one of its helper crates.
fn is_hyper_target(target: &str) -> bool {
    target.starts_with("hyper")
}

/// Whether a record may be written whatever `RUST_LOG` says. The MCP SDK logs tool arguments,
/// results, session ids and raw request lines at debug and trace (E-NET-89), so `rmcp` writes
/// nothing; `hyper` keeps only its warnings. This is a filter in front of the environment one, not
/// a directive: an `EnvFilter` lets the most specific directive win, so `rmcp::service=trace`
/// would outrank an `rmcp=off` directive.
fn is_record_allowed(target: &str, level: LevelFilter) -> bool {
    !is_sdk_target(target) && (!is_hyper_target(target) || level <= LevelFilter::WARN)
}

/// Write diagnostics to stderr only. The default level is `warn`; `RUST_LOG` overrides it, within
/// what [`is_record_allowed`] permits.
pub(crate) fn init_tracing() {
    let env = EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .from_env_lossy();
    let allowed = filter_fn(|metadata| {
        is_record_allowed(
            metadata.target(),
            LevelFilter::from_level(*metadata.level()),
        )
    });
    // A global subscriber can be installed only once; a failed install leaves logging off.
    let _ = tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_writer(io::stderr)
                .with_filter(env.and(allowed)),
        )
        .try_init();
}

pub(crate) fn build_runtime() -> Option<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()
}

/// Resolves on Ctrl+C. If the handler cannot be installed it never resolves, so the server
/// keeps running instead of exiting as if interrupted.
pub(crate) async fn interrupt() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

pub(crate) fn unexpected_failure() -> ExitCode {
    stderr_line(STARTUP_FAILURE);
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn sdk_records_are_never_allowed_and_hyper_keeps_only_warnings() {
        for target in [
            "rmcp",
            "rmcp::service",
            "rmcp::transport::streamable_http_server::tower",
            "rmcp_macros",
        ] {
            for level in [
                LevelFilter::ERROR,
                LevelFilter::WARN,
                LevelFilter::INFO,
                LevelFilter::DEBUG,
                LevelFilter::TRACE,
            ] {
                assert!(!is_record_allowed(target, level), "{target} {level}");
            }
        }
        assert!(is_record_allowed("hyper::proto::h1", LevelFilter::WARN));
        assert!(is_record_allowed("hyper_util::server", LevelFilter::ERROR));
        assert!(!is_record_allowed("hyper::proto::h1", LevelFilter::INFO));
        assert!(!is_record_allowed("hyper_util::server", LevelFilter::TRACE));
        assert!(is_record_allowed("jobniac_cli::modes", LevelFilter::TRACE));
    }

    #[test]
    fn stop_does_not_wait_for_a_blocked_read() {
        let (finished_tx, finished_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let Some(runtime) = build_runtime() else {
                return;
            };
            // The blocked read ends when `_hold` is dropped, after the runtime has returned.
            let (_hold, held) = mpsc::channel::<()>();
            let outcome = run_until_interrupt(
                runtime,
                async move {
                    let _ = tokio::task::spawn_blocking(move || held.recv()).await;
                },
                std::future::ready(()),
            );
            let _ = finished_tx.send(outcome.is_none());
        });
        assert_eq!(
            finished_rx.recv_timeout(Duration::from_secs(10)),
            Ok(true),
            "the runtime must end without waiting for the blocked read"
        );
    }
}
