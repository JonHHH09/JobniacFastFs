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
//! - **Grants.** The capabilities that delete data or reach the network are not part of the source
//!   environment. They are *grants*: variable pairs the supervisor holds and adds to the environment
//!   of every child it spawns, read under the same lock as the spawn
//!   ([`McpSupervisor::with_grants`]). Grants only ever shrink: [`McpSupervisor::retain_grants`]
//!   drops the grants that are no longer allowed and, in the same critical section, kills a live
//!   child that was started with them. A start that was still on its way to the lock spawns with the
//!   narrowed grants, and a later start, port change or restart can never bring a revoked grant
//!   back.
//! - **Termination.** Killing a child is confirmed, not assumed: [`McpChild::terminate`] reports
//!   whether the child is gone, and waits at most [`TERMINATE_WAIT`] for it (the lock is held
//!   meanwhile, so the wait is bounded). A child that cannot be confirmed gone is kept as the
//!   *survivor*, the status is `failed` with [`McpFailure::StopFailed`] (never `stopped`), a
//!   revocation answers [`Revocation::StopFailed`], and no new child is spawned until the survivor is
//!   confirmed gone (every start, stop and refresh tries again), so a second process never competes
//!   for the port or starts beside it. Closing the stdin pipe also tells the child to exit by
//!   itself, so a survivor normally disappears on its own.
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
    /// A child could not be confirmed gone after it was told to stop: it may still be running, so
    /// no new child is started until it is confirmed gone.
    StopFailed,
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
            Self::StopFailed => "stop_failed",
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

/// What a termination established about a child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Termination {
    /// The child exited and was reaped.
    Gone,
    /// The child could not be confirmed gone: the kill failed or it did not exit within the bound.
    MayBeAlive,
}

/// What a revocation of grants did to the running server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Revocation {
    /// No child was involved: the grants were narrowed for the next start.
    Unaffected,
    /// A child that held a revoked grant was stopped and confirmed gone.
    Stopped,
    /// A child that may hold a revoked grant could not be confirmed gone. The grants are narrowed,
    /// no new child starts until it is, and the status is `failed` (`stop_failed`).
    StopFailed,
}

/// A started child process.
pub trait McpChild: Send {
    /// Whether the child is still alive (it has not exited).
    fn is_running(&mut self) -> bool;

    /// Kills the child and waits for it, for a bounded time. Safe to call on a child that already
    /// exited, and safe to call again after [`Termination::MayBeAlive`].
    fn terminate(&mut self) -> Termination;
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

    fn terminate(&mut self) -> Termination {
        // The pipe closing tells the child to exit by itself, even if the kill below fails.
        drop(self.parent_pipe.take());
        // An error from the kill is not fatal and carries nothing worth reporting (the text could
        // name a path): whether the child is gone is decided by the bounded wait that follows.
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            drop(self.child.kill());
        }
        let child = &mut self.child;
        wait_for_exit(
            || {
                child
                    .try_wait()
                    .map(|status| status.is_some())
                    .map_err(|_| ())
            },
            TERMINATE_WAIT,
            TERMINATE_POLL,
        )
    }
}

/// How long a termination waits for the child to be gone. The supervisor holds its lock meanwhile.
pub const TERMINATE_WAIT: Duration = Duration::from_secs(2);

/// Pause between two checks of a terminating child.
const TERMINATE_POLL: Duration = Duration::from_millis(10);

/// Polls `exited` until it reports `Ok(true)` ([`Termination::Gone`]), or `wait` has passed, or it
/// fails ([`Termination::MayBeAlive`]). It never waits longer than `wait`.
fn wait_for_exit(
    mut exited: impl FnMut() -> Result<bool, ()>,
    wait: Duration,
    poll: Duration,
) -> Termination {
    let deadline = Instant::now() + wait;
    loop {
        match exited() {
            Ok(true) => return Termination::Gone,
            Ok(false) => {}
            Err(()) => return Termination::MayBeAlive,
        }
        if Instant::now() >= deadline {
            return Termination::MayBeAlive;
        }
        std::thread::sleep(poll);
    }
}

