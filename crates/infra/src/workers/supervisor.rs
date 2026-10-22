//! The parent side of the worker protocol: spawns the CLI binary as a worker child, runs one
//! request, and maps every outcome to a sanitized [`WorkerError`].
//!
//! One operation runs as follows:
//!
//! 1. the request must fit the limit of its kind (refused before anything starts);
//! 2. the operation waits for a permit of its kind, for at most one wall-clock budget and within
//!    its cancellation future; the child then gets a fresh budget of its own, so a queued call is
//!    not run with what the queue left over;
//! 3. the containment is prepared (Windows: the Job Object with its limits), the child is spawned
//!    with a cleared environment and an allowlist, and the child is attached to the containment;
//! 4. the parent writes the go byte and the request frame, then reads stdout up to the output limit
//!    while it drains stderr and waits for the exit. When the child exits, its whole tree is
//!    killed, so pipes close even if a descendant held them (macOS also kills the tree when the
//!    child's memory footprint exceeds its limit);
//! 5. on every exit path, success included, the tree is killed and the child is reaped. A failed
//!    run uses no partial output.
//!
//! Dropping the returned future also kills the child: `kill_on_drop` and the containment cover it.

use std::collections::HashMap;
use std::ffi::OsString;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::pin;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Semaphore;
use tokio::time::{Instant, sleep_until};

use crate::workers::env::child_environment;
use crate::workers::error::{ExitClass, WorkerError, WorkerErrorKind};
use crate::workers::kinds::{WorkerKind, WorkerLimits};
use crate::workers::platform::{self, Containment};
use crate::workers::protocol::{GO_BYTE, decode_response};
use crate::workers::sandbox;

/// The bytes read from a child's stdout in one step.
const READ_CHUNK_BYTES: usize = 64 * 1024;

/// The bytes read from a child's stderr in one step. Stderr is drained and discarded.
const DRAIN_CHUNK_BYTES: usize = 4096;

/// Runs worker children of the CLI binary, bounded per kind.
#[derive(Debug)]
pub struct WorkerSupervisor {
    program: PathBuf,
    source_environment: Vec<(OsString, OsString)>,
    permits: HashMap<WorkerKind, Arc<Semaphore>>,
    /// Replaces the wall-clock budget of every kind, so timing tests need not run for seconds.
    #[cfg(test)]
    wall_override: Option<std::time::Duration>,
}

impl WorkerSupervisor {
    /// A supervisor that spawns `program` as the worker binary, with variables taken from
    /// `source_environment` through the allowlist.
    #[must_use]
    pub fn new(program: PathBuf, source_environment: Vec<(OsString, OsString)>) -> Self {
        let permits = WorkerKind::ALL
            .into_iter()
            .map(|kind| (kind, Arc::new(Semaphore::new(kind.max_concurrent()))))
            .collect();
        Self {
            program,
            source_environment,
            permits,
            #[cfg(test)]
            wall_override: None,
        }
    }

    /// A supervisor that spawns the running executable, with the process environment.
    ///
    /// # Errors
    ///
    /// Returns an error when the path of the running executable cannot be determined.
    pub fn for_current_process() -> io::Result<Self> {
        Ok(Self::new(
            std::env::current_exe()?,
            std::env::vars_os().collect(),
        ))
    }

    /// The number of children of `kind` that may still start.
    #[must_use]
    pub fn available_permits(&self, kind: WorkerKind) -> usize {
        self.permits
            .get(&kind)
            .map_or(0, |semaphore| semaphore.available_permits())
    }

