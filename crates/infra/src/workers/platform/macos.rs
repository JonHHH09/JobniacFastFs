//! macOS containment: resource limits applied by the child to itself, and a memory watch of the
//! parent. The process group that is killed as one unit lives in `unix.rs`, shared with Linux.
//!
//! The parent never uses `pre_exec`. The child calls `setrlimit` in its own main before it reads
//! its request, and reads each value back. macOS does not enforce `RLIMIT_AS` or `RLIMIT_DATA`
//! (`setrlimit` refuses a finite value with `EINVAL`, and a process that was allowed to set one
//! still allocates past it), so the memory limit of a kind is enforced from the parent instead:
//! [`MemoryWatch`] reads the physical footprint of the child and the supervisor kills the tree when
//! it exceeds [`WorkerLimits::memory_bytes`]. This cap is enforced by polling, every 10 ms, so a
//! child can overshoot it by whatever it allocates within one interval; it is not a hard limit like
//! `RLIMIT_AS` on Linux. A footprint that cannot be read for any reason other than the process
//! being gone counts as over the limit (fail closed).

#![allow(unsafe_code)]

use std::io;
use std::time::Duration;

use rlimit::Resource;
use tokio::process::Child;
use tokio::time::{Interval, MissedTickBehavior, interval};

use super::unix::{apply_limit, reset_signals};
use crate::workers::kinds::WorkerLimits;

pub use super::unix::{Containment, classify_exit, self_in_job, wait_for_exit, working_directory};

/// How often the parent reads the memory footprint of a running child. `proc_pid_rusage` is cheap.
const MEMORY_POLL: Duration = Duration::from_millis(10);

/// The descriptor numbers that a child keeps: stdin, stdout and stderr.
const KEPT_DESCRIPTORS: libc::c_int = 3;

/// The highest descriptor number closed in the fallback when the soft limit of open files is
/// unlimited or unusable (`OPEN_MAX` of macOS).
const OPEN_MAX: libc::c_int = 10_240;

/// Applies the limits of a worker child to the calling process, and reads each value back.
///
/// CPU time, open files, file size and core dumps are set to the same soft and hard value. The
/// child fails when a read-back differs from the requested value. There is no address-space limit
/// (see the module documentation); the parent enforces the memory limit.
///
/// # Errors
///
/// Returns an error when a limit cannot be set or does not read back as requested.
pub fn child_apply_limits(limits: &WorkerLimits) -> io::Result<()> {
    // Nothing but stdin, stdout and stderr crosses into the child; anything else the parent leaked
    // is closed before the child opens anything of its own, and before the sandbox.
    close_inherited_descriptors()?;
    reset_signals()?;
    apply_limit(Resource::CPU, limits.cpu_seconds)?;
    ignore_file_size_signal()?;
    apply_limit(Resource::FSIZE, 0)?;
    apply_limit(Resource::CORE, 0)?;
    apply_limit(Resource::NOFILE, limits.open_files)?;
    Ok(())
}

