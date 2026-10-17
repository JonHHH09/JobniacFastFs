//! Windows containment: an unnamed Job Object per child, with memory, CPU-time, process-count and
//! UI limits, and kill-on-close.
//!
//! This module holds the only Win32 FFI of the worker supervisor. The job is created before the
//! child is spawned, so a failure to set the limits refuses the spawn. The child is attached right
//! after it starts. Nothing reaches the child before the attachment: the child blocks on the go
//! byte, which the parent writes only after it has verified the membership. Nothing is applied by
//! the child itself on Windows.

#![allow(unsafe_code)]

use std::ffi::OsString;
use std::io;
use std::mem;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::ptr;

use tokio::process::{Child, Command};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOB_OBJECT_LIMIT_PROCESS_TIME, JOBOBJECT_BASIC_UI_RESTRICTIONS,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOBOBJECTINFOCLASS, JobObjectBasicUIRestrictions,
    JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::core::BOOL;

use crate::workers::error::ExitClass;
use crate::workers::kinds::WorkerLimits;

/// One user-time unit of a job limit: 100 nanoseconds.
const HUNDRED_NANOSECONDS_PER_SECOND: i64 = 10_000_000;

/// `CREATE_NEW_PROCESS_GROUP`: the child does not share the console's Ctrl+C group.
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// `CREATE_NO_WINDOW`: the child never gets a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Every UI restriction class: handles, clipboard, system parameters, display settings, global
/// atoms, desktops and exit-windows. Mask `0xFF`.
const UI_RESTRICTIONS_ALL: u32 = 0xFF;

/// The fixed, system-owned working directory of every Windows child.
const SYSTEM_ROOT: &str = "SYSTEMROOT";

/// An unnamed Job Object. Closing the last handle kills every process still in the job.
#[derive(Debug)]
struct JobObject {
    handle: HANDLE,
}

// SAFETY: a job handle is a reference to a kernel object. The Win32 job functions are thread-safe,
// and this value is the only owner that closes the handle, exactly once, in `Drop`.
unsafe impl Send for JobObject {}

// SAFETY: the object forwards its calls only to thread-safe Win32 functions and never exposes the
// handle for mutation.
unsafe impl Sync for JobObject {}

impl JobObject {
    /// Creates a job with the limits of a worker operation.
    fn create(limits: &WorkerLimits) -> io::Result<Self> {
        let memory = usize::try_from(limits.memory_bytes).map_err(io::Error::other)?;
        let cpu_units = i64::try_from(limits.cpu_seconds)
            .ok()
            .and_then(|seconds| seconds.checked_mul(HUNDRED_NANOSECONDS_PER_SECOND))
            .ok_or_else(|| io::Error::other("cpu limit is out of range"))?;
        // SAFETY: both arguments are null, which CreateJobObjectW accepts for an unnamed job with
        // the default security. The returned handle is owned by the `JobObject` built next, and
        // only its `Drop` closes it.
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self { handle };

        // SAFETY: JOBOBJECT_EXTENDED_LIMIT_INFORMATION holds only integers and nested structs of
        // integers, so the all-zero bit pattern is a valid value for it.
        let mut extended: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
        extended.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_JOB_MEMORY
            | JOB_OBJECT_LIMIT_PROCESS_TIME
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        extended.BasicLimitInformation.PerProcessUserTimeLimit = cpu_units;
        extended.BasicLimitInformation.ActiveProcessLimit = 1;
        extended.ProcessMemoryLimit = memory;
        extended.JobMemoryLimit = memory;
        job.set(JobObjectExtendedLimitInformation, &extended)?;

        let restrictions = JOBOBJECT_BASIC_UI_RESTRICTIONS {
            UIRestrictionsClass: UI_RESTRICTIONS_ALL,
        };
        job.set(JobObjectBasicUIRestrictions, &restrictions)?;
        Ok(job)
    }

