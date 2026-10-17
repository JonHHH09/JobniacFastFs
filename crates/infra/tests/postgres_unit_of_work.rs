//! Unit-of-work and session-pool tests against a disposable PostgreSQL 18 server.
//!
//! The server-backed tests need `JOBNIAC_TEST_PG_PORT` and are skipped with a notice when it is
//! unset (see the testkit README). The unconfigured, unreachable and never-answering cases need no
//! server. All data is synthetic.

use std::collections::HashMap;
use std::error::Error;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jobniac_core::env::Environment;
use jobniac_core::errors::{AppError, PERSISTENCE_CONSTRAINT_MESSAGE, error_envelope};
use jobniac_core::transactions::{Hook, HookError, TransactionOptions, deferred, deferred_sync};
use jobniac_infra::postgres::config::{DatabaseConfig, load_database_config};
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::{
    PgError, PgTransaction, PgUnitOfWork, details_app_error,
};
use jobniac_testkit::pg::{TestDatabase, TestServer};

type TestResult = Result<(), Box<dyn Error>>;
type Events = Arc<Mutex<Vec<&'static str>>>;

const CREATE_ROWS: &str = "CREATE TABLE public.uow_rows (\
    id integer PRIMARY KEY, \
    label text NOT NULL, \
    CONSTRAINT uow_rows_label_unique UNIQUE (label))";

/// A table whose unique label is checked only at COMMIT.
const CREATE_DEFERRED: &str = "CREATE TABLE public.uow_deferred (\
    id integer PRIMARY KEY, \
    label text NOT NULL, \
    CONSTRAINT uow_deferred_label_unique UNIQUE (label) DEFERRABLE INITIALLY DEFERRED)";

const INSERT_ROW: &str = "INSERT INTO public.uow_rows (id, label) VALUES ($1, $2)";

fn default_options() -> TransactionOptions {
    TransactionOptions::default()
}

fn read_only_options() -> TransactionOptions {
    TransactionOptions {
        read_only: true,
        requires_new: false,
    }
}

fn requires_new_options() -> TransactionOptions {
    TransactionOptions {
        read_only: false,
        requires_new: true,
    }
}

/// A fresh database with the `uow_rows` table and a unit of work over `max_connections`
/// sessions, or `None` when no server is configured.
async fn start(
    max_connections: usize,
) -> Result<Option<(TestDatabase, PgUnitOfWork)>, Box<dyn Error>> {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(None);
    };
    let database = server.fresh_database().await;
    database
        .admin_client()
        .await
        .batch_execute(CREATE_ROWS)
        .await?;
    let config = config_of(&database)?;
    let pool = PgPool::with_limits(Some(config), max_connections, Duration::from_secs(2));
    Ok(Some((database, PgUnitOfWork::new(pool))))
}

async fn insert(tx: &PgTransaction, id: i32, label: &str) -> Result<(), PgError> {
    tx.client().execute(INSERT_ROW, &[&id, &label]).await?;
    Ok(())
}

async fn labels(database: &TestDatabase) -> Result<Vec<String>, Box<dyn Error>> {
    let client = database.admin_client().await;
    let rows = client
        .query("SELECT label FROM public.uow_rows ORDER BY id", &[])
        .await?;
    let mut labels = Vec::with_capacity(rows.len());
    for row in &rows {
        labels.push(row.try_get::<_, String>(0)?);
    }
    Ok(labels)
}

/// The envelope of a generic-path error, for comparisons that must not depend on the variant.
fn envelope_of(error: &AppError) -> jobniac_core::errors::ErrorEnvelope {
    error_envelope(error)
}

/// Maps a driver error the way the profile-details repositories do.
fn details(error: tokio_postgres::Error) -> AppError {
    details_app_error(PgError::from(error))
}

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

/// A hook that needs one session from `pool`. It runs after the committing scope released its
/// session, so with a pool of one session it succeeds only when the release happened first.
async fn probe_after_release(pool: PgPool, events: Events) -> Result<(), HookError> {
    let session = pool
        .acquire()
        .await
        .map_err(|_| HookError::new("Unavailable"))?;
    drop(session);
    events
        .lock()
        .map_err(|_| HookError::new("PoisonedLock"))?
        .push("commit two after release");
    Ok(())
}

