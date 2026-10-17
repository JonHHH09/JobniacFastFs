//! Database failures on the profile path, against a disposable PostgreSQL 18 server.
//!
//! The profile repositories map an integrity violation to a validation error and every other
//! database error to "Local database is unavailable". The tests here
//! check that: only SQLSTATE class 23 keeps its constraint, and
//! timeouts, lock waits and every other database failure come out as "unavailable" without the
//! SQLSTATE or the server message. The server-backed tests skip with a notice when
//! `JOBNIAC_TEST_PG_PORT` is unset. The unreachable-database cases need no server. All data is
//! synthetic.

mod profile_support;

use jobniac_core::errors::{PERSISTENCE_CONSTRAINT_MESSAGE, error_envelope};
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use profile_support::{TestResult, closed_port_config, details, list, request, search, start};
use serde_json::json;
use tokio_postgres::Client;

const CONTACT_ID: &str = "11111111-1111-1111-1111-111111111111";
const PROJECT_ID: &str = "77777777-7777-7777-7777-777777777777";
const TECHNOLOGY_ID: &str = "88888888-8888-8888-8888-888888888888";
const UNAVAILABLE: &str = "Local database is unavailable";

/// Installs a `BEFORE INSERT` trigger on `profile.profile_contacts` that raises with `code`. The
/// message carries a row value that must never reach the caller.
async fn raise_on_contact_insert(client: &Client, code: &str) -> TestResult {
    client
        .batch_execute(&format!(
            "CREATE OR REPLACE FUNCTION profile.test_raise_on_contact() RETURNS trigger \
             LANGUAGE plpgsql AS $fn$ BEGIN \
             RAISE EXCEPTION 'secret row value %', NEW.contact_value USING ERRCODE = '{code}'; \
             END $fn$; \
             DROP TRIGGER IF EXISTS test_raise_on_contact ON profile.profile_contacts; \
             CREATE TRIGGER test_raise_on_contact BEFORE INSERT ON profile.profile_contacts \
             FOR EACH ROW EXECUTE FUNCTION profile.test_raise_on_contact();"
        ))
        .await?;
    Ok(())
}

#[tokio::test]
async fn driver_failures_on_the_replace_path_map_to_the_expected_envelopes() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
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
    let client = fixture.client().await;
    let child_request = request(
        "failures@example.test",
        Some(CONTACT_ID),
        PROJECT_ID,
        TECHNOLOGY_ID,
    );

    let cases: [(&str, &str); 6] = [
        ("57014", UNAVAILABLE),
        ("55P03", UNAVAILABLE),
        ("22012", UNAVAILABLE),
        ("P0001", UNAVAILABLE),
        ("42601", UNAVAILABLE),
        ("23505", PERSISTENCE_CONSTRAINT_MESSAGE),
    ];
    for (code, expected) in cases {
        raise_on_contact_insert(&client, code).await?;
        let failure = service
            .update_profile(&profile_id, &json!(0), &child_request)
            .await
            .err()
            .ok_or(format!("SQLSTATE {code} must fail the replace"))?;
        let envelope = error_envelope(&failure);
        assert_eq!(envelope.message, expected, "SQLSTATE {code}");
        let rendered = format!("{failure:?} {}", envelope.message);
        assert!(
            !rendered.contains("secret row value"),
            "the server message must not leave the adapter ({code})"
        );
        if expected == UNAVAILABLE {
            assert!(
                !rendered.contains(code),
                "the SQLSTATE must not leave the adapter ({code})"
            );
        }
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
    client
        .batch_execute("DROP TRIGGER IF EXISTS test_raise_on_contact ON profile.profile_contacts")
        .await?;
    Ok(())
}

/// Sets the database-level timeouts. Only sessions opened after this call inherit them, so the
/// caller opens its own pool after it.
async fn set_database_timeouts(admin: &Client, statement: &str, lock: &str) -> TestResult {
    admin
        .batch_execute(&format!(
            "DO $$ BEGIN \
             EXECUTE format('ALTER DATABASE %I SET statement_timeout = %L', \
                            current_database(), '{statement}'); \
             EXECUTE format('ALTER DATABASE %I SET lock_timeout = %L', \
                            current_database(), '{lock}'); \
             END $$;"
        ))
        .await?;
    Ok(())
}

/// A profile whose table is locked by another session. A read and a replace then wait until the
/// configured statement or lock timeout cancels them (SQLSTATE 57014 or 55P03).
async fn assert_blocked_profile_path_is_unavailable(statement: &str, lock: &str) -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let created = details(&fixture.uow)
        .create_profile(&request(
            "timeouts@example.test",
            None,
            PROJECT_ID,
            TECHNOLOGY_ID,
        ))
        .await?;
    let profile_id = created.profile.id.to_string();
    let admin = fixture.client().await;
    set_database_timeouts(&admin, statement, lock).await?;
    let config = load_database_config(&fixture.database.environment())
        .ok_or("the disposable database configuration is invalid")?;
    let fresh = PgUnitOfWork::new(PgPool::new(Some(config)));
    let service = details(&fresh);

    admin
        .batch_execute("BEGIN; LOCK TABLE profile.profiles IN ACCESS EXCLUSIVE MODE;")
        .await?;
    let read = service
        .get_profile(&profile_id)
        .await
        .err()
        .ok_or("a read blocked by a lock must fail")?;
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
        .ok_or("a replace blocked by a lock must fail")?;
    admin.batch_execute("ROLLBACK").await?;

    for failure in [&read, &write] {
        let envelope = error_envelope(failure);
        assert_eq!(envelope.message, UNAVAILABLE, "{statement} / {lock}");
        let rendered = format!("{failure:?}");
        assert!(!rendered.contains("57014") && !rendered.contains("55P03"));
    }
    Ok(())
}

#[tokio::test]
async fn statement_timeouts_on_the_profile_path_are_unavailable() -> TestResult {
    assert_blocked_profile_path_is_unavailable("200ms", "5s").await
}

#[tokio::test]
async fn lock_timeouts_on_the_profile_path_are_unavailable() -> TestResult {
    assert_blocked_profile_path_is_unavailable("0", "200ms").await
}

/// A column whose stored type no longer decodes is corrupted data, not a database outage: the
/// repositories do not classify it as a database error.
#[tokio::test]
async fn a_retyped_column_is_an_internal_error_not_unavailable() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let created = details(&fixture.uow)
        .create_profile(&request(
            "retyped@example.test",
            None,
            PROJECT_ID,
            TECHNOLOGY_ID,
        ))
        .await?;
    let client = fixture.client().await;
    client
        .batch_execute(
            "ALTER TABLE profile.profiles ALTER COLUMN summary TYPE bytea \
             USING convert_to(summary, 'UTF8')",
        )
        .await?;
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
    let config = closed_port_config().ok_or("the loopback test configuration must be valid")?;
    let uow = PgUnitOfWork::new(PgPool::new(Some(config)));

    let list_failure = list(&uow)?
        .list_profiles(Some(2), None)
        .await
        .err()
        .ok_or("listing from a closed port must fail")?;
    let envelope = error_envelope(&list_failure);
    assert_eq!(envelope.message, UNAVAILABLE);
    assert!(!format!("{list_failure:?}").contains("127.0.0.1"));

    let search_failure = search(&uow)
        .search_profiles(&json!({"query": "ada"}))
        .await
        .err()
        .ok_or("searching a closed port must fail")?;
    let envelope = error_envelope(&search_failure);
    assert_eq!(envelope.message, UNAVAILABLE);
    assert!(!format!("{search_failure:?}").contains("127.0.0.1"));
    Ok(())
}
