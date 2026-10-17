//! PostgreSQL unit of work: REQUIRED, REQUIRES_NEW and read-only transactions over [`PgPool`].
//!
//! The scope semantics are those of the core port (`jobniac_core::transactions`).
//!
//! # Explicit handle, no ambient state
//!
//! No ambient variable holds the current connection: it lives in a [`PgTransaction`] value, and
//! each scope passes it to its body. The body is an async closure that receives
//! `&mut PgTransaction`, and repositories take the handle as an argument:
//!
//! ```text
//! uow.transaction(TransactionOptions::default(), async |tx| {
//!     profiles::update(tx, &command).await?;        // repository: fn(&PgTransaction, ...)
//!     tx.after_commit(deferred(|| async { Ok(()) }));
//!     Ok(())
//! }).await
//! ```
//!
//! # Pattern for the profile and job repositories
//!
//! * A repository function takes `tx: &PgTransaction` (or `&mut` when it opens a nested scope) and
//!   runs its SQL on [`PgTransaction::client`]. It never acquires a session and never connects.
//! * Transaction control is the scope's job. A repository never sends `BEGIN`, `COMMIT`,
//!   `ROLLBACK`, `SAVEPOINT` or a session-level `SET` through the client. `SET LOCAL` is allowed.
//!   Any other session setting is undone by the scope before the session is reused (`RESET ALL`,
//!   `RESET ROLE` and `RESET SESSION AUTHORIZATION` after the outermost scope ends).
//! * SQL is schema-qualified (`search_path` is `pg_catalog`).
//! * Row values are read with `Row::try_get`, never `Row::get`. `get` panics on a type mismatch or
//!   a missing column, and the release profile aborts on panic, so schema drift would kill the
//!   process. A `try_get` error maps through [`PgError::from`] like any other driver error.
//! * Driver errors map with `.map_err(PgError::from)` on the generic path, or through
//!   [`details_app_error`] on the profile-details path (see "Error policies").
//! * A repository returns `Result<T, E>` where `E: From<PgError>`. Use cases use `AppError`, which
//!   implements that conversion.
//!
//! # Scopes
//!
//! * [`PgUnitOfWork::transaction`] starts a transaction on a new session.
//! * [`PgTransaction::transaction`] with `requires_new: false` joins the transaction and runs the
//!   body on the same handle. A joined body that fails hands its error to the caller, which decides
//!   the outcome. Joining a read-only transaction for writing returns [`PgError::ReadOnlyJoin`].
//! * With `requires_new: true` it starts a second transaction on a second session. It commits or
//!   rolls back on its own and finishes before the enclosing body resumes.
//! * A started scope commits when its body returns `Ok` and rolls back when it returns `Err`. When
//!   `COMMIT` fails, the server has already ended the transaction, so the scope reports the error
//!   and sends no `ROLLBACK`. Rollback hooks still run, because the scope did not commit.
//!
//! # Error policies
//!
//! Both policies start from the same classification ([`classify`]):
//!
//! * Generic path (the `From<PgError>` conversion into `AppError`): class 23 is a persistence
//!   constraint. Statement and lock timeouts, and every other non-operational failure, are the
//!   internal error. Operational failures are "Local database is unavailable".
//! * Profile-details path ([`details_app_error`]): class 23 is a persistence constraint, and every
//!   other failure, including statement and lock timeouts, is "Local database is unavailable". No
//!   SQLSTATE text reaches a caller on either path.
//!
//! Client-side driver errors (no server SQLSTATE) are unavailable only when the connection is closed
//! or an I/O error caused them. Any other client error (a parameter or column type mismatch, a
//! missing column, a malformed response) is a repository bug and is internal.
//!
//! # Hooks
//!
//! Hooks run after the session is released, outside any transaction, in the order of
//! `jobniac_core::transactions`: after-commit hooks only when the scope committed, then
//! completion hooks in registration order, with after-rollback hooks only when it did not commit.
//! Hook execution reuses the core port itself (a [`TransactionContext`] frame that replays the
//! recorded hooks), so ordering and panic isolation are the core's. A failing hook is logged by the
//! core and never propagates. Hooks registered on a joined body belong to the enclosing scope.
//!
//! # Read-only scopes
//!
//! `TransactionOptions::read_only` runs `BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY`. A write
//! inside it fails with SQLSTATE 25006, which maps to [`PgError::Internal`]. The isolation level is
//! set here and never by a repository, so several reads of one scope share one snapshot.
//!
//! # Cancellation
//!
//! Dropping the future of a scope before it completes closes its session, so the server rolls back
//! the open transaction. Its hooks do not run, and the session does not return to the pool.

