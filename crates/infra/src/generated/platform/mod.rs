//! Platform primitives of the owned generated-asset namespace.
//!
//! Each target supplies the same operations behind `pub(crate)` names: Linux (descriptor-pinned
//! `/proc/self/fd` paths and anonymous `O_TMPFILE` publication), Windows (pinned handles with no
//! `FILE_SHARE_DELETE`, create-new private files, no-replace rename and protected DACL checks) and
//! a fail-closed stub for every other target. The generic protocol in
//! [`crate::generated::asset_files`] is identical on all of them.

use std::io;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::{
    Root, capability_bytes, create_private_directory, stamp_of, supported, sync_existing,
};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{
    Root, capability_bytes, create_private_directory, stamp_of, supported, sync_existing,
};

#[cfg(not(any(target_os = "linux", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) use unsupported::{
    Root, capability_bytes, create_private_directory, stamp_of, supported, sync_existing,
};

/// How an existing child is opened. Both platforms refuse links and non-regular files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// Read the content and metadata only.
    Read,
    /// Read, and keep the right to delete the held file (retirement and removal).
    ReadDelete,
    /// The namespace lock: exclusive against every other holder while open.
    Exclusive,
}

/// Outcome of an anonymous publication under a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// macOS fails closed here (see the platform modules), so only Linux and Windows use this.
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub(crate) enum Publish {
    /// The name was created with the complete content and verified.
    Created,
    /// Something already holds the name; the caller compares its content.
    NameExists,
}

/// Failure of an anonymous publication, before the caller's content comparison.
#[derive(Debug)]
// macOS fails closed here (see the platform modules), so only Linux and Windows use this.
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub(crate) enum PublishError {
    /// The published object does not match the content, or the name was swapped.
    Mismatch,
    /// The platform cannot publish without a named partial file (only the Linux primitives
    /// report this).
    #[allow(dead_code)]
    Unsupported,
    /// Any other I/O failure; the message is never surfaced.
    Io(io::Error),
}
