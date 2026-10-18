//! Preparing the directory of the database file.
//!
//! The path itself is resolved by `jobniac_core::database_location`; this is the filesystem half:
//! creating the missing directories owner-only before [`SqlitePool::open`](crate::sqlite::pool).

use std::io;
use std::path::Path;

/// Create the missing parent directories of `database_file`, owner-only.
///
/// On Unix every directory this call creates has mode `0700`; an existing directory is left as it
/// is (its mode is not changed). On Windows the new directories inherit the ACL of their parent,
/// which under `%LOCALAPPDATA%` is limited to the user, `SYSTEM` and administrators. Creating
/// nothing (the directory exists) is success.
///
/// # Errors
///
/// The I/O error of the creation. The caller reports it as an unavailable database, without the
/// path.
pub fn create_database_directory(database_file: &Path) -> io::Result<()> {
    let Some(parent) = database_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    create_owner_only(parent)
}

#[cfg(unix)]
fn create_owner_only(directory: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
}

#[cfg(not(unix))]
fn create_owner_only(directory: &Path) -> io::Result<()> {
    std::fs::create_dir_all(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_directories_are_created_and_the_call_is_idempotent() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let file = root.path().join("a").join("b").join("jobniac.sqlite3");
        create_database_directory(&file)?;
        assert!(root.path().join("a").join("b").is_dir());
        assert!(!file.exists(), "only the directory is created");
        create_database_directory(&file)
    }

    #[cfg(unix)]
    #[test]
    fn created_directories_are_owner_only_and_existing_ones_are_left_alone() -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| -> io::Result<u32> {
            Ok(std::fs::metadata(path)?.permissions().mode() & 0o777)
        };
        let root = tempfile::tempdir()?;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755))?;
        let file = root
            .path()
            .join("data")
            .join("nested")
            .join("jobniac.sqlite3");
        create_database_directory(&file)?;
        assert_eq!(mode(&root.path().join("data"))?, 0o700);
        assert_eq!(mode(&root.path().join("data").join("nested"))?, 0o700);
        assert_eq!(
            mode(root.path())?,
            0o755,
            "an existing directory is not changed"
        );
        Ok(())
    }

    #[test]
    fn a_bare_file_name_needs_no_directory() -> io::Result<()> {
        create_database_directory(Path::new("jobniac.sqlite3"))
    }

    #[test]
    fn a_file_in_the_way_is_an_error() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let blocker = root.path().join("blocker");
        std::fs::write(&blocker, b"x")?;
        assert!(create_database_directory(&blocker.join("jobniac.sqlite3")).is_err());
        Ok(())
    }
}