/// Waits until no application session of this database is open on the server. A cancelled
/// session is closed by the client, and the server then ends its backend.
async fn wait_for_no_app_sessions(database: &TestDatabase) -> Result<(), Box<dyn Error>> {
    let admin = database.admin_client().await;
    for _ in 0..200 {
        let row = admin
            .query_one(
                "SELECT count(*) FROM pg_catalog.pg_stat_activity \
                 WHERE application_name = 'jobniac' AND datname = $1",
                &[&database.database_name()],
            )
            .await?;
        if row.try_get::<_, i64>(0)? == 0 {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err("an application session is still open on the server".into())
}

#[tokio::test]
async fn committed_work_is_visible_after_the_scope_returns() -> TestResult {
    let Some((database, uow)) = start(2).await? else {
        return Ok(());
    };
    let outcome: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            insert(tx, 1, "committed").await
        })
        .await;
    outcome?;
    assert_eq!(labels(&database).await?, ["committed"]);
    Ok(())
}

#[tokio::test]
async fn a_body_error_rolls_back_and_is_returned_unchanged() -> TestResult {
    let Some((database, uow)) = start(2).await? else {
        return Ok(());
    };
    let outcome: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            insert(tx, 1, "rolled-back").await?;
            Err(PgError::Internal)
        })
        .await;
    assert_eq!(outcome, Err(PgError::Internal));
    assert!(labels(&database).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn read_only_scope_reads_and_rejects_writes_with_25006() -> TestResult {
    let Some((database, uow)) = start(2).await? else {
        return Ok(());
    };
    let mut denied_code: Option<String> = None;
    let outcome: Result<(), PgError> = uow
        .transaction(read_only_options(), async |tx| {
            let row = tx
                .client()
                .query_one("SELECT count(*) FROM public.uow_rows", &[])
                .await?;
            assert_eq!(row.try_get::<_, i64>(0)?, 0);
            assert!(tx.read_only());
            // The raw server error is inspected before the port classifies it.
            let denied = tx.client().execute(INSERT_ROW, &[&1_i32, &"denied"]).await;
            denied_code = denied
                .err()
                .and_then(|error| error.code().map(|code| code.code().to_owned()));
            Err(PgError::Internal)
        })
        .await;
    assert_eq!(denied_code.as_deref(), Some("25006"));
    assert_eq!(outcome, Err(PgError::Internal));
    assert!(labels(&database).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn unique_violation_maps_to_a_constraint_error_with_no_wire_details() -> TestResult {
    let Some((database, uow)) = start(2).await? else {
        return Ok(());
    };
    let first: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            insert(tx, 1, "duplicate").await
        })
        .await;
    first?;
    let second: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            insert(tx, 2, "duplicate").await
        })
        .await;
    let error = second.err().ok_or("the duplicate label was accepted")?;
    assert_eq!(
        error,
        PgError::Constraint {
            sqlstate: "23505".to_owned(),
            constraint: Some("uow_rows_label_unique".to_owned()),
        }
    );
    let app = AppError::from(error);
    assert!(matches!(app, AppError::PersistenceConstraint { .. }));
    let envelope = envelope_of(&app);
    assert_eq!(envelope.message, PERSISTENCE_CONSTRAINT_MESSAGE);
    assert!(envelope.details.is_empty());
    assert_eq!(labels(&database).await?, ["duplicate"]);
    Ok(())
}