impl Drop for ProcessChild {
    fn drop(&mut self) {
        // The last attempt, bounded like every other: nobody is left to retry or to be told, and
        // the closed stdin pipe has already asked the child to exit by itself.
        let _ = self.terminate();
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
pub fn child_environment<'a>(
    source: impl IntoIterator<Item = &'a (OsString, OsString)>,
    port: u16,
    token: &SecretString,
    database: Option<&Path>,
) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = source
        .into_iter()
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
    /// The capability variables every child is spawned with, on top of the source environment.
    /// Only [`McpSupervisor::retain_grants`] changes them, and only by removing pairs.
    grants: Vec<(OsString, OsString)>,
    /// A child that was told to stop and could not be confirmed gone. While it is here no new child
    /// is spawned (there is at most one, and then `child` is `None`).
    survivor: Option<Box<dyn McpChild>>,
}

impl Inner {
    /// Tries again to confirm the survivor gone. `true` when there is none left.
    fn reap_survivor(&mut self) -> bool {
        let Some(mut survivor) = self.survivor.take() else {
            return true;
        };
        if survivor.terminate() == Termination::Gone {
            return true;
        }
        self.survivor = Some(survivor);
        false
    }

    /// Kills the child, drops the token and invalidates every waiting start. Returns `true` when no
    /// child is left alive for all anyone can tell; `false` when one could not be confirmed gone
    /// (it is then kept as the survivor and the caller must not report `stopped`).
    fn shutdown(&mut self) -> bool {
        self.token = None;
        self.epoch += 1;
        let mut gone = self.reap_survivor();
        if let Some(mut child) = self.child.take()
            && child.terminate() == Termination::MayBeAlive
        {
            // No start spawns while a survivor is kept, so there is no second one to replace.
            self.survivor.get_or_insert(child);
            gone = false;
        }
        gone
    }

    /// The state after a stop: `stopped`, or `failed` with `stop_failed` when a child may be alive.
    fn settle_after_stop(&mut self, gone: bool) {
        if gone {
            self.failure = None;
            self.status = McpServerStatus::Stopped;
        } else {
            self.failure = Some(McpFailure::StopFailed);
            self.status = McpServerStatus::Failed;
        }
    }

