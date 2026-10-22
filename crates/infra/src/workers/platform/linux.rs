//! Linux containment: resource limits applied by the child to itself. The process group that is
//! killed as one unit lives in `unix.rs`, shared with macOS.
//!
//! The parent never uses `pre_exec`. The child calls `setrlimit` in its own main before it reads
//! its request, and reads each value back.

use std::io;

use rlimit::Resource;

use super::unix::{apply_limit, reset_signals};
use crate::workers::kinds::WorkerLimits;

pub use super::unix::{Containment, classify_exit, self_in_job, wait_for_exit, working_directory};

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
