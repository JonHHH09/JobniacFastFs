//! Runner behind the `pg-test` binary: run `cargo test` against one disposable PostgreSQL 18
//! cluster.
//!
//! The cluster lives in a fresh temporary directory, listens on a free loopback port and is stopped
//! and removed when the run ends. Arguments after `--` are passed to `cargo test`:
//!
//! ```text
//! JOBNIAC_POSTGRES_BIN=<PostgreSQL 18 bin directory> cargo run -p jobniac-testkit --bin pg-test -- -p <crate>
//! ```
//!
//! The cluster directory is created under `JOBNIAC_PG_SCRATCH` when that is set and non-empty,
//! otherwise under the platform temporary directory.
//!
//! Exit codes: the `cargo test` code on a normal run, `1` for a harness failure (a cluster that
//! cannot be stopped is kept for recovery and its path is printed), `2` when the environment is
//! refused (`JOBNIAC_POSTGRES_BIN` unset, binaries missing, or not PostgreSQL 18). An interrupt
//! (Ctrl-C or console close) ends the test process tree, and the cluster is still stopped and removed.
//!
//! The runner lives in the library, not in the binary, so its unit tests run under `cargo test`
//! without building `pg-test` itself. Cargo would otherwise try to relink the running executable
//! (Windows refuses that) when the child `cargo test` runs with a different feature set.

use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const BIN_VAR: &str = "JOBNIAC_POSTGRES_BIN";
const SCRATCH_VAR: &str = "JOBNIAC_PG_SCRATCH";
const POSTGRES_MAJOR: u32 = 18;
const ADMIN_USER: &str = "jobniac_test_admin";
const MARKER_FILE: &str = ".jobniac-postgres-cluster.json";
const MARKER_OWNER: &str = "jobniac-postgres-test";
const DATA_DIR: &str = "data";
const PASSWORD_FILE: &str = "password";
const LOG_FILE: &str = "postgres.log";
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);
const INITDB_TIMEOUT: Duration = Duration::from_secs(300);
// pg_ctl start waits up to 60 seconds (-t 60); the timeout leaves room for process start-up.
const START_TIMEOUT: Duration = Duration::from_secs(120);
// pg_ctl stop waits up to 30 seconds (-t 30).
const STOP_TIMEOUT: Duration = Duration::from_secs(90);
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const READY_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
// The bound covers `cargo test` as a whole, including compiling the test binaries. Recorded in
// the crate README.
const TEST_TIMEOUT: Duration = Duration::from_secs(30 * 60);
// Connection variables that must not leak into the test run (the TCP-only settings are explicit).
const PG_ENV_VARS: [&str; 8] = [
    "DATABASE_URL",
    "PGHOST",
    "PGPORT",
    "PGDATABASE",
    "PGUSER",
    "PGPASSWORD",
    "PGSERVICE",
    "PGSERVICEFILE",
];
// Application connection variables of the caller. The test run is pointed only at the disposable
// cluster.
const DB_ENV_VARS: [&str; 5] = [
    "JOBNIAC_DB_HOST",
    "JOBNIAC_DB_PORT",
    "JOBNIAC_DB_NAME",
    "JOBNIAC_DB_USER",
    "JOBNIAC_DB_PASSWORD",
];
/// Set by the Ctrl-C and console-close handler. Only the start, test and readiness waits honour
/// it; the stop of the cluster does not, so cleanup always runs to the end.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Why a run stopped before it could report the `cargo test` result.
enum Failure {
    /// The environment is not usable for the run (exit code 2).
    Refused(String),
    /// The harness failed while running the cluster (exit code 1).
    Failed(String),
}

type Step<T> = Result<T, Failure>;

/// Absolute paths of the three PostgreSQL programs the harness needs.
struct Programs {
    initdb: PathBuf,
    pg_ctl: PathBuf,
    pg_isready: PathBuf,
    postgres: PathBuf,
}

/// A child process run against the cluster: its program and arguments, and whether it is a
/// PostgreSQL client (`pg-run`), which gets the client variables and the bin directory on its PATH.
struct ChildCommand {
    program: OsString,
    args: Vec<OsString>,
    client_environment: bool,
}

/// Run the harness with the process arguments (after the program name) passed to `cargo test`,
/// and return the process exit code.
pub fn run_cli() -> ExitCode {
    if !install_interrupt_handler("pg-test") {
        return ExitCode::from(1);
    }
    let mut args = vec![OsString::from("test")];
    args.extend(env::args_os().skip(1));
    let child = ChildCommand {
        program: cargo_program(),
        args,
        client_environment: false,
    };
    report("pg-test", run(&child))
}

