//! The SQLite connection pool: one writer, a few readers.
//!
//! SQLite allows many readers and a single writer at a time (WAL mode), so the pool mirrors that:
//!
//! * One **writer** connection behind an async mutex. A read-write scope takes it for the whole
//!   scope, which serializes writers inside the process; `BEGIN IMMEDIATE` serializes them against
//!   other processes (the desktop app and the MCP server can open the same file).
//! * Up to [`DEFAULT_READERS`] **reader** connections, opened read-only on first use and kept for
//!   reuse, bounded by a semaphore. A read-only scope takes one for its snapshot.
//!
//! Pragmas applied to every connection: `foreign_keys=ON`, `busy_timeout=10000` (the old
//! `lock_timeout`), `synchronous=NORMAL`; the writer also switches the file to `journal_mode=WAL`
//! (a persistent property of the database file).
//!
//! A caller waits at most [`DEFAULT_ACQUIRE_TIMEOUT`] for a free connection; then the database is
//! reported unavailable. A connection is returned to the pool only by its release (crate-internal),
//! after the owning transaction ended. A reader that was dropped without release (for example
//! because its future was cancelled) is closed, which rolls back its transaction. A writer that
//! was dropped inside a transaction is rolled back by the next acquire.
//!
//! Opening and the first use of a reader run blocking file I/O: call [`SqlitePool::open`] from
//! startup code, not from an async worker without [`crate::blocking::run_blocking`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use tokio::sync::{OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

use crate::blocking::run_blocking;
use crate::sqlite::error::SqliteError;
use crate::sqlite::health::check_engine_version;

/// `busy_timeout` of application connections.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// Default number of reader connections.
pub const DEFAULT_READERS: usize = 3;

/// How long [`SqlitePool::acquire`] waits for a free connection before it reports the database
/// unavailable.
pub const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the unit of work retries `BEGIN` or `COMMIT` after `SQLITE_BUSY` (on top of the
/// engine's own `busy_timeout` wait per attempt).
pub const DEFAULT_BUSY_RETRIES: u32 = 2;

/// Pause between two busy retries.
pub const DEFAULT_BUSY_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Prepared statements cached per connection.
const STATEMENT_CACHE: usize = 64;

/// Tunable limits of a pool. [`Default`] is the production setting; tests shorten the waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolOptions {
    /// Reader connections. Below 1 is raised to 1.
    pub readers: usize,
    /// The engine's `busy_timeout` of every connection.
    pub busy_timeout: Duration,
    /// How long an acquire waits for a free connection.
    pub acquire_timeout: Duration,
    /// Busy retries of `BEGIN` and `COMMIT` in the unit of work.
    pub busy_retries: u32,
    /// Pause between busy retries.
    pub busy_retry_delay: Duration,
}

impl Default for PoolOptions {
    fn default() -> Self {
        Self {
            readers: DEFAULT_READERS,
            busy_timeout: BUSY_TIMEOUT,
            acquire_timeout: DEFAULT_ACQUIRE_TIMEOUT,
            busy_retries: DEFAULT_BUSY_RETRIES,
            busy_retry_delay: DEFAULT_BUSY_RETRY_DELAY,
        }
    }
}

/// One connection behind a std mutex.
///
/// `rusqlite::Connection` is `Send` but not `Sync`. The mutex makes the holder `Sync`, so a
/// transaction handle can be borrowed across an `.await`. It is never contended: one scope owns the
/// connection.
pub(crate) struct Conn(Mutex<Connection>);

