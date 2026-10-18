//! The supervisor of the loopback HTTP MCP server that the desktop app can run as a child process.
//!
//! The desktop process opens no listener. When the user turns the setting on, this
//! supervisor spawns the sidecar `jobniac-cli http` (next to the app executable) and stops it again
//! on toggle off, on a port change, when the window closes and when the app exits.
//!
//! - **Token.** Every start generates a fresh 32-byte token from the OS random source (64 hex
//!   characters, valid for the listener's 43 to 128 `[A-Za-z0-9_-]` rule). It is held in a
//!   [`SecretString`], in memory only; it is passed to the child only through the environment
//!   variable `JOBNIAC_HTTP_BEARER_TOKEN`, never through arguments, and is never stored or logged.
//! - **Child environment.** The child starts from an empty environment: the `JOBNIAC_*` variables of
//!   the app (database settings and flags) minus any inherited `JOBNIAC_HTTP_*` and the internal
//!   worker marker and parent-pipe marker, the minimal OS variables of the worker allowlist, and the
//!   port, token and `JOBNIAC_INTERNAL_PARENT_PIPE=1`. Its stdout and stderr are null. The home and
//!   data-directory variables do not cross, so the child could not find the per-user default
//!   database: the app passes the database file it resolved and opened as `JOBNIAC_DB_PATH`
//!   ([`McpSupervisor::with_database_path`]). The path travels only in the child's environment.
//! - **Lifetime.** The child's stdin is a pipe whose write end the supervisor holds. The OS closes
//!   it when the app dies for any reason (SIGKILL, crash, force-close), the child sees EOF and shuts
//!   down, so a hard-killed app leaves no listener behind. The std pipe is close-on-exec (Unix) or
//!   non-inheritable (Windows), so no other child of the app (a worker) holds the write end.
//! - **Status.** `running` means the child is alive and accepts a loopback TCP connection on the
//!   port. An early exit or a missing listener within the deadline is `failed`, with a fixed
//!   [`McpFailure`] code and no child output. A missing sidecar is `unavailable`.
//!
//! [`McpSupervisor::start`] blocks while it waits for the listener, so async callers run it on a
//! blocking thread. A concurrent [`McpSupervisor::stop`] ends that wait.

use std::ffi::OsString;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use jobniac_infra::workers::env::is_allowed;
use secrecy::{ExposeSecret, SecretString};

use crate::app_settings::is_valid_mcp_port;

/// The environment variable that carries the listener port to the child.
pub const HTTP_PORT_VARIABLE: &str = "JOBNIAC_HTTP_PORT";

/// The environment variable that carries the bearer token to the child.
pub const HTTP_TOKEN_VARIABLE: &str = "JOBNIAC_HTTP_BEARER_TOKEN";

/// The sidecar executable name without the platform suffix.
pub const SIDECAR_NAME: &str = "jobniac-cli";

/// How long a start waits for the child to accept a connection.
pub const READY_DEADLINE: Duration = Duration::from_secs(15);

/// The environment variable that carries the database file to the child.
const DATABASE_PATH_VARIABLE: &str = "JOBNIAC_DB_PATH";

/// The inherited prefix that never crosses into the child (the supervisor sets its own values).
const HTTP_VARIABLE_PREFIX: &str = "JOBNIAC_HTTP_";

/// The internal worker marker; a child that inherited it would act as a worker.
const WORKER_MARKER_VARIABLE: &str = "JOBNIAC_INTERNAL_WORKER";

/// The internal marker that tells `jobniac-cli http` to exit when its stdin closes. Set by the
/// supervisor to exactly `1`; never inherited.
pub const PARENT_PIPE_VARIABLE: &str = "JOBNIAC_INTERNAL_PARENT_PIPE";

/// Pause between two readiness probes.
const PROBE_INTERVAL: Duration = Duration::from_millis(50);

/// The longest a single loopback connect attempt may take.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(250);

/// After the first accepted connection the child must still be alive: a child that lost the bind
/// race exits within this time, and the connection came from another listener.
const SETTLE_TIME: Duration = Duration::from_millis(150);

/// The `CREATE_NO_WINDOW` process creation flag: the sidecar is a console program and must not open
/// a console window beside the desktop window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The state of the supervised server, as the Settings screen shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerStatus {
    /// No child runs and none was requested.
    Stopped,
    /// A child was spawned and is not accepting connections yet.
    Starting,
    /// The child is alive and accepts connections on the port.
    Running,
    /// The last start failed or the child died; see [`McpSupervisor::failure`].
    Failed,
    /// The sidecar executable does not exist next to the app.
    Unavailable,
}

