//! Scratch directories in the canonical (final-path) form of the system temporary folder.
//!
//! The import root, the generated root and the export refuse a path whose text differs from its
//! final path. On Windows `%TEMP%` can be an 8.3 alias (a short user-profile name) that the final path
//! expands, so a test that uses a root below `std::env::temp_dir()` as it stands would be refused.
//! The directories made here are built from the canonical temporary folder instead, without the
//! extended-length `\\?\` prefix. On other platforms the canonical form is the folder itself.

use std::io;
use std::path::PathBuf;

/// The system temporary folder in its final-path form, without the `\\?\` prefix.
///
/// # Errors
///
/// The I/O error of resolving the temporary folder.
pub fn canonical_temp_dir() -> io::Result<PathBuf> {
    std::fs::canonicalize(std::env::temp_dir()).map(strip_verbatim)
}

/// A fresh directory below [`canonical_temp_dir`], named with `prefix`. It is removed when the
/// returned guard is dropped.
///
/// # Errors
///
/// The I/O error of resolving the temporary folder or creating the directory.
pub fn scratch_dir(prefix: &str) -> io::Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(canonical_temp_dir()?)
}

/// Removes the `\\?\` verbatim prefix that `canonicalize` adds on Windows (a UNC path keeps it).
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let stripped = path
        .to_str()
        .and_then(|text| text.strip_prefix(r"\\?\"))
        .filter(|rest| !rest.starts_with("UNC\\"))
        .map(PathBuf::from);
    stripped.unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_scratch_directory_is_absolute_and_removed_on_drop() -> io::Result<()> {
        let directory = scratch_dir("jobniac-scratch-")?;
        let path: &Path = directory.path();
        assert!(path.is_absolute());
        assert!(path.is_dir());
        assert!(!path.to_string_lossy().starts_with(r"\\?\"));
        let owned = path.to_path_buf();
        drop(directory);
        assert!(!owned.exists());
        Ok(())
    }

    #[test]
    fn verbatim_prefix_is_only_stripped_for_drive_paths() {
        assert_eq!(
            strip_verbatim(PathBuf::from("/tmp/x")),
            PathBuf::from("/tmp/x")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\Temp")),
            PathBuf::from(r"C:\Temp")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\host\share")),
            PathBuf::from(r"\\?\UNC\host\share")
        );
    }
}