    /// Sets one information class of the job from a value of its exact layout.
    fn set<T>(&self, class: JOBOBJECTINFOCLASS, value: &T) -> io::Result<()> {
        let length = u32::try_from(mem::size_of::<T>()).map_err(io::Error::other)?;
        // SAFETY: `value` is a live reference for the whole call, and `length` is its exact size.
        // The handle is owned by `self`, so it is still open.
        let applied = unsafe {
            SetInformationJobObject(self.handle, class, ptr::from_ref(value).cast(), length)
        };
        if applied == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Assigns a live process to the job and verifies its membership.
    fn assign(&self, process: HANDLE) -> io::Result<()> {
        // SAFETY: `self.handle` is owned by `self`, and `process` is a process handle that the
        // caller's child object keeps open for the whole call.
        let assigned = unsafe { AssignProcessToJobObject(self.handle, process) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut member: BOOL = 0;
        // SAFETY: `member` is a valid, writable BOOL for the duration of the call, and both handles
        // are the same live handles as above.
        let queried = unsafe { IsProcessInJob(process, self.handle, &mut member) };
        if queried == 0 {
            return Err(io::Error::last_os_error());
        }
        if member == 0 {
            return Err(io::Error::other("child is not in the job"));
        }
        Ok(())
    }

    /// Terminates every process in the job, descendants included.
    fn terminate(&self) {
        // SAFETY: `self.handle` is a live job handle owned by `self`. The result is ignored
        // because the job is killed on close anyway, as the backstop.
        unsafe {
            TerminateJobObject(self.handle, 1);
        }
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        // SAFETY: `self.handle` came from CreateJobObjectW and is closed only here. Closing the
        // last handle kills every process still in the job (kill on job close).
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

/// The Job Object of one worker child.
#[derive(Debug)]
pub struct Containment {
    job: JobObject,
}

impl Containment {
    /// Creates the job with the limits of the operation, before anything is spawned.
    ///
    /// # Errors
    ///
    /// Returns an error when the job or its limits cannot be created.
    pub fn prepare(limits: &WorkerLimits) -> io::Result<Self> {
        Ok(Self {
            job: JobObject::create(limits)?,
        })
    }

    /// Starts the child suspended-free, in a new process group and without a console window.
    pub fn configure(command: &mut Command) {
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }

    /// Assigns the spawned child to the job and verifies the membership.
    ///
    /// # Errors
    ///
    /// Returns an error when the child has no process handle, cannot be assigned, or is not a
    /// member of the job afterwards.
    pub fn attach(&mut self, child: &Child) -> io::Result<()> {
        let process = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("child has no process handle"))?;
        self.job.assign(process as HANDLE)
    }

    /// Terminates the job, which kills the child and every descendant.
    pub fn kill_tree(&self) {
        self.job.terminate();
    }

    /// Nothing to forget: the job is held by handle, so terminating it after the leader is reaped
    /// cannot reach another process.
    pub fn leader_reaped(&mut self) {}

    /// Nothing to copy.
    #[must_use]
    pub const fn tree_killer(&self) -> TreeKiller {
        TreeKiller
    }
}

/// Carries nothing: the job is held by handle, so no group id can be recycled.
#[derive(Debug, Clone, Copy)]
pub struct TreeKiller;

/// Waits for the child to exit and reaps it.
///
/// # Errors
///
/// Returns the error of waiting for the child.
pub async fn wait_for_exit(child: &mut Child, _killer: TreeKiller) -> io::Result<ExitStatus> {
    child.wait().await
}

/// Nothing to apply on Windows: the limits live in the Job Object that the parent created.
///
/// # Errors
///
/// This operation does not fail on Windows.
pub fn child_apply_limits(_limits: &WorkerLimits) -> io::Result<()> {
    Ok(())
}

/// The fixed working directory of a child: the directory named by `SYSTEMROOT`, when it exists.
#[must_use]
pub fn working_directory(source: &[(OsString, OsString)]) -> Option<PathBuf> {
    source
        .iter()
        .find(|(name, _)| {
            name.to_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(SYSTEM_ROOT))
        })
        .map(|(_, value)| PathBuf::from(value))
        .filter(|directory| directory.is_dir())
}

/// The class of an exit status: its status code.
#[must_use]
pub fn classify_exit(status: ExitStatus) -> ExitClass {
    status.code().map_or(ExitClass::Unknown, ExitClass::Code)
}

/// Whether the calling process is a member of some Job Object. Used by the probe operation to
/// prove that the parent attached the child before it released the go byte.
///
/// Returns `None` when the query itself fails.
#[must_use]
pub fn self_in_job() -> Option<bool> {
    let mut member: BOOL = 0;
    // SAFETY: a null job handle asks whether the process belongs to any job. The pseudo-handle from
    // `GetCurrentProcess` needs no closing, and `member` is a valid, writable BOOL for the call.
    let queried = unsafe { IsProcessInJob(GetCurrentProcess(), ptr::null_mut(), &mut member) };
    (queried != 0).then_some(member != 0)
}
