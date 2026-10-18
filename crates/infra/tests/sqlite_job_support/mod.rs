//! Shared fixtures of the job SQLite tests.
//!
//! Every test works on its own migrated database file (see `sqlite_support`), so tests run in
//! parallel without a server. All data is synthetic: the reserved `.test` domain, fixed identifiers
//! and fixed instants.

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

#[path = "../sqlite_support/mod.rs"]
pub mod sqlite_support;

use std::error::Error;
use std::sync::Arc;

use jobniac_core::clock::{Clock, FixedClock};
use jobniac_core::jobs::{
    JobAggregate, JobRecord, JobSearchService, JobService, JobSkillRecord, TextIngestionRecord,
};
use jobniac_core::pagination::{CursorCodec, CursorKey};
use jobniac_infra::sqlite::jobs::SqliteJobRepository;
use jobniac_infra::sqlite::jobs::codec::{instant_text, uuid_text};
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use rusqlite::Connection;
use time::OffsetDateTime;
use uuid::Uuid;

pub use sqlite_support::SqliteFixture;

/// The result type of the job integration tests.
pub type TestResult = Result<(), Box<dyn Error>>;

/// A stored posting: `(field_key, term, weight)`.
pub type Posting = (String, String, i16);

/// A cursor secret of 32 or more characters, used only by the listing tests.
pub const CURSOR_SECRET: &str = "job-test-cursor-secret-0123456789abcdef";

/// A migrated disposable database and the unit of work over it.
pub struct Fixture {
    database: SqliteFixture,
    /// The unit of work whose pool points at the fixture database.
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

/// The cursor codec of the listing, signed with [`CURSOR_SECRET`].
pub fn codec() -> Result<CursorCodec, Box<dyn Error>> {
    let key = CursorKey::from_secret(CURSOR_SECRET).map_err(|error| error.to_string())?;
    Ok(CursorCodec::new(key))
}

/// The job use cases over `uow` with `clock` as the instant source.
pub fn service(
    uow: &SqliteUnitOfWork,
    clock: Arc<dyn Clock>,
) -> Result<JobService<SqliteUnitOfWork, SqliteJobRepository>, Box<dyn Error>> {
    Ok(JobService::new(
        uow.clone(),
        SqliteJobRepository,
        clock,
        codec()?,
    ))
}

/// The job use cases over `uow` with a clock that always reads `instant`.
pub fn fixed_service(
    uow: &SqliteUnitOfWork,
    instant: OffsetDateTime,
) -> Result<JobService<SqliteUnitOfWork, SqliteJobRepository>, Box<dyn Error>> {
    service(uow, Arc::new(FixedClock::new(instant)))
}

/// The bounded job search use case over `uow`.
pub fn search(uow: &SqliteUnitOfWork) -> JobSearchService<SqliteUnitOfWork, SqliteJobRepository> {
    JobSearchService::new(uow.clone(), SqliteJobRepository)
}

/// A unit of work with no database configured: every scope fails before a connection is attempted.
pub fn unconfigured_uow() -> SqliteUnitOfWork {
    SqliteUnitOfWork::new(SqlitePool::unconfigured())
}

/// A unit of work whose pool was closed, so no connection can be taken (the counterpart of a
/// server that nobody listens on).
pub fn closed_uow() -> SqliteUnitOfWork {
    let database = SqliteFixture::start();
    let pool = database.pool();
    pool.close();
    SqliteUnitOfWork::new(pool)
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

/// Inserts a text job row with its text provenance, directly, without skills or postings.
pub fn insert_text_job_row(
    connection: &Connection,
    id: Uuid,
    title: &str,
    created_at: OffsetDateTime,
) -> Result<(), Box<dyn Error>> {
    let fingerprint = format!("{:064x}", id.as_u128());
    let created = instant_text(created_at);
    connection.execute(
        "INSERT INTO job_jobs \
         (id, source_method, title, description, canonical_fingerprint, created_at, updated_at) \
         VALUES (?1, 'text', ?2, 'Other', ?3, ?4, ?4)",
        rusqlite::params![uuid_text(id), title, fingerprint, created],
    )?;
    connection.execute(
        "INSERT INTO job_job_text_ingestions (id, job_id, input_text_hash, created_at) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            uuid_text(Uuid::from_u128(id.as_u128() + 100_000)),
            uuid_text(id),
            fingerprint,
            created
        ],
    )?;
    Ok(())
}

/// Reads `(field_key, term, weight)` postings of one job, ordered by field key and term.
pub fn postings(connection: &Connection, job_id: Uuid) -> Result<Vec<Posting>, Box<dyn Error>> {
    let mut statement = connection.prepare(
        "SELECT field_key, term, weight FROM job_search_terms \
         WHERE job_id = ?1 ORDER BY field_key, term",
    )?;
    let rows = statement
        .query_map([uuid_text(job_id)], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// True when `postings` holds the exact `(field_key, term, weight)` triple.
pub fn has_posting(postings: &[Posting], field_key: &str, term: &str, weight: i16) -> bool {
    postings
        .iter()
        .any(|(key, value, found)| key == field_key && value == term && *found == weight)
}

/// Counts the rows of one table, by a fixed statement.
pub fn count(connection: &Connection, sql: &str) -> Result<i64, Box<dyn Error>> {
    Ok(connection.query_row(sql, [], |row| row.get(0))?)
}
