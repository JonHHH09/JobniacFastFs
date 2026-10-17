//! Personal details against a disposable PostgreSQL 18 server.
//!
//! The server-backed
//! tests need `JOBNIAC_TEST_PG_PORT` and skip with a notice when it is unset. All data is synthetic.

mod profile_support;

use jobniac_core::errors::error_envelope;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use profile_support::{TestResult, details, personal, request, start};
use serde_json::json;
use time::OffsetDateTime;
use time::macros::{date, datetime};
use uuid::Uuid;

const PROJECT_ID: &str = "77777777-7777-7777-7777-777777777777";
const TECHNOLOGY_ID: &str = "88888888-8888-8888-8888-888888888888";

/// Creates a profile at revision 0 and returns its identifier.
async fn create_profile(
    uow: &PgUnitOfWork,
    email: &str,
) -> Result<Uuid, Box<dyn std::error::Error>> {
    let graph = details(uow)
        .create_profile(&request(email, None, PROJECT_ID, TECHNOLOGY_ID))
        .await?;
    Ok(graph.profile.id)
}

#[tokio::test]
async fn an_unconfigured_personal_read_fails_with_the_unavailable_message() -> TestResult {
    let uow = PgUnitOfWork::new(PgPool::new(None));
    let error = personal(&uow)
        .get_personal_details(&Uuid::nil().to_string())
        .await
        .err()
        .ok_or("a read without a database must fail")?;
    assert_eq!(
        error_envelope(&error).message,
        "Local database is not configured"
    );
    Ok(())
}

#[tokio::test]
async fn the_first_insert_reads_back_and_a_long_nationality_round_trips() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile_id = create_profile(&fixture.uow, "personal@example.test").await?;
    let service = personal(&fixture.uow);
    let empty = service
        .get_personal_details(&profile_id.to_string())
        .await?;
    assert_eq!(empty.profile_revision, 0);
    assert!(empty.personal_details.is_none());

    let long_nationality = format!("  {}  ", "Nordic-Alpine ".repeat(12));
    let stripped = long_nationality.trim().to_owned();
    let written = service
        .update_personal_details(
            &profile_id.to_string(),
            &json!(0),
            &json!({"dateOfBirth": "1906-12-09", "nationality": long_nationality}),
        )
        .await?;
    assert_eq!(written.profile_revision, 1);
    let details = written
        .personal_details
        .as_ref()
        .ok_or("the written details must be present")?;
    assert_eq!(details.date_of_birth, Some(date!(1906 - 12 - 09)));
    assert_eq!(details.nationality.as_deref(), Some(stripped.as_str()));
    assert!(details.photo_document_id.is_none());

    let read_back = service
        .get_personal_details(&profile_id.to_string())
        .await?;
    assert_eq!(read_back, written);
    Ok(())
}

#[tokio::test]
async fn a_stored_nationality_is_normalized_on_read_without_rewriting_the_row() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile_id = create_profile(&fixture.uow, "stored@example.test").await?;
    let client = fixture.client().await;
    let stamp: OffsetDateTime = datetime!(2026-10-02 08:00 UTC);
    let raw = "  Swiss  ";
    client
        .execute(
            "INSERT INTO profile.profile_personal_details \
             (profile_id, date_of_birth, nationality, photo_document_id, created_at, updated_at) \
             VALUES ($1, NULL, $2, NULL, $3, $3)",
            &[&profile_id, &raw, &stamp],
        )
        .await?;

    let read = personal(&fixture.uow)
        .get_personal_details(&profile_id.to_string())
        .await?;
    let details = read
        .personal_details
        .ok_or("the stored details must be present")?;
    assert_eq!(details.nationality.as_deref(), Some("Swiss"));
    let stored: String = client
        .query_one(
            "SELECT nationality FROM profile.profile_personal_details WHERE profile_id = $1",
            &[&profile_id],
        )
        .await?
        .get(0);
    assert_eq!(stored, raw, "reading must not repair the stored value");
    Ok(())
}

#[tokio::test]
async fn a_stale_revision_is_refused_and_writes_nothing() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile_id = create_profile(&fixture.uow, "stale-personal@example.test").await?;
    let service = personal(&fixture.uow);
    service
        .update_personal_details(
            &profile_id.to_string(),
            &json!(0),
            &json!({"dateOfBirth": null, "nationality": "Dutch"}),
        )
        .await?;

    let failure = service
        .update_personal_details(
            &profile_id.to_string(),
            &json!(0),
            &json!({"dateOfBirth": null, "nationality": "Changed"}),
        )
        .await
        .err()
        .ok_or("a stale revision must conflict")?;
    assert_eq!(
        error_envelope(&failure).message,
        "Profile revision conflict"
    );
    let current = service
        .get_personal_details(&profile_id.to_string())
        .await?;
    assert_eq!(current.profile_revision, 1);
    let details = current
        .personal_details
        .ok_or("the first write must remain")?;
    assert_eq!(details.nationality.as_deref(), Some("Dutch"));
    Ok(())
}

#[tokio::test]
async fn concurrent_writers_at_one_revision_have_exactly_one_winner() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile_id = create_profile(&fixture.uow, "same-revision@example.test").await?;
    let service = personal(&fixture.uow);
    let id = profile_id.to_string();
    let revision = json!(0);
    let first_request = json!({"dateOfBirth": null, "nationality": "First"});
    let second_request = json!({"dateOfBirth": null, "nationality": "Second"});
    let (first, second) = tokio::join!(
        service.update_personal_details(&id, &revision, &first_request),
        service.update_personal_details(&id, &revision, &second_request),
    );
    let winners = [&first, &second]
        .iter()
        .filter(|outcome| outcome.is_ok())
        .count();
    assert_eq!(winners, 1, "exactly one writer must win");
    let current = service.get_personal_details(&id).await?;
    assert_eq!(current.profile_revision, 1);
    Ok(())
}

#[tokio::test]
async fn the_maximum_revision_cannot_overflow_or_create_details() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let profile_id = create_profile(&fixture.uow, "maximum@example.test").await?;
    let client = fixture.client().await;
    let maximum = i64::MAX;
    client
        .execute(
            "UPDATE profile.profiles SET revision = $1 WHERE id = $2",
            &[&maximum, &profile_id],
        )
        .await?;

    let failure = personal(&fixture.uow)
        .update_personal_details(
            &profile_id.to_string(),
            &json!(maximum),
            &json!({"dateOfBirth": null, "nationality": "Overflow"}),
        )
        .await
        .err()
        .ok_or("the maximum revision must conflict")?;
    assert_eq!(
        error_envelope(&failure).message,
        "Profile revision conflict"
    );
    let count: i64 = client
        .query_one(
            "SELECT count(*) FROM profile.profile_personal_details WHERE profile_id = $1",
            &[&profile_id],
        )
        .await?
        .get(0);
    assert_eq!(
        count, 0,
        "no details row may be created at the maximum revision"
    );
    Ok(())
}
