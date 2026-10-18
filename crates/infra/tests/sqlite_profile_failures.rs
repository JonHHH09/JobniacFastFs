//! Database failures on the profile path, against a disposable SQLite database.
//!
//! The profile repositories map an integrity violation to a persistence-constraint error and every
//! other database error to "Local database is unavailable". The tests here check that: only a
//! constraint keeps its class, and lock waits, engine errors and every other database failure come
//! out as "unavailable" without the engine message. The closed-pool cases need no file. All data
//! is synthetic.

mod sqlite_profile_support;

use std::time::Duration;

use jobniac_core::errors::{PERSISTENCE_CONSTRAINT_MESSAGE, error_envelope};
use jobniac_infra::sqlite::pool::{PoolOptions, SqlitePool};
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use rusqlite::Connection;
use serde_json::json;
use sqlite_profile_support::{TestResult, details, list, request, search, start};

const CONTACT_ID: &str = "11111111-1111-1111-1111-111111111111";
const PROJECT_ID: &str = "77777777-7777-7777-7777-777777777777";
const TECHNOLOGY_ID: &str = "88888888-8888-8888-8888-888888888888";
const UNAVAILABLE: &str = "Local database is unavailable";

/// Installs a `BEFORE INSERT` trigger on `profile_profile_contacts` that runs `body`. The bodies
/// below carry a row value that must never reach the caller.
fn on_contact_insert(connection: &Connection, body: &str) -> TestResult {
    connection.execute_batch(&format!(
        "DROP TRIGGER IF EXISTS test_on_contact; \
         CREATE TRIGGER test_on_contact BEFORE INSERT ON profile_profile_contacts \
         FOR EACH ROW BEGIN {body}; END;"
    ))?;
    Ok(())
}

#[tokio::test]
async fn driver_failures_on_the_replace_path_map_to_the_expected_envelopes() -> TestResult {
    let fixture = start();
    let service = details(&fixture.uow);
    let created = service
        .create_profile(&request(
            "failures@example.test",
            None,
            PROJECT_ID,
            TECHNOLOGY_ID,
        ))
        .await?;
    let profile_id = created.profile.id.to_string();
    let connection = fixture.connection();
    let child_request = request(
        "failures@example.test",
        Some(CONTACT_ID),
        PROJECT_ID,
        TECHNOLOGY_ID,
    );

    let cases: [(&str, &str, &str); 5] = [
        (
            "raise abort",
            "SELECT RAISE(ABORT, 'secret row value ' || NEW.contact_value)",
            PERSISTENCE_CONSTRAINT_MESSAGE,
        ),
        (
            "raise fail",
            "SELECT RAISE(FAIL, 'secret row value ' || NEW.contact_value)",
            PERSISTENCE_CONSTRAINT_MESSAGE,
        ),
        (
            "integer overflow",
            "SELECT abs(-9223372036854775807 - 1) || NEW.contact_value",
            UNAVAILABLE,
        ),
        (
            "malformed json",
            "SELECT json_extract('{secret ' || NEW.contact_value, '$')",
            UNAVAILABLE,
        ),
        (
            "unknown function",
            "SELECT no_such_function(NEW.contact_value)",
            UNAVAILABLE,
        ),
    ];
    for (label, body, expected) in cases {
        on_contact_insert(&connection, body)?;
        let failure = service
            .update_profile(&profile_id, &json!(0), &child_request)
            .await
            .err()
            .ok_or(format!("{label} must fail the replace"))?;
        let envelope = error_envelope(&failure);
        assert_eq!(envelope.message, expected, "{label}");
        let rendered = format!("{failure:?} {}", envelope.message);
        assert!(
            !rendered.contains("secret row value") && !rendered.contains("555-0101"),
            "the engine message must not leave the adapter ({label})"
        );
        let stored = service.get_profile(&profile_id).await?;
        assert_eq!(
            stored.profile.revision, 0,
            "a failed replace changes nothing"
        );
        assert!(
            stored.contacts.is_empty(),
            "a failed replace changes nothing"
        );
    }
    connection.execute_batch("DROP TRIGGER IF EXISTS test_on_contact")?;
    Ok(())
}

