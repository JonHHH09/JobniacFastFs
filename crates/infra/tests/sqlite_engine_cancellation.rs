//! A cancelled write scope gives up the write lock at once. All data is synthetic.

mod sqlite_support;

use std::time::Duration;

use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::sqlite::error::SqliteError;
use sqlite_support::{SqliteFixture, TestResult};

#[tokio::test]
async fn a_dropped_write_scope_releases_the_lock_for_another_connection() -> TestResult {
    let fixture = SqliteFixture::start();
    fixture
        .connection()
        .execute_batch("CREATE TABLE engine_probe (x INTEGER)")?;
    let uow = fixture.unit_of_work();

    // The scope writes, then waits forever: the timeout drops its future mid-transaction.
    let cancelled = tokio::time::timeout(
        Duration::from_millis(150),
        uow.transaction::<(), SqliteError>(TransactionOptions::default(), async |tx| {
            tx.run(|conn| {
                conn.execute("INSERT INTO engine_probe VALUES (1)", [])?;
                Ok(())
            })?;
            std::future::pending::<()>().await;
            Ok(())
        }),
    )
    .await;
    assert!(cancelled.is_err(), "the scope was cancelled");

    // Another connection (as another process would) writes immediately: no busy wait.
    let other = fixture.connection();
    other.busy_timeout(Duration::ZERO)?;
    other.execute_batch("BEGIN IMMEDIATE; INSERT INTO engine_probe VALUES (2); COMMIT")?;
    let rows: Vec<i64> = other
        .prepare("SELECT x FROM engine_probe ORDER BY x")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    assert_eq!(rows, [2], "the cancelled scope's write was rolled back");

    // The pool's own writer is free again.
    uow.transaction::<(), SqliteError>(TransactionOptions::default(), async |tx| {
        tx.run(|conn| {
            conn.execute("INSERT INTO engine_probe VALUES (3)", [])?;
            Ok(())
        })
    })
    .await?;
    Ok(())
}
