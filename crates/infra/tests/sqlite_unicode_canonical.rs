//! Rule 5 of the PostgreSQL port: the canonical lowercase forms (emails, link and contact types,
//! normalized skills, languages and technologies) are Unicode-aware, while SQLite `lower()` only
//! folds ASCII. The Rust writers normalize before every write, so what is stored is already the
//! Unicode lowercase form, and a duplicate that differs only by non-ASCII case is refused by Rust
//! (or by the ASCII-only index once Rust has normalized both values to the same text). All data is
//! synthetic.

mod sqlite_profile_support;

use serde_json::json;
use sqlite_profile_support::{TestResult, details, request, start};

const CONTACT_ID: &str = "11111111-1111-1111-1111-111111111111";
const PROJECT_ID: &str = "77777777-7777-7777-7777-777777777777";
const TECHNOLOGY_ID: &str = "88888888-8888-8888-8888-888888888888";

#[tokio::test]
async fn non_ascii_canonical_values_are_stored_in_unicode_lowercase() -> TestResult {
    let fixture = start();
    let service = details(&fixture.uow);
    let mut body = request(
        "\u{c4}RZTIN@Example.TEST",
        Some(CONTACT_ID),
        PROJECT_ID,
        TECHNOLOGY_ID,
    );
    body["skills"][0]["skill"] = json!(" \u{c9}COLE ");
    body["languages"][0]["language"] = json!(" \u{c5}NGSTR\u{d6}M ");
    body["projects"][0]["technologies"][0]["technology"] = json!(" \u{dc}BER ");
    let created = service.create_profile(&body).await?;
    assert_eq!(created.profile.email, "\u{e4}rztin@example.test");
    assert_eq!(created.skills[0].normalized_skill, "\u{e9}cole");
    assert_eq!(
        created.languages[0].normalized_language,
        "\u{e5}ngstr\u{f6}m"
    );
    assert_eq!(
        created.projects[0].technologies[0].normalized_technology,
        "\u{fc}ber"
    );
    let connection = fixture.connection();
    let stored: (String, String, String) = connection.query_row(
        "SELECT (SELECT email FROM profile_profiles), \
                (SELECT normalized_skill FROM profile_profile_skills), \
                (SELECT normalized_language FROM profile_profile_languages)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(stored.0, "\u{e4}rztin@example.test");
    assert_eq!(stored.1, "\u{e9}cole");
    assert_eq!(stored.2, "\u{e5}ngstr\u{f6}m");
    Ok(())
}

#[tokio::test]
async fn emails_that_differ_only_by_non_ascii_case_are_duplicates() -> TestResult {
    let fixture = start();
    let service = details(&fixture.uow);
    service
        .create_profile(&request(
            "\u{c4}RZTIN@example.test",
            None,
            PROJECT_ID,
            TECHNOLOGY_ID,
        ))
        .await?;
    // The second profile spells the same address in Unicode lowercase.
    let second = service
        .create_profile(&request(
            "\u{e4}rztin@example.test",
            None,
            "99999999-9999-9999-9999-999999999999",
            "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
        ))
        .await;
    assert!(second.is_err(), "the second spelling must be a duplicate");
    Ok(())
}
