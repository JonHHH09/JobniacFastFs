//! Case-insensitive uniqueness of education, experiences and projects is Unicode-aware: SQLite
//! `lower()` folds ASCII only, so the unique indexes sit on `*_key` columns that Rust fills with
//! the full Unicode lowercase of the trimmed value. All data is synthetic.

mod sqlite_profile_support;

use serde_json::{Value, json};
use sqlite_profile_support::{TestResult, details, request, start};

const PROJECT_ID: &str = "77777777-7777-7777-7777-777777777777";
const TECHNOLOGY_ID: &str = "88888888-8888-8888-8888-888888888888";

fn base() -> Value {
    request("grace@example.test", None, PROJECT_ID, TECHNOLOGY_ID)
}

#[tokio::test]
async fn education_that_differs_only_by_non_ascii_case_is_a_duplicate() -> TestResult {
    let fixture = start();
    let service = details(&fixture.uow);
    let mut body = base();
    let entry = |institution: &str| {
        json!({
            "institution": institution,
            "degree": " Dipl\u{f4}me ",
            "field": " \u{c9}conomie ",
            "startDate": "1930-01-01",
        })
    };
    body["education"] = json!([entry("Universit\u{e4}t Z\u{fc}rich")]);
    body["education"]
        .as_array_mut()
        .ok_or("education is an array")?
        .push(entry("UNIVERSIT\u{c4}T Z\u{dc}RICH"));
    assert!(service.create_profile(&body).await.is_err());

    // Distinct institutions are still accepted, and the keys hold the Unicode lowercase.
    body["education"][1] = entry("Universit\u{e4}t Bern");
    service.create_profile(&body).await?;
    let keys: (String, String, String) = fixture.connection().query_row(
        "SELECT institution_key, degree_key, field_key FROM profile_education \
         WHERE institution = ?1",
        ["Universit\u{e4}t Z\u{fc}rich"],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(
        keys,
        (
            "universit\u{e4}t z\u{fc}rich".to_owned(),
            "dipl\u{f4}me".to_owned(),
            "\u{e9}conomie".to_owned()
        )
    );
    Ok(())
}

#[tokio::test]
async fn experiences_that_differ_only_by_non_ascii_case_are_a_duplicate() -> TestResult {
    let fixture = start();
    let service = details(&fixture.uow);
    let mut body = base();
    let entry = |company: &str, title: &str| {
        json!({
            "company": company,
            "title": title,
            "startDate": "2001-01-01",
            "displayOrder": 0,
        })
    };
    body["experiences"] = json!([
        entry("Z\u{fc}rich AG", "\u{c4}ngstlicher Leiter"),
        entry("Z\u{dc}RICH AG", "\u{e4}ngstlicher leiter"),
    ]);
    assert!(service.create_profile(&body).await.is_err());

    body["experiences"][1] = entry("Z\u{dc}RICH AG", "Other");
    service.create_profile(&body).await?;
    let keys: (String, String) = fixture.connection().query_row(
        "SELECT company_key, title_key FROM profile_experiences WHERE company = 'Z\u{fc}rich AG'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(
        keys,
        (
            "z\u{fc}rich ag".to_owned(),
            "\u{e4}ngstlicher leiter".to_owned()
        )
    );
    Ok(())
}

#[tokio::test]
async fn projects_that_differ_only_by_non_ascii_case_are_a_duplicate() -> TestResult {
    let fixture = start();
    let service = details(&fixture.uow);
    let mut body = base();
    let entry = |id: &str, name: &str, technology: &str| {
        json!({
            "id": id,
            "name": name,
            "url": "https://example.test/p",
            "displayOrder": 0,
            "technologies": [{ "id": technology, "technology": "Rust", "displayOrder": 0 }],
        })
    };
    body["projects"] = json!([
        entry(PROJECT_ID, "\u{d6}ko-Rechner", TECHNOLOGY_ID),
        entry(
            "99999999-9999-9999-9999-999999999999",
            "\u{f6}ko-rechner",
            "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        ),
    ]);
    assert!(service.create_profile(&body).await.is_err());

    body["projects"][1]["name"] = json!("Other");
    service.create_profile(&body).await?;
    let key: String = fixture.connection().query_row(
        "SELECT name_key FROM profile_projects WHERE name = '\u{d6}ko-Rechner'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(key, "\u{f6}ko-rechner");
    Ok(())
}
