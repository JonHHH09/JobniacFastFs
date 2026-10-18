//! Keyset listing of profile identities against a disposable SQLite database.
//!
//! The unconfigured case needs no file. All data is synthetic.

mod sqlite_profile_support;

use jobniac_core::errors::error_envelope;
use jobniac_core::pagination::{CursorCodec, CursorKey};
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use rusqlite::params;
use sqlite_profile_support::{CURSOR_SECRET, TestResult, instant, list, start};
use time::{Duration, OffsetDateTime, macros::datetime};
use uuid::Uuid;

const CREATED_AT: OffsetDateTime = datetime!(2026-07-13 12:00 UTC);

#[tokio::test]
async fn list_pages_real_profiles_in_created_time_and_identifier_order() -> TestResult {
    let fixture = start();
    let client = fixture.connection();
    let ids: Vec<Uuid> = (1..=4).map(Uuid::from_u128).collect();
    for (index, id) in ids.iter().enumerate() {
        let number = index + 1;
        let summary: Option<String> = (number != 1).then(|| format!("Summary {number}"));
        let full_name = format!("Profile {number}");
        let email = format!("profile{number}@example.test");
        let revision = i64::try_from(number)?;
        client.execute(
            "INSERT INTO profile_profiles \
             (id, full_name, email, summary, created_at, updated_at, revision) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
            params![
                id.to_string(),
                full_name,
                email,
                summary,
                instant(CREATED_AT),
                revision
            ],
        )?;
    }

    let service = list(&fixture.uow)?;
    let first = service.list_profiles(Some(2), None).await?;
    let first_ids: Vec<Uuid> = first.profiles.iter().map(|profile| profile.id).collect();
    assert_eq!(first_ids, [ids[0], ids[1]]);
    let head = first
        .profiles
        .first()
        .ok_or("the first page must not be empty")?;
    assert_eq!(head.full_name, "Profile 1");
    assert_eq!(head.email, "profile1@example.test");
    assert_eq!(head.summary, None);
    assert_eq!(head.created_at, CREATED_AT);
    assert_eq!(head.updated_at, CREATED_AT);
    assert_eq!(head.revision, 1);
    assert!(head.raw_resume_text.is_none());
    assert!(head.embedding.is_none());

    let next_cursor = first
        .next_cursor
        .clone()
        .ok_or("the first page must carry a next cursor")?;
    let codec = CursorCodec::new(CursorKey::from_secret(CURSOR_SECRET)?);
    let watermark = codec
        .decode(Some(next_cursor.as_str()))?
        .ok_or("the next cursor must decode")?;

    // Rows created after the first page stay out of later pages, as the watermark rule requires.
    let later = watermark.snapshot_at + Duration::microseconds(1);
    let added = Uuid::from_u128(5);
    let added_name = "Added after first page";
    let added_email = "new@example.test";
    let zero: i64 = 0;
    client.execute(
        "INSERT INTO profile_profiles \
         (id, full_name, email, created_at, updated_at, revision) \
         VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
        params![
            added.to_string(),
            added_name,
            added_email,
            instant(later),
            zero
        ],
    )?;
    client.execute(
        "DELETE FROM profile_profiles WHERE id = ?1",
        params![ids[1].to_string()],
    )?;
    let updated_name = "Updated after first page";
    client.execute(
        "UPDATE profile_profiles SET full_name = ?1 WHERE id = ?2",
        params![updated_name, ids[2].to_string()],
    )?;

    let second = service
        .list_profiles(Some(2), Some(next_cursor.as_str()))
        .await?;
    let second_ids: Vec<Uuid> = second.profiles.iter().map(|profile| profile.id).collect();
    assert_eq!(second_ids, [ids[2], ids[3]]);
    let rest = second
        .profiles
        .first()
        .ok_or("the second page must not be empty")?;
    assert_eq!(rest.full_name, updated_name);
    assert!(second.next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn an_unconfigured_listing_fails_after_the_cursor_check_with_the_unavailable_message()
-> TestResult {
    let uow = SqliteUnitOfWork::new(SqlitePool::unconfigured());
    let service = list(&uow)?;
    let error = service
        .list_profiles(Some(2), None)
        .await
        .err()
        .ok_or("a listing without a database must fail")?;
    assert_eq!(
        error_envelope(&error).message,
        "Local database is not configured"
    );
    Ok(())
}

#[tokio::test]
async fn a_cursor_from_another_scope_is_refused_as_invalid() -> TestResult {
    let uow = SqliteUnitOfWork::new(SqlitePool::unconfigured());
    let key = CursorKey::from_secret(CURSOR_SECRET)?;
    let codec = CursorCodec::new(key);
    let foreign = codec.next_cursor(
        "jobs",
        &"0".repeat(64),
        CREATED_AT,
        CREATED_AT,
        Uuid::from_u128(1),
    )?;
    let service = list(&uow)?;
    let error = service
        .list_profiles(Some(2), Some(foreign.as_str()))
        .await
        .err()
        .ok_or("a foreign cursor must be refused")?;
    assert_eq!(error_envelope(&error).message, "Invalid pagination cursor");
    Ok(())
}