/// Run the harness with the process arguments (after the program name) as one command: the first
/// is the program, the rest its arguments. The command gets the cluster's `JOBNIAC_DB_*` and
/// `PG*` variables and the PostgreSQL bin directory on its PATH; the password is passed only in
/// the child environment and is never printed. The exit code is the command's.
pub fn run_command_cli() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let Some(program) = arguments.next() else {
        eprintln!("pg-run: give the command to run after --");
        return ExitCode::from(2);
    };
    if !install_interrupt_handler("pg-run") {
        return ExitCode::from(1);
    }
    let child = ChildCommand {
        program,
        args: arguments.collect(),
        client_environment: true,
    };
    report("pg-run", run(&child))
}

fn install_interrupt_handler(tool: &str) -> bool {
    let installed = ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::SeqCst)).is_ok();
    if !installed {
        eprintln!("{tool}: could not install the interrupt handler");
    }
    installed
}

fn report(tool: &str, result: Step<u8>) -> ExitCode {
    match result {
        Ok(code) => ExitCode::from(code),
        Err(Failure::Refused(message)) => {
            eprintln!("{tool}: {message}");
            ExitCode::from(2)
        }
        Err(Failure::Failed(message)) => {
            eprintln!("{tool}: {message}");
            ExitCode::from(1)
        }
    }
}

fn run(child: &ChildCommand) -> Step<u8> {
    let programs = programs_from_env()?;
    verify_postgres_major(&programs.postgres)?;

    let (root, token) = create_root()?;
    let mut started = false;
    let outcome = run_in_root(&programs, &root, child, &mut started);
    if started && let Err(message) = stop_cluster(&programs, &root.join(DATA_DIR)) {
        return Err(Failure::Failed(format!(
            "{message}; cluster retained for recovery at {}",
            root.display()
        )));
    }
    remove_root(&root, &token)?;
    outcome
}

fn programs_from_env() -> Step<Programs> {
    let bin_dir = env::var_os(BIN_VAR).map(PathBuf::from).ok_or_else(|| {
        Failure::Refused(format!(
            "{BIN_VAR} is not set; point it at the bin directory of a PostgreSQL {POSTGRES_MAJOR} installation"
        ))
    })?;
    let program = |name: &str| bin_dir.join(format!("{name}{}", env::consts::EXE_SUFFIX));
    let programs = Programs {
        initdb: program("initdb"),
        pg_ctl: program("pg_ctl"),
        pg_isready: program("pg_isready"),
        postgres: program("postgres"),
    };
    let present = [
        &programs.initdb,
        &programs.pg_ctl,
        &programs.pg_isready,
        &programs.postgres,
    ]
    .iter()
    .all(|path| path.is_file());
    if !present {
        return Err(Failure::Refused(format!(
            "{BIN_VAR} must contain initdb, pg_ctl, pg_isready and postgres"
        )));
    }
    Ok(programs)
}

fn verify_postgres_major(postgres: &Path) -> Step<()> {
    let mut child = Command::new(postgres)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Failure::Refused("could not execute the postgres binary".to_owned()))?;
    let status = wait_bounded(
        &mut child,
        VERSION_TIMEOUT,
        "postgres --version",
        kill_child,
        Some(&INTERRUPTED),
    )
    .map_err(Failure::Refused)?;
    let mut output = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut output);
    }
    if !status.success() {
        return Err(Failure::Refused(
            "postgres --version did not succeed".to_owned(),
        ));
    }
    match postgres_major(&output) {
        Some(POSTGRES_MAJOR) => Ok(()),
        _ => Err(Failure::Refused(format!(
            "the postgres binary is not PostgreSQL {POSTGRES_MAJOR}"
        ))),
    }
}

/// Create the owned cluster directory with its ownership marker. Returns the path and the token.
///
/// The token is generated first, so a failure there leaves nothing behind.
fn create_root() -> Step<(PathBuf, String)> {
    let token = random_hex(24).map_err(Failure::Failed)?;
    let parent = scratch_parent()?;
    let root = tempfile::Builder::new()
        .prefix("jobniac-pg18-")
        .tempdir_in(&parent)
        .map_err(|_| Failure::Failed("could not create the cluster directory".to_owned()))?
        .keep();
    let marker = marker_value(&token).to_string();
    if fs::write(root.join(MARKER_FILE), marker).is_err() {
        let _ = fs::remove_dir_all(&root);
        return Err(Failure::Failed(
            "could not write the cluster ownership marker".to_owned(),
        ));
    }
    Ok((root, token))
}