/// Why the last start failed. Fixed codes only: no child output, no path and no setting value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpFailure {
    /// The port is outside `1024..=65535`.
    InvalidPort,
    /// The OS random source did not answer, so no token could be made.
    TokenUnavailable,
    /// The process could not be created.
    SpawnFailed,
    /// The child exited before it accepted a connection (or died while running).
    ExitedEarly,
    /// The child stayed alive but never accepted a connection within the deadline.
    NotReachable,
    /// Something already listens on the port (or it cannot be bound), so no child was spawned.
    PortInUse,
}

impl McpFailure {
    /// The fixed code of the failure.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidPort => "invalid_port",
            Self::TokenUnavailable => "token_unavailable",
            Self::SpawnFailed => "spawn_failed",
            Self::ExitedEarly => "exited_early",
            Self::NotReachable => "not_reachable",
            Self::PortInUse => "port_in_use",
        }
    }
}

/// The endpoint and token of a running server, for a client configuration.
pub struct McpConnection {
    /// `http://127.0.0.1:<port>/mcp`.
    pub endpoint: String,
    /// The bearer token of this start.
    pub token: SecretString,
}

impl fmt::Debug for McpConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpConnection")
            .field("endpoint", &self.endpoint)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// What a spawner is asked to start: the complete child environment. It holds the token.
pub struct SpawnRequest {
    /// The complete environment of the child (it starts from an empty one).
    pub environment: Vec<(OsString, OsString)>,
}

impl fmt::Debug for SpawnRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The environment holds the token and database settings: counts only.
        f.debug_struct("SpawnRequest")
            .field("variables", &self.environment.len())
            .finish()
    }
}

/// Why a spawner could not start a child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    /// The sidecar executable does not exist.
    Unavailable,
    /// The process could not be created.
    Failed,
}

/// A started child process.
pub trait McpChild: Send {
    /// Whether the child is still alive (it has not exited).
    fn is_running(&mut self) -> bool;

    /// Kills the child and reaps it. Safe to call on a child that already exited.
    fn terminate(&mut self);
}

/// Starts the sidecar. The real implementation is [`ProcessSpawner`]; tests use a fake.
pub trait Spawner: Send + Sync {
    /// Whether the sidecar can be started at all (its executable exists).
    fn available(&self) -> bool;

    /// Whether `port` can be bound on the loopback interface right now. A start that finds it busy
    /// spawns nothing: a child that lost the bind race could still look alive while the connect
    /// probe reaches the foreign listener. The default tries a real bind and drops it at once.
    fn port_free(&self, port: u16) -> bool {
        TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
    }

    /// Starts one child with exactly `request.environment`.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnError`] when the sidecar is missing or the process cannot be created.
    fn spawn(&self, request: SpawnRequest) -> Result<Box<dyn McpChild>, SpawnError>;
}

/// The real spawner: `jobniac-cli http` through [`std::process::Command`].
#[derive(Debug, Clone)]
pub struct ProcessSpawner {
    sidecar: Option<PathBuf>,
}

impl ProcessSpawner {
    /// A spawner for the sidecar next to the running executable (Tauri's `externalBin` layout, and
    /// the `target` folder of a development build). No such file means [`Spawner::available`] is
    /// `false`.
    #[must_use]
    pub fn beside_current_exe() -> Self {
        let sidecar = std::env::current_exe().ok().and_then(|exe| {
            let candidate = exe
                .parent()?
                .join(format!("{SIDECAR_NAME}{}", std::env::consts::EXE_SUFFIX));
            candidate.is_file().then_some(candidate)
        });
        Self { sidecar }
    }

    /// A spawner for an explicit sidecar path (`None` for a missing sidecar).
    #[must_use]
    pub fn with_sidecar(sidecar: Option<PathBuf>) -> Self {
        Self { sidecar }
    }
}

impl Spawner for ProcessSpawner {
    fn available(&self) -> bool {
        self.sidecar.as_ref().is_some_and(|path| path.is_file())
    }

    fn spawn(&self, request: SpawnRequest) -> Result<Box<dyn McpChild>, SpawnError> {
        let Some(sidecar) = self.sidecar.as_ref().filter(|path| path.is_file()) else {
            return Err(SpawnError::Unavailable);
        };
        let mut command = Command::new(sidecar);
        command
            .arg("http")
            .env_clear()
            .envs(request.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|_| SpawnError::Failed)?;
        // The write end stays in the handle: the OS closes it when the app dies, which stops the child.
        let parent_pipe = child.stdin.take();
        Ok(Box::new(ProcessChild { child, parent_pipe }))
    }
}

/// A child of the real spawner. It is killed when dropped, so no path leaves it running.
struct ProcessChild {
    child: Child,
    /// The write end of the child's stdin; held only so that its closing signals the child.
    parent_pipe: Option<ChildStdin>,
}