    fn fail(&mut self, failure: McpFailure) {
        let gone = self.shutdown();
        self.status = McpServerStatus::Failed;
        // A child that may still run is the fact that matters most.
        self.failure = Some(if gone {
            failure
        } else {
            McpFailure::StopFailed
        });
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
                grants: Vec::new(),
                survivor: None,
            }),
        }
    }

    /// The capability variables the children are spawned with, on top of the source environment.
    /// Called once, before the first start; later the grants can only be narrowed.
    #[must_use]
    pub fn with_grants(self, grants: Vec<(OsString, OsString)>) -> Self {
        self.lock().grants = grants;
        self
    }

    /// Keeps only the grants for which `keep` is true (a grant that is not held is never added), so
    /// no secret value is copied to decide. When a grant was dropped and a child is alive or
    /// starting, the child is killed in the same critical section and the status becomes `stopped`
    /// ([`Revocation::Stopped`]). When that child cannot be confirmed gone the answer is
    /// [`Revocation::StopFailed`] and the status is `failed`: the caller must not report success,
    /// and no child starts until the survivor is confirmed gone. When a child was stopped the
    /// caller restarts the server if it is meant to run, and the new child gets the narrowed grants.
    ///
    /// A survivor is checked on **every** call, whether or not this call narrowed anything: a call
    /// that narrows nothing new (a capability that was never granted, or one an earlier call already
    /// revoked) must not turn a child that may still serve the capability into a success. While the
    /// survivor cannot be confirmed gone every call answers `StopFailed`; when it can, the call
    /// answers `Stopped` once (so the caller restarts the server if it is meant to run).
    ///
    /// This blocks for up to [`TERMINATE_WAIT`] per child that must be stopped: async callers use a
    /// blocking thread.
    pub fn retain_grants(&self, keep: impl Fn(&(OsString, OsString)) -> bool) -> Revocation {
        let mut inner = self.lock();
        let before = inner.grants.len();
        inner.grants.retain(|grant| keep(grant));
        let narrowed = inner.grants.len() != before;
        if inner.survivor.is_some() || (narrowed && inner.child.is_some()) {
            let gone = inner.shutdown();
            inner.settle_after_stop(gone);
            return if gone {
                Revocation::Stopped
            } else {
                Revocation::StopFailed
            };
        }
        Revocation::Unaffected
    }

    /// [`retain_grants`](Self::retain_grants) with the grants that are in `allowed`.
    pub fn restrict_grants(&self, allowed: &[(OsString, OsString)]) -> Revocation {
        self.retain_grants(|grant| allowed.contains(grant))
    }

    /// Whether the supervisor currently grants `name=value` to the children it spawns.
    #[must_use]
    pub fn has_grant(&self, name: &str, value: &str) -> bool {
        self.lock()
            .grants
            .iter()
            .any(|(grant, grant_value)| grant == name && grant_value == value)
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
            if !inner.shutdown() {
                // A child that may still run (or an earlier survivor) could not be confirmed
                // gone: spawn nothing beside it.
                inner.settle_after_stop(false);
                return inner.status;
            }
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
            // The grants are read here, under the lock of the spawn: a revocation either ran before
            // (this child never gets the grant) or runs after (it kills this child).
            let request = SpawnRequest {
                environment: child_environment(
                    self.source_environment.iter().chain(inner.grants.iter()),
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
            // A survivor that exited since is noticed here, so a failed stop heals by itself.
            if inner.survivor.is_some()
                && inner.reap_survivor()
                && inner.failure == Some(McpFailure::StopFailed)
            {
                inner.settle_after_stop(true);
            }
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
    ///
    /// When a child cannot be confirmed gone the status is `failed` (`stop_failed`), not `stopped`:
    /// another stop, a start or a refresh tries again.
    pub fn stop(&self) {
        let mut inner = self.lock();
        let gone = inner.shutdown();
        inner.settle_after_stop(gone);
    }

    /// Like [`stop`](Self::stop), and every later [`start`](Self::start) spawns nothing. For the
    /// window close and the app exit, where a start that has not reached the lock yet must not
    /// leave a child behind. A plain `stop` (the toggle) still allows later starts.
    pub fn close(&self) {
        let mut inner = self.lock();
        inner.closed = true;
        let gone = inner.shutdown();
        inner.settle_after_stop(gone);
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
        /// While set, the child cannot be confirmed gone (the kill "fails"): it stays alive.
        stubborn: Arc<AtomicBool>,
    }

    impl McpChild for FakeChild {
        fn is_running(&mut self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }

        fn terminate(&mut self) -> Termination {
            self.terminated.fetch_add(1, Ordering::SeqCst);
            if self.stubborn.load(Ordering::SeqCst) {
                return Termination::MayBeAlive;
            }
            self.alive.store(false, Ordering::SeqCst);
            Termination::Gone
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
        stubborn: Arc<AtomicBool>,
    }

    impl FakeSpawner {
        fn new() -> Self {
            Self {
                stubborn: Arc::new(AtomicBool::new(false)),
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
                stubborn: Arc::clone(&self.stubborn),
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

    const GRANT_DELETE: &str = "JOBNIAC_DELETION_ENABLED";
    const GRANT_LINK: &str = "JOBNIAC_LINK_FETCH_ENABLED";

    fn grant(name: &str) -> (OsString, OsString) {
        (OsString::from(name), OsString::from("1"))
    }

    fn supervisor_with_grants(spawner: &Arc<FakeSpawner>, grants: &[&str]) -> McpSupervisor {
        supervisor(spawner).with_grants(grants.iter().map(|name| grant(name)).collect())
    }

    fn last_has(spawner: &FakeSpawner, name: &str) -> bool {
        spawner.last_value(name).is_some()
    }

    #[test]
    fn grants_reach_every_child_the_supervisor_spawns() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor_with_grants(&spawner, &[GRANT_DELETE]);
        assert!(supervisor.has_grant(GRANT_DELETE, "1"));
        assert!(!supervisor.has_grant(GRANT_LINK, "1"));
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert_eq!(spawner.last_value(GRANT_DELETE).as_deref(), Some("1"));
        assert!(!last_has(&spawner, GRANT_LINK));
        // A stop and a start, and a start on another port, keep the grants that are still held.
        supervisor.stop();
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert_eq!(spawner.last_value(GRANT_DELETE).as_deref(), Some("1"));
        Ok(())
    }

    #[test]
    fn no_grant_means_no_flag_in_the_child() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert!(!last_has(&spawner, GRANT_DELETE));
        assert!(!last_has(&spawner, GRANT_LINK));
        Ok(())
    }

    #[test]
    fn revoking_a_grant_kills_the_live_child_and_no_later_start_brings_it_back()
    -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor_with_grants(&spawner, &[GRANT_DELETE, GRANT_LINK]);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        let old_token = supervisor
            .connection()
            .map(|c| c.token.expose_secret().to_owned());
        assert_eq!(spawner.terminated.load(Ordering::SeqCst), 0);

        assert_eq!(
            supervisor.restrict_grants(&[grant(GRANT_LINK)]),
            Revocation::Stopped,
            "a live child held the revoked grant"
        );
        // The child holding the grant is gone before the call returns, and nothing runs now.
        assert_eq!(spawner.terminated.load(Ordering::SeqCst), 1);
        assert_eq!(supervisor.status(), McpServerStatus::Stopped);
        assert!(supervisor.connection().is_none());
        assert!(!supervisor.has_grant(GRANT_DELETE, "1"));
        assert!(supervisor.has_grant(GRANT_LINK, "1"));

        // Start, stop and start, and a port change: none of them reuses the revoked grant.
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert!(!last_has(&spawner, GRANT_DELETE));
        assert_eq!(spawner.last_value(GRANT_LINK).as_deref(), Some("1"));
        supervisor.stop();
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert!(!last_has(&spawner, GRANT_DELETE));
        let (_other, other_port) = open_port()?;
        assert_eq!(supervisor.start(other_port), McpServerStatus::Running);
        assert!(!last_has(&spawner, GRANT_DELETE));
        // The new child has a new token.
        let new_token = supervisor
            .connection()
            .map(|c| c.token.expose_secret().to_owned());
        assert_ne!(old_token, new_token);
        Ok(())
    }

    #[test]
    fn a_restriction_that_drops_nothing_leaves_the_child_alone() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor_with_grants(&spawner, &[GRANT_DELETE]);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert_eq!(
            supervisor.restrict_grants(&[grant(GRANT_DELETE), grant(GRANT_LINK)]),
            Revocation::Unaffected
        );
        assert_eq!(spawner.terminated.load(Ordering::SeqCst), 0);
        assert_eq!(supervisor.status(), McpServerStatus::Running);
        Ok(())
    }

    #[test]
    fn a_restriction_can_never_add_a_grant() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor_with_grants(&spawner, &[GRANT_DELETE]);
        assert_eq!(
            supervisor.restrict_grants(&[grant(GRANT_DELETE), grant(GRANT_LINK)]),
            Revocation::Unaffected
        );
        assert!(!supervisor.has_grant(GRANT_LINK, "1"));
        // Revoked once, not granted again by a wider allow list.
        assert_eq!(supervisor.restrict_grants(&[]), Revocation::Unaffected);
        assert_eq!(
            supervisor.restrict_grants(&[grant(GRANT_DELETE)]),
            Revocation::Unaffected
        );
        assert!(!supervisor.has_grant(GRANT_DELETE, "1"));
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert!(!last_has(&spawner, GRANT_DELETE));
        assert!(!last_has(&spawner, GRANT_LINK));
        Ok(())
    }

    #[test]
    fn a_revocation_without_a_child_changes_the_next_start() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor_with_grants(&spawner, &[GRANT_DELETE]);
        assert_eq!(
            supervisor.restrict_grants(&[]),
            Revocation::Unaffected,
            "no child was stopped"
        );
        assert_eq!(supervisor.status(), McpServerStatus::Stopped);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert!(!last_has(&spawner, GRANT_DELETE));
        Ok(())
    }

    #[test]
    fn a_failed_server_start_does_not_keep_a_revoked_grant_for_the_retry()
    -> Result<(), std::io::Error> {
        let port = closed_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor_with_grants(&spawner, &[GRANT_DELETE]);
        // Nothing listens: the start fails after the spawn. The child is already gone.
        assert_eq!(supervisor.start(port), McpServerStatus::Failed);
        assert_eq!(
            supervisor.restrict_grants(&[]),
            Revocation::Unaffected,
            "the failed child is already dead"
        );
        assert_eq!(supervisor.start(port), McpServerStatus::Failed);
        assert!(!last_has(&spawner, GRANT_DELETE));
        Ok(())
    }

    /// A spawner that parks inside `spawn` until released, to hold a start in flight.
    struct GatedSpawner {
        inner: Arc<FakeSpawner>,
        entered: Mutex<std::sync::mpsc::Sender<()>>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl Spawner for GatedSpawner {
        fn available(&self) -> bool {
            true
        }

        fn port_free(&self, _port: u16) -> bool {
            true
        }

        fn spawn(&self, request: SpawnRequest) -> Result<Box<dyn McpChild>, SpawnError> {
            if let Ok(entered) = self.entered.lock() {
                let _ = entered.send(());
            }
            if let Ok(release) = self.release.lock() {
                let _ = release.recv_timeout(Duration::from_secs(5));
            }
            Spawner::spawn(&self.inner, request)
        }
    }

    #[test]
    fn a_revocation_racing_a_start_that_is_spawning_kills_that_child() -> Result<(), std::io::Error>
    {
        let (_listener, port) = open_port()?;
        let inner = Arc::new(FakeSpawner::new());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let gated = GatedSpawner {
            inner: Arc::clone(&inner),
            entered: Mutex::new(entered_tx),
            release: Mutex::new(release_rx),
        };
        let supervisor = Arc::new(
            McpSupervisor::new(Arc::new(gated), Vec::new())
                .with_ready_deadline(Duration::from_secs(5))
                .with_grants(vec![grant(GRANT_DELETE)]),
        );
        let starter = {
            let supervisor = Arc::clone(&supervisor);
            std::thread::spawn(move || supervisor.start(port))
        };
        // The start holds the lock and has built its request with the grant.
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| std::io::Error::other("the start never reached the spawn"))?;
        let revoker = {
            let supervisor = Arc::clone(&supervisor);
            std::thread::spawn(move || supervisor.restrict_grants(&[]))
        };
        // The revocation waits for the lock; releasing the spawn lets both finish.
        release_tx
            .send(())
            .map_err(|_| std::io::Error::other("the spawner is gone"))?;
        let stopped = revoker
            .join()
            .map_err(|_| std::io::Error::other("the revoker panicked"))?;
        let status = starter
            .join()
            .map_err(|_| std::io::Error::other("the starter panicked"))?;
        // That child was spawned with the grant, so the revocation stopped it: nothing runs, and
        // the start does not report a server that holds the revoked grant.
        assert_eq!(stopped, Revocation::Stopped);
        assert_eq!(inner.terminated.load(Ordering::SeqCst), 1);
        assert_ne!(status, McpServerStatus::Running);
        assert_eq!(supervisor.status(), McpServerStatus::Stopped);
        assert!(supervisor.connection().is_none());
        assert_eq!(inner.last_value(GRANT_DELETE).as_deref(), Some("1"));
        // The next start has no grant (the gate is released ahead of it).
        release_tx
            .send(())
            .map_err(|_| std::io::Error::other("the spawner is gone"))?;
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert!(!last_has(&inner, GRANT_DELETE));
        Ok(())
    }

    #[test]
    fn a_revocation_that_wins_the_race_means_the_start_never_gets_the_grant()
    -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = Arc::new(supervisor_with_grants(&spawner, &[GRANT_DELETE]));
        let revoker = {
            let supervisor = Arc::clone(&supervisor);
            std::thread::spawn(move || supervisor.restrict_grants(&[]))
        };
        let revoked = revoker
            .join()
            .map_err(|_| std::io::Error::other("the revoker panicked"))?;
        assert_eq!(revoked, Revocation::Unaffected);
        let starter = {
            let supervisor = Arc::clone(&supervisor);
            std::thread::spawn(move || supervisor.start(port))
        };
        starter
            .join()
            .map_err(|_| std::io::Error::other("the starter panicked"))?;
        assert!(!last_has(&spawner, GRANT_DELETE));
        Ok(())
    }

    // ---- Termination that cannot be confirmed ------------------------------------------------

    /// A supervisor whose children refuse to be confirmed gone while `stubborn` is set.
    fn stubborn_supervisor(spawner: &Arc<FakeSpawner>, grants: &[&str]) -> McpSupervisor {
        supervisor_with_grants(spawner, grants)
    }

    #[test]
    fn a_child_that_cannot_be_confirmed_gone_is_a_failed_stop_never_a_stopped_server()
    -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = stubborn_supervisor(&spawner, &[GRANT_DELETE]);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        spawner.stubborn.store(true, Ordering::SeqCst);

        supervisor.stop();
        // The child may still run: the status says failed, with the fixed code, and no connection
        // details of the old server are kept.
        assert_eq!(supervisor.status(), McpServerStatus::Failed);
        assert_eq!(supervisor.failure(), Some(McpFailure::StopFailed));
        assert_eq!(
            supervisor.failure().map(McpFailure::code),
            Some("stop_failed")
        );
        assert!(supervisor.connection().is_none());

        // No second child is spawned beside it, whatever the caller asks for.
        let spawned = spawner.requests.lock().map_or(0, |requests| requests.len());
        assert_eq!(supervisor.start(port), McpServerStatus::Failed);
        assert_eq!(supervisor.failure(), Some(McpFailure::StopFailed));
        let other = closed_port()?;
        assert_eq!(supervisor.start(other), McpServerStatus::Failed);
        assert_eq!(
            spawner.requests.lock().map_or(0, |requests| requests.len()),
            spawned
        );

        // Once the child can be confirmed gone, the next start retries it and then spawns.
        spawner.stubborn.store(false, Ordering::SeqCst);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert_eq!(
            spawner.requests.lock().map_or(0, |requests| requests.len()),
            spawned + 1
        );
        Ok(())
    }

    #[test]
    fn a_revocation_that_cannot_stop_the_child_says_so_and_no_start_restores_the_grant()
    -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = stubborn_supervisor(&spawner, &[GRANT_DELETE, GRANT_LINK]);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        spawner.stubborn.store(true, Ordering::SeqCst);

        assert_eq!(
            supervisor.restrict_grants(&[grant(GRANT_LINK)]),
            Revocation::StopFailed
        );
        // The grant is gone for every later start; the status is not `stopped`.
        assert!(!supervisor.has_grant(GRANT_DELETE, "1"));
        assert_eq!(supervisor.status(), McpServerStatus::Failed);
        assert_eq!(supervisor.failure(), Some(McpFailure::StopFailed));
        let spawned = spawner.requests.lock().map_or(0, |requests| requests.len());
        assert_eq!(supervisor.start(port), McpServerStatus::Failed);
        assert_eq!(
            spawner.requests.lock().map_or(0, |requests| requests.len()),
            spawned
        );

        // A wider allow list adds nothing back and narrows nothing new, but the survivor may
        // still be alive, so it is not a success: it keeps answering `StopFailed`.
        assert_eq!(
            supervisor.restrict_grants(&[grant(GRANT_DELETE), grant(GRANT_LINK)]),
            Revocation::StopFailed
        );
        assert!(!supervisor.has_grant(GRANT_DELETE, "1"));

        // The survivor is confirmed gone: the next child has no revoked grant.
        spawner.stubborn.store(false, Ordering::SeqCst);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert!(!last_has(&spawner, GRANT_DELETE));
        assert_eq!(spawner.last_value(GRANT_LINK).as_deref(), Some("1"));
        Ok(())
    }

    #[test]
    fn a_second_revocation_retries_a_survivor_and_reports_the_result() -> Result<(), std::io::Error>
    {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = stubborn_supervisor(&spawner, &[GRANT_DELETE, GRANT_LINK]);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        spawner.stubborn.store(true, Ordering::SeqCst);
        assert_eq!(
            supervisor.restrict_grants(&[grant(GRANT_LINK)]),
            Revocation::StopFailed
        );
        // Still stubborn: revoking the other grant also fails, honestly.
        assert_eq!(supervisor.restrict_grants(&[]), Revocation::StopFailed);
        // Released: the next call finds the survivor gone and says so, once.
        spawner.stubborn.store(false, Ordering::SeqCst);
        assert_eq!(supervisor.restrict_grants(&[]), Revocation::Stopped);
        assert_eq!(supervisor.restrict_grants(&[]), Revocation::Unaffected);
        assert_eq!(supervisor.refresh(), McpServerStatus::Stopped);
        assert_eq!(supervisor.failure(), None);
        Ok(())
    }

    #[test]
    fn a_refresh_heals_a_failed_stop_once_the_survivor_is_gone() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = stubborn_supervisor(&spawner, &[]);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        spawner.stubborn.store(true, Ordering::SeqCst);
        supervisor.stop();
        assert_eq!(supervisor.refresh(), McpServerStatus::Failed);
        assert_eq!(supervisor.failure(), Some(McpFailure::StopFailed));
        spawner.stubborn.store(false, Ordering::SeqCst);
        assert_eq!(supervisor.refresh(), McpServerStatus::Stopped);
        assert_eq!(supervisor.failure(), None);
        Ok(())
    }

    #[test]
    fn an_early_exit_that_cannot_be_reaped_is_reported_as_a_failed_stop()
    -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner {
            child_alive: false,
            ..FakeSpawner::new()
        });
        spawner.stubborn.store(true, Ordering::SeqCst);
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.start(port), McpServerStatus::Failed);
        assert_eq!(supervisor.failure(), Some(McpFailure::StopFailed));
        Ok(())
    }

    #[test]
    fn closing_with_a_stubborn_child_is_a_failed_stop_and_still_closes()
    -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = supervisor(&spawner);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        spawner.stubborn.store(true, Ordering::SeqCst);
        supervisor.close();
        assert_eq!(supervisor.status(), McpServerStatus::Failed);
        assert_eq!(supervisor.failure(), Some(McpFailure::StopFailed));
        // After a close nothing spawns, stubborn child or not.
        spawner.stubborn.store(false, Ordering::SeqCst);
        let spawned = spawner.requests.lock().map_or(0, |requests| requests.len());
        assert_eq!(supervisor.start(port), McpServerStatus::Stopped);
        assert_eq!(
            spawner.requests.lock().map_or(0, |requests| requests.len()),
            spawned
        );
        Ok(())
    }

    #[test]
    fn the_wait_for_a_terminating_child_is_bounded_and_honest() {
        let quick = Duration::from_millis(1);
        // Gone at once, and gone after a few checks.
        assert_eq!(
            wait_for_exit(|| Ok(true), Duration::from_secs(1), quick),
            Termination::Gone
        );
        let mut checks = 0;
        assert_eq!(
            wait_for_exit(
                || {
                    checks += 1;
                    Ok(checks >= 4)
                },
                Duration::from_secs(5),
                quick,
            ),
            Termination::Gone
        );
        assert_eq!(checks, 4);
        // Never gone: it gives up after the bound instead of waiting for ever.
        let started = Instant::now();
        assert_eq!(
            wait_for_exit(|| Ok(false), Duration::from_millis(60), quick),
            Termination::MayBeAlive
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        // A status check that fails cannot confirm anything.
        assert_eq!(
            wait_for_exit(|| Err(()), Duration::from_secs(1), quick),
            Termination::MayBeAlive
        );
        // The real bound is short, so the supervisor lock is never held for long.
        assert!(TERMINATE_WAIT <= Duration::from_secs(5));
    }

    #[test]
    fn the_status_and_debug_text_of_a_failed_stop_show_no_secret() -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = stubborn_supervisor(&spawner, &[GRANT_DELETE]);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        let token = supervisor
            .connection()
            .map(|connection| connection.token.expose_secret().to_owned())
            .ok_or_else(|| std::io::Error::other("no token while running"))?;
        spawner.stubborn.store(true, Ordering::SeqCst);
        supervisor.stop();
        let text = format!("{supervisor:?} {:?}", supervisor.failure());
        assert!(!text.contains(&token), "{text}");
        assert!(!text.contains("srv/jobniac-synthetic"), "{text}");
        Ok(())
    }

    #[test]
    fn every_revocation_retries_a_survivor_even_when_it_narrows_nothing()
    -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = stubborn_supervisor(&spawner, &[GRANT_DELETE]);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        spawner.stubborn.store(true, Ordering::SeqCst);
        // The first revocation cannot stop the child.
        assert_eq!(supervisor.restrict_grants(&[]), Revocation::StopFailed);
        let spawned = spawner.requests.lock().map_or(0, |requests| requests.len());

        // No grant is held any more, so nothing is newly narrowed. The survivor is still alive, so
        // the answer must stay `StopFailed`, whatever is asked, however often.
        for _ in 0..3 {
            assert_eq!(supervisor.restrict_grants(&[]), Revocation::StopFailed);
            assert_eq!(
                supervisor.restrict_grants(&[grant(GRANT_DELETE), grant(GRANT_LINK)]),
                Revocation::StopFailed
            );
            assert_eq!(supervisor.retain_grants(|_| true), Revocation::StopFailed);
            assert_eq!(supervisor.status(), McpServerStatus::Failed);
            assert_eq!(supervisor.failure(), Some(McpFailure::StopFailed));
        }
        // Each call tried the survivor again, and spawned nothing.
        assert!(spawner.terminated.load(Ordering::SeqCst) >= 7);
        assert_eq!(
            spawner.requests.lock().map_or(0, |requests| requests.len()),
            spawned
        );

        // Once the survivor is confirmed gone the next call says so, once: the server is stopped,
        // and the caller restarts it if it is meant to run.
        spawner.stubborn.store(false, Ordering::SeqCst);
        assert_eq!(supervisor.restrict_grants(&[]), Revocation::Stopped);
        assert_eq!(supervisor.status(), McpServerStatus::Stopped);
        assert_eq!(supervisor.failure(), None);
        assert_eq!(supervisor.restrict_grants(&[]), Revocation::Unaffected);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        assert!(!last_has(&spawner, GRANT_DELETE));
        Ok(())
    }

    #[test]
    fn a_revocation_of_a_grant_that_was_never_held_still_reports_a_survivor()
    -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        // Only deletion is granted; link capture never was.
        let supervisor = stubborn_supervisor(&spawner, &[GRANT_DELETE]);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        spawner.stubborn.store(true, Ordering::SeqCst);
        assert_eq!(
            supervisor.restrict_grants(&[grant(GRANT_LINK)]),
            Revocation::StopFailed
        );
        // Revoking link capture, which was never granted, narrows nothing: not a success.
        assert_eq!(
            supervisor.restrict_grants(&[grant(GRANT_LINK)]),
            Revocation::StopFailed
        );
        assert!(!supervisor.has_grant(GRANT_DELETE, "1"));
        Ok(())
    }

    #[test]
    fn a_survivor_left_by_a_plain_stop_is_reported_by_a_later_revocation()
    -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = stubborn_supervisor(&spawner, &[GRANT_DELETE]);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        spawner.stubborn.store(true, Ordering::SeqCst);
        // The survivor did not come from a revocation at all (the server was toggled off).
        supervisor.stop();
        assert_eq!(supervisor.failure(), Some(McpFailure::StopFailed));
        // A revocation that holds and narrows nothing new still reports it.
        assert_eq!(
            supervisor.restrict_grants(&[grant(GRANT_DELETE)]),
            Revocation::StopFailed
        );
        assert!(supervisor.has_grant(GRANT_DELETE, "1"));
        Ok(())
    }

    #[test]
    fn a_survivor_is_also_reported_when_the_revocation_narrows_something_new()
    -> Result<(), std::io::Error> {
        let (_listener, port) = open_port()?;
        let spawner = Arc::new(FakeSpawner::new());
        let supervisor = stubborn_supervisor(&spawner, &[GRANT_DELETE, GRANT_LINK]);
        assert_eq!(supervisor.start(port), McpServerStatus::Running);
        spawner.stubborn.store(true, Ordering::SeqCst);
        assert_eq!(
            supervisor.restrict_grants(&[grant(GRANT_LINK)]),
            Revocation::StopFailed
        );
        // A second grant is narrowed while the first survivor still lives.
        assert_eq!(supervisor.restrict_grants(&[]), Revocation::StopFailed);
        assert!(!supervisor.has_grant(GRANT_LINK, "1"));
        Ok(())
    }

    // ---- A real process -----------------------------------------------------------------------

    /// A synthetic long-lived process that touches nothing: it only waits.
    fn waiting_process() -> Result<Child, std::io::Error> {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("ping.exe");
            command.args(["-n", "60", "127.0.0.1"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sleep");
            command.arg("60");
            command
        };
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }

    #[test]
    fn a_real_child_is_killed_and_confirmed_gone_within_the_bound() -> Result<(), std::io::Error> {
        let mut child = waiting_process()?;
        let parent_pipe = child.stdin.take();
        let mut process = ProcessChild { child, parent_pipe };
        assert!(process.is_running());
        let started = Instant::now();
        assert_eq!(process.terminate(), Termination::Gone);
        assert!(started.elapsed() < TERMINATE_WAIT + Duration::from_secs(3));
        assert!(!process.is_running());
        // Terminating again, as every later start and stop does, is a no-op that still says gone.
        assert_eq!(process.terminate(), Termination::Gone);
        Ok(())
    }

    #[test]
    fn a_real_child_that_already_exited_is_gone_at_once() -> Result<(), std::io::Error> {
        let mut child = waiting_process()?;
        let parent_pipe = child.stdin.take();
        child.kill()?;
        child.wait()?;
        let mut process = ProcessChild { child, parent_pipe };
        assert_eq!(process.terminate(), Termination::Gone);
        Ok(())
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
            assert_eq!(child.terminate(), Termination::Gone);
        }
        assert_eq!(supervisor.refresh(), McpServerStatus::Failed);
        assert_eq!(supervisor.failure(), Some(McpFailure::ExitedEarly));
        Ok(())
    }
}