    /// Runs one operation of `kind` with `request` as its body and returns the response body.
    ///
    /// `cancel` resolves when the caller cancels; the child tree is then killed and the call
    /// returns [`WorkerErrorKind::Cancelled`].
    ///
    /// # Errors
    ///
    /// Returns a [`WorkerError`] for each failure category; no child output is ever returned after
    /// a failure.
    pub async fn run<C: Future<Output = ()>>(
        &self,
        kind: WorkerKind,
        request: &[u8],
        cancel: C,
    ) -> Result<Vec<u8>, WorkerError> {
        if request.len() > kind.request_limit() {
            return Err(WorkerError::new(WorkerErrorKind::InvalidRequest));
        }
        #[allow(unused_mut)]
        let mut limits = kind.limits();
        #[cfg(test)]
        if let Some(wall) = self.wall_override {
            limits.wall = wall;
        }
        let queue_deadline = Instant::now() + limits.wall;
        let mut cancel = pin!(cancel);
        let semaphore = self
            .permits
            .get(&kind)
            .cloned()
            .ok_or(WorkerError::new(WorkerErrorKind::SetupFailed))?;
        let permit = tokio::select! {
            biased;
            () = &mut cancel => return Err(WorkerError::new(WorkerErrorKind::Cancelled)),
            () = sleep_until(queue_deadline) => {
                return Err(WorkerError::new(WorkerErrorKind::TimedOut));
            }
            acquired = semaphore.acquire_owned() => acquired
                .map_err(|_| WorkerError::new(WorkerErrorKind::SetupFailed))?,
        };
        // The permit wait is bounded by its own budget above; the child is timed from here.
        let deadline = Instant::now() + limits.wall;
        let outcome = self
            .run_child(kind, request, &limits, deadline, &mut cancel)
            .await;
        drop(permit);
        outcome
    }

    /// Spawns, attaches and exchanges with one child, then reaps it and checks its exit.
    async fn run_child<C: Future<Output = ()>>(
        &self,
        kind: WorkerKind,
        request: &[u8],
        limits: &WorkerLimits,
        deadline: Instant,
        cancel: &mut std::pin::Pin<&mut C>,
    ) -> Result<Vec<u8>, WorkerError> {
        let setup = WorkerError::new(WorkerErrorKind::SetupFailed);
        sandbox::warn_once_if_degraded();
        let working_directory =
            platform::working_directory(&self.source_environment).ok_or(setup)?;
        let environment = child_environment(&self.source_environment, kind);
        let mut containment = Containment::prepare(limits).map_err(|_| setup)?;

        let mut command = Command::new(&self.program);
        command
            .arg("__worker")
            .arg(kind.name())
            .env_clear()
            .envs(environment)
            .current_dir(working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        Containment::configure(&mut command);

        let mut child = command
            .spawn()
            .map_err(|_| WorkerError::new(WorkerErrorKind::SpawnFailed))?;
        if containment.attach(&child).is_err() {
            // The child is blocked on the go byte, so it has done nothing yet.
            containment.kill_tree();
            let _ = child.start_kill();
            return Err(setup);
        }

        let mut exit = None;
        let exchanged = {
            let exchange = exchange(&mut child, &mut containment, request, limits, &mut exit);
            tokio::select! {
                biased;
                () = &mut *cancel => Err(WorkerError::new(WorkerErrorKind::Cancelled)),
                () = sleep_until(deadline) => Err(WorkerError::new(WorkerErrorKind::TimedOut)),
                result = exchange => result,
            }
        };

        // Every path ends here. An unreaped leader keeps its group id reserved, so the tree is
        // killed first, then the leader is reaped and the group id is forgotten. A reaped leader
        // was already handled by `exchange`.
        if exit.is_none() {
            containment.kill_tree();
            let _ = child.start_kill();
            let _ = child.wait().await;
            containment.leader_reaped();
        }
        let stdout = exchanged?;
        let status = exit.ok_or(WorkerError::new(WorkerErrorKind::ChildFailed(
            ExitClass::Unknown,
        )))?;
        if !status.success() {
            return Err(WorkerError::new(WorkerErrorKind::ChildFailed(
                platform::classify_exit(status),
            )));
        }
        decode_response(&stdout)
    }
}

/// Writes the request, then reads stdout up to the output limit, drains stderr and records the
/// exit status of the child. The tree is killed as soon as the child exits, so the stdout pipe
/// closes even when a descendant still holds it.
async fn exchange(
    child: &mut Child,
    containment: &mut Containment,
    request: &[u8],
    limits: &WorkerLimits,
    exit: &mut Option<ExitStatus>,
) -> Result<Vec<u8>, WorkerError> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or(WorkerError::new(WorkerErrorKind::SpawnFailed))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or(WorkerError::new(WorkerErrorKind::SpawnFailed))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or(WorkerError::new(WorkerErrorKind::SpawnFailed))?;