/// Ignores `SIGXFSZ`, so that a write the zero file-size limit refuses fails with `EFBIG` instead
/// of ending the child. The Security framework, which loads the system trust store for the fetch
/// kinds before the sandbox, opens keychain databases that it may try to write; those writes must
/// fail softly, and the limit still stops every byte.
fn ignore_file_size_signal() -> io::Result<()> {
    // SAFETY: `signal` with `SIG_IGN` installs no handler code; the arguments are plain integers.
    let previous = unsafe { libc::signal(libc::SIGXFSZ, libc::SIG_IGN) };
    if previous == libc::SIG_ERR {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Closes every descriptor above stderr.
///
/// macOS has no `close_range`. The open descriptors are listed through `/dev/fd`; when that
/// directory cannot be read, every number up to the soft limit of open files (at most `OPEN_MAX`)
/// is closed instead.
///
/// # Errors
///
/// Returns an error when the soft limit cannot be read in the fallback.
pub fn close_inherited_descriptors() -> io::Result<()> {
    if let Ok(entries) = std::fs::read_dir("/dev/fd") {
        let numbers: Vec<libc::c_int> = entries
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
            .filter(|number| *number >= KEPT_DESCRIPTORS)
            .collect();
        for number in numbers {
            close_descriptor(number);
        }
        return Ok(());
    }
    let (soft, _hard) = rlimit::getrlimit(Resource::NOFILE)?;
    // An unlimited soft limit reads as `u64::MAX`; closing up to there would never end.
    let last = libc::c_int::try_from(soft).map_or(OPEN_MAX, |soft| soft.min(OPEN_MAX));
    for number in KEPT_DESCRIPTORS..last {
        close_descriptor(number);
    }
    Ok(())
}

fn close_descriptor(number: libc::c_int) {
    // SAFETY: closing a descriptor number of this process. The listing's own descriptor is already
    // closed by the time the iterator is dropped, so an `EBADF` is ignored.
    unsafe {
        libc::close(number);
    }
}

/// The parent's watch over the memory of one child.
///
/// It polls the physical footprint of the child (`ri_phys_footprint`, the number that Activity
/// Monitor shows as memory) every 10 ms. A watch holds no task: the supervisor polls
/// [`MemoryWatch::exceeded`] inside its own exchange, so dropping the exchange ends the watch on
/// every exit path. The cap is enforced by polling, so a child can overshoot it by what it
/// allocates within one interval.
#[derive(Debug)]
pub struct MemoryWatch {
    pid: Option<libc::pid_t>,
    limit: u64,
    ticks: Interval,
}

impl MemoryWatch {
    /// A watch over `child`, with `limit` bytes of footprint allowed. A child without a pid (it
    /// has already been reaped) is never over the limit.
    #[must_use]
    pub fn new(child: &Child, limit: u64) -> Self {
        let mut ticks = interval(MEMORY_POLL);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Self {
            pid: child.id().and_then(|pid| libc::pid_t::try_from(pid).ok()),
            limit,
            ticks,
        }
    }

    /// Resolves once the footprint of the child exceeds the limit. Pends for as long as it does
    /// not. Cancel-safe.
    pub async fn exceeded(&mut self) {
        let Some(pid) = self.pid else {
            return std::future::pending().await;
        };
        loop {
            self.ticks.tick().await;
            match physical_footprint(pid) {
                Reading::Bytes(bytes) if bytes > self.limit => return,
                // Any other failure to read is treated as over the limit: fail closed.
                Reading::Unreadable => return,
                Reading::Bytes(_) | Reading::Gone => {}
            }
        }
    }
}

/// What a read of the footprint of a child found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reading {
    /// The physical footprint in bytes.
    Bytes(u64),
    /// The process is gone or a zombie (`ESRCH`): there is nothing left to limit.
    Gone,
    /// The read failed for another reason, so the footprint is unknown.
    Unreadable,
}

/// Maps the result of `proc_pid_rusage` (`status`, its `errno` and the footprint field) to a
/// [`Reading`]. Only `ESRCH` is "no reading"; every other error is [`Reading::Unreadable`].
fn interpret(status: libc::c_int, errno: i32, footprint: u64) -> Reading {
    if status == 0 {
        Reading::Bytes(footprint)
    } else if errno == libc::ESRCH {
        Reading::Gone
    } else {
        Reading::Unreadable
    }
}

/// The physical footprint of the process `pid`.
fn physical_footprint(pid: libc::pid_t) -> Reading {
    // SAFETY: `rusage_info_v4` is plain old data for which all-zero bytes are a valid value.
    let mut usage: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: `proc_pid_rusage` only writes a `rusage_info_v4` into the buffer for the flavor
    // `RUSAGE_INFO_V4`, and the buffer is exactly that structure. The parent may read the usage of
    // a child of its own user.
    let status = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V4,
            std::ptr::from_mut(&mut usage).cast(),
        )
    };
    let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
    interpret(status, errno, usage.ri_phys_footprint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_footprint_of_a_running_process_is_readable() {
        let pid = libc::pid_t::try_from(std::process::id()).expect("pid");
        match physical_footprint(pid) {
            Reading::Bytes(bytes) => assert!(bytes > 0),
            other => panic!("footprint of this process: {other:?}"),
        }
    }

    #[test]
    fn the_footprint_of_a_missing_process_is_gone() {
        // The largest pid that macOS hands out is far below this value.
        assert_eq!(physical_footprint(libc::pid_t::MAX), Reading::Gone);
    }

    /// Only `ESRCH` means "nothing to read"; every other failure is unreadable, which the watch
    /// treats as over the limit.
    #[test]
    fn only_a_missing_process_is_not_a_reading() {
        assert_eq!(interpret(0, 0, 7), Reading::Bytes(7));
        assert_eq!(interpret(-1, libc::ESRCH, 0), Reading::Gone);
        for errno in [libc::EPERM, libc::EACCES, libc::EINVAL, libc::ENOMEM, 0] {
            assert_eq!(
                interpret(-1, errno, 0),
                Reading::Unreadable,
                "errno {errno}"
            );
        }
    }

    /// The memory watch resolves for a child over its limit and never for one under it.
    #[tokio::test]
    async fn the_watch_resolves_only_above_the_limit() {
        let mut command = tokio::process::Command::new("/bin/sleep");
        command.arg("30").kill_on_drop(true);
        let child = command.spawn().expect("spawn");
        let mut over = MemoryWatch::new(&child, 0);
        tokio::time::timeout(Duration::from_secs(20), over.exceeded())
            .await
            .expect("a child over a zero limit is detected");
        let mut under = MemoryWatch::new(&child, u64::MAX);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), under.exceeded())
                .await
                .is_err(),
            "a child under its limit is left alone"
        );
    }
}
