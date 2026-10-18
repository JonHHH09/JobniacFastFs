//! SQLite unit of work: read-write and read-only scopes over [`SqlitePool`].
//!
//! The scope semantics are those of the core port (`jobniac_core::transactions`) and of the
//! PostgreSQL adapter (`PgUnitOfWork`). As there, the current connection lives in an explicit
//! [`SqliteTransaction`] value that each scope passes to its body, and repositories take it as an
//! argument:
//!
//! ```text
//! uow.transaction(TransactionOptions::default(), async |tx| {
//!     tx.run(|conn| { conn.execute("...", params)?; Ok(()) })?;   // repository SQL
//!     tx.before_commit(Box::new(|conn| check_something(conn)));    // replaces a deferred trigger
//!     tx.after_commit(deferred(|| async { Ok(()) }));
//!     Ok(())
//! }).await
//! ```
//!
//! # Differences from PostgreSQL
//!
//! * A read-write scope runs `BEGIN IMMEDIATE` on the pool's single writer, so writers are
//!   serialized and a write never fails late with a lock upgrade error. A read-only scope runs
//!   `BEGIN DEFERRED` on a read-only reader, so all its reads share one WAL snapshot.
//! * `SQLITE_BUSY` on `BEGIN` or `COMMIT` is retried a bounded number of times
//!   ([`PoolOptions::busy_retries`](crate::sqlite::pool::PoolOptions)) with a short pause, after the
//!   engine's own `busy_timeout` wait per attempt. It replaces the 40001 serialization retry.
//!   An exhausted retry is [`SqliteError::Busy`], reported as "Local database is unavailable".
//! * There is one writer, so a `requires_new` write scope inside a write scope would wait for
//!   itself. It is refused with [`SqliteError::NestedWrite`]. `requires_new` is fine from a
//!   read-only scope (the write uses the writer) and for a read-only inner scope.
//! * [`SqliteTransaction::savepoint`] opens a savepoint that rolls back on `Err` and releases on
//!   `Ok`, inside the current transaction.
//! * [`SqliteTransaction::before_commit`] registers a check that runs on the transaction's
//!   connection just before `COMMIT`, in registration order. It replaces the deferred PostgreSQL
//!   constraint triggers (see `RULES_TO_PORT.md`). A failing check rolls the scope back.
//!
//! # Repository pattern
//!
//! * A repository function takes `tx: &SqliteTransaction` (or `&mut`) and runs its SQL through
//!   [`SqliteTransaction::run`], which hands it the `rusqlite::Connection` on the blocking bridge
//!   ([`crate::blocking::run_blocking`]). It never acquires a connection.
//! * Transaction control is the scope's job. A repository never sends `BEGIN`, `COMMIT`,
//!   `ROLLBACK` or `SAVEPOINT` itself.
//! * Row values are read with `Row::get` through `?` (a type mismatch is a `rusqlite::Error`,
//!   which maps to [`SqliteError::Internal`]), so schema drift never panics.
//! * A repository returns `Result<T, E>` where `E: From<SqliteError>`. Use cases use `AppError`.
//!
//! # Hooks
//!
//! As for PostgreSQL: hooks run after the connection is released, outside any transaction, in the
//! order of `jobniac_core::transactions`. A failing hook is logged by the core and never
//! propagates. Hooks and checks registered on a joined body belong to the enclosing scope.
//!
//! # Cancellation
//!
//! Dropping the future of a read-write scope before it completes leaves its transaction open on the
//! writer; the next acquire rolls it back. Its hooks do not run.

use std::fmt;
use std::future::Future;
use std::sync::{Mutex, PoisonError};

use jobniac_core::errors::AppError;
use jobniac_core::profiles::repository::UnitOfWork;
use jobniac_core::transactions::{Hook, TransactionContext, TransactionOptions};
use rusqlite::Connection;

use crate::blocking::run_blocking;
use crate::sqlite::error::SqliteError;
use crate::sqlite::pool::{PoolOptions, PooledConnection, SqlitePool};

/// Begin of a read-write scope: takes the write lock at once.
const BEGIN_WRITE_SQL: &str = "BEGIN IMMEDIATE";