/// The directory that holds the cluster: `JOBNIAC_PG_SCRATCH` when set and non-empty (created when
/// missing), otherwise the platform temporary directory.
fn scratch_parent() -> Step<PathBuf> {
    match env::var_os(SCRATCH_VAR).filter(|value| !value.is_empty()) {
        Some(configured) => {
            let parent = PathBuf::from(configured);
            fs::create_dir_all(&parent).map_err(|_| {
                Failure::Failed(format!("could not create the {SCRATCH_VAR} directory"))
            })?;
            Ok(parent)
        }
        None => Ok(env::temp_dir()),
    }
}

fn run_in_root(
    programs: &Programs,
    root: &Path,
    child: &ChildCommand,
    started: &mut bool,
) -> Step<u8> {
    let data_dir = root.join(DATA_DIR);
    let password_file = root.join(PASSWORD_FILE);
    let log_file = root.join(LOG_FILE);

    let password = random_hex(32).map_err(Failure::Failed)?;
    write_password_file(&password_file, &password)?;
    let init = run_initdb(programs, &data_dir, &password_file);
    // The password is only needed by initdb and by the test environment; the file goes first.
    let _ = fs::remove_file(&password_file);
    init?;

    let port = free_loopback_port()?;
    // Set before the start: a start that fails half-way still needs a stop.
    *started = true;
    start_server(programs, &data_dir, &log_file, root, port)?;
    wait_until_ready(programs, port)?;
    run_child(programs, child, port, &password)
}

fn write_password_file(path: &Path, password: &str) -> Step<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| Failure::Failed("could not create the password file".to_owned()))?;
    file.write_all(password.as_bytes())
        .map_err(|_| Failure::Failed("could not write the password file".to_owned()))
}

fn run_initdb(programs: &Programs, data_dir: &Path, password_file: &Path) -> Step<()> {
    let mut child = Command::new(&programs.initdb)
        .arg("-D")
        .arg(data_dir)
        .arg("-U")
        .arg(ADMIN_USER)
        .arg("--pwfile")
        .arg(password_file)
        .arg("--auth-host=scram-sha-256")
        .arg("--auth-local=reject")
        .arg("-E")
        .arg("UTF8")
        .arg("--locale=C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Failure::Failed("could not start initdb".to_owned()))?;
    let status = wait_bounded(
        &mut child,
        INITDB_TIMEOUT,
        "initdb",
        kill_child,
        Some(&INTERRUPTED),
    )
    .map_err(Failure::Failed)?;
    if status.success() {
        Ok(())
    } else {
        Err(Failure::Failed(
            "initdb could not create the cluster".to_owned(),
        ))
    }
}

fn start_server(
    programs: &Programs,
    data_dir: &Path,
    log_file: &Path,
    root: &Path,
    port: u16,
) -> Step<()> {
    // pg_ctl leaves the server running with the inherited handles; the standard streams are
    // detached (the server log goes to the -l file) so this process never waits on them.
    let mut child = Command::new(&programs.pg_ctl)
        .arg("-D")
        .arg(data_dir)
        .arg("-l")
        .arg(log_file)
        .arg("-o")
        .arg(server_options(port, root))
        .arg("-w")
        .arg("-t")
        .arg("60")
        .arg("start")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Failure::Failed("could not start pg_ctl".to_owned()))?;
    let status = wait_bounded(
        &mut child,
        START_TIMEOUT,
        "pg_ctl start",
        kill_child,
        Some(&INTERRUPTED),
    )
    .map_err(Failure::Failed)?;
    if status.success() {
        Ok(())
    } else {
        Err(Failure::Failed(
            "PostgreSQL 18 did not start on the loopback port".to_owned(),
        ))
    }
}

/// Poll `pg_isready` (no credentials) until the server accepts connections, bounded by
/// `READY_TIMEOUT`.
fn wait_until_ready(programs: &Programs, port: u16) -> Step<()> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if INTERRUPTED.load(Ordering::SeqCst) {
            return Err(Failure::Failed(
                "interrupted while waiting for PostgreSQL 18 to accept connections".to_owned(),
            ));
        }
        if pg_isready_succeeds(programs, port) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Failure::Failed(
                "PostgreSQL 18 did not accept connections on the loopback port".to_owned(),
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn pg_isready_succeeds(programs: &Programs, port: u16) -> bool {
    let spawned = Command::new(&programs.pg_isready)
        .arg("-h")
        .arg("127.0.0.1")
        .arg("-p")
        .arg(port.to_string())
        .arg("-d")
        .arg("postgres")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        return false;
    };
    matches!(
        wait_bounded(&mut child, READY_PROBE_TIMEOUT, "pg_isready", kill_child, None),
        Ok(status) if status.success()
    )
}

