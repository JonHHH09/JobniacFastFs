//! Linux containment: resource limits applied by the child to itself, and a process group that is
//! killed as one unit.
//!
//! The parent never uses `pre_exec`. The child calls `setrlimit` in its own main before it reads
//! its request, and reads each value back. The parent starts the child in its own process group
//! (`process_group(0)`) and kills the group on every exit path, so descendants die with the child.
//! The group is always killed while the leader is still unreaped (a zombie keeps its pid, and so the
//! group id, reserved), never after the reap, when the id could already name an unrelated group.

#![allow(unsafe_code)]

use std::ffi::OsString;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::ExitStatus;

use rlimit::Resource;
use tokio::process::{Child, Command};

use crate::workers::error::ExitClass;
use crate::workers::kinds::WorkerLimits;

/// The fixed, neutral working directory of every Linux child.
const NEUTRAL_DIRECTORY: &str = "/";

/// The process group of one child, killed as a unit.
#[derive(Debug)]
pub struct Containment {
    /// The group id, which is the pid of the child (`process_group(0)`). Set by `attach`.
    process_group: Option<i32>,
}

impl Drop for Containment {
    fn drop(&mut self) {
        // Dropping the call (cancellation, timeout) kills the group too, so descendants that the
        // child started do not outlive it. `kill_tree` is a no-op before `attach`.
        self.kill_tree();
    }
}

impl Containment {
    /// Nothing to create before the spawn: the limits are applied by the child itself.
    ///
    /// # Errors
    ///
    /// This preparation does not fail on Linux.
    pub fn prepare(_limits: &WorkerLimits) -> io::Result<Self> {
        Ok(Self {
            process_group: None,
        })
    }

    /// Starts the child in its own process group, so that its descendants share its group id.
    pub fn configure(command: &mut Command) {
        command.process_group(0);
    }

    /// Records the group id of the spawned child.
    ///
    /// # Errors
    ///
    /// Returns an error when the child has no pid (it has already been reaped).
    pub fn attach(&mut self, child: &Child) -> io::Result<()> {
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("child has no process id"))?;
        self.process_group = Some(i32::try_from(pid).map_err(io::Error::other)?);
        Ok(())
    }

    /// Sends `SIGKILL` to the whole process group, descendants included.
    ///
    /// Only valid while the leader is unreaped (its pid, and so the group id, is still reserved)
    /// or before [`Containment::leader_reaped`] has been called. After the reap the id may be
    /// reused, so the group id is forgotten there and this becomes a no-op.
    pub fn kill_tree(&self) {
        if let Some(process_group) = self.process_group {
            kill_group(process_group);
        }
    }

    /// Called right after the leader has been reaped. Only forgets the group id, so that no later
    /// kill, including the one in `Drop`, can name a recycled pid. The final kill of descendants
    /// that still hold the output pipes is made by [`wait_for_exit`], before the reap.
    pub fn leader_reaped(&mut self) {
        self.process_group = None;
    }

    /// A copy of the group id for [`wait_for_exit`], which kills the group before it reaps.
    #[must_use]
    pub fn tree_killer(&self) -> TreeKiller {
        TreeKiller(self.process_group)
    }
}

/// Sends `SIGKILL` to a process group. The caller guarantees that the group id is still reserved.
fn kill_group(process_group: i32) {
    // SAFETY: `killpg` only sends a signal to a process group. It reads no memory from this
    // process and the arguments are plain integers. Errors (such as an empty group) are ignored
    // by design: the group may already be gone.
    unsafe {
        libc::killpg(process_group, libc::SIGKILL);
    }
}

/// The group that [`wait_for_exit`] kills between the exit of the leader and its reap.
#[derive(Debug, Clone, Copy)]
pub struct TreeKiller(Option<i32>);

/// Waits until the leader has exited without reaping it, kills its process group, and only then
/// reaps it. The group id is reserved by the unreaped (zombie) leader for the whole kill, so the
/// signal cannot reach a recycled id, and descendants that outlived the leader (and may hold the
/// output pipes) die.
///
/// # Errors
///
/// Returns the error of reaping the child.
pub async fn wait_for_exit(child: &mut Child, killer: TreeKiller) -> io::Result<ExitStatus> {
    if let (Some(process_group), Some(pid)) = (killer.0, child.id()) {
        // `waitid` with `WNOWAIT` blocks until the exit and leaves the zombie in place. A failure
        // (for example the child was already reaped) skips the kill: reaping then proceeds as before.
        let exited = tokio::task::spawn_blocking(move || wait_exited_unreaped(pid)).await;
        if matches!(exited, Ok(Ok(()))) {
            kill_group(process_group);
        }
    }
    child.wait().await
}

/// Blocks until the child `pid` has exited, without reaping it.
///
/// The blocking thread ends once the child exits; when the exchange is cancelled the caller's
/// cleanup kills and reaps the child, which releases it.
fn wait_exited_unreaped(pid: u32) -> io::Result<()> {
    let pid = libc::id_t::from(pid);
    loop {
        // SAFETY: `siginfo_t` is plain old data for which all-zero bytes are a valid value, and
        // `waitid` only writes into it. `P_PID` with a pid of our own child and
        // `WEXITED | WNOWAIT` observes the exit without consuming it.
        let status = unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT)
        };
        if status == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINTR) {
            return Err(error);
        }
    }
}

