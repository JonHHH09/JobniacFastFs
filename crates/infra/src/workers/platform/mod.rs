//! Platform containment of worker children.
//!
//! Each platform module exposes the same surface, so the supervisor and the child entry point are
//! platform-neutral:
//!
//! - `Containment`: created before the child is spawned, configured on its command, attached to
//!   the spawned child, and used to kill the whole child tree on every exit path;
//! - `child_apply_limits`: the limits the child applies to itself before it reads its request;
//! - `working_directory`: the fixed, neutral working directory of a child;
//! - `wait_for_exit`: waits for the child and reaps it (Linux kills the process group first, while
//!   the unreaped leader still reserves the group id);
//! - `classify_exit`: the class of an exit status, without its output.
//!
//! Platforms other than Windows and Linux fail closed: `Containment::prepare` refuses.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{
    Containment, child_apply_limits, classify_exit, self_in_job, wait_for_exit, working_directory,
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod linux_sandbox;
#[cfg(target_os = "linux")]
pub use linux::{
    Containment, child_apply_limits, classify_exit, self_in_job, wait_for_exit, working_directory,
};
#[cfg(all(target_os = "linux", any(test, feature = "worker-probe")))]
pub use linux_sandbox::probe_syscall;
#[cfg(target_os = "linux")]
pub use linux_sandbox::{
    apply as child_apply_sandbox, close_inherited_descriptors, confined, host_landlock_abi,
};

#[cfg(not(any(windows, target_os = "linux")))]
mod unsupported;
#[cfg(not(any(windows, target_os = "linux")))]
pub use unsupported::{
    Containment, child_apply_limits, classify_exit, self_in_job, wait_for_exit, working_directory,
};