use std::error::Error as StdError;
use std::fmt;
use std::time::Duration;

use jobniac_core::errors::{AppError, ApplicationError, ErrorCode};
use jobniac_core::transactions::{Hook, TransactionContext, TransactionOptions};
use tokio_postgres::Client;

use crate::postgres::pool::{PgPool, PooledSession};
use crate::postgres::sqlstate::{LOCK_NOT_AVAILABLE, QUERY_CANCELED, SERIALIZATION_FAILURE};

/// Message of [`PgError::NotConfigured`] (the database-unavailable text).
pub const NOT_CONFIGURED_MESSAGE: &str = "Local database is not configured";

/// Upper bound of the session reset that runs before a session is reused.
const RESET_TIMEOUT: Duration = Duration::from_secs(5);

/// Begin of a read-write scope (the READ COMMITTED default).
const BEGIN_SQL: &str = "BEGIN";

/// Begin of a read-only scope: REPEATABLE READ, so one snapshot serves the whole scope.
const BEGIN_READ_ONLY_SQL: &str = "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY";

/// Statement that undoes session-level settings. `RESET ALL` restores the configured defaults but
/// leaves the role alone, so the role is reset separately.
const SESSION_RESET_SQL: &str = "RESET ALL; RESET ROLE; RESET SESSION AUTHORIZATION";

/// A classified database failure of the unit of work.
///
/// No variant carries server message text, so no row value or SQL reaches a caller. The
/// [`PgError::Constraint`] variant keeps only the SQLSTATE and the constraint name, which stay
/// internal (`AppError::PersistenceConstraint`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PgError {
    /// No validated `JOBNIAC_DB_*` configuration exists.
    NotConfigured,
    /// Connection, pool wait or operational failure.
    Unavailable,
    /// SQLSTATE 40001: a serialization failure. Only the matching snapshot loop retries it; every other
    /// conversion reports the database as unavailable.
    Serialization,
    /// SQLSTATE class 23: integrity-constraint violation.
    Constraint {
        /// Five-character SQLSTATE.
        sqlstate: String,
        /// Name of the violated constraint or index, when the server reports one.
        constraint: Option<String>,
    },
    /// SQLSTATE 57014: the statement was canceled, typically by `statement_timeout`.
    QueryCanceled,
    /// SQLSTATE 55P03: a lock could not be acquired within `lock_timeout`.
    LockNotAvailable,
    /// A write was requested in a read-only scope that a joined scope cannot change.
    ReadOnlyJoin,
    /// Any other database failure (for example SQLSTATE 25006 or class 22, 42 and P), or a
    /// client-side bug such as a type mismatch in a repository.
    Internal,
}

impl fmt::Display for PgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::NotConfigured => "not configured",
            Self::Unavailable => "unavailable",
            Self::Serialization => "serialization failure",
            Self::Constraint { .. } => "constraint violation",
            Self::QueryCanceled => "query canceled",
            Self::LockNotAvailable => "lock not available",
            Self::ReadOnlyJoin => "read-only join",
            Self::Internal => "internal",
        };
        write!(f, "database error: {kind}")
    }
}

impl std::error::Error for PgError {}

impl From<tokio_postgres::Error> for PgError {
    fn from(error: tokio_postgres::Error) -> Self {
        match error.as_db_error() {
            None => classify_client_error(&error),
            Some(db) => classify(db.code().code(), db.constraint()),
        }
    }
}