    // The child reads the whole request before it writes anything, so writing first cannot
    // deadlock. A failed write means the child already exited, and its exit status reports that.
    let _ = write_request(&mut stdin, request).await;
    drop(stdin);

    let output_limit = usize::try_from(limits.output_bytes).unwrap_or(usize::MAX);
    let mut output = Vec::new();
    let mut chunk = vec![0_u8; READ_CHUNK_BYTES];
    let mut sink = vec![0_u8; DRAIN_CHUNK_BYTES];
    let mut stdout_open = true;
    let mut stderr_open = true;
    // macOS cannot limit the memory of a child itself, so the parent watches it (a no-op
    // elsewhere).
    let mut memory = platform::MemoryWatch::new(child, limits.memory_bytes);
    let mut over_memory = false;
    let mut wait = pin!(platform::wait_for_exit(child, containment.tree_killer()));

    loop {
        if exit.is_some() && !stdout_open {
            break;
        }
        tokio::select! {
            biased;
            () = memory.exceeded(), if exit.is_none() && !over_memory => {
                // The tree dies as if a limit had ended it: the exit arrives as a signal and
                // becomes a child failure, and the output is discarded.
                over_memory = true;
                containment.kill_tree();
            }
            read = stdout.read(&mut chunk), if stdout_open => {
                let read = read.map_err(|_| WorkerError::new(WorkerErrorKind::ProtocolViolation))?;
                if read == 0 {
                    stdout_open = false;
                } else {
                    if output.len().saturating_add(read) > output_limit {
                        return Err(WorkerError::new(WorkerErrorKind::OutputLimitExceeded));
                    }
                    output.extend_from_slice(&chunk[..read]);
                }
            }
            read = stderr.read(&mut sink), if stderr_open => {
                if matches!(read, Ok(0) | Err(_)) {
                    stderr_open = false;
                }
            }
            status = &mut wait, if exit.is_none() => {
                let status = status
                    .map_err(|_| WorkerError::new(WorkerErrorKind::ChildFailed(ExitClass::Unknown)))?;
                *exit = Some(status);
                containment.leader_reaped();
            }
        }
    }
    Ok(output)
}

/// Writes the go byte and the request frame.
async fn write_request(stdin: &mut ChildStdin, request: &[u8]) -> io::Result<()> {
    let length = u32::try_from(request.len()).map_err(io::Error::other)?;
    stdin.write_all(&[GO_BYTE]).await?;
    stdin.write_all(&length.to_be_bytes()).await?;
    stdin.write_all(request).await?;
    stdin.flush().await
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    /// A queued call is timed from the moment it gets its permit, not from the moment it queued.
    #[tokio::test]
    async fn a_queued_call_gets_its_full_budget_after_the_permit_wait() {
        let directory = std::env::temp_dir().join(format!("jobniac-sup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).expect("directory");
        let script = directory.join("worker.sh");
        // Runs for 0.6 s and fails: the outcome is a child failure, never a timeout.
        std::fs::write(&script, "#!/bin/sh\nsleep 0.6\nexit 3\n").expect("script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("mode");

        let mut supervisor = WorkerSupervisor::new(script, Vec::new());
        supervisor.wall_override = Some(Duration::from_millis(1000));
        let kind = WorkerKind::Probe;
        let semaphore = Arc::clone(&supervisor.permits[&kind]);
        let held = semaphore
            .clone()
            .acquire_many_owned(u32::try_from(kind.max_concurrent()).expect("permits"))
            .await
            .expect("hold every permit");
        let released = tokio::spawn(async move {
            // Queued for 0.7 s of its 1 s budget; the child then needs 0.6 s.
            tokio::time::sleep(Duration::from_millis(700)).await;
            drop(held);
        });
        let outcome = supervisor
            .run(kind, b"{}", std::future::pending::<()>())
            .await;
        released.await.expect("release task");
        let _ = std::fs::remove_dir_all(&directory);
        let error = outcome.expect_err("the script is not a worker");
        assert_eq!(
            error.kind(),
            WorkerErrorKind::ChildFailed(ExitClass::Code(3))
        );
    }
}
