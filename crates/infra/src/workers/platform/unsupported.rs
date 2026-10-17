//! Containment for platforms without a worker sandbox. Every worker operation fails closed.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

use tokio::process::{Child, Command};

use crate::workers::error::ExitClass;
use crate::workers::kinds::WorkerLimits;

/// Containment that refuses to start: this platform has no worker sandbox.
#[derive(Debug)]
pub struct Containment;

impl Containment {
    /// Always refuses: no sandbox exists on this platform.
    ///
    /// # Errors
    ///
    /// Always returns [`io::ErrorKind::Unsupported`].
    pub fn prepare(_limits: &WorkerLimits) -> io::Result<Self> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    /// Nothing to configure.
    pub fn configure(_command: &mut Command) {}

    /// Never reached, because [`Containment::prepare`] refuses first.
    ///
    /// # Errors
    ///
    /// Always returns [`io::ErrorKind::Unsupported`].
    pub fn attach(&mut self, _child: &Child) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    /// Nothing to kill.
    pub fn kill_tree(&self) {}

    /// Nothing to forget.
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

/// Nothing to apply: the child refuses to run on this platform.
///
/// # Errors
///
/// Always returns [`io::ErrorKind::Unsupported`].
pub fn child_apply_limits(_limits: &WorkerLimits) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// No working directory: the platform has no fixed neutral directory.
#[must_use]
pub fn working_directory(_source: &[(OsString, OsString)]) -> Option<PathBuf> {
    None
}

/// Job membership of the calling process: no containment exists on this platform.
#[must_use]
pub fn self_in_job() -> Option<bool> {
    None
}

/// The class of an exit status.
#[must_use]
pub fn classify_exit(status: ExitStatus) -> ExitClass {
    status.code().map_or(ExitClass::Unknown, ExitClass::Code)
}