/// Applies the limits of a worker child to the calling process, and reads each value back.
///
/// Address space, CPU time, open files, file size and core dumps are set to the same soft and hard
/// value. The child fails when a read-back differs from the requested value.
///
/// # Errors
///
/// Returns an error when a limit cannot be set or does not read back as requested.
pub fn child_apply_limits(limits: &WorkerLimits) -> io::Result<()> {
    // Nothing but stdin, stdout and stderr crosses into the child; anything else the parent leaked
    // is closed before the child opens anything of its own.
    crate::workers::platform::close_inherited_descriptors()?;
    reset_signals()?;
    apply_limit(Resource::AS, limits.memory_bytes)?;
    apply_limit(Resource::CPU, limits.cpu_seconds)?;
    apply_limit(Resource::FSIZE, 0)?;
    apply_limit(Resource::CORE, 0)?;
    apply_limit(Resource::NOFILE, limits.open_files)?;
    Ok(())
}

/// Signals whose dispositions a child resets to the default (E-NET-32): a parent that ignored them
/// would otherwise pass `SIG_IGN` across `exec`.
const RESET_SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGXCPU];

/// Resets `SIGTERM`, `SIGINT` and `SIGXCPU` to their default dispositions.
fn reset_signals() -> io::Result<()> {
    for signal in RESET_SIGNALS {
        // SAFETY: `signal` with `SIG_DFL` installs no handler code, only restores the default
        // disposition of a standard signal; the arguments are plain integers.
        let previous = unsafe { libc::signal(signal, libc::SIG_DFL) };
        if previous == libc::SIG_ERR {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn apply_limit(resource: Resource, value: u64) -> io::Result<()> {
    rlimit::setrlimit(resource, value, value)?;
    if rlimit::getrlimit(resource)? != (value, value) {
        return Err(io::Error::other("resource limit does not read back"));
    }
    Ok(())
}

/// The fixed working directory of a child.
#[must_use]
pub fn working_directory(_source: &[(OsString, OsString)]) -> Option<PathBuf> {
    Some(PathBuf::from(NEUTRAL_DIRECTORY))
}

/// Job membership of the calling process. Linux has no Job Object, so the question has no answer.
#[must_use]
pub fn self_in_job() -> Option<bool> {
    None
}

/// The class of an exit status: a status code, or the signal that ended the child.
#[must_use]
pub fn classify_exit(status: ExitStatus) -> ExitClass {
    if let Some(code) = status.code() {
        ExitClass::Code(code)
    } else if let Some(signal) = status.signal() {
        ExitClass::Signal(signal)
    } else {
        ExitClass::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disposition(signal: libc::c_int) -> libc::sighandler_t {
        // SAFETY: a null new action only queries the current one, written into zeroed storage.
        unsafe {
            let mut old: libc::sigaction = std::mem::zeroed();
            assert_eq!(libc::sigaction(signal, std::ptr::null(), &mut old), 0);
            old.sa_sigaction
        }
    }

    #[test]
    fn ignored_signals_are_reset_to_the_default() {
        for signal in RESET_SIGNALS {
            // SAFETY: ignoring a standard signal installs no handler code.
            unsafe {
                libc::signal(signal, libc::SIG_IGN);
            }
            assert_eq!(disposition(signal), libc::SIG_IGN);
        }
        reset_signals().expect("reset");
        for signal in RESET_SIGNALS {
            assert_eq!(disposition(signal), libc::SIG_DFL, "signal {signal}");
        }
    }

    /// The final kill reaches a descendant that outlived the leader, and it is issued while the
    /// leader is still a zombie (the id is reserved), then the leader is reaped.
    #[test]
    fn descendants_of_an_exited_leader_are_killed_before_the_reap() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let mut command = Command::new("/bin/sh");
            command
                .arg("-c")
                .arg("sleep 60 & echo $!; exit 7")
                .stdout(std::process::Stdio::piped());
            Containment::configure(&mut command);
            let mut child = command.spawn().expect("spawn");
            let mut containment =
                Containment::prepare(&crate::workers::kinds::WorkerKind::Probe.limits())
                    .expect("prepare");
            containment.attach(&child).expect("attach");
            let mut line = String::new();
            let mut stdout = tokio::io::BufReader::new(child.stdout.take().expect("stdout"));
            tokio::io::AsyncBufReadExt::read_line(&mut stdout, &mut line)
                .await
                .expect("read pid");
            let descendant: i32 = line.trim().parse().expect("pid");
            let status = wait_for_exit(&mut child, containment.tree_killer())
                .await
                .expect("exit");
            containment.leader_reaped();
            assert_eq!(status.code(), Some(7));
            // The killed descendant is reparented and reaped by init, or lingers as a zombie.
            let mut alive = true;
            for _ in 0..100 {
                let state = std::fs::read_to_string(format!("/proc/{descendant}/stat"))
                    .ok()
                    .and_then(|stat| stat.rsplit(')').next().map(str::to_owned));
                alive = state.is_some_and(|rest| !rest.trim_start().starts_with('Z'));
                if !alive {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            assert!(!alive, "the descendant was killed with the group");
        });
    }
}
