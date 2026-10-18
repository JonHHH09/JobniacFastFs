//! Shared fixtures of the profile SQLite tests.
//!
//! Every test works on its own migrated database file in a temporary directory, so no server or
//! feature flag is needed. All data is synthetic: the reserved `.test` domain, fixed identifiers
//! and fixed instants.

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

#[path = "../sqlite_support/mod.rs"]
mod sqlite_support;

use std::error::Error;
use std::sync::Arc;

use jobniac_core::clock::SystemClock;
use jobniac_core::pagination::{CursorCodec, CursorKey};
use jobniac_core::profiles::{
    PersonalDetailsService, ProfileListService, ProfileSearchService, ProfileService,
};
use jobniac_infra::sqlite::profiles::SqliteProfileRepository;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use rusqlite::Connection;
use serde_json::{Value, json};
use time::OffsetDateTime;

pub use sqlite_support::SqliteFixture;

/// The result type of the profile integration tests.
pub type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

/// A cursor secret of 32 or more characters, used only by the list tests.
pub const CURSOR_SECRET: &str = "profile-test-cursor-secret-0123456789";

/// A migrated database file and the unit of work over it.
pub struct Fixture {
    /// The database file; removed with the fixture.
    pub database: SqliteFixture,
    /// The unit of work whose pool points at [`Fixture::database`].
    pub uow: SqliteUnitOfWork,
}

impl Fixture {
    /// A raw connection to the fixture database, for seeding and inspection.
    pub fn connection(&self) -> Connection {
        self.database.connection()
    }
}

/// A fresh database migrated to the packaged schema.
pub fn start() -> Fixture {
    let database = SqliteFixture::start();
    let uow = database.unit_of_work();
    Fixture { database, uow }
}

/// The profile aggregate use cases over `uow`, with the process clock.
pub fn details(
    uow: &SqliteUnitOfWork,
) -> ProfileService<SqliteUnitOfWork, SqliteProfileRepository> {
    ProfileService::new(uow.clone(), SqliteProfileRepository, Arc::new(SystemClock))
}

/// The personal-details use cases over `uow`, with the process clock.
pub fn personal(
    uow: &SqliteUnitOfWork,
) -> PersonalDetailsService<SqliteUnitOfWork, SqliteProfileRepository> {
    PersonalDetailsService::new(uow.clone(), SqliteProfileRepository, Arc::new(SystemClock))
}

/// The bounded search use case over `uow`.
pub fn search(
    uow: &SqliteUnitOfWork,
) -> ProfileSearchService<SqliteUnitOfWork, SqliteProfileRepository> {
    ProfileSearchService::new(uow.clone(), SqliteProfileRepository)
}

/// The keyset listing use case over `uow`, signed with [`CURSOR_SECRET`].
pub fn list(
    uow: &SqliteUnitOfWork,
) -> Result<
    ProfileListService<SqliteUnitOfWork, SqliteProfileRepository>,
    Box<dyn Error + Send + Sync>,
> {
    let key = CursorKey::from_secret(CURSOR_SECRET).map_err(|error| error.to_string())?;
    Ok(ProfileListService::new(
        uow.clone(),
        SqliteProfileRepository,
        CursorCodec::new(key),
    ))
}

/// The column form of an instant: fixed-width RFC 3339 UTC with six fraction digits.
pub fn instant(value: OffsetDateTime) -> String {
    let utc = value.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
        utc.year(),
        u8::from(utc.month()),
        utc.day(),
        utc.hour(),
        utc.minute(),
        utc.second(),
        utc.nanosecond() / 1_000
    )
}

/// A complete profile write request with one child of every kind.
pub fn request(
    email: &str,
    contact_id: Option<&str>,
    project_id: &str,
    technology_id: &str,
) -> Value {
    let contacts: Vec<Value> = match contact_id {
        None => Vec::new(),
        Some(id) => vec![json!({
            "id": id,
            "contactType": " Phone ",
            "contactValue": " 555-0101 ",
            "label": " work ",
        })],
    };
    json!({
        "fullName": " Grace Hopper ",
        "email": email,
        "summary": " Compiler pioneer ",
        "contacts": contacts,
        "links": [{
            "linkType": " Portfolio ",
            "url": "https://example.test/grace/?q=1",
            "label": " Website ",
        }],
        "skills": [{
            "skill": " COBOL ",
            "normalizedSkill": null,
            "category": " Legacy ",
            "displayOrder": 1,
        }],
        "languages": [{
            "language": " English ",
            "normalizedLanguage": null,
            "proficiency": " Fluent ",
        }],
        "education": [{
            "institution": " Yale ",
            "degree": " PhD ",
            "field": " Mathematics ",
            "location": " New Haven ",
            "startDate": "1930-01-01",
            "endDate": "1934-01-01",
            "relevantFocus": " Computing ",
        }],
        "experiences": [{
            "company": " Navy ",
            "title": " Engineer ",
            "location": " Arlington ",
            "startDate": "1943-01-01",
            "description": " Compiler development ",
            "displayOrder": 2,
        }],
        "projects": [{
            "id": project_id,
            "name": " Compiler ",
            "url": "https://example.test/compiler",
            "description": " A project ",
            "displayOrder": 3,
            "technologies": [{
                "id": technology_id,
                "technology": " COBOL ",
                "normalizedTechnology": null,
                "displayOrder": 4,
            }],
        }],
    })
}

/// Reads `(field_key, term, weight)` postings of one profile, ordered by field key and term.
pub fn postings(
    connection: &Connection,
    profile_id: uuid::Uuid,
) -> Result<Vec<(String, String, i16)>, Box<dyn Error + Send + Sync>> {
    let mut statement = connection.prepare(
        "SELECT field_key, term, weight FROM profile_search_terms \
         WHERE profile_id = ?1 ORDER BY field_key, term",
    )?;
    let rows = statement.query_map([profile_id.to_string()], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// True when `postings` holds the exact `(field_key, term, weight)` triple.
pub fn has_posting(
    postings: &[(String, String, i16)],
    field_key: &str,
    term: &str,
    weight: i16,
) -> bool {
    postings
        .iter()
        .any(|(key, value, found)| key == field_key && value == term && *found == weight)
}
