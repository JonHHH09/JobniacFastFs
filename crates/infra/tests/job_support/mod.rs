//! Shared fixtures of the job PostgreSQL tests.
//!
//! Every server-backed test works on its own disposable, migrated database. Without a configured
//! server the fixture is `None` and the test returns early with a skip notice. The unreachable and
//! unconfigured cases need no server. All data is synthetic: the reserved `.test` domain, fixed
//! identifiers and fixed instants.

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

use std::error::Error;
use std::sync::Arc;

use jobniac_core::clock::{Clock, FixedClock};
use jobniac_core::env::Environment;
use jobniac_core::jobs::{
    JobAggregate, JobRecord, JobSearchService, JobService, JobSkillRecord, TextIngestionRecord,
};
use jobniac_core::pagination::{CursorCodec, CursorKey};
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::jobs::PgJobRepository;
use jobniac_infra::postgres::migrations::migrate;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use jobniac_testkit::pg::{TestDatabase, TestServer};
use time::OffsetDateTime;
use tokio_postgres::Client;
use uuid::Uuid;

/// The result type of the job integration tests.
pub type TestResult = Result<(), Box<dyn Error>>;

/// A cursor secret of 32 or more characters, used only by the listing tests.
pub const CURSOR_SECRET: &str = "job-test-cursor-secret-0123456789abcdef";

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

/// The cursor codec of the listing, signed with [`CURSOR_SECRET`].
pub fn codec() -> Result<CursorCodec, Box<dyn Error>> {
    let key = CursorKey::from_secret(CURSOR_SECRET).map_err(|error| error.to_string())?;
    Ok(CursorCodec::new(key))
}

/// The job use cases over `uow` with `clock` as the instant source.
pub fn service(
    uow: &PgUnitOfWork,
    clock: Arc<dyn Clock>,
) -> Result<JobService<PgUnitOfWork, PgJobRepository>, Box<dyn Error>> {
    Ok(JobService::new(
        uow.clone(),
        PgJobRepository,
        clock,
        codec()?,
    ))
}

/// The job use cases over `uow` with a clock that always reads `instant`.
pub fn fixed_service(
    uow: &PgUnitOfWork,
    instant: OffsetDateTime,
) -> Result<JobService<PgUnitOfWork, PgJobRepository>, Box<dyn Error>> {
    service(uow, Arc::new(FixedClock::new(instant)))
}

/// The bounded job search use case over `uow`.
pub fn search(uow: &PgUnitOfWork) -> JobSearchService<PgUnitOfWork, PgJobRepository> {
    JobSearchService::new(uow.clone(), PgJobRepository)
}

/// A unit of work with no database configured: every scope fails before a connection is attempted.
pub fn unconfigured_uow() -> PgUnitOfWork {
    PgUnitOfWork::new(PgPool::new(None))
}

/// A unit of work whose database is a loopback port nobody listens on.
pub fn closed_port_uow() -> Result<PgUnitOfWork, Box<dyn Error>> {
    let config = load_database_config(&Environment::from_pairs([
        ("JOBNIAC_DB_HOST", "127.0.0.1"),
        ("JOBNIAC_DB_PORT", "1"),
        ("JOBNIAC_DB_NAME", "jobniac_unreachable"),
        ("JOBNIAC_DB_USER", "app_user"),
        ("JOBNIAC_DB_PASSWORD", "synthetic-password"),
    ]))
    .ok_or("the closed-port configuration is invalid")?;
    Ok(PgUnitOfWork::new(PgPool::new(Some(config))))
}

/// A text-sourced aggregate: one `Python` skill
/// (identifier + 1000), the text provenance (identifier + 2000), the input hash and the canonical
/// fingerprint both derived from `identifier`, and `created_at` for both timestamps.
pub fn text_graph(identifier: u128, created_at: OffsetDateTime) -> JobAggregate {
    let job_id = Uuid::from_u128(identifier);
    JobAggregate {
        job: JobRecord {
            id: job_id,
            source_method: "text".to_owned(),
            source_label: Some("manual paste".to_owned()),
            title: "Platform Engineer".to_owned(),
            company: Some("Example Corp".to_owned()),
            location: Some("Remote".to_owned()),
            description: "Build platform services with Python".to_owned(),
            experience_requirement: Some("5+ years".to_owned()),
            employment_type: Some("Full-time".to_owned()),
            seniority: Some("Senior".to_owned()),
            posted_at: None,
            canonical_fingerprint: format!("{identifier:064x}"),
            created_at,
            updated_at: created_at,
            revision: 0,
        },
        skills: vec![JobSkillRecord {
            id: Uuid::from_u128(identifier + 1000),
            job_id,
            skill: "Python".to_owned(),
            normalized_skill: "python".to_owned(),
            required: true,
            display_order: 0,
            created_at,
        }],
        link_ingestion: None,
        text_ingestion: Some(TextIngestionRecord {
            id: Uuid::from_u128(identifier + 2000),
            job_id,
            source_label: Some("manual paste".to_owned()),
            input_text_hash: format!("{identifier:064x}"),
            created_at,
        }),
    }
}

/// Reads `(field_key, term, weight)` postings of one job, ordered by field key and term.
pub async fn postings(
    client: &Client,
    job_id: Uuid,
) -> Result<Vec<(String, String, i16)>, Box<dyn Error>> {
    let rows = client
        .query(
            "SELECT field_key, term, weight FROM job_schema.search_terms \
             WHERE job_id = $1 ORDER BY field_key, term",
            &[&job_id],
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

/// Counts the rows of one table of the `job_schema`, by a fixed statement.
pub async fn count(client: &Client, sql: &str) -> Result<i64, Box<dyn Error>> {
    let row = client.query_one(sql, &[]).await?;
    Ok(row.get(0))
}
