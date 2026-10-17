//! Shared fixtures of the profile PostgreSQL tests.
//!
//! Every test works on its own disposable, migrated database. Without a configured server the
//! fixture is `None` and the test returns early with a skip notice. All data is synthetic: the
//! reserved `.test` domain, fixed identifiers and fixed instants.

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

use std::error::Error;
use std::sync::Arc;

use jobniac_core::clock::SystemClock;
use jobniac_core::env::Environment;
use jobniac_core::pagination::{CursorCodec, CursorKey};
use jobniac_core::profiles::{
    PersonalDetailsService, ProfileListService, ProfileSearchService, ProfileService,
};
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::migrations::migrate;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::profiles::PgProfileRepository;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use jobniac_testkit::pg::{TestDatabase, TestServer};
use serde_json::{Value, json};
use tokio_postgres::Client;

/// The result type of the profile integration tests.
pub type TestResult = Result<(), Box<dyn Error>>;

/// A cursor secret of 32 or more characters, used only by the list tests.
pub const CURSOR_SECRET: &str = "profile-test-cursor-secret-0123456789";

/// A migrated disposable database and the unit of work over it.
pub struct Fixture {
    /// The disposable database; dropped with the fixture.
    pub database: TestDatabase,
    /// The unit of work whose pool points at [`Fixture::database`].
    pub uow: PgUnitOfWork,
}

impl Fixture {
    /// A superuser connection to the fixture database, for seeding and inspection.
    pub async fn client(&self) -> Client {
        self.database.admin_client().await
    }
}

/// A fresh database migrated to the packaged schema, or `None` when no server is configured.
pub async fn start() -> Result<Option<Fixture>, Box<dyn Error>> {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(None);
    };
    let database = server.fresh_database().await;
    let config = load_database_config(&database.environment())
        .ok_or("the disposable database configuration is invalid")?;
    migrate(&config).await.map_err(|error| error.0)?;
    let uow = PgUnitOfWork::new(PgPool::new(Some(config)));
    Ok(Some(Fixture { database, uow }))
}

/// The profile aggregate use cases over `uow`, with the process clock.
pub fn details(uow: &PgUnitOfWork) -> ProfileService<PgUnitOfWork, PgProfileRepository> {
    ProfileService::new(uow.clone(), PgProfileRepository, Arc::new(SystemClock))
}

/// The personal-details use cases over `uow`, with the process clock.
pub fn personal(uow: &PgUnitOfWork) -> PersonalDetailsService<PgUnitOfWork, PgProfileRepository> {
    PersonalDetailsService::new(uow.clone(), PgProfileRepository, Arc::new(SystemClock))
}

/// The bounded search use case over `uow`.
pub fn search(uow: &PgUnitOfWork) -> ProfileSearchService<PgUnitOfWork, PgProfileRepository> {
    ProfileSearchService::new(uow.clone(), PgProfileRepository)
}

/// The keyset listing use case over `uow`, signed with [`CURSOR_SECRET`].
pub fn list(
    uow: &PgUnitOfWork,
) -> Result<ProfileListService<PgUnitOfWork, PgProfileRepository>, Box<dyn Error>> {
    let key = CursorKey::from_secret(CURSOR_SECRET).map_err(|error| error.to_string())?;
    Ok(ProfileListService::new(
        uow.clone(),
        PgProfileRepository,
        CursorCodec::new(key),
    ))
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
pub async fn postings(
    client: &Client,
    profile_id: uuid::Uuid,
) -> Result<Vec<(String, String, i16)>, Box<dyn Error>> {
    let rows = client
        .query(
            "SELECT field_key, term, weight FROM profile.search_terms \
             WHERE profile_id = $1 ORDER BY field_key, term",
            &[&profile_id],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| (row.get(0), row.get(1), row.get(2)))
        .collect())
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

/// The environment of a loopback database on a closed port, for the unavailable-database case.
pub fn closed_port_config() -> Option<jobniac_infra::postgres::config::DatabaseConfig> {
    load_database_config(&Environment::from_pairs([
        ("JOBNIAC_DB_HOST", "127.0.0.1"),
        ("JOBNIAC_DB_PORT", "1"),
        ("JOBNIAC_DB_NAME", "jobniac_unreachable"),
        ("JOBNIAC_DB_USER", "app_user"),
        ("JOBNIAC_DB_PASSWORD", "synthetic-password"),
    ]))
}