impl McpChild for ProcessChild {
    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn terminate(&mut self) {
        drop(self.parent_pipe.take());
        // Errors mean the child already exited; the wait reaps it either way.
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

impl Drop for ProcessChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// The OS random source did not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a bearer token could not be generated")]
pub struct TokenError;

/// A fresh bearer token: 32 random bytes as 64 lower-case hex characters.
///
/// # Errors
///
/// Returns [`TokenError`] when the OS random source fails.
pub fn generate_token() -> Result<SecretString, TokenError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| TokenError)?;
    let text: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(SecretString::from(text))
}

/// The complete environment of the child, built from the app's `source` variables.
///
/// Kept: `JOBNIAC_*` variables except `JOBNIAC_HTTP_*` and the internal worker and parent-pipe
/// markers, and the variables of the worker allowlist (`PATH`, the temp and locale variables, the
/// Windows system root). Everything else (proxy settings, interpreter overrides, other secrets) is
/// dropped. The supervisor's port, token and parent-pipe marker are added last, so no inherited
/// value can replace them. With a `database` file, that file is the child's `JOBNIAC_DB_PATH`
/// whatever the app inherited.
#[must_use]
pub fn child_environment(
    source: &[(OsString, OsString)],
    port: u16,
    token: &SecretString,
    database: Option<&Path>,
) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = source
        .iter()
        .filter(|(name, _)| name.to_str().is_some_and(crosses_into_child))
        .filter(|(name, _)| {
            database.is_none()
                || !name
                    .to_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(DATABASE_PATH_VARIABLE))
        })
        .cloned()
        .collect();
    if let Some(database) = database {
        environment.push((
            DATABASE_PATH_VARIABLE.into(),
            database.as_os_str().to_owned(),
        ));
    }
    environment.push((HTTP_PORT_VARIABLE.into(), port.to_string().into()));
    environment.push((HTTP_TOKEN_VARIABLE.into(), token.expose_secret().into()));
    environment.push((PARENT_PIPE_VARIABLE.into(), "1".into()));
    environment
}

/// Whether the variable `name` may cross from the app into the child.
fn crosses_into_child(name: &str) -> bool {
    let upper = name.to_uppercase();
    if upper.starts_with(HTTP_VARIABLE_PREFIX)
        || upper == WORKER_MARKER_VARIABLE
        || upper == PARENT_PIPE_VARIABLE
    {
        return false;
    }
    upper.starts_with("JOBNIAC_") || is_allowed(name)
}

/// The listener URL of `port`.
fn endpoint(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// Whether something accepts a loopback TCP connection on `port` right now.
fn accepts_connection(port: u16) -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).is_ok()
}

struct Inner {
    status: McpServerStatus,
    failure: Option<McpFailure>,
    child: Option<Box<dyn McpChild>>,
    port: u16,
    token: Option<SecretString>,
    /// Incremented by every start and stop, so a start that waits for the listener can tell that it
    /// was replaced or stopped meanwhile.
    epoch: u64,
    /// Set by [`McpSupervisor::close`] (window close or app exit): no start spawns after it, so a
    /// start that was still on its way to the lock cannot leave a child behind the exit.
    closed: bool,
}

impl Inner {
    /// Kills and reaps the child, drops the token and invalidates every waiting start.
    fn shutdown(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.terminate();
        }
        self.token = None;
        self.epoch += 1;
    }

    fn fail(&mut self, failure: McpFailure) {
        self.shutdown();
        self.status = McpServerStatus::Failed;
        self.failure = Some(failure);
    }
}

/// Starts, watches and stops the `jobniac-cli http` child. See the module documentation.
pub struct McpSupervisor {
    spawner: Arc<dyn Spawner>,
    source_environment: Vec<(OsString, OsString)>,
    database_path: Option<PathBuf>,
    ready_deadline: Duration,
    inner: Mutex<Inner>,
}