impl Conn {
    fn new(connection: Connection) -> Self {
        Self(Mutex::new(connection))
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        // A poisoned lock still holds a usable connection (the panic left no half-written state
        // that SQLite does not already roll back).
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn with<R>(&self, work: impl FnOnce(&Connection) -> R) -> R {
        work(&self.lock())
    }
}

struct Target {
    path: PathBuf,
    writer: Arc<tokio::sync::Mutex<Conn>>,
    reader_permits: Arc<Semaphore>,
    idle_readers: Mutex<Vec<Conn>>,
    max_idle: usize,
}

struct Shared {
    target: Option<Target>,
    options: PoolOptions,
    closed: AtomicBool,
}

/// A pool of connections to one database file. Cloning is cheap: all clones share the same writer
/// and readers. An unconfigured pool fails every acquire with [`SqliteError::NotConfigured`].
#[derive(Clone)]
pub struct SqlitePool {
    shared: Arc<Shared>,
}

impl fmt::Debug for SqlitePool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SqlitePool")
            .field("configured", &self.configured())
            .field("idle_readers", &self.idle_readers())
            .field("closed", &self.shared.closed.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl SqlitePool {
    /// A pool that is not configured: every acquire fails with [`SqliteError::NotConfigured`].
    pub fn unconfigured() -> Self {
        Self::build(None, PoolOptions::default())
    }

    /// Open the database file with the default limits, creating it when it does not exist.
    ///
    /// The parent directory must exist (see `database_location`). The writer connection is opened
    /// and configured now, so an unusable file is reported at startup.
    ///
    /// # Errors
    ///
    /// [`SqliteError::Unavailable`] when the file cannot be opened, the engine is older than the
    /// minimum version, or WAL mode cannot be enabled.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SqliteError> {
        Self::open_with(path, PoolOptions::default())
    }

    /// [`SqlitePool::open`] with explicit limits.
    ///
    /// # Errors
    ///
    /// As [`SqlitePool::open`].
    pub fn open_with(path: impl AsRef<Path>, options: PoolOptions) -> Result<Self, SqliteError> {
        check_engine_version()?;
        let path = path.as_ref().to_path_buf();
        let writer = open_writer(&path, &options)?;
        let readers = options.readers.max(1);
        let target = Target {
            path,
            writer: Arc::new(tokio::sync::Mutex::new(writer)),
            reader_permits: Arc::new(Semaphore::new(readers)),
            idle_readers: Mutex::new(Vec::new()),
            max_idle: readers,
        };
        Ok(Self::build(Some(target), options))
    }

    fn build(target: Option<Target>, options: PoolOptions) -> Self {
        Self {
            shared: Arc::new(Shared {
                target,
                options,
                closed: AtomicBool::new(false),
            }),
        }
    }

    /// Whether the pool has a database file.
    pub fn configured(&self) -> bool {
        self.shared.target.is_some()
    }

    /// The limits of this pool.
    pub fn options(&self) -> PoolOptions {
        self.shared.options
    }

    /// The database file of this pool, when it is configured.
    pub fn path(&self) -> Option<&Path> {
        self.shared
            .target
            .as_ref()
            .map(|target| target.path.as_path())
    }

    /// Take the writer (`read_only` false) or a reader (`read_only` true), waiting at most the
    /// acquire timeout.
    ///
    /// # Errors
    ///
    /// * [`SqliteError::NotConfigured`] when the pool has no database file.
    /// * [`SqliteError::Unavailable`] when the pool is closed, no connection frees up in time, or a
    ///   reader cannot be opened.
    pub async fn acquire(&self, read_only: bool) -> Result<PooledConnection, SqliteError> {
        let shared = &self.shared;
        let Some(target) = shared.target.as_ref() else {
            return Err(SqliteError::NotConfigured);
        };
        if shared.closed.load(Ordering::SeqCst) {
            return Err(SqliteError::Unavailable);
        }
        let timeout = shared.options.acquire_timeout;
        let lease = if read_only {
            let permit =
                tokio::time::timeout(timeout, Arc::clone(&target.reader_permits).acquire_owned())
                    .await
                    .map_err(|_| SqliteError::Unavailable)?
                    .map_err(|_| SqliteError::Unavailable)?;
            let conn = match target.take_idle() {
                Some(conn) => conn,
                None => run_blocking(|| open_reader(&target.path, &shared.options))?,
            };
            Lease::Reader { conn, permit }
        } else {
            let guard = tokio::time::timeout(timeout, Arc::clone(&target.writer).lock_owned())
                .await
                .map_err(|_| SqliteError::Unavailable)?;
            // A scope whose future was cancelled left its transaction open on the writer.
            guard.with(|connection| {
                if !connection.is_autocommit() {
                    let _ = connection.execute_batch("ROLLBACK");
                }
            });
            Lease::Writer(guard)
        };
        Ok(PooledConnection {
            lease,
            shared: Arc::clone(shared),
        })
    }

    /// Close the pool: idle readers are dropped and later acquires fail. Connections in use stay
    /// usable until they are released.
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        if let Some(target) = self.shared.target.as_ref() {
            target.lock_idle().clear();
        }
    }

