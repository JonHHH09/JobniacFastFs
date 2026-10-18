//! Opening the local SQLite database for the composition root and the CLI.
//!
//! The location comes from the environment snapshot (`JOBNIAC_DB_PATH` or the per-user default, see
//! [`jobniac_core::database_location`]). Opening creates the owner-only directory, brings the schema
//! up to date and opens the connection pool. Every failure is collapsed to `None`: the callers keep
//! their fixed `Local database is not configured` and health DOWN behaviour, and no message names
//! a path.

use std::path::PathBuf;

use jobniac_core::database_location::resolve_database_path;
use jobniac_core::env::Environment;
use jobniac_core::generated_root::RootPlatform;
use jobniac_infra::sqlite::location::create_database_directory;
use jobniac_infra::sqlite::migrations::migrate;
use jobniac_infra::sqlite::pool::SqlitePool;

use crate::generation::platform_name;

/// The database file of `env`, or `None` when no location can be resolved.
#[must_use]
pub fn database_path(env: &Environment) -> Option<PathBuf> {
    resolve_database_path(env, RootPlatform::from_platform_name(platform_name()))
        .ok()
        .map(PathBuf::from)
}

/// Resolves the database file, creates its directory, migrates it and opens the pool.
///
/// `None` when the location cannot be resolved, the directory cannot be created, the schema cannot
/// be brought up to date (including a database written by a newer version) or the file cannot be
/// opened.
#[must_use]
pub fn open_database(env: &Environment) -> Option<SqlitePool> {
    let path = database_path(env)?;
    prepare(&path)
}

/// [`open_database`] for an already resolved `path`.
#[must_use]
pub fn prepare(path: &std::path::Path) -> Option<SqlitePool> {
    create_database_directory(path).ok()?;
    migrate(path).ok()?;
    SqlitePool::open(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_environment_has_no_database() {
        let env = Environment::default();
        assert!(database_path(&env).is_none());
        assert!(open_database(&env).is_none());
    }

    #[test]
    fn an_unusable_override_opens_nothing() {
        let env = Environment::from_pairs([("JOBNIAC_DB_PATH", "relative/jobniac.sqlite3")]);
        assert!(open_database(&env).is_none());
    }

    #[test]
    fn the_override_is_created_migrated_and_opened() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let file = root.path().join("nested").join("jobniac.sqlite3");
        let text = file.to_str().ok_or("temporary path is not Unicode")?;
        let env = Environment::from_pairs([("JOBNIAC_DB_PATH", text)]);
        let pool = open_database(&env).ok_or("the database must open")?;
        assert_eq!(pool.path(), Some(file.as_path()));
        assert!(file.exists());
        // A second start finds the schema current.
        assert!(open_database(&env).is_some());
        Ok(())
    }
}
