//! Transaction scope of the job adapters: the snapshot of a read-only scope that serves several
//! reads, the refusal of a write in a read-only scope, and the unavailable and unconfigured
//! envelopes of the job repository.
//!
//! All data is synthetic.

mod sqlite_job_support;

use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::jobs::JobRepository;
use jobniac_core::profiles::repository::{READ_ONLY, READ_WRITE};
use jobniac_infra::sqlite::jobs::SqliteJobRepository;
use sqlite_job_support::{
    TestResult, closed_uow, insert_text_job_row, start, text_graph, unconfigured_uow,
};
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

const CREATED_AT: OffsetDateTime = datetime!(2026-09-29 10:00 UTC);
const UNAVAILABLE: &str = "Local database is unavailable";
const UNCONFIGURED: &str = "Local database is not configured";

/// With no database file configured, the job repository refuses every scope before it opens a
/// connection, and a closed pool reports the database as unavailable without any detail.
#[tokio::test]
async fn an_unconfigured_or_closed_store_refuses_every_scope() -> TestResult {
    let graph = text_graph(50, CREATED_AT);

    let unconfigured = unconfigured_uow();
    let create = unconfigured
        .transaction(READ_WRITE, async |tx| {
            SqliteJobRepository.create_text_job(tx, &graph).await
        })
        .await
        .err()
        .ok_or("a create without a database must be refused")?;
    assert_eq!(error_envelope(&create).message, UNCONFIGURED);

    let closed = closed_uow();
    let read = closed
        .transaction(READ_ONLY, async |tx| {
            SqliteJobRepository.get_job(tx, graph.job.id).await
        })
        .await
        .err()
        .ok_or("a read of a closed pool must be refused")?;
    let envelope = error_envelope(&read);
    assert_eq!(envelope.message, UNAVAILABLE);
    assert!(envelope.details.is_empty());
    Ok(())
}

/// A read-only scope is one snapshot: several job reads in one scope all succeed, and a job
/// committed by another connection in between is not visible to the later reads of the scope.
#[tokio::test]
async fn a_read_only_scope_serves_several_job_reads_from_one_snapshot() -> TestResult {
    let fixture = start();
    let first = text_graph(60, CREATED_AT);
    let second = text_graph(61, CREATED_AT);
    let (first_id, second_id) = (first.job.id, second.job.id);
    for graph in [&first, &second] {
        fixture
            .uow
            .transaction(READ_WRITE, async |tx| {
                SqliteJobRepository.create_text_job(tx, graph).await
            })
            .await?;
    }
    let late = Uuid::from_u128(62);
    let connection = fixture.connection();

    let (first_read, second_read, late_read) = fixture
        .uow
        .transaction(READ_ONLY, async |tx| {
            let first_read = SqliteJobRepository.get_job(tx, first_id).await?;
            insert_text_job_row(&connection, late, "Late job", CREATED_AT)
                .map_err(|_| AppError::Internal)?;
            let second_read = SqliteJobRepository.get_job(tx, second_id).await?;
            let late_read = SqliteJobRepository.get_job(tx, late).await?;
            Ok::<_, AppError>((first_read, second_read, late_read))
        })
        .await?;
    assert_eq!(first_read, Some(first));
    assert_eq!(second_read, Some(second));
    assert!(late_read.is_none(), "the snapshot predates the late job");

    let after = fixture
        .uow
        .transaction(READ_ONLY, async |tx| {
            SqliteJobRepository.get_job(tx, late).await
        })
        .await?;
    assert!(after.is_some(), "a new scope sees the committed job");
    Ok(())
}

/// A write attempted in a read-only scope is refused by the database and reported as an internal
/// error, and nothing is stored.
#[tokio::test]
async fn a_write_in_a_read_only_scope_is_refused() -> TestResult {
    let fixture = start();
    let graph = text_graph(70, CREATED_AT);
    let error = fixture
        .uow
        .transaction(READ_ONLY, async |tx| {
            SqliteJobRepository.create_text_job(tx, &graph).await
        })
        .await
        .err()
        .ok_or("a create in a read-only scope must be refused")?;
    assert_eq!(error_envelope(&error).message, UNAVAILABLE);
    let stored = fixture
        .uow
        .transaction(READ_ONLY, async |tx| {
            SqliteJobRepository.get_job(tx, graph.job.id).await
        })
        .await?;
    assert!(stored.is_none());
    Ok(())
}
