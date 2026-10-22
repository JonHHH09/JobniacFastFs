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
//! Linux and macOS share the process-group handling in `unix.rs`. macOS adds a `MemoryWatch`, which
//! the supervisor polls to enforce the memory limit that `setrlimit` cannot enforce there; other
//! platforms have a watch that never fires.
//!
//! Platforms other than Windows, Linux and macOS fail closed: `Containment::prepare` refuses.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{
    Containment, child_apply_limits, classify_exit, self_in_job, wait_for_exit, working_directory,
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;

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

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_sandbox;
#[cfg(target_os = "macos")]
pub use macos::{
    Containment, MemoryWatch, child_apply_limits, classify_exit, close_inherited_descriptors,
    self_in_job, wait_for_exit, working_directory,
};
#[cfg(target_os = "macos")]
pub use macos_sandbox::{apply as child_apply_sandbox, confined};

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub use unsupported::{
    Containment, child_apply_limits, classify_exit, self_in_job, wait_for_exit, working_directory,
};

/// The parent's memory watch of a child. Only macOS needs one (see `macos.rs`); here it never
/// fires.
#[cfg(not(target_os = "macos"))]
#[derive(Debug)]
pub struct MemoryWatch;

#[cfg(not(target_os = "macos"))]
impl MemoryWatch {
    /// A watch that never fires.
    #[must_use]
    pub fn new(_child: &tokio::process::Child, _limit: u64) -> Self {
        Self
    }

    /// Pends forever: the memory limit is enforced by the platform itself.
    pub async fn exceeded(&mut self) {
        std::future::pending::<()>().await;
    }
}