#[tokio::test]
async fn a_commit_time_constraint_failure_runs_rollback_hooks_only() -> TestResult {
    let Some((database, uow)) = start(2).await? else {
        return Ok(());
    };
    database
        .admin_client()
        .await
        .batch_execute(CREATE_DEFERRED)
        .await?;
    let events: Events = Arc::default();
    let outcome: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            // Both rows are accepted here; the deferred unique check fails at COMMIT.
            tx.client()
                .execute(
                    "INSERT INTO public.uow_deferred (id, label) VALUES ($1, $2)",
                    &[&1_i32, &"same"],
                )
                .await?;
            tx.client()
                .execute(
                    "INSERT INTO public.uow_deferred (id, label) VALUES ($1, $2)",
                    &[&2_i32, &"same"],
                )
                .await?;
            tx.after_commit(record(&events, "commit"));
            tx.after_rollback(record(&events, "rollback"));
            tx.after_completion(record(&events, "completion"));
            Ok(())
        })
        .await;
    assert_eq!(
        outcome,
        Err(PgError::Constraint {
            sqlstate: "23505".to_owned(),
            constraint: Some("uow_deferred_label_unique".to_owned()),
        })
    );
    assert_eq!(snapshot(&events), ["rollback", "completion"]);
    let count = database
        .admin_client()
        .await
        .query_one("SELECT count(*) FROM public.uow_deferred", &[])
        .await?
        .try_get::<_, i64>(0)?;
    assert_eq!(count, 0);
    Ok(())
}

#[tokio::test]
async fn statement_timeout_is_reported_as_query_canceled() -> TestResult {
    let Some((_database, uow)) = start(2).await? else {
        return Ok(());
    };
    let outcome: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            tx.client()
                .batch_execute("SET LOCAL statement_timeout = '50ms'")
                .await?;
            tx.client().query_one("SELECT pg_sleep(2)", &[]).await?;
            Ok(())
        })
        .await;
    assert_eq!(outcome, Err(PgError::QueryCanceled));
    Ok(())
}

#[tokio::test]
async fn statement_timeout_on_the_details_path_is_unavailable_without_sqlstate_text() -> TestResult
{
    let Some((_database, uow)) = start(2).await? else {
        return Ok(());
    };
    let outcome: Result<(), AppError> = uow
        .transaction(default_options(), async |tx| {
            tx.client()
                .batch_execute("SET LOCAL statement_timeout = '50ms'")
                .await
                .map_err(details)?;
            tx.client()
                .query_one("SELECT pg_sleep(2)", &[])
                .await
                .map_err(details)?;
            Ok(())
        })
        .await;
    let error = outcome.err().ok_or("the statement was not canceled")?;
    let envelope = envelope_of(&error);
    assert_eq!(envelope.code.code(), "internal_error");
    assert_eq!(envelope.message, "Local database is unavailable");
    assert!(!envelope.message.contains("57014"));
    Ok(())
}

#[tokio::test]
async fn lock_timeout_is_reported_as_lock_not_available() -> TestResult {
    let Some((database, uow)) = start(2).await? else {
        return Ok(());
    };
    let admin = database.admin_client().await;
    admin
        .batch_execute("BEGIN; LOCK TABLE public.uow_rows IN ACCESS EXCLUSIVE MODE")
        .await?;
    let outcome: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            tx.client()
                .batch_execute("SET LOCAL lock_timeout = '100ms'")
                .await?;
            tx.client()
                .query_one("SELECT count(*) FROM public.uow_rows", &[])
                .await?;
            Ok(())
        })
        .await;
    admin.batch_execute("ROLLBACK").await?;
    assert_eq!(outcome, Err(PgError::LockNotAvailable));
    // The generic path gives a statement timeout and a lock timeout the same envelope.
    assert_eq!(
        envelope_of(&AppError::from(PgError::QueryCanceled)),
        envelope_of(&AppError::from(PgError::LockNotAvailable))
    );
    Ok(())
}

#[tokio::test]
async fn client_side_driver_errors_are_internal() -> TestResult {
    let Some((_database, uow)) = start(2).await? else {
        return Ok(());
    };
    // A text value for an integer parameter is rejected by the client before it is sent.
    let to_sql: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            tx.client()
                .execute(INSERT_ROW, &[&"not a number", &"x"])
                .await?;
            Ok(())
        })
        .await;
    assert_eq!(to_sql, Err(PgError::Internal));

    // A text column read as an integer fails in the client.
    let from_sql: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            let row = tx.client().query_one("SELECT 'text'::text", &[]).await?;
            row.try_get::<_, i32>(0)?;
            Ok(())
        })
        .await;
    assert_eq!(from_sql, Err(PgError::Internal));

    // A column name that the row does not have.
    let column: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            let row = tx.client().query_one("SELECT 1 AS present", &[]).await?;
            row.try_get::<_, i32>("missing")?;
            Ok(())
        })
        .await;
    assert_eq!(column, Err(PgError::Internal));
    Ok(())
}

