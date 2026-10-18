//! Shared fixtures of the matching and review SQLite tests.
//!
//! Every test works on its own migrated database file (see `sqlite_support`), so tests run in
//! parallel without a server. All data is synthetic: the reserved `.test` domain, fixed
//! identifiers and fixed instants. Profiles are seeded with raw SQL, because the profile
//! repository is a separate adapter; jobs are seeded through the job repository, so the rows are
//! the ones the application writes.

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

#[path = "../sqlite_job_support/mod.rs"]
pub mod sqlite_job_support;

use std::error::Error;
use std::sync::Arc;

use jobniac_core::clock::{Clock, FixedClock};
use jobniac_core::jobs::JobRepository;
use jobniac_core::matching::MatchAnalysisService;
use jobniac_core::pagination::{CursorCodec, CursorKey};
use jobniac_core::reviews::MatchReviewService;
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::sqlite::codec::{instant_text, uuid_text};
use jobniac_infra::sqlite::jobs::SqliteJobRepository;
use jobniac_infra::sqlite::match_reviews::SqliteMatchReviewRepository;
use jobniac_infra::sqlite::matching::SqliteMatchingRepository;
use jobniac_infra::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};
use rusqlite::Connection;
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

pub use sqlite_job_support::{TestResult, start, unconfigured_uow};

/// A cursor secret of 32 or more characters, used only by the listing tests.
pub const CURSOR_SECRET: &str = "match-test-cursor-secret-0123456789abcdef";

/// The creation and update instant of a seeded profile.
pub const PROFILE_AT: OffsetDateTime = datetime!(2025-01-01 00:00 UTC);

/// The cursor codec of the listings, signed with [`CURSOR_SECRET`].
pub fn codec() -> Result<CursorCodec, Box<dyn Error>> {
    let key = CursorKey::from_secret(CURSOR_SECRET).map_err(|error| error.to_string())?;
    Ok(CursorCodec::new(key))
}

/// The matching use cases over `uow` with `clock` as the instant source.
pub fn analysis(
    uow: &SqliteUnitOfWork,
    clock: Arc<dyn Clock>,
) -> Result<MatchAnalysisService<SqliteMatchingRepository>, Box<dyn Error>> {
    Ok(MatchAnalysisService::new(
        SqliteMatchingRepository::new(uow.clone()),
        codec()?,
        clock,
    ))
}

/// The review use cases over `uow` with `clock` as the instant source.
pub fn reviews(
    uow: &SqliteUnitOfWork,
    clock: Arc<dyn Clock>,
) -> Result<MatchReviewService<SqliteMatchReviewRepository>, Box<dyn Error>> {
    Ok(MatchReviewService::new(
        SqliteMatchReviewRepository::new(uow.clone()),
        codec()?,
        clock,
    ))
}

/// A clock that always reads `instant`.
pub fn fixed_clock(instant: OffsetDateTime) -> Arc<dyn Clock> {
    Arc::new(FixedClock::new(instant))
}

/// Creates a synthetic profile with one skill and one experience row, with raw SQL. Returns its
/// identifier. The profile's revision instant is [`PROFILE_AT`].
pub fn seed_profile(connection: &Connection, email: &str) -> Result<Uuid, Box<dyn Error>> {
    let id = Uuid::new_v4();
    let at = instant_text(PROFILE_AT);
    connection.execute(
        "INSERT INTO profile_profiles (id, full_name, email, created_at, updated_at) \
         VALUES (?1, 'Synthetic Person', ?2, ?3, ?3)",
        rusqlite::params![uuid_text(id), email, at],
    )?;
    connection.execute(
        "INSERT INTO profile_profile_skills \
         (id, profile_id, skill, normalized_skill, display_order, created_at) \
         VALUES (?1, ?2, 'Python', 'python', 0, ?3)",
        rusqlite::params![uuid_text(Uuid::new_v4()), uuid_text(id), at],
    )?;
    connection.execute(
        "INSERT INTO profile_experiences (id, profile_id, company, title, created_at, \
         company_key, title_key) \
         VALUES (?1, ?2, 'Example Corp', 'Engineer', ?3, 'example corp', 'engineer')",
        rusqlite::params![uuid_text(Uuid::new_v4()), uuid_text(id), at],
    )?;
    Ok(id)
}

/// Inserts a text-sourced synthetic job created at `created_at` and returns its identifier.
pub async fn seed_job(
    uow: &SqliteUnitOfWork,
    identifier: u128,
    created_at: OffsetDateTime,
) -> Result<Uuid, Box<dyn Error>> {
    let graph = sqlite_job_support::text_graph(identifier, created_at);
    let (stored, _created) = uow
        .transaction(
            TransactionOptions::default(),
            async |tx: &mut SqliteTransaction| {
                SqliteJobRepository.create_text_job(tx, &graph).await
            },
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
    Ok(stored.job.id)
}

/// Counts the rows of one table with a fixed statement.
pub fn count(connection: &Connection, sql: &str) -> Result<i64, Box<dyn Error>> {
    Ok(connection.query_row(sql, [], |row| row.get(0))?)
}
