//! The shared SQLite fixture migrates a fresh file and serves both the pool and raw connections.

mod sqlite_support;

use sqlite_support::{SqliteFixture, TestResult};

#[tokio::test]
async fn fixture_opens_a_migrated_database() -> TestResult {
    let fixture = SqliteFixture::start();
    let tables: i64 = fixture.connection().query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = 'profile_profiles'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(tables, 1);
    assert!(fixture.pool().configured());
    Ok(())
}