/// Classify an error that has no server SQLSTATE.
///
/// A closed connection, or an I/O error as the cause, is a connection failure. Every other client
/// error comes from the client itself. `tokio-postgres` reports TLS, authentication, connect and
/// timeout failures only while a session is being opened, and [`PgPool`] maps those itself, so they
/// do not reach this function.
fn classify_client_error(error: &tokio_postgres::Error) -> PgError {
    let io_cause = error
        .source()
        .is_some_and(|cause| cause.is::<std::io::Error>());
    if error.is_closed() || io_cause {
        PgError::Unavailable
    } else {
        PgError::Internal
    }
}

/// Classify a server SQLSTATE.
///
/// Class 23 becomes [`PgError::Constraint`]. 57014 and 55P03 are kept apart so callers can tell
/// them from a generic failure. Other operational classes (see `is_operational`) become
/// [`PgError::Unavailable`]. Everything else is [`PgError::Internal`].
pub fn classify(code: &str, constraint: Option<&str>) -> PgError {
    if code.starts_with("23") {
        return PgError::Constraint {
            sqlstate: code.to_owned(),
            constraint: constraint.map(str::to_owned),
        };
    }
    if code == QUERY_CANCELED {
        return PgError::QueryCanceled;
    }
    if code == LOCK_NOT_AVAILABLE {
        return PgError::LockNotAvailable;
    }
    if code == SERIALIZATION_FAILURE {
        return PgError::Serialization;
    }
    if is_operational(code) {
        return PgError::Unavailable;
    }
    PgError::Internal
}

/// Whether the SQLSTATE is an operational failure, which maps to unavailable. Class `H` is
/// operational only as `HV` (`H0000` is a base driver error, so it is internal).
pub(crate) fn is_operational(code: &str) -> bool {
    matches!(
        code.get(..2),
        Some(
            "08" | "27"
                | "28"
                | "2F"
                | "38"
                | "39"
                | "3B"
                | "40"
                | "53"
                | "54"
                | "55"
                | "57"
                | "58"
                | "F0"
                | "HV"
        )
    )
}

impl From<PgError> for AppError {
    fn from(error: PgError) -> Self {
        match error {
            PgError::NotConfigured => {
                ApplicationError::with_message(ErrorCode::InternalError, NOT_CONFIGURED_MESSAGE)
                    .into()
            }
            PgError::Unavailable | PgError::Serialization => {
                ApplicationError::database_unavailable().into()
            }
            PgError::Constraint {
                sqlstate,
                constraint,
            } => AppError::PersistenceConstraint {
                sqlstate: Some(sqlstate),
                constraint,
            },
            // The envelope is the generic internal error.
            PgError::QueryCanceled
            | PgError::LockNotAvailable
            | PgError::ReadOnlyJoin
            | PgError::Internal => AppError::Internal,
        }
    }
}

/// Error policy of the profile-details path.
///
/// Integrity failures are a persistence constraint. Every other database failure, including
/// statement and lock timeouts, is "Local database is unavailable", so the envelope never shows
/// SQLSTATE text. The not-configured error keeps its own message. Profile and deletion repositories
/// map their errors through this function; the generic path keeps [`From<PgError> for AppError`].
pub fn details_app_error(error: PgError) -> AppError {
    match error {
        PgError::NotConfigured => AppError::from(PgError::NotConfigured),
        constraint @ PgError::Constraint { .. } => AppError::from(constraint),
        _ => ApplicationError::database_unavailable().into(),
    }
}

/// A hook as registered, kept in registration order until the scope ends.
enum PendingHook {
    Commit(Hook),
    Rollback(Hook),
    Completion(Hook),
}

/// Entry point of the PostgreSQL unit of work. Cheap to clone; clones share the pool.
#[derive(Clone, Debug)]
pub struct PgUnitOfWork {
    pool: PgPool,
}

