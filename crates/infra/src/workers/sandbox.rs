//! Self-confinement of worker children.
//!
//! A child calls [`apply`] once, after it has loaded what it needs from the file system and before
//! it handles untrusted input. On Linux the call installs `no_new_privs`, a Landlock ruleset and a
//! seccomp-BPF filter (see `platform/linux_sandbox.rs`). On other platforms the Job Object of the
//! parent is the only containment, and [`apply`] reports [`SandboxLevel::NotApplicable`].

use crate::workers::child::WorkerFault;

/// What a child may still do once confined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxProfile {
    /// PDF text extraction: no file system access, no sockets at all.
    PdfExtract,
    /// PDF page rendering: like [`SandboxProfile::PdfExtract`], plus read-only access to the system
    /// font directories that PDFium searches for fonts a document does not embed.
    PdfRender,
    /// Outbound fetch: no file system access; IPv4 and IPv6 sockets only.
    Fetch,
}

/// The confinement that [`apply`] actually reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxLevel {
    /// Landlock fully enforced (file system, and TCP for the PDF profile, and scopes) plus seccomp.
    Full,
    /// Landlock enforced in part (an older ABI than v6) plus seccomp.
    Degraded,
    /// The kernel has no usable Landlock; only `no_new_privs` and seccomp apply.
    SeccompOnly,
    /// This platform has no child self-confinement (Windows relies on its Job Object).
    NotApplicable,
}

impl SandboxLevel {
    /// A fixed name for reports and tests.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Degraded => "degraded",
            Self::SeccompOnly => "seccomp-only",
            Self::NotApplicable => "not-applicable",
        }
    }
}

/// Confines the calling process for `profile`.
///
/// # Errors
///
/// Returns [`WorkerFault`] when a mandatory layer cannot be installed (Linux: `no_new_privs` or the
/// seccomp filter), so the child refuses to run unconfined. A missing Landlock is not an error: the
/// returned level says what was reached.
#[cfg(target_os = "linux")]
pub fn apply(profile: SandboxProfile) -> Result<SandboxLevel, WorkerFault> {
    crate::workers::platform::child_apply_sandbox(profile).map_err(|_| WorkerFault)
}

/// Confines the calling process for `profile`. This platform has no self-confinement.
///
/// # Errors
///
/// Never fails here.
#[cfg(not(target_os = "linux"))]
pub fn apply(_profile: SandboxProfile) -> Result<SandboxLevel, WorkerFault> {
    Ok(SandboxLevel::NotApplicable)
}

/// The tripwire a child checks before it returns a response: whether the confinement of its kind is
/// in force (Linux: `no_new_privs` set and a seccomp filter installed). A kind whose operation
/// returns without having confined itself fails here, so skipping the confinement is not silent.
///
/// # Errors
///
/// Returns [`WorkerFault`] when the confinement is not in force.
pub fn verify_confined() -> Result<(), WorkerFault> {
    #[cfg(target_os = "linux")]
    {
        if crate::workers::platform::confined() {
            Ok(())
        } else {
            Err(WorkerFault)
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

/// The level that a child on this host would reach, for a parent that reports a degraded host. It
/// does not confine anything.
#[must_use]
pub fn host_level() -> SandboxLevel {
    #[cfg(target_os = "linux")]
    {
        match crate::workers::platform::host_landlock_abi() {
            Some(version) if version >= 6 => SandboxLevel::Full,
            Some(_) => SandboxLevel::Degraded,
            None => SandboxLevel::SeccompOnly,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        SandboxLevel::NotApplicable
    }
}

/// Logs once per process that children on this host are confined less than fully. The supervisor
/// calls it before it spawns a child, so an operator sees the degradation without any child output
/// (children write no diagnostics). Nothing is logged where children are fully confined or where
/// the platform has no self-confinement.
pub fn warn_once_if_degraded() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| match host_level() {
        SandboxLevel::Degraded => tracing::warn!(
            "worker sandbox is degraded: this kernel supports an older Landlock ABI than 6; \
             seccomp and the partial Landlock ruleset still apply"
        ),
        SandboxLevel::SeccompOnly => tracing::warn!(
            "worker sandbox is seccomp-only: this kernel has no usable Landlock, so worker \
             children keep file system access"
        ),
        SandboxLevel::Full | SandboxLevel::NotApplicable => {}
    });
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// The test process never confines itself, so the tripwire must refuse it.
    #[test]
    fn the_tripwire_refuses_an_unconfined_process() {
        assert!(verify_confined().is_err());
    }
}