/// Loopback-only server options. Windows builds reject a Unix socket directory, so only TCP is
/// configured there; POSIX moves the socket into the owned root, away from distribution defaults.
fn server_options(port: u16, root: &Path) -> String {
    if cfg!(windows) {
        format!("-h 127.0.0.1 -p {port}")
    } else {
        format!("-h 127.0.0.1 -p {port} -k \"{}\"", root.display())
    }
}

fn free_loopback_port() -> Step<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|_| Failure::Failed("could not reserve a loopback port".to_owned()))?;
    let port = listener
        .local_addr()
        .map_err(|_| Failure::Failed("could not read the reserved loopback port".to_owned()))?
        .port();
    Ok(port)
}

fn run_child(programs: &Programs, spec: &ChildCommand, port: u16, password: &str) -> Step<u8> {
    let mut command = Command::new(&spec.program);
    command.args(&spec.args).stdin(Stdio::null());
    // The caller's connection variables go first: a later `env` call would otherwise be undone by
    // the removal, and the child must see only the disposable cluster.
    for name in PG_ENV_VARS.iter().chain(DB_ENV_VARS.iter()) {
        command.env_remove(name);
    }
    command
        .env("JOBNIAC_TEST_PG_PORT", port.to_string())
        .env("JOBNIAC_TEST_PG_USER", ADMIN_USER)
        .env("JOBNIAC_TEST_PG_PASSWORD", password)
        .env("JOBNIAC_TEST_DB", "1")
        .env("JOBNIAC_DB_HOST", "127.0.0.1")
        .env("JOBNIAC_DB_PORT", port.to_string())
        .env("JOBNIAC_DB_NAME", "postgres")
        .env("JOBNIAC_DB_USER", ADMIN_USER)
        .env("JOBNIAC_DB_PASSWORD", password);
    if spec.client_environment {
        eprintln!(
            "pg-run: disposable PostgreSQL {POSTGRES_MAJOR} cluster on 127.0.0.1:{port}, database postgres, user {ADMIN_USER}; the password is passed in the child environment only"
        );
        command
            .env("PGHOST", "127.0.0.1")
            .env("PGPORT", port.to_string())
            .env("PGUSER", ADMIN_USER)
            .env("PGPASSWORD", password)
            .env("PGDATABASE", "postgres");
        if let Some(bin_dir) = programs.postgres.parent() {
            let mut paths = vec![bin_dir.to_path_buf()];
            if let Some(existing) = env::var_os("PATH") {
                paths.extend(env::split_paths(&existing));
            }
            if let Ok(joined) = env::join_paths(paths) {
                command.env("PATH", joined);
            }
        }
    }
    // The test child leads its own process group on POSIX, so a timeout can end all of it.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .spawn()
        .map_err(|_| Failure::Failed("could not start the test command".to_owned()))?;
    let status = wait_bounded(
        &mut child,
        TEST_TIMEOUT,
        "the test command",
        kill_tree,
        Some(&INTERRUPTED),
    )
    .map_err(Failure::Failed)?;
    Ok(exit_code(status))
}

fn cargo_program() -> OsString {
    env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

fn stop_cluster(programs: &Programs, data_dir: &Path) -> Result<(), String> {
    let mut child = Command::new(&programs.pg_ctl)
        .arg("-D")
        .arg(data_dir)
        .arg("-m")
        .arg("fast")
        .arg("-w")
        .arg("-t")
        .arg("30")
        .arg("stop")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "could not start pg_ctl stop".to_owned())?;
    // Not interruptible: the cleanup must reach the end even after Ctrl-C.
    let status = wait_bounded(&mut child, STOP_TIMEOUT, "pg_ctl stop", kill_child, None)?;
    if status.success() {
        Ok(())
    } else {
        Err("owned PostgreSQL server could not be stopped".to_owned())
    }
}