impl fmt::Debug for McpSupervisor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The source environment and the token are secrets: the status only.
        f.debug_struct("McpSupervisor")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl McpSupervisor {
    /// A supervisor that starts children through `spawner`. `source_environment` is the app's own
    /// environment (`std::env::vars_os()`); [`child_environment`] selects what the child gets.
    #[must_use]
    pub fn new(spawner: Arc<dyn Spawner>, source_environment: Vec<(OsString, OsString)>) -> Self {
        Self {
            spawner,
            source_environment,
            database_path: None,
            ready_deadline: READY_DEADLINE,
            inner: Mutex::new(Inner {
                status: McpServerStatus::Stopped,
                failure: None,
                child: None,
                port: 0,
                token: None,
                epoch: 0,
                closed: false,
            }),
        }
    }

    /// The database file the app opened; every child gets it as `JOBNIAC_DB_PATH`.
    #[must_use]
    pub fn with_database_path(mut self, path: Option<PathBuf>) -> Self {
        self.database_path = path;
        self
    }

    /// Replaces the readiness deadline (tests use a short one).
    #[must_use]
    pub const fn with_ready_deadline(mut self, deadline: Duration) -> Self {
        self.ready_deadline = deadline;
        self
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // Every writer leaves the state consistent, so a poisoned lock is still usable.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The current status. A stopped server whose sidecar is missing reads as `unavailable`, so the
    /// Settings screen can say so before the first start; the sidecar is re-checked on every call,
    /// so one that appears later is picked up.
    #[must_use]
    pub fn status(&self) -> McpServerStatus {
        let status = self.lock().status;
        if status == McpServerStatus::Stopped && !self.spawner.available() {
            return McpServerStatus::Unavailable;
        }
        status
    }

    /// Why the last start failed, while the status is `failed`.
    #[must_use]
    pub fn failure(&self) -> Option<McpFailure> {
        self.lock().failure
    }

    /// The endpoint and token of the running server, or `None` unless the status is `running`.
    #[must_use]
    pub fn connection(&self) -> Option<McpConnection> {
        let inner = self.lock();
        if inner.status != McpServerStatus::Running {
            return None;
        }
        let token = inner.token.as_ref()?;
        Some(McpConnection {
            endpoint: endpoint(inner.port),
            token: SecretString::from(token.expose_secret().to_owned()),
        })
    }

    /// Stops a running child, then starts a new one on `port` with a fresh token, and waits until
    /// it accepts a connection, exits, or the deadline passes. Returns the resulting status.
    ///
    /// This blocks for up to the readiness deadline: async callers use a blocking thread. A
    /// [`stop`](Self::stop) from another thread ends the wait with `stopped`.
    pub fn start(&self, port: u16) -> McpServerStatus {
        let epoch = {
            let mut inner = self.lock();
            if inner.closed {
                // The app is exiting: spawn nothing.
                return McpServerStatus::Stopped;
            }
            inner.shutdown();
            inner.failure = None;
            inner.port = port;
            if !is_valid_mcp_port(port) {
                inner.fail(McpFailure::InvalidPort);
                return inner.status;
            }
            // `unavailable` is never stored: a stopped server re-checks the sidecar in `status`.
            if !self.spawner.available() {
                inner.status = McpServerStatus::Stopped;
                return McpServerStatus::Unavailable;
            }
            if !self.spawner.port_free(port) {
                inner.fail(McpFailure::PortInUse);
                return inner.status;
            }
            let Ok(token) = generate_token() else {
                inner.fail(McpFailure::TokenUnavailable);
                return inner.status;
            };
            let request = SpawnRequest {
                environment: child_environment(
                    &self.source_environment,
                    port,
                    &token,
                    self.database_path.as_deref(),
                ),
            };
            match self.spawner.spawn(request) {
                Ok(child) => {
                    inner.child = Some(child);
                    inner.token = Some(token);
                    inner.status = McpServerStatus::Starting;
                }
                Err(SpawnError::Unavailable) => {
                    inner.status = McpServerStatus::Stopped;
                    return McpServerStatus::Unavailable;
                }
                Err(SpawnError::Failed) => {
                    inner.fail(McpFailure::SpawnFailed);
                    return inner.status;
                }
            }
            inner.epoch
        };
        self.wait_until_ready(epoch, port)
    }

    /// Polls the child and the port until the start is decided.
    fn wait_until_ready(&self, epoch: u64, port: u16) -> McpServerStatus {
        let started = Instant::now();
        loop {
            if let Some(status) = self.check_child(epoch) {
                return status;
            }
            if accepts_connection(port) {
                std::thread::sleep(SETTLE_TIME);
                let mut inner = self.lock();
                if inner.epoch != epoch {
                    return inner.status;
                }
                if inner.child.as_mut().is_some_and(|child| child.is_running()) {
                    inner.status = McpServerStatus::Running;
                } else {
                    inner.fail(McpFailure::ExitedEarly);
                }
                return inner.status;
            }
            if started.elapsed() >= self.ready_deadline {
                let mut inner = self.lock();
                if inner.epoch == epoch {
                    inner.fail(McpFailure::NotReachable);
                }
                return inner.status;
            }
            std::thread::sleep(PROBE_INTERVAL);
        }
    }

    /// `Some(status)` when the waiting start is decided: it was replaced or stopped (the current
    /// status), or the child exited (`failed`). `None` while the child is alive.
    fn check_child(&self, epoch: u64) -> Option<McpServerStatus> {
        let mut inner = self.lock();
        if inner.epoch != epoch {
            return Some(inner.status);
        }
        if inner.child.as_mut().is_some_and(|child| child.is_running()) {
            return None;
        }
        inner.fail(McpFailure::ExitedEarly);
        Some(inner.status)
    }

    /// Re-checks a running child. A child that died after a successful start becomes `failed`.
    /// Returns the status.
    pub fn refresh(&self) -> McpServerStatus {
        {
            let mut inner = self.lock();
            if inner.status == McpServerStatus::Running
                && !inner.child.as_mut().is_some_and(|child| child.is_running())
            {
                inner.fail(McpFailure::ExitedEarly);
            }
        }
        self.status()
    }

    /// Kills and reaps the child and returns to `stopped`. Idempotent: stopping a stopped (or
    /// failed, or unavailable-but-stopped) supervisor only clears the failure.
    pub fn stop(&self) {
        let mut inner = self.lock();
        inner.shutdown();
        inner.failure = None;
        inner.status = McpServerStatus::Stopped;
    }

    /// Like [`stop`](Self::stop), and every later [`start`](Self::start) spawns nothing. For the
    /// window close and the app exit, where a start that has not reached the lock yet must not
    /// leave a child behind. A plain `stop` (the toggle) still allows later starts.
    pub fn close(&self) {
        let mut inner = self.lock();
        inner.closed = true;
        inner.shutdown();
        inner.failure = None;
        inner.status = McpServerStatus::Stopped;
    }
}

impl Drop for McpSupervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    const SHORT_DEADLINE: Duration = Duration::from_millis(400);

    /// The token rule of the listener (`LoopbackHttpConfig::new`): 43 to 128 of `[A-Za-z0-9_-]`.
    fn is_valid_listener_token(token: &str) -> bool {
        (43..=128).contains(&token.len())
            && token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    }

    struct FakeChild {
        alive: Arc<AtomicBool>,
        terminated: Arc<AtomicUsize>,
    }

    impl McpChild for FakeChild {
        fn is_running(&mut self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }

        fn terminate(&mut self) {
            self.alive.store(false, Ordering::SeqCst);
            self.terminated.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A spawner whose child is alive or already dead, and that records every request.
    struct FakeSpawner {
        available: bool,
        /// Flips a missing sidecar to present, as if it was installed later.
        appeared: AtomicBool,
        /// Simulates a foreign listener: the pre-spawn bind fails.
        port_busy: bool,
        spawn_error: Option<SpawnError>,
        child_alive: bool,
        requests: Mutex<Vec<Vec<(OsString, OsString)>>>,
        terminated: Arc<AtomicUsize>,
    }

    impl FakeSpawner {
        fn new() -> Self {
            Self {
                available: true,
                appeared: AtomicBool::new(false),
                port_busy: false,
                spawn_error: None,
                child_alive: true,
                requests: Mutex::new(Vec::new()),
                terminated: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn last_value(&self, name: &str) -> Option<String> {
            let requests = self.requests.lock().ok()?;
            let last = requests.last()?;
            last.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.to_string_lossy().into_owned())
        }
    }

    impl Spawner for Arc<FakeSpawner> {
        fn available(&self) -> bool {
            self.available || self.appeared.load(Ordering::SeqCst)
        }

        fn port_free(&self, _port: u16) -> bool {
            !self.port_busy
        }

        fn spawn(&self, request: SpawnRequest) -> Result<Box<dyn McpChild>, SpawnError> {
            if let Some(error) = self.spawn_error {
                return Err(error);
            }
            if let Ok(mut requests) = self.requests.lock() {
                requests.push(request.environment);
            }
            Ok(Box::new(FakeChild {
                alive: Arc::new(AtomicBool::new(self.child_alive)),
                terminated: Arc::clone(&self.terminated),
            }))
        }
    }

    fn supervisor(spawner: &Arc<FakeSpawner>) -> McpSupervisor {
        let source = vec![
            (
                OsString::from("JOBNIAC_DB_PATH"),
                OsString::from("/srv/jobniac-synthetic/jobniac.sqlite3"),
            ),
            (
                OsString::from("JOBNIAC_HTTP_BEARER_TOKEN"),
                OsString::from("inherited-token"),
            ),
            (OsString::from("JOBNIAC_HTTP_PORT"), OsString::from("1111")),
            (
                OsString::from("jobniac_http_extra"),
                OsString::from("inherited"),
            ),
            (
                OsString::from("JOBNIAC_INTERNAL_WORKER"),
                OsString::from("pdf"),
            ),
            (
                OsString::from("jobniac_internal_parent_pipe"),
                OsString::from("0"),
            ),
            (OsString::from("PATH"), OsString::from("/usr/bin")),
            (OsString::from("HTTPS_PROXY"), OsString::from("proxy.test")),
            (OsString::from("UNRELATED_SECRET"), OsString::from("x")),
        ];
        McpSupervisor::new(Arc::new(Arc::clone(spawner)), source)
            .with_ready_deadline(SHORT_DEADLINE)
    }

    /// A loopback listener that accepts connections for the length of a test.
    fn open_port() -> Result<(TcpListener, u16), std::io::Error> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        Ok((listener, port))
    }

    /// A port that nothing listens on.
    fn closed_port() -> Result<u16, std::io::Error> {
        let (listener, port) = open_port()?;
        drop(listener);
        Ok(port)
    }

    #[test]
    fn a_live_child_that_accepts_connections_is_running() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert_eq!(supervisor.status(), McpServerStatus::Running);
        assert_eq!(supervisor.failure(), None);
        let connection = supervisor.connection().map(|c| (c.endpoint.clone(), c));
        let Some((endpoint, connection)) = connection else {
            return Err(std::io::Error::other("no connection while running"));
        };
        assert_eq!(endpoint, format!("http://127.0.0.1:{port}/mcp"));
        assert_eq!(
            Some(connection.token.expose_secret().to_owned()),
            spawner.last_value(HTTP_TOKEN_VARIABLE)
        );
        Ok(())
    }

    #[test]
    fn a_child_that_exits_early_is_failed_and_reaped() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner {
            child_alive: false,
            ..FakeSpawner::new()
        });
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.start(port), McpServerStatus::Failed);
        assert_eq!(supervisor.failure(), Some(McpFailure::ExitedEarly));
        assert!(supervisor.connection().is_none());
        assert_eq!(spawner.terminated.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn a_child_that_never_opens_the_port_is_failed_after_the_deadline() -> Result<(), std::io::Error>
    {
        let port = closed_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor(&spawner);
        let started = Instant::now();
        assert_eq!(supervisor.start(port), McpServerStatus::Failed);
        assert!(started.elapsed() >= SHORT_DEADLINE);
        assert_eq!(supervisor.failure(), Some(McpFailure::NotReachable));
        assert_eq!(failure_code(&supervisor), Some("not_reachable"));
        // The stuck child is killed, not left running.
        assert_eq!(spawner.terminated.load(Ordering::SeqCst), 1);
        Ok(())
    }

    fn failure_code(supervisor: &McpSupervisor) -> Option<&'static str> {
        supervisor.failure().map(McpFailure::code)
    }

    #[test]
    fn stop_kills_the_child_and_is_idempotent() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        supervisor.stop();
        assert_eq!(supervisor.status(), McpServerStatus::Stopped);
        assert!(supervisor.connection().is_none());
        assert_eq!(spawner.terminated.load(Ordering::SeqCst), 1);
        supervisor.stop();
        supervisor.stop();
        assert_eq!(supervisor.status(), McpServerStatus::Stopped);
        assert_eq!(spawner.terminated.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn stopping_a_supervisor_that_never_started_is_a_no_op() {
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor(&spawner);
        supervisor.stop();
        assert_eq!(supervisor.status(), McpServerStatus::Stopped);
        assert_eq!(spawner.terminated.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_missing_sidecar_is_unavailable_before_and_after_a_start() {
        let spawner = Arc::new(FakeSpawner {
            available: false,
            ..FakeSpawner::new()
        });
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.status(), McpServerStatus::Unavailable);
        assert_eq!(supervisor.start(8765), McpServerStatus::Unavailable);
        assert_eq!(supervisor.status(), McpServerStatus::Unavailable);
        assert!(
            spawner
                .requests
                .lock()
                .is_ok_and(|requests| requests.is_empty())
        );
    }

    #[test]
    fn a_busy_port_is_failed_and_spawns_nothing() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner {
            port_busy: true,
            ..FakeSpawner::new()
        });
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.start(port), McpServerStatus::Failed);
        assert_eq!(supervisor.failure(), Some(McpFailure::PortInUse));
        assert_eq!(failure_code(&supervisor), Some("port_in_use"));
        assert!(supervisor.connection().is_none());
        assert!(
            spawner
                .requests
                .lock()
                .is_ok_and(|requests| requests.is_empty())
        );
        Ok(())
    }

    #[test]
    fn the_default_port_check_sees_a_real_listener() -> Result<(), std::io::Error> {
        let (listener, port) = open_port()?;
        let spawner = ProcessSpawner::with_sidecar(None);
        assert!(!spawner.port_free(port));
        drop(listener);
        assert!(spawner.port_free(port));
        Ok(())
    }

    #[test]
    fn a_start_after_close_spawns_nothing_but_after_stop_it_works() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor(&spawner);
        supervisor.stop();
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        supervisor.close();
        assert_eq!(supervisor.status(), McpServerStatus::Stopped);
        assert_eq!(spawner.terminated.load(Ordering::SeqCst), 1);
        assert_eq!(supervisor.start(port), McpServerStatus::Stopped);
        assert_eq!(supervisor.status(), McpServerStatus::Stopped);
        assert!(supervisor.connection().is_none());
        assert!(
            spawner
                .requests
                .lock()
                .is_ok_and(|requests| requests.len() == 1)
        );
        Ok(())
    }

    #[test]
    fn a_sidecar_that_appears_later_is_picked_up() {
        let spawner = Arc::new(FakeSpawner {
            available: false,
            ..FakeSpawner::new()
        });
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.start(8765), McpServerStatus::Unavailable);
        assert_eq!(supervisor.status(), McpServerStatus::Unavailable);
        spawner.appeared.store(true, Ordering::SeqCst);
        assert_eq!(supervisor.status(), McpServerStatus::Stopped);
        assert_eq!(supervisor.refresh(), McpServerStatus::Stopped);
    }

    #[test]
    fn a_spawner_that_reports_the_sidecar_missing_is_unavailable() {
        let spawner = Arc::new(FakeSpawner {
            spawn_error: Some(SpawnError::Unavailable),
            ..FakeSpawner::new()
        });
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.start(8765), McpServerStatus::Unavailable);
    }

    #[test]
    fn a_process_that_cannot_be_created_is_failed() {
        let spawner = Arc::new(FakeSpawner {
            spawn_error: Some(SpawnError::Failed),
            ..FakeSpawner::new()
        });
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.start(8765), McpServerStatus::Failed);
        assert_eq!(supervisor.failure(), Some(McpFailure::SpawnFailed));
    }

    #[test]
    fn an_invalid_port_never_spawns() {
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.start(80), McpServerStatus::Failed);
        assert_eq!(supervisor.failure(), Some(McpFailure::InvalidPort));
        assert!(
            spawner
                .requests
                .lock()
                .is_ok_and(|requests| requests.is_empty())
        );
    }

    #[test]
    fn the_real_spawner_with_no_sidecar_is_unavailable() {
        let spawner = ProcessSpawner::with_sidecar(None);
        assert!(!spawner.available());
        assert_eq!(
            spawner
                .spawn(SpawnRequest {
                    environment: Vec::new()
                })
                .err(),
            Some(SpawnError::Unavailable)
        );
        let missing = ProcessSpawner::with_sidecar(Some(PathBuf::from("no-such-sidecar")));
        assert!(!missing.available());
    }

    #[test]
    fn the_child_gets_the_database_the_app_resolved_without_home_variables()
    -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        // The app resolved the per-user default (no JOBNIAC_DB_PATH) from its own HOME.
        let source = vec![
            (OsString::from("HOME"), OsString::from("/home/synthetic")),
            (
                OsString::from("XDG_DATA_HOME"),
                OsString::from("/home/synthetic/data"),
            ),
            (OsString::from("PATH"), OsString::from("/usr/bin")),
        ];
        let resolved = PathBuf::from("/home/synthetic/data/jobniac/jobniac.sqlite3");
        let supervisor = McpSupervisor::new(Arc::new(Arc::clone(&spawner)), source)
            .with_ready_deadline(SHORT_DEADLINE)
            .with_database_path(Some(resolved.clone()));
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert_eq!(
            spawner.last_value("JOBNIAC_DB_PATH"),
            Some(resolved.to_string_lossy().into_owned())
        );
        assert_eq!(spawner.last_value("HOME"), None);
        assert_eq!(spawner.last_value("XDG_DATA_HOME"), None);
        Ok(())
    }

    #[test]
    fn the_resolved_database_replaces_an_inherited_override() {
        let source = vec![
            (OsString::from("jobniac_db_path"), OsString::from("/old/db")),
            (
                OsString::from("JOBNIAC_DB_PATH"),
                OsString::from("/older/db"),
            ),
        ];
        let token = SecretString::from("t".repeat(64));
        let environment = child_environment(&source, 4000, &token, Some(Path::new("/new/db")));
        let values: Vec<&OsString> = environment
            .iter()
            .filter(|(name, _)| {
                name.to_string_lossy()
                    .eq_ignore_ascii_case("JOBNIAC_DB_PATH")
            })
            .map(|(_, value)| value)
            .collect();
        assert_eq!(values, [&OsString::from("/new/db")]);
        // Without a resolved path the inherited value is left as it is.
        let kept = child_environment(&source, 4000, &token, None);
        assert_eq!(
            kept.iter()
                .filter(|(name, _)| name
                    .to_string_lossy()
                    .eq_ignore_ascii_case("JOBNIAC_DB_PATH"))
                .count(),
            2
        );
    }

    #[test]
    fn the_child_environment_has_the_port_and_a_valid_token_and_no_inherited_http_values()
    -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        let requests = spawner
            .requests
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?;
        let Some(environment) = requests.last() else {
            return Err(std::io::Error::other("no spawn request"));
        };
        let names: Vec<String> = environment
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        // Exactly one port and one token variable, and they are the supervisor's.
        assert_eq!(
            names
                .iter()
                .filter(|name| *name == HTTP_PORT_VARIABLE)
                .count(),
            1
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| *name == HTTP_TOKEN_VARIABLE)
                .count(),
            1
        );
        let value = |name: &str| {
            environment
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.to_string_lossy().into_owned())
        };
        assert_eq!(value(HTTP_PORT_VARIABLE), Some(port.to_string()));
        let token = value(HTTP_TOKEN_VARIABLE).unwrap_or_default();
        assert!(is_valid_listener_token(&token));
        assert_ne!(token, "inherited-token");
        // No other inherited JOBNIAC_HTTP_* variable, in any case.
        assert!(
            !names
                .iter()
                .any(|name| name.eq_ignore_ascii_case("JOBNIAC_HTTP_EXTRA"))
        );
        // Database settings and the minimal OS variables cross; the rest does not.
        assert_eq!(
            value("JOBNIAC_DB_PATH"),
            Some("/srv/jobniac-synthetic/jobniac.sqlite3".to_owned())
        );
        assert_eq!(value("PATH"), Some("/usr/bin".to_owned()));
        assert_eq!(value("HTTPS_PROXY"), None);
        assert_eq!(value("UNRELATED_SECRET"), None);
        assert_eq!(value(WORKER_MARKER_VARIABLE), None);
        // The parent-pipe marker is the supervisor's: exactly one, exactly `1`, set last.
        assert_eq!(
            names
                .iter()
                .filter(|name| name.eq_ignore_ascii_case(PARENT_PIPE_VARIABLE))
                .count(),
            1
        );
        assert_eq!(value(PARENT_PIPE_VARIABLE), Some("1".to_owned()));
        assert_eq!(
            environment
                .last()
                .map(|(name, _)| name.to_string_lossy().into_owned()),
            Some(PARENT_PIPE_VARIABLE.to_owned())
        );
        Ok(())
    }

    #[test]
    fn the_token_is_never_an_argument_and_is_redacted_in_debug_output() -> Result<(), std::io::Error>
    {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor(&spawner);
        supervisor.start(port);
        let token = spawner.last_value(HTTP_TOKEN_VARIABLE).unwrap_or_default();
        assert!(!token.is_empty());
        assert!(!format!("{supervisor:?}").contains(&token));
        let connection = supervisor
            .connection()
            .ok_or_else(|| std::io::Error::other("not running"))?;
        assert!(!format!("{connection:?}").contains(&token));
        let request = SpawnRequest {
            environment: vec![("A".into(), token.clone().into())],
        };
        assert!(!format!("{request:?}").contains(&token));
        Ok(())
    }

    #[test]
    fn every_start_makes_a_valid_and_different_token() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor(&spawner);
        let mut tokens = Vec::new();
        for _ in 0..3 {
            assert_eq!(supervisor.start(port), McpServerStatus::Running);
            tokens.push(spawner.last_value(HTTP_TOKEN_VARIABLE).unwrap_or_default());
        }
        for token in &tokens {
            assert!(is_valid_listener_token(token), "invalid token");
        }
        tokens.sort();
        tokens.dedup();
        assert_eq!(tokens.len(), 3);
        // A restart killed the previous child each time.
        assert_eq!(spawner.terminated.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[test]
    fn generated_tokens_are_valid_unique_and_hex() -> Result<(), TokenError> {
        let first = generate_token()?;
        let second = generate_token()?;
        assert_eq!(first.expose_secret().len(), 64);
        assert!(is_valid_listener_token(first.expose_secret()));
        assert_ne!(first.expose_secret(), second.expose_secret());
        Ok(())
    }

    #[test]
    fn a_stop_during_the_wait_ends_the_start_as_stopped() -> Result<(), std::io::Error> {
        let port = closed_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor =
            Arc::new(supervisor(&spawner).with_ready_deadline(Duration::from_secs(10)));
        let waiting = {
            let supervisor = Arc::clone(&supervisor);
            std::thread::spawn(move || supervisor.start(port))
        };
        while supervisor.lock().status != McpServerStatus::Starting {
            std::thread::sleep(Duration::from_millis(5));
        }
        supervisor.stop();
        let status = waiting
            .join()
            .map_err(|_| std::io::Error::other("start panicked"))?;
        assert_eq!(status, McpServerStatus::Stopped);
        assert_eq!(supervisor.status(), McpServerStatus::Stopped);
        Ok(())
    }

    #[test]
    fn a_child_that_dies_after_starting_is_failed_on_refresh() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert_eq!(supervisor.refresh(), McpServerStatus::Running);
        // Kill the child behind the supervisor's back.
        if let Some(child) = supervisor.lock().child.as_mut() {
            child.terminate();
        }
        assert_eq!(supervisor.refresh(), McpServerStatus::Failed);
        assert_eq!(supervisor.failure(), Some(McpFailure::ExitedEarly));
        Ok(())
    }
}
