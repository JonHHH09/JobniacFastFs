//! A migrated SQLite database in a fresh temporary directory, shared by the SQLite integration tests.
//!
//! Every test gets its own file, so tests run in parallel without a server or a feature flag. All
//! data is synthetic.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use jobniac_infra::sqlite::migrations::migrate;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use rusqlite::Connection;
use tempfile::TempDir;

pub type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// A migrated database file and a pool over it. The directory is removed on drop.
pub struct SqliteFixture {
    _dir: TempDir,
    path: PathBuf,
    pool: SqlitePool,
}

impl SqliteFixture {
    /// Creates the file, applies every packaged migration and opens a pool.
    pub fn start() -> Self {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("jobniac.sqlite3");
        migrate(&path).expect("packaged migrations apply");
        let pool = SqlitePool::open(&path).expect("pool opens");
        Self {
            _dir: dir,
            path,
            pool,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn pool(&self) -> SqlitePool {
        self.pool.clone()
    }

    pub fn unit_of_work(&self) -> SqliteUnitOfWork {
        SqliteUnitOfWork::new(self.pool.clone())
    }

    /// A separate raw connection for seeding and asserting rows directly, outside the pool.
    pub fn connection(&self) -> Connection {
        let connection = Connection::open(&self.path).expect("raw connection opens");
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 10000;")
            .expect("raw connection pragmas");
        connection
    }
}
