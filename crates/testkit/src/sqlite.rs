//! A database file in a temporary directory, for tests that run the application against SQLite.
//!
//! The helper only names a location: the application creates, migrates and opens the file itself
//! (`jobniac_runtime::database::open_database`, `jobniac-cli migrate`). The directory and the file
//! are removed when the value is dropped.

use std::io;
use std::path::{Path, PathBuf};

use jobniac_core::database_location::{DATABASE_FILE, DB_PATH_VARIABLE};

/// A database location inside a fresh temporary directory.
#[derive(Debug)]
pub struct TempDatabase {
    directory: tempfile::TempDir,
    file: PathBuf,
    text: String,
}

impl TempDatabase {
    /// A location whose file does not exist yet.
    ///
    /// # Errors
    ///
    /// The I/O error of creating the directory, or `InvalidData` when the temporary path is not
    /// valid Unicode.
    pub fn new() -> io::Result<Self> {
        let directory = tempfile::tempdir()?;
        let file = directory.path().join(DATABASE_FILE);
        let text = file
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "temporary path"))?
            .to_owned();
        Ok(Self {
            directory,
            file,
            text,
        })
    }

    /// The database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.file
    }

    /// The directory that holds the file.
    #[must_use]
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }

    /// The file path as text, the value of `JOBNIAC_DB_PATH`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The `JOBNIAC_DB_PATH` pair that points the application at this file.
    #[must_use]
    pub fn environment_pair(&self) -> (&'static str, &str) {
        (DB_PATH_VARIABLE, &self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_location_is_absolute_unique_and_removed_on_drop() -> io::Result<()> {
        let first = TempDatabase::new()?;
        let second = TempDatabase::new()?;
        assert!(first.path().is_absolute());
        assert_ne!(first.path(), second.path());
        assert!(!first.path().exists());
        assert_eq!(
            first.environment_pair(),
            ("JOBNIAC_DB_PATH", first.as_str())
        );
        let directory = first.directory().to_path_buf();
        drop(first);
        assert!(!directory.exists());
        Ok(())
    }
}