#[tokio::test]
async fn a_terminated_session_is_unavailable() -> TestResult {
    let Some((database, uow)) = start(2).await? else {
        return Ok(());
    };
    let admin = database.admin_client().await;
    let outcome: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            let row = tx
                .client()
                .query_one("SELECT pg_backend_pid()", &[])
                .await?;
            let pid: i32 = row.try_get(0)?;
            admin
                .execute("SELECT pg_terminate_backend($1)", &[&pid])
                .await?;
            // Wait until the backend is gone, so the next statement sees a dead session.
            for _ in 0..200 {
                let alive = admin
                    .query_one(
                        "SELECT count(*) FROM pg_catalog.pg_stat_activity WHERE pid = $1",
                        &[&pid],
                    )
                    .await?
                    .try_get::<_, i64>(0)?;
                if alive == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            tx.client().query_one("SELECT 1", &[]).await?;
            Ok(())
        })
        .await;
    assert_eq!(outcome, Err(PgError::Unavailable));
    Ok(())
}

#[tokio::test]
async fn application_sessions_carry_the_shared_session_policy() -> TestResult {
    let Some((_database, uow)) = start(2).await? else {
        return Ok(());
    };
    let settings: Result<HashMap<String, String>, PgError> = uow
        .transaction(default_options(), async |tx| {
            let rows = tx
                .client()
                .query(
                    "SELECT name, setting FROM pg_catalog.pg_settings \
                     WHERE name IN ('application_name', 'search_path', 'statement_timeout', \
                     'lock_timeout', 'idle_in_transaction_session_timeout', \
                     'default_transaction_read_only')",
                    &[],
                )
                .await?;
            let mut settings = HashMap::new();
            for row in &rows {
                settings.insert(row.try_get::<_, String>(0)?, row.try_get::<_, String>(1)?);
            }
            Ok(settings)
        })
        .await;
    let settings = settings?;
    let expected = [
        ("application_name", "jobniac"),
        ("search_path", "pg_catalog"),
        ("statement_timeout", "30000"),
        ("lock_timeout", "10000"),
        ("idle_in_transaction_session_timeout", "120000"),
        ("default_transaction_read_only", "off"),
    ];
    for (name, value) in expected {
        assert_eq!(
            settings.get(name).map(String::as_str),
            Some(value),
            "{name}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_session_setting_does_not_outlive_its_scope() -> TestResult {
    let Some((_database, uow)) = start(1).await? else {
        return Ok(());
    };
    // A committed session-level SET is undone before the one session is reused.
    let first: Result<i32, PgError> = uow
        .transaction(default_options(), async |tx| {
            tx.client()
                .batch_execute("SET search_path = public, pg_catalog")
                .await?;
            let row = tx
                .client()
                .query_one("SELECT pg_backend_pid()", &[])
                .await?;
            Ok(row.try_get::<_, i32>(0)?)
        })
        .await;
    let first_pid = first?;
    let second: Result<(i32, String), PgError> = uow
        .transaction(default_options(), async |tx| {
            let row = tx
                .client()
                .query_one("SELECT pg_backend_pid()", &[])
                .await?;
            let search_path = tx.client().query_one("SHOW search_path", &[]).await?;
            Ok((
                row.try_get::<_, i32>(0)?,
                search_path.try_get::<_, String>(0)?,
            ))
        })
        .await;
    let (second_pid, search_path) = second?;
    assert_eq!(second_pid, first_pid, "the session was reused");
    assert_eq!(search_path, "pg_catalog");
    Ok(())
}

#[tokio::test]
async fn a_role_or_session_authorization_does_not_outlive_its_scope() -> TestResult {
    let Some((database, uow)) = start(1).await? else {
        return Ok(());
    };
    // Role names are cluster-wide, so the role is named after the disposable database.
    let role = database.database_name();
    let admin = database.admin_client().await;
    admin
        .batch_execute(&format!(
            "CREATE ROLE \"{role}\" NOLOGIN; GRANT \"{role}\" TO CURRENT_USER"
        ))
        .await?;
    let superuser = admin
        .query_one(
            "SELECT rolsuper FROM pg_roles WHERE rolname = current_user",
            &[],
        )
        .await?
        .try_get::<_, bool>(0)?;

    let baseline: Result<(String, String), PgError> = uow
        .transaction(default_options(), async |tx| {
            let row = tx
                .client()
                .query_one("SELECT current_user::text, session_user::text", &[])
                .await?;
            Ok((row.try_get::<_, String>(0)?, row.try_get::<_, String>(1)?))
        })
        .await;
    let baseline = baseline?;

    // The committed scope changes the role; SET SESSION AUTHORIZATION only where it is allowed.
    let changed: Result<String, PgError> = uow
        .transaction(default_options(), async |tx| {
            tx.client()
                .batch_execute(&format!("SET ROLE \"{role}\""))
                .await?;
            if superuser {
                tx.client()
                    .batch_execute(&format!("SET SESSION AUTHORIZATION \"{role}\""))
                    .await?;
            }
            let row = tx
                .client()
                .query_one("SELECT current_user::text", &[])
                .await?;
            Ok(row.try_get::<_, String>(0)?)
        })
        .await;
    assert_eq!(changed?, role);

    // The next scope runs on the same session and sees the original identities again.
    let after: Result<(String, String), PgError> = uow
        .transaction(default_options(), async |tx| {
            let row = tx
                .client()
                .query_one("SELECT current_user::text, session_user::text", &[])
                .await?;
            Ok((row.try_get::<_, String>(0)?, row.try_get::<_, String>(1)?))
        })
        .await;
    assert_eq!(after?, baseline);
    admin
        .batch_execute(&format!("DROP ROLE \"{role}\""))
        .await?;
    Ok(())
}

#[tokio::test]
async fn commit_hooks_run_in_order_after_the_session_is_released() -> TestResult {
    // One session only: the hook can acquire it only after the committing scope released it.
    let Some((database, uow)) = start(1).await? else {
        return Ok(());
    };
    let events: Events = Arc::default();
    let pool = uow.pool().clone();
    let hook_events = Arc::clone(&events);
    let outcome: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            insert(tx, 1, "hooked").await?;
            tx.after_commit(record(&events, "commit one"));
            tx.after_rollback(record(&events, "rollback"));
            let probe_pool = pool.clone();
            let probe_events = Arc::clone(&hook_events);
            tx.after_commit(deferred(move || {
                probe_after_release(probe_pool, probe_events)
            }));
            tx.after_completion(record(&events, "completion"));
            assert!(snapshot(&events).is_empty(), "hooks wait for the commit");
            Ok(())
        })
        .await;
    outcome?;
    assert_eq!(
        snapshot(&events),
        ["commit one", "commit two after release", "completion"]
    );
    assert_eq!(labels(&database).await?, ["hooked"]);
    Ok(())
}

