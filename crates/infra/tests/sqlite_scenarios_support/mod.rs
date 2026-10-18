//! Synchronization helpers of the SQLite scenario tests: ordered writers on one file without a
//! fixed sleep as the synchronization.
//!
//! * [`WriteHold`] is a raw connection that keeps `BEGIN IMMEDIATE`. Every pool writer and every
//!   raw writer on the file waits behind it, and runs only after [`WriteHold::commit`].
//! * [`PauseClock`] pauses a repository in the middle of its write transaction. A replacement reads
//!   the clock after `BEGIN IMMEDIATE` and again after its first writes, so blocking the Nth read
//!   stops the writer at a known point while it holds the file lock. The test learns that it is
//!   there through a channel ([`PauseHandle::reached`]) and lets it go with
//!   [`PauseHandle::release`]. Use it on a multi-thread runtime.
//! * [`assert_still_waiting`] is the negative check that a contender has not run. It can only fail
//!   when a contender passes a held lock, so a slow machine weakens it but never breaks it.
//!
//! All data is synthetic.

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use jobniac_core::clock::Clock;
use jobniac_infra::sqlite::pool::SqlitePool;
use rusqlite::Connection;
use rusqlite::types::ValueRef;
use time::OffsetDateTime;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// How long a paused repository may be held before it gives up and continues. A bound only: a
/// paused task is released by the test long before.
const PAUSE_BOUND: Duration = Duration::from_secs(30);

/// How long [`assert_still_waiting`] watches a contender.
const WAIT_WINDOW: Duration = Duration::from_millis(150);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A raw connection with the pragmas of a pooled one (`busy_timeout` of ten seconds).
pub fn raw_connection(path: &Path) -> Connection {
    let connection = Connection::open(path).expect("raw connection opens");
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 10000;")
        .expect("raw connection pragmas");
    connection
}

/// A second pool over the same file: its own writer and readers, so it contends through the file
/// lock like another process would.
pub fn second_pool(path: &Path) -> SqlitePool {
    SqlitePool::open(path).expect("second pool opens")
}

/// A raw connection that holds the write lock (`BEGIN IMMEDIATE`) until it commits.
pub struct WriteHold {
    connection: Connection,
}

impl WriteHold {
    /// Takes the write lock now.
    pub fn begin(path: &Path) -> Self {
        let connection = raw_connection(path);
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .expect("the write lock is free");
        Self { connection }
    }

    /// The held connection, for the changes the winner commits.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Commits the held transaction, which lets every waiting writer continue.
    pub fn commit(self) {
        self.connection
            .execute_batch("COMMIT")
            .expect("the held transaction commits");
    }
}

/// Runs one raw statement on its own connection on a blocking thread. It waits behind a held
/// write lock for at most the connection's `busy_timeout`.
pub fn spawn_raw_write(
    path: &Path,
    sql: &'static str,
    key: String,
) -> JoinHandle<rusqlite::Result<usize>> {
    let path: PathBuf = path.to_path_buf();
    tokio::task::spawn_blocking(move || raw_connection(&path).execute(sql, [key]))
}

/// Checks that `handle` has not finished within a short window (see the module notes).
pub async fn assert_still_waiting<T>(handle: &JoinHandle<T>, what: &str) {
    tokio::time::sleep(WAIT_WINDOW).await;
    assert!(!handle.is_finished(), "{what} must still be waiting");
}

/// A clock that blocks its Nth reading until the test releases it, and otherwise reads `at`.
#[derive(Debug)]
pub struct PauseClock {
    at: OffsetDateTime,
    pause_on: usize,
    calls: AtomicUsize,
    reached: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<Receiver<()>>>,
}

/// The test side of a [`PauseClock`].
pub struct PauseHandle {
    reached: oneshot::Receiver<()>,
    release: Sender<()>,
}

impl PauseClock {
    /// A clock over `at` that pauses at its `pause_on`th reading (counted from one).
    pub fn new(at: OffsetDateTime, pause_on: usize) -> (Arc<Self>, PauseHandle) {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let clock = Arc::new(Self {
            at,
            pause_on,
            calls: AtomicUsize::new(0),
            reached: Mutex::new(Some(reached_tx)),
            release: Mutex::new(Some(release_rx)),
        });
        (
            clock,
            PauseHandle {
                reached: reached_rx,
                release: release_tx,
            },
        )
    }
}

impl Clock for PauseClock {
    fn now(&self) -> OffsetDateTime {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.pause_on {
            if let Some(reached) = lock(&self.reached).take() {
                let _ = reached.send(());
            }
            let release = lock(&self.release).take();
            if let Some(release) = release {
                // Parks this worker only; the other workers keep running.
                jobniac_infra::blocking::run_blocking(|| release.recv_timeout(PAUSE_BOUND).ok());
            }
        }
        self.at
    }
}

impl PauseHandle {
    /// Waits until the repository is paused inside its transaction.
    pub async fn reached(&mut self) {
        (&mut self.reached)
            .await
            .expect("the paused clock must be read");
    }

    /// Lets the paused repository continue.
    pub fn release(self) {
        let _ = self.release.send(());
    }
}

/// A clock that returns its instants in order and then repeats the last one.
#[derive(Debug)]
pub struct TickClock(Mutex<Vec<OffsetDateTime>>);

impl TickClock {
    /// A clock over `ticks`, which must not be empty.
    pub fn new(ticks: Vec<OffsetDateTime>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(ticks)))
    }
}

impl Clock for TickClock {
    fn now(&self) -> OffsetDateTime {
        let mut ticks = lock(&self.0);
        if ticks.len() > 1 {
            ticks.remove(0)
        } else {
            ticks.first().copied().unwrap_or(OffsetDateTime::UNIX_EPOCH)
        }
    }
}

/// The rows of several tables, keyed by table.
pub type TableState = Vec<(&'static str, Vec<String>)>;

/// Every row of each of `tables` as text (blobs as their length), sorted, for before/after
/// comparisons of a refused or rolled-back write.
pub fn table_state(
    connection: &Connection,
    tables: &[&'static str],
) -> Result<TableState, Box<dyn Error + Send + Sync>> {
    let mut result = Vec::new();
    for table in tables {
        let mut statement = connection.prepare(&format!("SELECT * FROM {table}"))?;
        let columns = statement.column_count();
        let mut rows = statement.query([])?;
        let mut texts = Vec::new();
        while let Some(row) = rows.next()? {
            let mut cells = Vec::with_capacity(columns);
            for index in 0..columns {
                cells.push(match row.get_ref(index)? {
                    ValueRef::Null => "NULL".to_owned(),
                    ValueRef::Integer(value) => value.to_string(),
                    ValueRef::Real(value) => value.to_string(),
                    ValueRef::Text(bytes) => String::from_utf8_lossy(bytes).into_owned(),
                    ValueRef::Blob(bytes) => format!("blob:{}", bytes.len()),
                });
            }
            texts.push(cells.join("|"));
        }
        texts.sort();
        result.push((*table, texts));
    }
    Ok(result)
}