/// A profile whose database is write-locked by another connection. A replace waits for the
/// engine's busy timeout and the bounded retries, then fails as unavailable. A read is not
/// blocked in WAL mode.
#[tokio::test]
async fn a_write_lock_held_elsewhere_is_unavailable_and_reads_continue() -> TestResult {
    let fixture = start();
    let created = details(&fixture.uow)
        .create_profile(&request(
            "timeouts@example.test",
            None,
            PROJECT_ID,
            TECHNOLOGY_ID,
        ))
        .await?;
    let profile_id = created.profile.id.to_string();
    let options = PoolOptions {
        busy_timeout: Duration::from_millis(100),
        busy_retries: 1,
        busy_retry_delay: Duration::from_millis(10),
        ..PoolOptions::default()
    };
    let pool = SqlitePool::open_with(fixture.database.path(), options)?;
    let service = details(&SqliteUnitOfWork::new(pool));

    let holder = fixture.connection();
    holder.execute_batch("BEGIN IMMEDIATE")?;
    let write = service
        .update_profile(
            &profile_id,
            &json!(0),
            &request(
                "timeouts@example.test",
                Some(CONTACT_ID),
                PROJECT_ID,
                TECHNOLOGY_ID,
            ),
        )
        .await
        .err()
        .ok_or("a replace blocked by a write lock must fail")?;
    let read = service.get_profile(&profile_id).await?;
    holder.execute_batch("ROLLBACK")?;

    let envelope = error_envelope(&write);
    assert_eq!(envelope.message, UNAVAILABLE);
    assert!(!format!("{write:?}").contains("locked"));
    assert_eq!(read.profile.revision, 0);
    Ok(())
}

/// A column whose stored value no longer decodes is corrupted data, not a database outage: the
/// repositories do not classify it as a database error.
#[tokio::test]
async fn a_corrupted_value_is_an_internal_error_not_unavailable() -> TestResult {
    let fixture = start();
    let created = details(&fixture.uow)
        .create_profile(&request(
            "retyped@example.test",
            None,
            PROJECT_ID,
            TECHNOLOGY_ID,
        ))
        .await?;
    let connection = fixture.connection();
    // A stored text that is not an instant is valid TEXT for the schema but cannot decode.
    connection.execute(
        "UPDATE profile_profiles SET created_at = 'not an instant', updated_at = 'not an instant' \
         WHERE id = ?1",
        [created.profile.id.to_string()],
    )?;
    let failure = details(&fixture.uow)
        .get_profile(&created.profile.id.to_string())
        .await
        .err()
        .ok_or("a column that no longer decodes must fail the read")?;
    assert_eq!(
        error_envelope(&failure).message,
        "Unexpected application error"
    );
    Ok(())
}

#[tokio::test]
async fn list_and_search_database_failures_are_scrubbed() -> TestResult {
    let fixture = start();
    let pool = fixture.database.pool();
    pool.close();
    let uow = SqliteUnitOfWork::new(pool);

    let list_failure = list(&uow)?
        .list_profiles(Some(2), None)
        .await
        .err()
        .ok_or("listing from a closed pool must fail")?;
    let envelope = error_envelope(&list_failure);
    assert_eq!(envelope.message, UNAVAILABLE);
    assert!(!format!("{list_failure:?}").contains("jobniac.sqlite3"));

    let search_failure = search(&uow)
        .search_profiles(&json!({"query": "ada"}))
        .await
        .err()
        .ok_or("searching a closed pool must fail")?;
    let envelope = error_envelope(&search_failure);
    assert_eq!(envelope.message, UNAVAILABLE);
    assert!(!format!("{search_failure:?}").contains("jobniac.sqlite3"));
    Ok(())
}
