//! Operator-directed PDF export: a new, private, complete file with no partial name and no
//! replacement.
//!
//! * Linux ([`platform_linux`]): an anonymous `O_TMPFILE` inode in the held parent, written, fsynced
//!   and linked in with `linkat` through its `/proc/self/fd` name. `EEXIST` means no replacement.
//!   It uses the rustix safe API and contains no `unsafe` code.
//! * Windows ([`platform_windows`]): an exclusive, owner-only `.jobniac-tmp-` file in a pinned
//!   directory chain, then a no-replace rename on the held handle. The temporary name is briefly
//!   visible.
//! * macOS ([`platform_macos`]): an exclusive, owner-only `.jobniac-tmp-` file in the held parent,
//!   fsynced with `F_FULLFSYNC`, then a no-replace `renameatx_np` (`RENAME_EXCL`). The temporary
//!   name is briefly visible and is removed on every failure. It uses the rustix safe API.
//! * Every other target refuses before it touches the filesystem.
//!
//! This module is orchestration only: destination syntax, the protected-root snapshot and the
//! mapping to the two fixed outcomes ([`ExportFailure`]). It contains no FFI. Every system call
//! lives in a platform module, and the only `unsafe` code is in `platform_windows`, with a
//! `SAFETY:` comment on each block.
//!
//! Follow-up: the generated-root platform layer (`crate::generated::platform`) repeats the
//! held-chain, `geteuid` and private-DACL primitives of this exporter. The two should be merged
//! into one platform layer; that is deliberately not done here.

use std::path::Path;

use jobniac_core::artifacts::{
    ArtifactExporter, DocumentArtifact, ExportFailure, validate_artifact,
};

#[cfg(target_os = "linux")]
mod platform_linux;
#[cfg(target_os = "macos")]
mod platform_macos;
#[cfg(windows)]
mod platform_windows;

/// The longest destination name, in UTF-8 bytes.
const MAX_NAME_BYTES: usize = 4096;

/// The prefix of the temporary file on Windows and macOS (`TEMP_PREFIX`).
pub const TEMP_PREFIX: &str = ".jobniac-tmp-";

/// The native exporter. It holds the protected roots, which the destination may never equal, lie
/// inside, or be reached through an ancestor that is one of them.
#[derive(Debug, Clone, Default)]
pub struct AtomicExporter {
    // Other targets refuse before they read this (see `export_inner`).
    #[cfg_attr(
        not(any(target_os = "linux", target_os = "macos", windows)),
        allow(dead_code)
    )]
    protected_roots: Vec<String>,
}

impl AtomicExporter {
    /// An exporter that protects the given roots (the import root and the generated root).
    /// Roots that are not valid on this platform are dropped, which fails closed for them only.
    #[must_use]
    pub fn new(protected_roots: Vec<String>) -> Self {
        Self { protected_roots }
    }
}

/// Creates a new, empty, owner-only directory: mode 0700 on POSIX,
/// and on Windows a protected DACL that grants only SYSTEM and the current user, with
/// inheritance to children. An existing name is an error.
///
/// # Errors
///
/// The operating-system error of the creation, or an error on a platform without support.
pub fn create_private_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(windows)]
    {
        platform_windows::create_private_directory(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(std::io::Error::other("unsupported platform"))
    }
}

/// The internal failure. `Refused` leaves no name; `Published` means the link or rename succeeded.
#[derive(Debug)]
// Other targets refuse before they use this (see `export_inner`).
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", windows)),
    allow(dead_code)
)]
enum Failure {
    Refused,
    Published,
}

impl ArtifactExporter for AtomicExporter {
    fn export(
        &self,
        artifact: &DocumentArtifact,
        destination: &str,
    ) -> Result<String, ExportFailure> {
        let mut published = false;
        let outcome = export_inner(self, artifact, destination, &mut published);
        match outcome {
            Ok(name) => Ok(name),
            Err(_) if published => Err(ExportFailure::Uncertain),
            Err(_) => Err(ExportFailure::Refused),
        }
    }
}