/// Remove the cluster directory when it still carries this run's ownership marker.
fn remove_root(root: &Path, token: &str) -> Step<()> {
    if !is_owned(root, token) {
        return Err(Failure::Failed(format!(
            "cluster directory {} lost its ownership marker; directory retained",
            root.display()
        )));
    }
    fs::remove_dir_all(root).map_err(|_| {
        Failure::Failed(format!(
            "could not remove the cluster directory {}",
            root.display()
        ))
    })
}

fn is_owned(root: &Path, token: &str) -> bool {
    fs::read_to_string(root.join(MARKER_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .is_some_and(|marker| marker == marker_value(token))
}

fn marker_value(token: &str) -> Value {
    json!({ "owner": MARKER_OWNER, "token": token })
}

/// Wait for a child process. When the timeout expires, or `interrupt` is set, `terminate` ends the
/// child and an error is returned.
fn wait_bounded(
    child: &mut Child,
    timeout: Duration,
    label: &str,
    terminate: fn(&mut Child),
    interrupt: Option<&AtomicBool>,
) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if interrupt.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            terminate(child);
            return Err(format!("{label} was interrupted"));
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(_) => {
                terminate(child);
                return Err(format!("could not wait for {label}"));
            }
        }
        if Instant::now() >= deadline {
            terminate(child);
            return Err(format!("{label} timed out"));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn kill_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// End a child and every process it started. On Windows `taskkill /T` walks the tree; on POSIX
/// the child leads its own process group (see `run_cargo_test`).
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID"])
            .arg(child.id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(format!("-{}", child.id()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    kill_child(child);
}

fn exit_code(status: ExitStatus) -> u8 {
    exit_code_from(status.code())
}

/// Map a child's exit code to this process's exit code; codes outside `0..=255` become `1`.
fn exit_code_from(code: Option<i32>) -> u8 {
    code.and_then(|code| u8::try_from(code).ok()).unwrap_or(1)
}

fn random_hex(bytes: usize) -> Result<String, String> {
    let mut buffer = vec![0_u8; bytes];
    getrandom::fill(&mut buffer)
        .map_err(|_| "the operating system random source is unavailable".to_owned())?;
    Ok(hex::encode(buffer))
}

/// The major version from `postgres --version` output, such as `postgres (PostgreSQL) 18.0`.
fn postgres_major(output: &str) -> Option<u32> {
    output.split_whitespace().find_map(|token| {
        let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let (major, rest) = token.split_once('.')?;
        if major.is_empty() || !major.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        if !rest.as_bytes().first()?.is_ascii_digit() {
            return None;
        }
        major.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn major_version_is_read_from_version_output() {
        assert_eq!(postgres_major("postgres (PostgreSQL) 18.0\n"), Some(18));
        assert_eq!(postgres_major("postgres (PostgreSQL) 17.4"), Some(17));
        assert_eq!(
            postgres_major("postgres (PostgreSQL) 18.1 (Debian 18.1-1)"),
            Some(18)
        );
        assert_eq!(postgres_major("postgres (PostgreSQL) 18"), None);
        assert_eq!(postgres_major("unrelated output"), None);
    }

    #[test]
    fn exit_codes_are_passed_through_or_fail() {
        assert_eq!(exit_code_from(Some(101)), 101);
        assert_eq!(exit_code_from(Some(0)), 0);
        assert_eq!(exit_code_from(Some(300)), 1);
        assert_eq!(exit_code_from(Some(-1)), 1);
        assert_eq!(exit_code_from(None), 1);
    }

    #[test]
    fn ownership_marker_must_match_the_token() {
        let root = env::temp_dir().join(format!(
            "jobniac-testkit-marker-{}",
            random_hex(4).unwrap_or_default()
        ));
        fs::create_dir_all(&root).expect("create a scratch directory");
        fs::write(root.join(MARKER_FILE), marker_value("token-a").to_string())
            .expect("write the marker");
        assert!(is_owned(&root, "token-a"));
        assert!(!is_owned(&root, "token-b"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn interrupt_ends_the_child_and_fails_the_wait() {
        let interrupted = AtomicBool::new(true);
        let mut child = Command::new(cargo_program())
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cargo is available to the test run");
        let result = wait_bounded(
            &mut child,
            Duration::from_secs(60),
            "probe",
            kill_child,
            Some(&interrupted),
        );
        assert_eq!(result.err(), Some("probe was interrupted".to_owned()));
    }

    #[test]
    fn server_options_are_loopback_only() {
        let options = server_options(54_321, Path::new("/scratch/root"));
        assert!(options.starts_with("-h 127.0.0.1 -p 54321"));
        assert!(!options.contains("0.0.0.0"));
    }
}
