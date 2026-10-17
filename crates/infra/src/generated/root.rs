//! Filesystem side of the generated root: private creation of a missing default directory and
//! plain-directory checks.
//!
//! Path resolution itself is pure and lives in `jobniac_core::generated_root`. This module only
//! touches the filesystem, and only to create the missing components of a root path that the
//! caller has already admitted.

use crate::generated::platform;
use jobniac_core::generated_root::RootPlatform;
use std::fs;
use std::io;
use std::path::Path;

/// The platform class of the running host, for the default-root resolver.
#[must_use]
pub const fn host_platform() -> RootPlatform {
    if cfg!(windows) {
        RootPlatform::Windows
    } else if cfg!(target_os = "macos") {
        RootPlatform::MacOs
    } else {
        RootPlatform::Other
    }
}

/// Whether `path` is a real directory: not a symbolic link, junction or other reparse point.
#[must_use]
pub fn is_plain_directory(path: &str) -> bool {
    let Ok(info) = fs::symlink_metadata(path) else {
        return false;
    };
    if !info.is_dir() || info.is_symlink() {
        return false;
    }
    !is_reparse_point(&info)
}

#[cfg(windows)]
fn is_reparse_point(info: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    info.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
const fn is_reparse_point(_info: &fs::Metadata) -> bool {
    false
}

/// Creates the missing components of `path` with private permissions (owner only on POSIX; a
/// protected DACL for the owner, SYSTEM and Administrators on Windows).
///
/// Existing components are never modified, and a component that appears concurrently is accepted.
/// Returns `false` on any failure, when a file stands in
/// the way, or when the final path is not a plain directory afterwards.
#[must_use]
pub fn ensure_private_directory(path: &str) -> bool {
    let target = Path::new(path);
    let mut missing: Vec<&Path> = Vec::new();
    let mut probe = target;
    while fs::symlink_metadata(probe).is_err() {
        missing.push(probe);
        match probe.parent() {
            Some(parent) if parent != probe => probe = parent,
            _ => return false,
        }
    }
    for component in missing.iter().rev() {
        let Some(text) = component.to_str() else {
            return false;
        };
        match platform::create_private_directory(text) {
            Ok(()) => {}
            // Another process (the desktop app and an MCP server starting together on first run)
            // created it after the probe. The final plain-directory check still rejects a file or
            // a link, and opening the root later validates owner and mode.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return false,
        }
    }
    is_plain_directory(path)
}