/// Begin of a read-only scope: one snapshot from the first read.
const BEGIN_READ_ONLY_SQL: &str = "BEGIN DEFERRED";

/// A check that runs on the transaction's connection just before `COMMIT`.
pub type PreCommitCheck = Box<dyn FnOnce(&Connection) -> Result<(), SqliteError> + Send + 'static>;

/// A hook as registered, kept in registration order until the scope ends.
enum PendingHook {
    Commit(Hook),
    Rollback(Hook),
    Completion(Hook),
}

/// Entry point of the SQLite unit of work. Cheap to clone; clones share the pool.
#[derive(Clone, Debug)]
pub struct SqliteUnitOfWork {
    pool: SqlitePool,
}

impl SqliteUnitOfWork {
    /// A unit of work over `pool`.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// The pool this unit of work draws connections from.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Run `body` in a new transaction on its own connection (the outermost scope).
    ///
    /// The scope commits when `body` returns `Ok` and its pre-commit checks pass, and rolls back
    /// when `body` or a check returns `Err` or when `COMMIT` fails. The connection is released,
    /// then the hooks run, and then the outcome is returned.
    ///
    /// # Errors
    ///
    /// Returns the body's error unchanged, or the error converted from the failing database step
    /// (for example `E: From<SqliteError>` for the acquire, `BEGIN`, a pre-commit check or
    /// `COMMIT`).
    pub async fn transaction<T, E>(
        &self,
        options: TransactionOptions,
        body: impl AsyncFnOnce(&mut SqliteTransaction) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<SqliteError>,
    {
        let conn = self
            .pool
            .acquire(options.read_only)
            .await
            .map_err(E::from)?;
        let mut transaction = SqliteTransaction {
            uow: self.clone(),
            conn,
            read_only: options.read_only,
            reusable: true,
            savepoints: 0,
            hooks: Mutex::new(Vec::new()),
            checks: Mutex::new(Vec::new()),
        };
        let outcome = transaction.run_scope(body).await;
        let committed = outcome.is_ok();
        let SqliteTransaction {
            conn,
            reusable,
            hooks,
            ..
        } = transaction;
        conn.release(reusable);
        run_hooks(
            hooks.into_inner().unwrap_or_else(PoisonError::into_inner),
            committed,
        )
        .await;
        outcome
    }

    /// Run `hook` now, because no transaction is active (the core port runs it immediately).
    pub async fn after_commit(&self, hook: Hook) {
        run_hooks(vec![PendingHook::Commit(hook)], true).await;
    }

    /// Run `hook` now, because no transaction is active.
    pub async fn after_completion(&self, hook: Hook) {
        run_hooks(vec![PendingHook::Completion(hook)], true).await;
    }

    /// Drop `hook`: an after-rollback hook registered outside a transaction never runs.
    pub fn after_rollback(&self, hook: Hook) {
        drop(hook);
    }
}

impl UnitOfWork for SqliteUnitOfWork {
    type Tx = SqliteTransaction;

    fn transaction<T, F>(
        &self,
        options: TransactionOptions,
        body: F,
    ) -> impl Future<Output = Result<T, AppError>>
    where
        F: AsyncFnOnce(&mut SqliteTransaction) -> Result<T, AppError>,
    {
        SqliteUnitOfWork::transaction::<T, AppError>(self, options, body)
    }
}

/// The explicit handle of an active transaction. Repositories run their SQL through it.
///
/// It is created only by [`SqliteUnitOfWork::transaction`] and handed to the body of a scope. Its
/// connection is released when the scope that created it ends.
pub struct SqliteTransaction {
    uow: SqliteUnitOfWork,
    conn: PooledConnection,
    read_only: bool,
    /// False when the connection can no longer be trusted (a failed rollback).
    reusable: bool,
    /// Savepoints currently open, for unique names.
    savepoints: u32,
    hooks: Mutex<Vec<PendingHook>>,
    checks: Mutex<Vec<PreCommitCheck>>,
}

impl fmt::Debug for SqliteTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SqliteTransaction")
            .field("read_only", &self.read_only)
            .field("savepoints", &self.savepoints)
            .finish_non_exhaustive()
    }
}