#[tokio::test]
async fn rollback_hooks_run_after_rollback_and_before_completion() -> TestResult {
    let Some((_database, uow)) = start(2).await? else {
        return Ok(());
    };
    let events: Events = Arc::default();
    let outcome: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            tx.after_commit(record(&events, "commit"));
            tx.after_rollback(record(&events, "rollback"));
            tx.after_completion(record(&events, "completion"));
            Err(PgError::Internal)
        })
        .await;
    assert_eq!(outcome, Err(PgError::Internal));
    assert_eq!(snapshot(&events), ["rollback", "completion"]);
    Ok(())
}

#[tokio::test]
async fn a_joined_scope_shares_the_handle_and_its_hooks_belong_to_the_outer_scope() -> TestResult {
    let Some((database, uow)) = start(2).await? else {
        return Ok(());
    };
    let events: Events = Arc::default();
    let outcome: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            tx.after_rollback(record(&events, "outer rollback"));
            let inner: Result<(), PgError> = tx
                .transaction(default_options(), async |tx| {
                    insert(tx, 1, "joined").await?;
                    tx.after_commit(record(&events, "joined commit"));
                    Err(PgError::Internal)
                })
                .await;
            assert_eq!(inner, Err(PgError::Internal));
            // The joined failure is handed to the caller, which decides the outcome.
            inner
        })
        .await;
    assert_eq!(outcome, Err(PgError::Internal));
    assert!(labels(&database).await?.is_empty());
    assert_eq!(snapshot(&events), ["outer rollback"]);
    Ok(())
}