    /// Number of idle reader connections kept for reuse.
    pub fn idle_readers(&self) -> usize {
        self.shared
            .target
            .as_ref()
            .map_or(0, |target| target.lock_idle().len())
    }
}

impl Target {
    fn lock_idle(&self) -> MutexGuard<'_, Vec<Conn>> {
        self.idle_readers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn take_idle(&self) -> Option<Conn> {
        self.lock_idle().pop()
    }
}

enum Lease {
    Writer(OwnedMutexGuard<Conn>),
    Reader {
        conn: Conn,
        permit: OwnedSemaphorePermit,
    },
}

/// One connection checked out of a [`SqlitePool`].
///
/// Holds the writer, or one reader slot, until it is released or dropped. Use it through the
/// owning transaction; repositories never own a connection.
pub struct PooledConnection {
    lease: Lease,
    shared: Arc<Shared>,
}

impl fmt::Debug for PooledConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledConnection")
            .field("writer", &self.is_writer())
            .finish_non_exhaustive()
    }
}

impl PooledConnection {
    /// Whether this is the writer connection.
    pub fn is_writer(&self) -> bool {
        matches!(self.lease, Lease::Writer(_))
    }

    /// Run `work` on the connection.
    pub fn with<R>(&self, work: impl FnOnce(&Connection) -> R) -> R {
        match &self.lease {
            Lease::Writer(guard) => guard.with(work),
            Lease::Reader { conn, .. } => conn.with(work),
        }
    }

    /// Return the connection to the pool. A reader that was marked not reusable (or whose pool is
    /// closed) is closed instead. The writer is always kept: it frees the pool's single slot.
    pub(crate) fn release(self, reusable: bool) {
        let Self { lease, shared } = self;
        match lease {
            Lease::Writer(guard) => drop(guard),
            Lease::Reader { conn, permit } => {
                if let Some(target) = shared.target.as_ref() {
                    let mut idle = target.lock_idle();
                    if reusable
                        && !shared.closed.load(Ordering::SeqCst)
                        && idle.len() < target.max_idle
                    {
                        idle.push(conn);
                    }
                }
                // The slot is freed last, so a waiting caller finds the idle connection.
                drop(permit);
            }
        }
    }
}

/// Apply the pragmas every connection carries.
fn configure(
    connection: &Connection,
    options: &PoolOptions,
    writer: bool,
) -> Result<(), SqliteError> {
    connection.busy_timeout(options.busy_timeout)?;
    if writer {
        let mode: String =
            connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(SqliteError::Unavailable);
        }
    }
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
    Ok(())
}

/// Open the writer: read-write, created when missing, in WAL mode. The file name is never read as
/// a URI.
fn open_writer(path: &Path, options: &PoolOptions) -> Result<Conn, SqliteError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(path, flags)?;
    configure(&connection, options, true)?;
    Ok(Conn::new(connection))
}

/// Open a reader: read-only, so a write in a read-only scope fails in the engine.
fn open_reader(path: &Path, options: &PoolOptions) -> Result<Conn, SqliteError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(path, flags)?;
    configure(&connection, options, false)?;
    Ok(Conn::new(connection))
}

