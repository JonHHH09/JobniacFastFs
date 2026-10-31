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

/// Resolves the links in the base directory of the per-user default root.
///
/// The default is derived from the home directory, which may legitimately be reached through a
/// link (macOS `/var` temporary directories, a relocated or network home, `/home` pointing at
/// `/var/home`), while opening the root never follows a link. The last `verbatim` components of
/// `path` (the app folder and the leaf, so `2` for `<base>/<app>/generated`) are kept exactly as
/// given; everything above them is the base. The existing part of the base is replaced by its real
/// location and its not yet created rest is kept. A link standing in place of a verbatim component
/// is therefore still refused when the root is opened. Nothing is created. Windows paths and paths
/// that cannot be resolved are returned unchanged (a canonical Windows path carries the verbatim
/// `\\?\` prefix, which the open refuses).
#[must_use]
pub fn resolve_parent_links(path: &str, verbatim: usize) -> String {
    if cfg!(windows) {
        return path.to_owned();
    }
    let mut tail = Vec::new();
    let mut base = Path::new(path);
    for _ in 0..verbatim {
        let (Some(up), Some(name)) = (base.parent(), base.file_name()) else {
            return path.to_owned();
        };
        tail.push(name);
        base = up;
    }
    let mut missing = Vec::new();
    let mut probe = base;
    let mut resolved = loop {
        if let Ok(real) = fs::canonicalize(probe) {
            break real;
        }
        match (probe.parent(), probe.file_name()) {
            (Some(up), Some(name)) => {
                missing.push(name);
                probe = up;
            }
            _ => return path.to_owned(),
        }
    };
    resolved.extend(missing.into_iter().rev());
    resolved.extend(tail.into_iter().rev());
    resolved
        .to_str()
        .map_or_else(|| path.to_owned(), str::to_owned)
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn text(path: &Path) -> Result<&str, &'static str> {
        path.to_str().ok_or("temporary path is not Unicode")
    }

    #[test]
    fn a_linked_base_is_replaced_by_its_real_location_and_the_rest_is_kept() -> TestResult {
        let dir = tempfile::tempdir()?;
        let top = fs::canonicalize(dir.path())?;
        let real = top.join("real");
        fs::create_dir(&real)?;
        let link = top.join("link");
        symlink(&real, &link)?;
        let requested = link.join("missing").join("Jobniac").join("generated");
        let resolved = resolve_parent_links(text(&requested)?, 2);
        assert_eq!(
            resolved,
            text(&real.join("missing").join("Jobniac").join("generated"))?
        );
        Ok(())
    }

    #[test]
    fn the_leaf_is_never_resolved() -> TestResult {
        let dir = tempfile::tempdir()?;
        let base = fs::canonicalize(dir.path())?;
        let elsewhere = base.join("elsewhere");
        fs::create_dir(&elsewhere)?;
        symlink(&elsewhere, base.join("generated"))?;
        let requested = base.join("generated");
        assert_eq!(
            resolve_parent_links(text(&requested)?, 1),
            text(&requested)?
        );
        Ok(())
    }

    #[test]
    fn a_linked_app_folder_is_not_resolved() -> TestResult {
        let dir = tempfile::tempdir()?;
        let top = fs::canonicalize(dir.path())?;
        let elsewhere = top.join("elsewhere");
        fs::create_dir(&elsewhere)?;
        symlink(&elsewhere, top.join("jobniac"))?;
        let requested = top.join("jobniac").join("generated");
        assert_eq!(
            resolve_parent_links(text(&requested)?, 2),
            text(&requested)?
        );
        Ok(())
    }

    #[test]
    fn a_linked_leaf_below_a_linked_base_is_not_resolved() -> TestResult {
        let dir = tempfile::tempdir()?;
        let top = fs::canonicalize(dir.path())?;
        let real = top.join("real");
        fs::create_dir_all(real.join("jobniac"))?;
        let elsewhere = top.join("elsewhere");
        fs::create_dir(&elsewhere)?;
        symlink(&elsewhere, real.join("jobniac").join("generated"))?;
        let link = top.join("link");
        symlink(&real, &link)?;
        let requested = link.join("jobniac").join("generated");
        let expected = real.join("jobniac").join("generated");
        let resolved = resolve_parent_links(text(&requested)?, 2);
        assert_eq!(resolved, text(&expected)?);
        assert!(!is_plain_directory(&resolved));
        Ok(())
    }

    #[test]
    fn a_dangling_linked_base_is_returned_unchanged_and_refused() -> TestResult {
        let dir = tempfile::tempdir()?;
        let top = fs::canonicalize(dir.path())?;
        symlink(top.join("nowhere"), top.join("dangling"))?;
        let requested = top.join("dangling").join("jobniac").join("generated");
        let resolved = resolve_parent_links(text(&requested)?, 2);
        assert_eq!(resolved, text(&requested)?);
        assert!(!is_plain_directory(&resolved));
        assert!(!ensure_private_directory(&resolved));
        Ok(())
    }

    #[test]
    fn a_path_that_cannot_be_resolved_is_returned_unchanged() {
        assert_eq!(resolve_parent_links("relative", 1), "relative");
        assert_eq!(resolve_parent_links("/", 1), "/");
        assert_eq!(resolve_parent_links("/a", 2), "/a");
    }
}