#[tokio::test]
async fn a_requires_new_scope_commits_on_its_own_session() -> TestResult {
    let Some((database, uow)) = start(2).await? else {
        return Ok(());
    };
    let outcome: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            insert(tx, 1, "outer").await?;
            let inner: Result<(), PgError> = tx
                .transaction(requires_new_options(), async |tx| {
                    insert(tx, 2, "inner").await
                })
                .await;
            inner?;
            Err(PgError::Internal)
        })
        .await;
    assert_eq!(outcome, Err(PgError::Internal));
    // The outer scope rolled back; the nested scope committed before it.
    assert_eq!(labels(&database).await?, ["inner"]);
    Ok(())
}

#[tokio::test]
async fn a_failed_requires_new_scope_rolls_back_alone_and_the_outer_scope_continues() -> TestResult
{
    let Some((database, uow)) = start(2).await? else {
        return Ok(());
    };
    let events: Events = Arc::default();
    let outcome: Result<(), PgError> = uow
        .transaction(default_options(), async |tx| {
            insert(tx, 1, "outer").await?;
            let inner: Result<(), PgError> = tx
                .transaction(requires_new_options(), async |tx| {
                    insert(tx, 2, "inner").await?;
                    tx.after_commit(record(&events, "inner commit"));
                    tx.after_rollback(record(&events, "inner rollback"));
                    Err(PgError::Internal)
                })
                .await;
            // The caller decides that the inner failure is not fatal to the outer scope.
            assert_eq!(inner, Err(PgError::Internal));
            Ok(())
        })
        .await;
    outcome?;
    assert_eq!(labels(&database).await?, ["outer"]);
    assert_eq!(snapshot(&events), ["inner rollback"]);
    Ok(())
}