/// Open a short-lived connection for a one-shot task (migrations, health probe, backup): read-write
/// or read-only, with the pool pragmas and the given `busy_timeout`. The caller closes it by
/// dropping it.
pub(crate) fn open_standalone(
    path: &Path,
    read_only: bool,
    busy_timeout: Duration,
) -> Result<Connection, SqliteError> {
    let options = PoolOptions {
        busy_timeout,
        ..PoolOptions::default()
    };
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
    };
    let connection = Connection::open_with_flags(path, flags)?;
    configure(&connection, &options, !read_only)?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_pool(options: PoolOptions) -> Result<(tempfile::TempDir, SqlitePool), SqliteError> {
        let directory = tempfile::tempdir().map_err(|_| SqliteError::Internal)?;
        let pool = SqlitePool::open_with(directory.path().join("pool.sqlite3"), options)?;
        Ok((directory, pool))
    }

    #[test]
    fn unconfigured_pool_reports_its_state() {
        let pool = SqlitePool::unconfigured();
        assert!(!pool.configured());
        assert_eq!(pool.idle_readers(), 0);
        assert!(pool.path().is_none());
        assert!(format!("{pool:?}").contains("configured: false"));
    }

    #[tokio::test]
    async fn acquire_without_a_file_fails_before_waiting() {
        let pool = SqlitePool::unconfigured();
        assert!(matches!(
            pool.acquire(false).await,
            Err(SqliteError::NotConfigured)
        ));
        assert!(matches!(
            pool.acquire(true).await,
            Err(SqliteError::NotConfigured)
        ));
    }

    #[test]
    fn open_applies_the_pragmas() -> Result<(), Box<dyn std::error::Error>> {
        let (_directory, pool) = temp_pool(PoolOptions::default())?;
        let Some(target) = pool.shared.target.as_ref() else {
            return Err("the pool is configured".into());
        };
        let writer = target.writer.try_lock()?;
        let (mode, keys, sync, busy) = writer.with(|c| -> rusqlite::Result<_> {
            Ok((
                c.pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))?,
                c.pragma_query_value(None, "foreign_keys", |r| r.get::<_, i64>(0))?,
                c.pragma_query_value(None, "synchronous", |r| r.get::<_, i64>(0))?,
                c.pragma_query_value(None, "busy_timeout", |r| r.get::<_, i64>(0))?,
            ))
        })?;
        assert_eq!(mode, "wal");
        assert_eq!(keys, 1);
        assert_eq!(sync, 1, "NORMAL");
        assert_eq!(busy, 10_000);
        Ok(())
    }

    #[tokio::test]
    async fn readers_are_read_only_and_reused() -> Result<(), Box<dyn std::error::Error>> {
        let (_directory, pool) = temp_pool(PoolOptions::default())?;
        let reader = pool.acquire(true).await?;
        assert!(!reader.is_writer());
        let write = reader.with(|c| c.execute_batch("CREATE TABLE t (x INTEGER)"));
        assert!(write.is_err(), "a reader cannot write");
        reader.release(true);
        assert_eq!(pool.idle_readers(), 1);
        let again = pool.acquire(true).await?;
        assert_eq!(pool.idle_readers(), 0, "the idle reader was reused");
        again.release(false);
        assert_eq!(
            pool.idle_readers(),
            0,
            "a reader marked not reusable is closed"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_second_writer_waits_and_then_reports_unavailable()
    -> Result<(), Box<dyn std::error::Error>> {
        let options = PoolOptions {
            acquire_timeout: Duration::from_millis(30),
            ..PoolOptions::default()
        };
        let (_directory, pool) = temp_pool(options)?;
        let first = pool.acquire(false).await?;
        assert!(first.is_writer());
        assert!(matches!(
            pool.acquire(false).await,
            Err(SqliteError::Unavailable)
        ));
        first.release(true);
        pool.acquire(false).await?.release(true);
        Ok(())
    }

    #[tokio::test]
    async fn a_closed_pool_refuses_acquires() -> Result<(), Box<dyn std::error::Error>> {
        let (_directory, pool) = temp_pool(PoolOptions::default())?;
        pool.acquire(true).await?.release(true);
        pool.close();
        assert_eq!(pool.idle_readers(), 0);
        assert!(matches!(
            pool.acquire(true).await,
            Err(SqliteError::Unavailable)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn a_writer_dropped_inside_a_transaction_is_rolled_back_by_the_next_acquire()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_directory, pool) = temp_pool(PoolOptions::default())?;
        let writer = pool.acquire(false).await?;
        writer.with(|c| {
            c.execute_batch("CREATE TABLE t (x INTEGER); BEGIN IMMEDIATE; INSERT INTO t VALUES (1)")
        })?;
        drop(writer);
        let next = pool.acquire(false).await?;
        let count: i64 = next.with(|c| c.query_row("SELECT count(*) FROM t", [], |r| r.get(0)))?;
        assert_eq!(count, 0);
        assert!(next.with(Connection::is_autocommit));
        Ok(())
    }

    #[test]
    fn a_directory_is_not_a_database() {
        let Ok(directory) = tempfile::tempdir() else {
            return;
        };
        assert!(matches!(
            SqlitePool::open(directory.path()),
            Err(SqliteError::Unavailable)
        ));
    }
}
