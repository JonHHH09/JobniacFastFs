//! Shared fixtures of the matching and review PostgreSQL tests.
//!
//! Every server-backed test works on its own disposable, migrated database. Without a configured
//! server the fixture is `None` and the test returns early with a skip notice. The unreachable and
//! unconfigured cases need no server. All data is synthetic: the reserved `.test` domain, fixed
//! identifiers and fixed instants. Profiles and jobs are seeded through the profile and job fixtures
//! of the sibling test modules, so the rows are the ones the application writes.

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

#[path = "../job_support/mod.rs"]
pub mod job_support;
#[path = "../profile_support/mod.rs"]
pub mod profile_support;

use std::error::Error;
use std::sync::Arc;

use jobniac_core::clock::{Clock, FixedClock};
use jobniac_core::env::Environment;
use jobniac_core::jobs::JobRepository;
use jobniac_core::matching::MatchAnalysisService;
use jobniac_core::pagination::{CursorCodec, CursorKey};
use jobniac_core::profiles::ProfileService;
use jobniac_core::reviews::MatchReviewService;
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::jobs::PgJobRepository;
use jobniac_infra::postgres::match_reviews::PgMatchReviewRepository;
use jobniac_infra::postgres::matching::{MATCHING_APPLICATION_NAME, PgMatchingRepository};
use jobniac_infra::postgres::migrations::migrate;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::profiles::PgProfileRepository;
use jobniac_infra::postgres::unit_of_work::{PgTransaction, PgUnitOfWork};
use jobniac_testkit::pg::{TestDatabase, TestServer};
use time::OffsetDateTime;
use tokio_postgres::Client;
use uuid::Uuid;

/// The result type of the matching integration tests.
pub type TestResult = Result<(), Box<dyn Error>>;

/// A cursor secret of 32 or more characters, used only by the listing tests.
pub const CURSOR_SECRET: &str = "match-test-cursor-secret-0123456789abcdef";

/// A migrated disposable database and the matching unit of work over it.
pub struct Fixture {
    /// The disposable database; dropped with the fixture.
    pub database: TestDatabase,
    /// The unit of work whose pool is named `jobniac-matching`.
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
    let uow = PgUnitOfWork::new(PgPool::with_application_name(
        Some(config),
        MATCHING_APPLICATION_NAME,
    ));
    Ok(Some(Fixture { database, uow }))
}

/// The cursor codec of the listings, signed with [`CURSOR_SECRET`].
pub fn codec() -> Result<CursorCodec, Box<dyn Error>> {
    let key = CursorKey::from_secret(CURSOR_SECRET).map_err(|error| error.to_string())?;
    Ok(CursorCodec::new(key))
}

/// The matching use cases over `uow` with `clock` as the instant source.
pub fn analysis(
    uow: &PgUnitOfWork,
    clock: Arc<dyn Clock>,
) -> Result<MatchAnalysisService<PgMatchingRepository>, Box<dyn Error>> {
    Ok(MatchAnalysisService::new(
        PgMatchingRepository::new(uow.clone()),
        codec()?,
        clock,
    ))
}

/// The review use cases over `uow` with `clock` as the instant source.
pub fn reviews(
    uow: &PgUnitOfWork,
    clock: Arc<dyn Clock>,
) -> Result<MatchReviewService<PgMatchReviewRepository>, Box<dyn Error>> {
    Ok(MatchReviewService::new(
        PgMatchReviewRepository::new(uow.clone()),
        codec()?,
        clock,
    ))
}

/// A clock that always reads `instant`.
pub fn fixed_clock(instant: OffsetDateTime) -> Arc<dyn Clock> {
    Arc::new(FixedClock::new(instant))
}

/// The profile use cases over `uow`, with the process clock (as the profile fixtures use).
pub fn profiles(uow: &PgUnitOfWork) -> ProfileService<PgUnitOfWork, PgProfileRepository> {
    profile_support::details(uow)
}

/// Creates a synthetic profile with one skill and its own project and technology rows. Returns its
/// identifier.
pub async fn seed_profile(uow: &PgUnitOfWork, email: &str) -> Result<Uuid, Box<dyn Error>> {
    let project = Uuid::new_v4().to_string();
    let technology = Uuid::new_v4().to_string();
    seed_profile_with(uow, email, &project, &technology).await
}

/// Creates a synthetic profile whose project and technology rows use the given identifiers.
pub async fn seed_profile_with(
    uow: &PgUnitOfWork,
    email: &str,
    project: &str,
    technology: &str,
) -> Result<Uuid, Box<dyn Error>> {
    let created = profiles(uow)
        .create_profile(&profile_support::request(email, None, project, technology))
        .await
        .map_err(|error| format!("{error:?}"))?;
    Ok(created.profile.id)
}

/// Inserts a text-sourced synthetic job created at `created_at` and returns its identifier.
pub async fn seed_job(
    uow: &PgUnitOfWork,
    identifier: u128,
    created_at: OffsetDateTime,
) -> Result<Uuid, Box<dyn Error>> {
    let graph = job_support::text_graph(identifier, created_at);
    let (stored, _created) = uow
        .transaction(
            TransactionOptions::default(),
            async |tx: &mut PgTransaction| PgJobRepository.create_text_job(tx, &graph).await,
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
    Ok(stored.job.id)
}

/// Counts the rows of one `match` table with a fixed statement.
pub async fn count(client: &Client, sql: &str) -> Result<i64, Box<dyn Error>> {
    let row = client.query_one(sql, &[]).await?;
    Ok(row.get(0))
}

/// Environment of a loopback database that nobody listens on.
pub fn closed_port_environment() -> Environment {
    Environment::from_pairs([
        ("JOBNIAC_DB_HOST", "127.0.0.1"),
        ("JOBNIAC_DB_PORT", "1"),
        ("JOBNIAC_DB_NAME", "jobniac_unreachable"),
        ("JOBNIAC_DB_USER", "app_user"),
        ("JOBNIAC_DB_PASSWORD", "synthetic-password"),
    ])
}