impl PgUnitOfWork {
    /// A unit of work over `pool`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The pool this unit of work draws sessions from.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Run `body` in a new transaction on its own session (the outermost scope).
    ///
    /// The scope commits when `body` returns `Ok`, and rolls back when it returns `Err` or when
    /// `COMMIT` fails. Session-level settings are reset, the session is released, then the hooks
    /// run, and then the outcome is returned.
    ///
    /// # Errors
    ///
    /// Returns the body's error unchanged, or the error converted from the failing database step
    /// (for example `E: From<PgError>` for the acquire, `BEGIN`, or `COMMIT`).
    pub async fn transaction<T, E>(
        &self,
        options: TransactionOptions,
        body: impl AsyncFnOnce(&mut PgTransaction) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<PgError>,
    {
        let session = self.pool.acquire().await.map_err(E::from)?;
        let mut transaction = PgTransaction {
            uow: self.clone(),
            session,
            read_only: options.read_only,
            open: false,
            reusable: true,
            hooks: Vec::new(),
        };
        let outcome = transaction.run(body).await;
        let committed = outcome.is_ok();
        transaction.reset_session().await;
        let PgTransaction {
            session,
            reusable,
            hooks,
            ..
        } = transaction;
        session.release(reusable);
        run_hooks(hooks, committed).await;
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

/// The explicit handle of an active transaction. Repositories run their SQL through it.
///
/// It is created only by [`PgUnitOfWork::transaction`] and handed to the body of a scope. Its
/// session is released when the scope that created it ends.
pub struct PgTransaction {
    uow: PgUnitOfWork,
    session: PooledSession,
    read_only: bool,
    /// True between a successful `BEGIN` and the `COMMIT` or `ROLLBACK` that ends it.
    open: bool,
    /// False when the session can no longer be trusted (a failed rollback or reset).
    reusable: bool,
    hooks: Vec<PendingHook>,
}

impl fmt::Debug for PgTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgTransaction")
            .field("read_only", &self.read_only)
            .field("open", &self.open)
            .finish_non_exhaustive()
    }
}

impl PgTransaction {
    /// The client of this transaction's session. Run repository SQL through it.
    ///
    /// Transaction control must not be sent through it; see the repository pattern in the module
    /// documentation. The scope ends the transaction.
    pub fn client(&self) -> &Client {
        self.session.client()
    }

    /// Whether this transaction was started read-only.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// Run `body` in a nested scope with the given read-only and `requires_new` options.
    ///
    /// # Errors
    ///
    /// * [`PgError::ReadOnlyJoin`] (converted to `E`) when a write scope joins a read-only one.
    /// * The body's error, or the error of a failed `BEGIN` or `COMMIT` of a new scope.
    pub async fn transaction<T, E>(
        &mut self,
        options: TransactionOptions,
        body: impl AsyncFnOnce(&mut PgTransaction) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<PgError>,
    {
        if options.requires_new {
            return self.uow.transaction(options, body).await;
        }
        if self.read_only && !options.read_only {
            return Err(E::from(PgError::ReadOnlyJoin));
        }
        body(&mut *self).await
    }

    /// Register a hook for the outermost commit of the scope this handle belongs to.
    pub fn after_commit(&mut self, hook: Hook) {
        self.hooks.push(PendingHook::Commit(hook));
    }

    /// Register a hook that runs only after a rollback of the scope.
    pub fn after_rollback(&mut self, hook: Hook) {
        self.hooks.push(PendingHook::Rollback(hook));
    }

    /// Register a hook that runs after either outcome of the scope.
    pub fn after_completion(&mut self, hook: Hook) {
        self.hooks.push(PendingHook::Completion(hook));
    }

    /// `BEGIN`. A read-only scope begins at REPEATABLE READ and read-only, so every read of the scope sees one snapshot and no repository needs a
    /// `SET TRANSACTION` of its own (which PostgreSQL refuses after the first statement).
    async fn begin(&mut self) -> Result<(), PgError> {
        let sql = if self.read_only {
            BEGIN_READ_ONLY_SQL
        } else {
            BEGIN_SQL
        };
        self.session.client().batch_execute(sql).await?;
        self.open = true;
        Ok(())
    }

    /// `COMMIT`. The transaction is over after this call, whatever its result: a failed `COMMIT`
    /// leaves the server with no open transaction, so no `ROLLBACK` is sent.
    async fn commit(&mut self) -> Result<(), PgError> {
        self.open = false;
        self.session.client().batch_execute("COMMIT").await?;
        Ok(())
    }

    /// `ROLLBACK` if the transaction is still open. A failure marks the session not reusable.
    async fn rollback_quietly(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        if self
            .session
            .client()
            .batch_execute("ROLLBACK")
            .await
            .is_err()
        {
            self.reusable = false;
        }
    }

    /// Undo session-level settings before the session is reused (outermost scope only).
    ///
    /// A body can run a session-level `SET` that its committed transaction keeps. This resets such
    /// settings to the session defaults, so the next scope on the pooled session starts clean. It
    /// is one extra round trip per scope. A failure, or a timeout, marks the session not reusable.
    async fn reset_session(&mut self) {
        if !self.reusable || self.open {
            return;
        }
        let reset = self.session.client().batch_execute(SESSION_RESET_SQL);
        match tokio::time::timeout(RESET_TIMEOUT, reset).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) | Err(_) => self.reusable = false,
        }
    }