#[tokio::test]
async fn a_write_scope_cannot_join_a_read_only_scope() -> TestResult {
    let Some((database, uow)) = start(2).await? else {
        return Ok(());
    };
    let outcome: Result<(), PgError> = uow
        .transaction(read_only_options(), async |tx| {
            tx.transaction(default_options(), async |tx| insert(tx, 1, "refused").await)
                .await
        })
        .await;
    assert_eq!(outcome, Err(PgError::ReadOnlyJoin));
    assert!(labels(&database).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_cancelled_scope_leaves_no_rows_runs_no_hooks_and_is_not_reused() -> TestResult {
    let Some((database, uow)) = start(1).await? else {
        return Ok(());
    };
    let events: Events = Arc::default();
    let scope = uow.transaction::<(), PgError>(default_options(), async |tx| {
        insert(tx, 1, "cancelled").await?;
        tx.after_commit(record(&events, "commit"));
        tx.after_rollback(record(&events, "rollback"));
        tx.after_completion(record(&events, "completion"));
        tx.client().batch_execute("SELECT pg_sleep(1)").await?;
        Ok(())
    });
    let cancelled = tokio::time::timeout(Duration::from_millis(200), scope).await;
    assert!(cancelled.is_err(), "the scope outlived its deadline");
    assert!(
        snapshot(&events).is_empty(),
        "no hook runs after a cancellation"
    );
    assert_eq!(
        uow.pool().idle_count(),
        0,
        "the session is not returned to the pool"
    );
    // The closed session is ended on the server, so the uncommitted row never appears.
    wait_for_no_app_sessions(&database).await?;
    assert!(labels(&database).await?.is_empty());
    // The pool slot was released, so a new scope gets a session.
    let again: Result<(), PgError> = uow.transaction(default_options(), async |_| Ok(())).await;
    again?;
    Ok(())
}

#[tokio::test]
async fn the_pool_bounds_sessions_in_use() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let config = config_of(&database)?;
    let pool = PgPool::with_limits(Some(config), 1, Duration::from_millis(100));
    let held = pool.acquire().await?;
    let waited = pool.acquire().await;
    assert_eq!(waited.err(), Some(PgError::Unavailable));
    drop(held);
    let again = pool.acquire().await?;
    drop(again);
    Ok(())
}

#[tokio::test]
async fn an_unconfigured_unit_of_work_reports_the_unavailable_message() -> TestResult {
    let uow = PgUnitOfWork::new(PgPool::new(None));
    let outcome: Result<(), AppError> = uow.transaction(default_options(), async |_| Ok(())).await;
    let error = outcome
        .err()
        .ok_or("an unconfigured pool started a transaction")?;
    let envelope = envelope_of(&error);
    assert_eq!(envelope.code.code(), "internal_error");
    assert_eq!(envelope.message, "Local database is not configured");
    assert!(envelope.details.is_empty());
    Ok(())
}

#[tokio::test]
async fn an_unreachable_server_is_reported_as_unavailable_without_details() -> TestResult {
    let environment = Environment::from_pairs([
        ("JOBNIAC_DB_HOST", "127.0.0.1".to_owned()),
        ("JOBNIAC_DB_PORT", "1".to_owned()),
        ("JOBNIAC_DB_NAME", "jobniac_unreachable".to_owned()),
        ("JOBNIAC_DB_USER", "jobniac_app".to_owned()),
        ("JOBNIAC_DB_PASSWORD", "synthetic-password".to_owned()),
    ]);
    let config = load_database_config(&environment)
        .ok_or("test environment is not a valid JOBNIAC_DB_* set")?;
    let uow = PgUnitOfWork::new(PgPool::with_limits(Some(config), 1, Duration::from_secs(1)));
    let outcome: Result<(), PgError> = uow.transaction(default_options(), async |_| Ok(())).await;
    assert_eq!(outcome, Err(PgError::Unavailable));
    let envelope = envelope_of(&AppError::from(PgError::Unavailable));
    assert_eq!(envelope.message, "Local database is unavailable");
    Ok(())
}

/// A listener that accepts TCP connections but never answers the startup message. The whole
/// connect (startup included) must end at the connect timeout, reported as unavailable.
#[tokio::test]
async fn a_connection_that_never_completes_startup_is_unavailable_at_the_connect_timeout()
-> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let environment = Environment::from_pairs([
        ("JOBNIAC_DB_HOST", "127.0.0.1".to_owned()),
        ("JOBNIAC_DB_PORT", port.to_string()),
        ("JOBNIAC_DB_NAME", "jobniac_silent".to_owned()),
        ("JOBNIAC_DB_USER", "jobniac_app".to_owned()),
        ("JOBNIAC_DB_PASSWORD", "synthetic-password".to_owned()),
    ]);
    let config = load_database_config(&environment)
        .ok_or("test environment is not a valid JOBNIAC_DB_* set")?;
    let uow = PgUnitOfWork::new(PgPool::with_limits(Some(config), 1, Duration::from_secs(1)));
    let started = Instant::now();
    let outcome: Result<(), PgError> = uow.transaction(default_options(), async |_| Ok(())).await;
    let elapsed = started.elapsed();
    drop(listener);
    assert_eq!(outcome, Err(PgError::Unavailable));
    assert!(
        elapsed >= Duration::from_secs(4),
        "the connect ended before its timeout: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "the connect was not bounded: {elapsed:?}"
    );
    Ok(())
}

/// The validated configuration of a disposable database.
fn config_of(database: &TestDatabase) -> Result<DatabaseConfig, Box<dyn Error>> {
    let config = load_database_config(&database.environment())
        .ok_or("test environment is not a valid JOBNIAC_DB_* set")?;
    Ok(config)
}