impl SqliteTransaction {
    /// Run repository SQL on this transaction's connection, on the blocking bridge.
    ///
    /// Transaction control must not be sent through it; see the repository pattern in the module
    /// documentation. The scope ends the transaction.
    ///
    /// # Errors
    ///
    /// The error `work` returns. Convert a `rusqlite::Error` with `?` (it becomes a classified
    /// [`SqliteError`]).
    pub fn run<R>(
        &self,
        work: impl FnOnce(&Connection) -> Result<R, SqliteError>,
    ) -> Result<R, SqliteError> {
        run_blocking(|| self.conn.with(work))
    }

    /// Whether this transaction was started read-only.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// Run `body` in a nested scope with the given read-only and `requires_new` options.
    ///
    /// # Errors
    ///
    /// * [`SqliteError::ReadOnlyJoin`] (converted to `E`) when a write scope joins a read-only one.
    /// * [`SqliteError::NestedWrite`] when a `requires_new` write scope is requested inside a write
    ///   scope.
    /// * The body's error, or the error of a failed `BEGIN` or `COMMIT` of a new scope.
    pub async fn transaction<T, E>(
        &mut self,
        options: TransactionOptions,
        body: impl AsyncFnOnce(&mut SqliteTransaction) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<SqliteError>,
    {
        if options.requires_new {
            if !options.read_only && !self.read_only {
                return Err(E::from(SqliteError::NestedWrite));
            }
            return self.uow.transaction(options, body).await;
        }
        if self.read_only && !options.read_only {
            return Err(E::from(SqliteError::ReadOnlyJoin));
        }
        body(&mut *self).await
    }

    /// Run `body` inside a savepoint of this transaction. The savepoint is released when `body`
    /// returns `Ok`; when it returns `Err`, the work of the savepoint is rolled back (the
    /// enclosing transaction stays open) and the error is returned.
    ///
    /// # Errors
    ///
    /// The body's error, or the converted error of a failed `SAVEPOINT` or `RELEASE`.
    pub async fn savepoint<T, E>(
        &mut self,
        body: impl AsyncFnOnce(&mut SqliteTransaction) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<SqliteError>,
    {
        self.savepoints += 1;
        let name = format!("jobniac_sp_{}", self.savepoints);
        let started = self.execute(&format!("SAVEPOINT {name}"));
        if let Err(error) = started {
            self.savepoints -= 1;
            return Err(E::from(error));
        }
        let outcome = body(&mut *self).await;
        let finished = match &outcome {
            Ok(_) => self.execute(&format!("RELEASE {name}")),
            Err(_) => self
                .execute(&format!("ROLLBACK TO {name}"))
                .and_then(|()| self.execute(&format!("RELEASE {name}"))),
        };
        self.savepoints -= 1;
        match (outcome, finished) {
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(error)) => Err(E::from(error)),
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(_)) => {
                // The savepoint cannot be unwound, so the connection is not trusted any more.
                self.reusable = false;
                Err(error)
            }
        }
    }

    /// Register a check that runs on this connection just before the outermost `COMMIT`, in
    /// registration order. A failing check rolls the scope back and its error is returned.
    pub fn before_commit(&mut self, check: PreCommitCheck) {
        self.checks
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .push(check);
    }

    /// Register a hook for the outermost commit of the scope this handle belongs to.
    pub fn after_commit(&mut self, hook: Hook) {
        self.push_hook(PendingHook::Commit(hook));
    }

    /// Register a hook that runs only after a rollback of the scope.
    pub fn after_rollback(&mut self, hook: Hook) {
        self.push_hook(PendingHook::Rollback(hook));
    }

    /// Register a hook that runs after either outcome of the scope.
    pub fn after_completion(&mut self, hook: Hook) {
        self.push_hook(PendingHook::Completion(hook));
    }

    fn push_hook(&mut self, hook: PendingHook) {
        self.hooks
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .push(hook);
    }

    /// Run one statement on the connection (scope control: savepoints).
    fn execute(&self, sql: &str) -> Result<(), SqliteError> {
        self.run(|conn| conn.execute_batch(sql).map_err(SqliteError::from))
    }

    /// `ROLLBACK` if the connection is still inside a transaction. A failure marks the
    /// connection not reusable.
    fn rollback_quietly(&mut self) {
        let open = self.conn.with(|conn| !conn.is_autocommit());
        if open
            && self
                .conn
                .with(|conn| conn.execute_batch("ROLLBACK"))
                .is_err()
        {
            self.reusable = false;
        }
    }