    /// Begin, run `body`, then commit or roll back. Errors are converted to `E` on the way out.
    async fn run<T, E>(
        &mut self,
        body: impl AsyncFnOnce(&mut PgTransaction) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<PgError>,
    {
        if let Err(error) = self.begin().await {
            self.rollback_quietly().await;
            return Err(E::from(error));
        }
        match body(&mut *self).await {
            Ok(value) => match self.commit().await {
                Ok(()) => Ok(value),
                Err(error) => Err(E::from(error)),
            },
            Err(error) => {
                self.rollback_quietly().await;
                Err(error)
            }
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
    use std::sync::{Arc, Mutex};

    use jobniac_core::transactions::{HookError, deferred, deferred_sync};

    use super::*;

    type Events = Arc<Mutex<Vec<&'static str>>>;

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

    #[test]
    fn sqlstate_classes_map_to_the_planned_categories() {
        let cases: [(&str, PgError); 10] = [
            (
                "23505",
                PgError::Constraint {
                    sqlstate: "23505".to_owned(),
                    constraint: Some("uq".to_owned()),
                },
            ),
            (
                "23514",
                PgError::Constraint {
                    sqlstate: "23514".to_owned(),
                    constraint: None,
                },
            ),
            ("57014", PgError::QueryCanceled),
            ("55P03", PgError::LockNotAvailable),
            ("08006", PgError::Unavailable),
            ("40001", PgError::Serialization),
            ("40P01", PgError::Unavailable),
            ("53300", PgError::Unavailable),
            ("25006", PgError::Internal),
            ("P0001", PgError::Internal),
        ];
        for (code, expected) in cases {
            let constraint = (code == "23505").then_some("uq");
            assert_eq!(classify(code, constraint), expected, "{code}");
        }
        assert_eq!(classify("22012", None), PgError::Internal);
        assert_eq!(classify("42P01", None), PgError::Internal);
    }

    /// The outcome name of a classified error, as the golden vectors spell it.
    fn outcome_name(error: &PgError) -> &'static str {
        match error {
            PgError::Constraint { .. } => "constraint",
            PgError::Unavailable | PgError::Serialization => "unavailable",
            PgError::QueryCanceled => "query_canceled",
            PgError::LockNotAvailable => "lock_not_available",
            PgError::NotConfigured | PgError::ReadOnlyJoin | PgError::Internal => "internal",
        }
    }

    /// Every SQLSTATE of the golden vectors gets the expected outcome. 57014 and 55P03 are kept as
    /// their own variants, so their `raw_kind` is compared instead of `outcome`.
    #[test]
    fn sqlstate_outcomes_match_the_golden_vectors() -> Result<(), String> {
        use serde_json::Value;

        let text = include_str!("../../tests/golden/sqlstate_classes.json");
        let vectors: Vec<Value> = serde_json::from_str(text).map_err(|error| error.to_string())?;
        if vectors.is_empty() {
            return Err("the golden vectors are empty".to_owned());
        }
        for vector in &vectors {
            let field = |name: &str| {
                vector
                    .get(name)
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("vector without {name}"))
            };
            let code = field("sqlstate")?;
            let outcome = field("outcome")?;
            let expected = match vector.get("raw_kind").and_then(Value::as_str) {
                Some(raw) if outcome == "internal" => raw,
                _ => outcome,
            };
            let actual = outcome_name(&classify(code, None));
            if actual != expected {
                return Err(format!(
                    "{code}: classifier gives {actual}, golden vectors give {expected}"
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn app_error_conversion_keeps_the_envelopes() {
        let not_configured: AppError = PgError::NotConfigured.into();
        let envelope = jobniac_core::errors::error_envelope(&not_configured);
        assert_eq!(envelope.code, ErrorCode::InternalError);
        assert_eq!(envelope.message, NOT_CONFIGURED_MESSAGE);
        assert!(envelope.details.is_empty());

        let unavailable: AppError = PgError::Unavailable.into();
        let envelope = jobniac_core::errors::error_envelope(&unavailable);
        assert_eq!(envelope.message, "Local database is unavailable");

        let constraint: AppError = PgError::Constraint {
            sqlstate: "23505".to_owned(),
            constraint: Some("uq".to_owned()),
        }
        .into();
        assert!(matches!(
            constraint,
            AppError::PersistenceConstraint {
                sqlstate: Some(ref state),
                constraint: Some(ref name),
            } if state == "23505" && name == "uq"
        ));
        for error in [
            PgError::QueryCanceled,
            PgError::LockNotAvailable,
            PgError::ReadOnlyJoin,
            PgError::Internal,
        ] {
            assert_eq!(AppError::from(error), AppError::Internal);
        }
    }

    /// A statement timeout and a lock timeout through the generic path give the same
    /// envelope, and it carries no SQLSTATE text.
    #[test]
    fn statement_and_lock_timeouts_share_one_envelope_without_sqlstate_text() {
        let statement =
            jobniac_core::errors::error_envelope(&AppError::from(PgError::QueryCanceled));
        let lock = jobniac_core::errors::error_envelope(&AppError::from(PgError::LockNotAvailable));
        assert_eq!(statement, lock);
        assert!(!statement.message.contains("57014"));
        assert!(!statement.message.contains("55P03"));
    }

    /// The profile-details policy turns every failure except a constraint into "unavailable".
    #[test]
    fn details_policy_maps_timeouts_and_internal_failures_to_unavailable() {
        let unavailable = jobniac_core::errors::error_envelope(&AppError::from(
            ApplicationError::database_unavailable(),
        ));
        for error in [
            PgError::Unavailable,
            PgError::Serialization,
            PgError::QueryCanceled,
            PgError::LockNotAvailable,
            PgError::ReadOnlyJoin,
            PgError::Internal,
        ] {
            let envelope = jobniac_core::errors::error_envelope(&details_app_error(error.clone()));
            assert_eq!(envelope, unavailable, "{error:?}");
        }
        let constraint = details_app_error(PgError::Constraint {
            sqlstate: "23505".to_owned(),
            constraint: Some("uq".to_owned()),
        });
        assert!(matches!(constraint, AppError::PersistenceConstraint { .. }));
        let not_configured = details_app_error(PgError::NotConfigured);
        assert_eq!(
            jobniac_core::errors::error_envelope(&not_configured).message,
            NOT_CONFIGURED_MESSAGE
        );
    }

    #[test]
    fn errors_render_without_values() {
        assert_eq!(
            PgError::Constraint {
                sqlstate: "23505".to_owned(),
                constraint: Some("secret_row_value".to_owned()),
            }
            .to_string(),
            "database error: constraint violation"
        );
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
    async fn outside_a_transaction_hooks_run_now_and_rollback_hooks_are_dropped() {
        let events: Events = Arc::default();
        let uow = PgUnitOfWork::new(PgPool::new(None));
        uow.after_commit(record(&events, "commit")).await;
        uow.after_rollback(record(&events, "rollback"));
        uow.after_completion(record(&events, "completion")).await;
        assert_eq!(snapshot(&events), ["commit", "completion"]);
    }

    #[tokio::test]
    async fn an_unconfigured_unit_of_work_fails_before_any_scope_starts() {
        let uow = PgUnitOfWork::new(PgPool::new(None));
        let outcome: Result<(), AppError> = uow
            .transaction(TransactionOptions::default(), async |_| Ok(()))
            .await;
        assert_eq!(outcome, Err(AppError::from(PgError::NotConfigured)));
    }
}
