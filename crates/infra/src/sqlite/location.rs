//! Preparing the directory of the database file.
//!
//! The path itself is resolved by `jobniac_core::database_location`; this is the filesystem half:
//! creating the missing directories owner-only before [`SqlitePool::open`](crate::sqlite::pool),
//! and keeping the database file itself owner-only ([`secure_database_file`]).

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

/// Make sure the database file exists and is owner-only, before SQLite opens it.
///
/// On Unix a missing file is created with mode `0600` (SQLite gives the `-wal` and `-shm` files the
/// mode of the main file), and an existing file with looser permissions is tightened to `0600`. A
/// file that is already `0600` or stricter is left alone. On Windows nothing is changed: the file
/// inherits the ACL of its directory, which under `%LOCALAPPDATA%` is limited to the user, `SYSTEM`
/// and administrators. The owner-only DACL primitives of the exporter are private to its handle
/// chain and are not reused for a file that SQLite opens by name.
///
/// # Errors
///
/// The I/O error of the creation or the permission change. The caller reports it as an
/// unavailable database, without the path.
pub fn secure_database_file(database_file: &Path) -> io::Result<()> {
    secure_file(database_file)
}

#[cfg(unix)]
fn secure_file(file: &Path) -> io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    // `create_new` never truncates; the mode applies only to the file this call creates.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(file)
    {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let mode = std::fs::metadata(file)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(mode & 0o700))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn secure_file(_file: &Path) -> io::Result<()> {
    Ok(())
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

    #[cfg(unix)]
    #[test]
    fn the_database_file_is_created_owner_only_and_a_loose_one_is_tightened() -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| -> io::Result<u32> {
            Ok(std::fs::metadata(path)?.permissions().mode() & 0o777)
        };
        let root = tempfile::tempdir()?;
        let file = root.path().join("jobniac.sqlite3");
        secure_database_file(&file)?;
        assert_eq!(mode(&file)?, 0o600);
        std::fs::write(&file, b"kept")?;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o664))?;
        secure_database_file(&file)?;
        assert_eq!(mode(&file)?, 0o600);
        assert_eq!(std::fs::read(&file)?, b"kept", "the content is not touched");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o400))?;
        secure_database_file(&file)?;
        assert_eq!(mode(&file)?, 0o400, "a stricter file is left alone");
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