    /// Run the registered pre-commit checks in order, stopping at the first failure.
    fn run_checks(&mut self) -> Result<(), SqliteError> {
        let checks = std::mem::take(
            self.checks
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for check in checks {
            run_blocking(|| self.conn.with(check))?;
        }
        Ok(())
    }

    /// Begin, run `body`, run the checks, then commit or roll back. Errors are converted to `E` on
    /// the way out.
    async fn run_scope<T, E>(
        &mut self,
        body: impl AsyncFnOnce(&mut SqliteTransaction) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<SqliteError>,
    {
        let options = self.uow.pool().options();
        let begin = if self.read_only {
            BEGIN_READ_ONLY_SQL
        } else {
            BEGIN_WRITE_SQL
        };
        if let Err(error) = execute_retrying(&self.conn, &options, begin).await {
            self.rollback_quietly();
            return Err(E::from(error));
        }
        match body(&mut *self).await {
            Ok(value) => {
                if let Err(error) = self.run_checks() {
                    self.rollback_quietly();
                    return Err(E::from(error));
                }
                match execute_retrying(&self.conn, &options, "COMMIT").await {
                    Ok(()) => Ok(value),
                    Err(error) => {
                        // A failed COMMIT can leave the transaction open (for example when the
                        // disk is full); an open one is rolled back here.
                        self.rollback_quietly();
                        Err(E::from(error))
                    }
                }
            }
            Err(error) => {
                self.rollback_quietly();
                Err(error)
            }
        }
    }
}

/// Run `sql`, retrying a busy failure up to `options.busy_retries` times with a pause between
/// attempts. Any other failure, and the last busy one, is returned.
async fn execute_retrying(
    conn: &PooledConnection,
    options: &PoolOptions,
    sql: &str,
) -> Result<(), SqliteError> {
    let mut retries = 0;
    loop {
        let outcome = run_blocking(|| {
            conn.with(|connection| connection.execute_batch(sql))
                .map_err(SqliteError::from)
        });
        match outcome {
            Err(error) if error.is_busy() && retries < options.busy_retries => {
                retries += 1;
                tokio::time::sleep(options.busy_retry_delay).await;
            }
            other => return other,
        }
    }
}

/// Run the recorded hooks of one scope with the core port's ordering and isolation.
///
/// The hooks are replayed into a fresh core frame whose body returns `Ok` for a commit and `Err`
/// for a rollback. The core then runs after-commit hooks only on commit, completion hooks in
/// registration order, and after-rollback hooks only on rollback.
async fn run_hooks(hooks: Vec<PendingHook>, committed: bool) {
    if hooks.is_empty() {
        return;
    }
    let outcome: Result<(), ()> = if committed { Ok(()) } else { Err(()) };
    let mut context = TransactionContext::new();
    let _ = context
        .transaction(TransactionOptions::default(), async move |scope| {
            for hook in hooks {
                match hook {
                    PendingHook::Commit(hook) => scope.after_commit(hook).await,
                    PendingHook::Rollback(hook) => scope.after_rollback(hook),
                    PendingHook::Completion(hook) => scope.after_completion(hook).await,
                }
            }
            outcome
        })
        .await;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use jobniac_core::transactions::{HookError, deferred, deferred_sync};
    use tempfile::TempDir;

    use super::*;

    type Events = Arc<Mutex<Vec<&'static str>>>;

    const WRITE: TransactionOptions = TransactionOptions {
        read_only: false,
        requires_new: false,
    };
    const READ: TransactionOptions = TransactionOptions {
        read_only: true,
        requires_new: false,
    };

    fn record(events: &Events, label: &'static str) -> Hook {
        let events = Arc::clone(events);
        deferred_sync(move || {
            events
                .lock()
                .map_err(|_| HookError::new("PoisonedLock"))?
                .push(label);
            Ok(())
        })
    }

    fn snapshot(events: &Events) -> Vec<&'static str> {
        events.lock().map(|guard| guard.clone()).unwrap_or_default()
    }

    /// A unit of work over a temporary database with a table `t (x INTEGER PRIMARY KEY)`.
    fn fixture(options: PoolOptions) -> Result<(TempDir, SqliteUnitOfWork), SqliteError> {
        let directory = tempfile::tempdir().map_err(|_| SqliteError::Internal)?;
        let pool = SqlitePool::open_with(directory.path().join("uow.sqlite3"), options)?;
        pool.open_schema_for_tests()?;
        Ok((directory, SqliteUnitOfWork::new(pool)))
    }

    impl SqlitePool {
        /// Create the test table on the writer.
        fn open_schema_for_tests(&self) -> Result<(), SqliteError> {
            let Some(path) = self.path() else {
                return Err(SqliteError::NotConfigured);
            };
            let connection = Connection::open(path)?;
            connection.execute_batch("CREATE TABLE t (x INTEGER PRIMARY KEY) STRICT")?;
            Ok(())
        }
    }

    async fn insert(uow: &SqliteUnitOfWork, values: &[i64]) -> Result<(), SqliteError> {
        uow.transaction(WRITE, async |tx| {
            for value in values {
                tx.run(|conn| {
                    conn.execute("INSERT INTO t (x) VALUES (?1)", [value])?;
                    Ok(())
                })?;
            }
            Ok(())
        })
        .await
    }

    async fn rows(uow: &SqliteUnitOfWork) -> Result<Vec<i64>, SqliteError> {
        uow.transaction(READ, async |tx| {
            tx.run(|conn| {
                let mut statement = conn.prepare("SELECT x FROM t ORDER BY x")?;
                let rows = statement
                    .query_map([], |row| row.get::<_, i64>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
        })
        .await
    }

    #[tokio::test]
    async fn a_write_scope_commits_and_a_read_scope_sees_it() -> Result<(), SqliteError> {
        let (_directory, uow) = fixture(PoolOptions::default())?;
        insert(&uow, &[1, 2]).await?;
        assert_eq!(rows(&uow).await?, [1, 2]);
        Ok(())
    }

    #[tokio::test]
    async fn an_error_rolls_the_scope_back() -> Result<(), SqliteError> {
        let (_directory, uow) = fixture(PoolOptions::default())?;
        let outcome: Result<(), SqliteError> = uow
            .transaction(WRITE, async |tx| {
                tx.run(|conn| {
                    conn.execute("INSERT INTO t (x) VALUES (1)", [])?;
                    Ok(())
                })?;
                Err(SqliteError::Internal)
            })
            .await;
        assert_eq!(outcome, Err(SqliteError::Internal));
        assert_eq!(rows(&uow).await?, Vec::<i64>::new());
        // The writer is usable again.
        insert(&uow, &[5]).await?;
        assert_eq!(rows(&uow).await?, [5]);
        Ok(())
    }

    #[tokio::test]
    async fn a_constraint_violation_is_classified_and_rolls_back() -> Result<(), SqliteError> {
        let (_directory, uow) = fixture(PoolOptions::default())?;
        insert(&uow, &[1]).await?;
        let outcome = insert(&uow, &[2, 1]).await;
        assert!(matches!(
            outcome,
            Err(SqliteError::Constraint { ref sqlstate, .. }) if sqlstate == "23505"
        ));
        assert_eq!(
            rows(&uow).await?,
            [1],
            "the failed scope left nothing behind"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_savepoint_rolls_back_alone_and_releases_on_success() -> Result<(), SqliteError> {
        let (_directory, uow) = fixture(PoolOptions::default())?;
        uow.transaction(WRITE, async |tx| {
            tx.run(|conn| {
                conn.execute("INSERT INTO t (x) VALUES (1)", [])
                    .map(drop)
                    .map_err(Into::into)
            })?;
            let failed: Result<(), SqliteError> = tx
                .savepoint(async |tx| {
                    tx.run(|conn| {
                        conn.execute("INSERT INTO t (x) VALUES (2)", [])
                            .map(drop)
                            .map_err(Into::into)
                    })?;
                    Err(SqliteError::Internal)
                })
                .await;
            assert_eq!(failed, Err(SqliteError::Internal));
            tx.savepoint(async |tx| {
                tx.run(|conn| {
                    conn.execute("INSERT INTO t (x) VALUES (3)", [])
                        .map(drop)
                        .map_err(Into::into)
                })
            })
            .await?;
            Ok::<(), SqliteError>(())
        })
        .await?;
        assert_eq!(rows(&uow).await?, [1, 3]);
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_pre_commit_check_rolls_the_scope_back() -> Result<(), SqliteError> {
        let (_directory, uow) = fixture(PoolOptions::default())?;
        let outcome: Result<(), SqliteError> = uow
            .transaction(WRITE, async |tx| {
                tx.run(|conn| {
                    conn.execute("INSERT INTO t (x) VALUES (1)", [])
                        .map(drop)
                        .map_err(Into::into)
                })?;
                // Checks run in registration order and the first failure stops the scope.
                tx.before_commit(Box::new(|conn| {
                    let count: i64 = conn.query_row("SELECT count(*) FROM t", [], |r| r.get(0))?;
                    if count == 1 {
                        Ok(())
                    } else {
                        Err(SqliteError::constraint("first"))
                    }
                }));
                tx.before_commit(Box::new(|_| Err(SqliteError::constraint("second"))));
                tx.before_commit(Box::new(|_| Err(SqliteError::constraint("third"))));
                Ok(())
            })
            .await;
        assert_eq!(outcome, Err(SqliteError::constraint("second")));
        assert_eq!(rows(&uow).await?, Vec::<i64>::new());
        Ok(())
    }

    #[tokio::test]
    async fn a_passing_check_lets_the_scope_commit() -> Result<(), SqliteError> {
        let (_directory, uow) = fixture(PoolOptions::default())?;
        uow.transaction(WRITE, async |tx| {
            tx.run(|conn| {
                conn.execute("INSERT INTO t (x) VALUES (9)", [])
                    .map(drop)
                    .map_err(Into::into)
            })?;
            tx.before_commit(Box::new(|conn| {
                let count: i64 = conn.query_row("SELECT count(*) FROM t", [], |r| r.get(0))?;
                assert_eq!(count, 1, "the check sees the uncommitted write");
                Ok(())
            }));
            Ok::<(), SqliteError>(())
        })
        .await?;
        assert_eq!(rows(&uow).await?, [9]);
        Ok(())
    }

    #[tokio::test]
    async fn a_write_in_a_read_scope_fails_as_internal() -> Result<(), SqliteError> {
        let (_directory, uow) = fixture(PoolOptions::default())?;
        let outcome: Result<(), SqliteError> = uow
            .transaction(READ, async |tx| {
                tx.run(|conn| {
                    conn.execute("INSERT INTO t (x) VALUES (1)", [])
                        .map(drop)
                        .map_err(Into::into)
                })
            })
            .await;
        assert_eq!(outcome, Err(SqliteError::Internal));
        Ok(())
    }

    #[tokio::test]
    async fn joining_a_read_scope_for_writing_is_refused() -> Result<(), SqliteError> {
        let (_directory, uow) = fixture(PoolOptions::default())?;
        let outcome: Result<(), SqliteError> = uow
            .transaction(READ, async |tx| {
                tx.transaction(WRITE, async |_| Ok(())).await
            })
            .await;
        assert_eq!(outcome, Err(SqliteError::ReadOnlyJoin));
        Ok(())
    }

    #[tokio::test]
    async fn requires_new_uses_a_second_connection_unless_it_would_need_a_second_writer()
    -> Result<(), SqliteError> {
        let (_directory, uow) = fixture(PoolOptions::default())?;
        let new_write = TransactionOptions {
            read_only: false,
            requires_new: true,
        };
        let new_read = TransactionOptions {
            read_only: true,
            requires_new: true,
        };
        // A read scope can start an independent write scope, which commits on its own.
        uow.transaction(READ, async |tx| {
            tx.transaction(new_write, async |inner| {
                inner.run(|conn| {
                    conn.execute("INSERT INTO t (x) VALUES (1)", [])
                        .map(drop)
                        .map_err(Into::into)
                })
            })
            .await
        })
        .await?;
        assert_eq!(rows(&uow).await?, [1]);
        // A write scope can start an independent read scope.
        let seen: Vec<i64> = uow
            .transaction(WRITE, async |tx| {
                tx.transaction(new_read, async |inner| {
                    inner.run(|conn| {
                        let mut statement = conn.prepare("SELECT x FROM t")?;
                        Ok(statement
                            .query_map([], |row| row.get::<_, i64>(0))?
                            .collect::<Result<Vec<_>, _>>()?)
                    })
                })
                .await
            })
            .await?;
        assert_eq!(seen, [1]);
        // A write scope cannot start a second writer: it would wait for itself.
        let nested: Result<(), SqliteError> = uow
            .transaction(WRITE, async |tx| {
                tx.transaction(new_write, async |_| Ok(())).await
            })
            .await;
        assert_eq!(nested, Err(SqliteError::NestedWrite));
        Ok(())
    }

    /// A second process holding the write lock makes `BEGIN IMMEDIATE` busy. The bounded retry
    /// waits it out when it ends in time, and reports "busy" when it does not.
    #[tokio::test]
    async fn busy_begin_is_retried_a_bounded_number_of_times()
    -> Result<(), Box<dyn std::error::Error>> {
        let options = PoolOptions {
            busy_timeout: Duration::from_millis(10),
            busy_retries: 40,
            busy_retry_delay: Duration::from_millis(20),
            ..PoolOptions::default()
        };
        let (directory, uow) = fixture(options)?;
        let path = directory.path().join("uow.sqlite3");

        // The other connection holds the write lock for 200 ms.
        let other = Connection::open(&path)?;
        other.execute_batch("BEGIN IMMEDIATE")?;
        let holder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            other.execute_batch("COMMIT")
        });
        let started = std::time::Instant::now();
        insert(&uow, &[1]).await?;
        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "the retry waited"
        );
        holder.join().map_err(|_| "the lock holder panicked")??;
        assert_eq!(rows(&uow).await?, [1]);

        // With a single retry the same contention is reported, and nothing is written.
        let short = PoolOptions {
            busy_timeout: Duration::from_millis(10),
            busy_retries: 1,
            busy_retry_delay: Duration::from_millis(5),
            ..PoolOptions::default()
        };
        let strict = SqliteUnitOfWork::new(SqlitePool::open_with(&path, short)?);
        let other = Connection::open(&path)?;
        other.execute_batch("BEGIN IMMEDIATE")?;
        assert_eq!(insert(&strict, &[2]).await, Err(SqliteError::Busy));
        let unavailable = AppError::from(SqliteError::Busy);
        assert_eq!(unavailable, AppError::from(SqliteError::Unavailable));
        other.execute_batch("COMMIT")?;
        // After the lock is gone the same writer works: the failed scope left it clean.
        insert(&strict, &[2]).await?;
        assert_eq!(rows(&strict).await?, [1, 2]);
        Ok(())
    }

    /// A reader scope keeps its snapshot while the writer commits more rows.
    #[tokio::test]
    async fn a_read_scope_keeps_one_snapshot() -> Result<(), SqliteError> {
        let (_directory, uow) = fixture(PoolOptions::default())?;
        insert(&uow, &[1]).await?;
        let uow_for_writer = uow.clone();
        let (before, after) = uow
            .transaction(READ, async |tx| {
                let count = |tx: &SqliteTransaction| {
                    tx.run(|conn| {
                        Ok(conn.query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0))?)
                    })
                };
                let before = count(tx)?;
                insert(&uow_for_writer, &[2]).await?;
                let after = count(tx)?;
                Ok::<_, SqliteError>((before, after))
            })
            .await?;
        assert_eq!((before, after), (1, 1));
        assert_eq!(rows(&uow).await?, [1, 2]);
        Ok(())
    }

    #[tokio::test]
    async fn commit_hooks_run_in_order_only_after_commit() {
        let events: Events = Arc::default();
        let pending = vec![
            PendingHook::Commit(record(&events, "commit")),
            PendingHook::Rollback(record(&events, "rollback")),
            PendingHook::Completion(record(&events, "completion")),
            PendingHook::Commit(record(&events, "second commit")),
        ];
        run_hooks(pending, true).await;
        assert_eq!(snapshot(&events), ["commit", "second commit", "completion"]);
    }

    #[tokio::test]
    async fn rollback_hooks_keep_registration_order_around_completion_hooks() {
        let events: Events = Arc::default();
        let pending = vec![
            PendingHook::Completion(record(&events, "first completion")),
            PendingHook::Commit(record(&events, "commit")),
            PendingHook::Rollback(record(&events, "rollback")),
            PendingHook::Completion(record(&events, "last completion")),
        ];
        run_hooks(pending, false).await;
        assert_eq!(
            snapshot(&events),
            ["first completion", "rollback", "last completion"]
        );
    }

    #[tokio::test]
    async fn a_failing_hook_does_not_stop_later_hooks() {
        let events: Events = Arc::default();
        let pending = vec![
            PendingHook::Commit(deferred_sync(|| Err(HookError::new("RuntimeError")))),
            PendingHook::Commit(record(&events, "next")),
            PendingHook::Completion(deferred(|| async { Ok(()) })),
            PendingHook::Completion(record(&events, "completed")),
        ];
        run_hooks(pending, true).await;
        assert_eq!(snapshot(&events), ["next", "completed"]);
    }

    #[tokio::test]
    async fn scope_hooks_follow_the_outcome() -> Result<(), SqliteError> {
        let (_directory, uow) = fixture(PoolOptions::default())?;
        let events: Events = Arc::default();
        uow.transaction(WRITE, async |tx| {
            tx.after_commit(record(&events, "commit"));
            tx.after_rollback(record(&events, "rollback"));
            tx.after_completion(record(&events, "completion"));
            Ok::<(), SqliteError>(())
        })
        .await?;
        assert_eq!(snapshot(&events), ["commit", "completion"]);

        let events: Events = Arc::default();
        let outcome: Result<(), SqliteError> = uow
            .transaction(WRITE, async |tx| {
                tx.after_commit(record(&events, "commit"));
                tx.after_rollback(record(&events, "rollback"));
                tx.after_completion(record(&events, "completion"));
                Err(SqliteError::Internal)
            })
            .await;
        assert!(outcome.is_err());
        assert_eq!(snapshot(&events), ["rollback", "completion"]);
        Ok(())
    }

    #[tokio::test]
    async fn outside_a_transaction_hooks_run_now_and_rollback_hooks_are_dropped() {
        let events: Events = Arc::default();
        let uow = SqliteUnitOfWork::new(SqlitePool::unconfigured());
        uow.after_commit(record(&events, "commit")).await;
        uow.after_rollback(record(&events, "rollback"));
        uow.after_completion(record(&events, "completion")).await;
        assert_eq!(snapshot(&events), ["commit", "completion"]);
    }

    #[tokio::test]
    async fn an_unconfigured_unit_of_work_fails_before_any_scope_starts() {
        let uow = SqliteUnitOfWork::new(SqlitePool::unconfigured());
        let outcome: Result<(), AppError> = uow.transaction(WRITE, async |_| Ok(())).await;
        assert_eq!(outcome, Err(AppError::from(SqliteError::NotConfigured)));
    }

    /// The scope future must be `Send` (it is spawned by the runtimes) and the handle `Sync`
    /// (repositories borrow it across awaits).
    #[tokio::test]
    async fn scopes_are_send_and_the_handle_is_sync() -> Result<(), Box<dyn std::error::Error>> {
        fn assert_sync<T: Sync>() {}
        assert_sync::<SqliteTransaction>();
        let (_directory, uow) = fixture(PoolOptions::default())?;
        let task = tokio::spawn(async move { insert(&uow, &[7]).await });
        task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn the_core_port_runs_scopes_with_app_errors() -> Result<(), AppError> {
        let (_directory, uow) = fixture(PoolOptions::default()).map_err(AppError::from)?;
        let insert_one = async |tx: &mut SqliteTransaction| -> Result<(), AppError> {
            tx.run(|conn| {
                conn.execute("INSERT INTO t (x) VALUES (1)", [])?;
                Ok(())
            })?;
            Ok(())
        };
        UnitOfWork::transaction(&uow, WRITE, insert_one).await?;
        let outcome = UnitOfWork::transaction(&uow, WRITE, insert_one).await;
        assert!(matches!(
            outcome,
            Err(AppError::PersistenceConstraint { .. })
        ));
        Ok(())
    }
}