/// The export body. Every error becomes a [`Failure`]; no OS text is kept.
fn export_inner(
    exporter: &AtomicExporter,
    artifact: &DocumentArtifact,
    destination: &str,
    published: &mut bool,
) -> Result<String, Failure> {
    let artifact = validate_artifact(artifact.clone()).map_err(|_| Failure::Refused)?;
    let (parent, name) = split_destination(destination).ok_or(Failure::Refused)?;
    #[cfg(target_os = "linux")]
    {
        platform_linux::publish(exporter, &artifact, &parent, &name, published)
    }
    #[cfg(target_os = "macos")]
    {
        platform_macos::publish(exporter, &artifact, &parent, &name, published)
    }
    #[cfg(windows)]
    {
        platform_windows::publish(exporter, &artifact, &parent, &name, published)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (exporter, artifact, parent, name, published);
        Err(Failure::Refused)
    }
}

/// Splits and checks a destination: the platform syntax, a `.pdf` name with a stem, and the
/// 4096-byte name limit. Returns the parent directory and the file name.
fn split_destination(destination: &str) -> Option<(String, String)> {
    let (parent, name) = if cfg!(windows) {
        windows_syntax(destination)?
    } else {
        posix_syntax(destination)?
    };
    let lower = name.to_ascii_lowercase();
    let has_stem = name.len() > 4;
    if !has_stem || !lower.ends_with(".pdf") || name.len() > MAX_NAME_BYTES {
        return None;
    }
    Some((parent, name))
}

/// POSIX syntax: absolute, no `//` prefix, no NUL, no backslash, no `.` or `..` component.
fn posix_syntax(destination: &str) -> Option<(String, String)> {
    if !destination.starts_with('/')
        || destination.starts_with("//")
        || destination.contains('\0')
        || destination.contains('\\')
        || destination
            .split('/')
            .any(|part| part == "." || part == "..")
    {
        return None;
    }
    let index = destination.rfind('/')?;
    let parent = if index == 0 {
        "/".to_owned()
    } else {
        destination[..index].to_owned()
    };
    let name = destination[index + 1..].to_owned();
    if name.is_empty() {
        return None;
    }
    Some((parent, name))
}

/// Windows syntax: drive-absolute only, through the shared lexical rules.
fn windows_syntax(destination: &str) -> Option<(String, String)> {
    if destination.starts_with(['\\', '/']) || destination.contains('\0') {
        return None;
    }
    let normalized = crate::files::windows_path::normalize_drive_path(destination).ok()?;
    let index = normalized.rfind('\\')?;
    let parent = if index == 2 {
        normalized[..3].to_owned()
    } else {
        normalized[..index].to_owned()
    };
    let name = normalized[index + 1..].to_owned();
    if name.is_empty() {
        return None;
    }
    Some((parent, name))
}

/// Protected-root snapshot: the real path of each root, or its text when it does not exist.
// Other targets refuse before they use this (see `export_inner`).
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", windows)),
    allow(dead_code)
)]
fn protected_snapshot(roots: &[String]) -> Vec<String> {
    roots
        .iter()
        .map(|root| {
            std::fs::canonicalize(root)
                .map_or_else(|_| root.clone(), |path| path.to_string_lossy().into_owned())
        })
        .collect()
}

/// Whether `path` equals or lies inside any configured or resolved protected root.
// Other targets refuse before they use this (see `export_inner`).
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", windows)),
    allow(dead_code)
)]
fn inside_protected(path: &Path, roots: &[String], snapshot: &[String]) -> bool {
    roots
        .iter()
        .chain(snapshot.iter())
        .map(Path::new)
        .any(|root| path == root || path.starts_with(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_requires_a_pdf_name_with_a_stem() {
        assert!(
            split_destination(if cfg!(windows) {
                r"C:\x\a.PDF"
            } else {
                "/x/a.PDF"
            })
            .is_some()
        );
        assert!(
            split_destination(if cfg!(windows) {
                r"C:\x\.pdf"
            } else {
                "/x/.pdf"
            })
            .is_none()
        );
        assert!(
            split_destination(if cfg!(windows) {
                r"C:\x\a.txt"
            } else {
                "/x/a.txt"
            })
            .is_none()
        );
    }

    #[test]
    fn posix_syntax_refuses_relative_traversal_and_backslashes() {
        assert!(posix_syntax("relative/a.pdf").is_none());
        assert!(posix_syntax("/x/../a.pdf").is_none());
        assert!(posix_syntax("//net/a.pdf").is_none());
        assert!(posix_syntax("/x\\a.pdf").is_none());
        assert!(posix_syntax("/x/a\0.pdf").is_none());
    }

    #[test]
    fn name_limit_is_utf8_bytes() {
        let long = format!("/{}.pdf", "a".repeat(MAX_NAME_BYTES));
        assert!(split_destination(&long).is_none());
    }
}
