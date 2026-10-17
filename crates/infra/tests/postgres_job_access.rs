//! Connection policy and transaction scope of the job adapters: the `PGSERVICE` refusal (no
//! connection is attempted), the snapshot of a read-only scope that serves several reads, and the
//! application names of the job and job search sessions.
//!
//! The refusal test needs no server. The other tests skip with a
//! notice when `JOBNIAC_TEST_PG_PORT` is unset. All data is synthetic.

mod job_support;

use std::io::ErrorKind;
use std::net::TcpListener;

use job_support::{TestResult, start, text_graph};
use jobniac_core::env::Environment;
use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::jobs::JobRepository;
use jobniac_core::profiles::repository::{READ_ONLY, READ_WRITE};
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::jobs::PgJobRepository;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::session::{APPLICATION_NAME, JOB_SEARCH_APPLICATION_NAME};
use jobniac_infra::postgres::unit_of_work::{PgError, PgTransaction, PgUnitOfWork};
use time::OffsetDateTime;
use time::macros::datetime;

const CREATED_AT: OffsetDateTime = datetime!(2026-09-29 10:00 UTC);
const UNAVAILABLE: &str = "Local database is unavailable";

/// With `PGSERVICE` set, the job repository refuses every connection before it connects. A loopback listener records whether any connection arrived.
#[tokio::test]
async fn test_repository_rejects_unvalidated_libpq_service() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port().to_string();
    let config = load_database_config(&Environment::from_pairs([
        ("JOBNIAC_DB_HOST", "127.0.0.1"),
        ("JOBNIAC_DB_PORT", port.as_str()),
        ("JOBNIAC_DB_NAME", "jobniac_service"),
        ("JOBNIAC_DB_USER", "app_user"),
        ("JOBNIAC_DB_PASSWORD", "synthetic-password"),
        ("PGSERVICE", "synthetic-service"),
    ]))
    .ok_or("a configuration with PGSERVICE set must still validate")?;
    let uow = PgUnitOfWork::new(PgPool::new(Some(config)));
    let graph = text_graph(50, CREATED_AT);

    let create = uow
        .transaction(READ_WRITE, async |tx| {
            PgJobRepository.create_text_job(tx, &graph).await
        })
        .await
        .err()
        .ok_or("a create with PGSERVICE set must be refused")?;
    assert_eq!(error_envelope(&create).message, UNAVAILABLE);

    let read = uow
        .transaction(READ_ONLY, async |tx| {
            PgJobRepository.get_job(tx, graph.job.id).await
        })
        .await
        .err()
        .ok_or("a read with PGSERVICE set must be refused")?;
    assert_eq!(error_envelope(&read).message, UNAVAILABLE);

    match listener.accept() {
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(()),
        Ok(_) => Err("the refused repository attempted a connection".into()),
        Err(error) => Err(error.into()),
    }
}

/// The isolation level and the read-only flag of a scope, as the server reports them.
async fn scope_settings(tx: &PgTransaction) -> Result<(String, String), PgError> {
    let row = tx
        .client()
        .query_one(
            "SELECT current_setting('transaction_isolation'), \
             current_setting('transaction_read_only')",
            &[],
        )
        .await?;
    Ok((row.get(0), row.get(1)))
}

/// A read-only scope begins at REPEATABLE READ, so several job reads in one scope all succeed and
/// see one snapshot. The repositories set no isolation level themselves.
#[tokio::test]
async fn a_read_only_scope_serves_several_job_reads_from_one_snapshot() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let first = text_graph(60, CREATED_AT);
    let second = text_graph(61, CREATED_AT);
    let (first_id, second_id) = (first.job.id, second.job.id);
    for graph in [&first, &second] {
        fixture
            .uow
            .transaction(READ_WRITE, async |tx| {
                PgJobRepository.create_text_job(tx, graph).await
            })
            .await?;
    }

    let (settings, first_read, second_read) = fixture
        .uow
        .transaction(READ_ONLY, async |tx| {
            let settings = scope_settings(tx).await?;
            let first_read = PgJobRepository.get_job(tx, first_id).await?;
            let second_read = PgJobRepository.get_job(tx, second_id).await?;
            Ok::<_, AppError>((settings, first_read, second_read))
        })
        .await?;
    assert_eq!(
        settings,
        ("repeatable read".to_owned(), "on".to_owned()),
        "a read-only scope is REPEATABLE READ and read-only"
    );
    assert_eq!(first_read, Some(first));
    assert_eq!(second_read, Some(second));
    Ok(())
}

/// The application name of a session that a unit of work opened.
async fn application_name(uow: &PgUnitOfWork) -> Result<String, PgError> {
    uow.transaction(READ_ONLY, async |tx| {
        let row = tx
            .client()
            .query_one(
                "SELECT application_name FROM pg_stat_activity WHERE pid = pg_backend_pid()",
                &[],
            )
            .await?;
        Ok::<String, PgError>(row.get(0))
    })
    .await
}

/// Job search sessions are named `jobniac-job-search`, and the other job sessions `jobniac`, so the
/// two can be told apart in `pg_stat_activity`.
#[tokio::test]
async fn job_search_sessions_are_named_apart_from_the_other_job_sessions() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let config = load_database_config(&fixture.database.environment())
        .ok_or("the disposable database configuration is invalid")?;
    let search_uow = PgUnitOfWork::new(PgPool::with_application_name(
        Some(config),
        JOB_SEARCH_APPLICATION_NAME,
    ));
    assert_eq!(
        application_name(&search_uow).await?,
        JOB_SEARCH_APPLICATION_NAME
    );
    assert_eq!(application_name(&fixture.uow).await?, APPLICATION_NAME);
    Ok(())
}
