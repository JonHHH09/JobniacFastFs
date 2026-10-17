//! Trusted Arbeitnow candidate import against a disposable PostgreSQL 18 server.
//!
//! Covered: link provenance round trip, reuse by URL
//! even when the content changed, reuse by fingerprint under another URL, concurrent imports of one
//! candidate, independent candidates, the rollback of a failed provenance insert and a scan followed
//! by an import through the real services over a synthetic board. The tests skip
//! with a notice when `JOBNIAC_TEST_PG_PORT` is unset and panic instead when `CI` is set. All data
//! is synthetic.

use std::error::Error;
use std::sync::Arc;

use jobniac_core::arbeitnow::board::{Page, UpstreamJob};
use jobniac_core::arbeitnow::candidates::{ArbeitnowCandidate, CandidateCodec};
use jobniac_core::arbeitnow::cursor::CursorKey;
use jobniac_core::arbeitnow::failure::BoardFailure;
use jobniac_core::arbeitnow::import::{
    ArbeitnowJobImportService, ImportOutcome, ImportRequest, ImportStatus, TrustedImportRepository,
};
use jobniac_core::arbeitnow::instant::ArbeitnowInstant;
use jobniac_core::arbeitnow::scan::{ArbeitnowBoard, ArbeitnowScanService, ScanRequest};
use jobniac_core::clock::FixedClock;
use jobniac_core::jobs::JobAggregate;
use jobniac_infra::arbeitnow::repository::PostgresArbeitnowImportRepository;
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::migrations::migrate;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use jobniac_testkit::pg::{TestDatabase, TestServer};
use time::macros::datetime;
use tokio_postgres::Client;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const KEY: [u8; 32] = [0; 32];
const URL: &str = "https://arbeitnow.com/view/test-job";

/// A migrated disposable database with the repository, the import service and a token issuer.
struct Fixture {
    database: TestDatabase,
    repository: PostgresArbeitnowImportRepository,
    service: Arc<ArbeitnowJobImportService<PostgresArbeitnowImportRepository>>,
    issuer: CandidateCodec,
}

fn codec() -> Result<CandidateCodec, Box<dyn Error>> {
    Ok(CandidateCodec::with_instant_source(&KEY, || {
        ArbeitnowInstant::new(500, 0)
    })?)
}

async fn start() -> Result<Option<Fixture>, Box<dyn Error>> {
    let Some(server) = TestServer::from_env() else {
        assert!(
            std::env::var_os("CI").is_none(),
            "JOBNIAC_TEST_PG_PORT must be set in CI so that the Arbeitnow PostgreSQL tests run"
        );
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(None);
    };
    let database = server.fresh_database().await;
    let config = load_database_config(&database.environment())
        .ok_or("the disposable database configuration is invalid")?;
    migrate(&config).await.map_err(|error| error.0)?;
    let repository =
        PostgresArbeitnowImportRepository::new(PgUnitOfWork::new(PgPool::new(Some(config))));
    let service = Arc::new(ArbeitnowJobImportService::new(
        codec()?,
        repository.clone(),
        Arc::new(FixedClock::new(datetime!(2026-10-09 12:00 UTC))),
    ));
    Ok(Some(Fixture {
        database,
        repository,
        service,
        issuer: codec()?,
    }))
}

fn candidate(slug: &str, title: &str) -> Result<ArbeitnowCandidate, Box<dyn Error>> {
    Ok(ArbeitnowCandidate {
        issued_at: ArbeitnowInstant::new(100, 0)?,
        expires_at: ArbeitnowInstant::new(1000, 0)?,
        slug: slug.to_owned(),
        canonical_url: format!("https://arbeitnow.com/view/{slug}"),
        company: "Example Works GmbH".to_owned(),
        title: title.to_owned(),
        location: "Berlin".to_owned(),
        remote: true,
        posted_at: Some(ArbeitnowInstant::new(1_760_000_000, 0)?),
        tags: vec!["Python".to_owned(), "SQL".to_owned()],
        job_types: vec!["full_time".to_owned()],
        description_excerpt: "Build things".to_owned(),
    })
}

fn token(fixture: &Fixture, slug: &str, title: &str) -> Result<String, Box<dyn Error>> {
    Ok(fixture.issuer.issue(&candidate(slug, title)?)?)
}

async fn import(fixture: &Fixture, token: String) -> Result<ImportOutcome, Box<dyn Error>> {
    Ok(fixture
        .service
        .import(ImportRequest {
            candidate_token: Some(token),
        })
        .await?)
}

/// Row counts of `jobs`, `job_link_ingestions`, `job_skills` and `job_text_ingestions`.
async fn counts(client: &Client) -> Result<(i64, i64, i64, i64), Box<dyn Error>> {
    let mut counts = [0i64; 4];
    for (slot, table) in counts.iter_mut().zip([
        "jobs",
        "job_link_ingestions",
        "job_skills",
        "job_text_ingestions",
    ]) {
        let row = client
            .query_one(&format!("SELECT count(*) FROM job_schema.{table}"), &[])
            .await?;
        *slot = row.get(0);
    }
    Ok((counts[0], counts[1], counts[2], counts[3]))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn import_persists_link_provenance_and_round_trips() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let outcome = import(&fixture, token(&fixture, "test-job", "Platform Engineer")?).await?;
    assert_eq!(outcome.status, ImportStatus::CreatedJob);
    let graph = outcome.job;
    let stored = fixture
        .repository
        .find_by_normalized_url(URL)
        .await?
        .ok_or("the imported job must be found by its URL")?;
    assert_eq!(stored, graph);
    assert_eq!(graph.job.source_label.as_deref(), Some("arbeitnow"));
    assert_eq!(
        graph.job.posted_at,
        Some(datetime!(2025-10-09 08:53:20 UTC))
    );
    let link = graph.link_ingestion.as_ref().ok_or("link provenance")?;
    assert_eq!(
        link.fields.get("httpStatus"),
        Some(&serde_json::Value::Null)
    );
    assert_eq!(link.fields.get("url"), Some(&serde_json::json!(URL)));
    assert_eq!(
        link.fields.get("sourceTitle"),
        Some(&serde_json::json!("Arbeitnow scan candidate (remote=true)"))
    );
    let skills: Vec<&str> = graph.skills.iter().map(|s| s.skill.as_str()).collect();
    assert_eq!(skills, ["Python", "SQL"]);
    assert_eq!(
        counts(&fixture.database.admin_client().await).await?,
        (1, 1, 2, 0)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeat_import_reuses_by_url_and_changed_content_still_reuses() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let first = import(&fixture, token(&fixture, "test-job", "Platform Engineer")?).await?;
    let again = import(&fixture, token(&fixture, "test-job", "Renamed")?).await?;
    assert_eq!(again.status, ImportStatus::ReusedExistingJob);
    assert_eq!(again.job, first.job);
    assert_eq!(
        counts(&fixture.database.admin_client().await).await?,
        (1, 1, 2, 0)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fingerprint_match_under_another_url_reuses_the_stored_job() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let first = import(&fixture, token(&fixture, "test-job", "Platform Engineer")?)
        .await?
        .job;
    let mut twin: JobAggregate = first.clone();
    let twin_id = Uuid::from_u128(1);
    twin.job.id = twin_id;
    for (index, skill) in twin.skills.iter_mut().enumerate() {
        skill.id = Uuid::from_u128(10 + index as u128);
        skill.job_id = twin_id;
    }
    let link = twin.link_ingestion.as_mut().ok_or("link provenance")?;
    link.id = Uuid::from_u128(2);
    link.job_id = twin_id;
    link.normalized_url = "https://arbeitnow.com/view/x".to_owned();
    link.fields.insert(
        "url".to_owned(),
        serde_json::json!("https://arbeitnow.com/view/x"),
    );
    let (found, created) = fixture.repository.create_link_job(twin).await?;
    assert!(!created);
    assert_eq!(found, first);
    assert_eq!(
        counts(&fixture.database.admin_client().await).await?,
        (1, 1, 2, 0)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_imports_of_one_candidate_create_exactly_one_complete_job() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let token = token(&fixture, "test-job", "Platform Engineer")?;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let service = Arc::clone(&fixture.service);
        let token = token.clone();
        tasks.spawn(async move {
            service
                .import(ImportRequest {
                    candidate_token: Some(token),
                })
                .await
        });
    }
    let mut outcomes = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        outcomes.push(joined??);
    }
    let created = outcomes
        .iter()
        .filter(|outcome| outcome.status == ImportStatus::CreatedJob)
        .count();
    let reused = outcomes
        .iter()
        .filter(|outcome| outcome.status == ImportStatus::ReusedExistingJob)
        .count();
    assert_eq!((created, reused), (1, 7));
    let mut ids: Vec<Uuid> = outcomes.iter().map(|outcome| outcome.job.job.id).collect();
    ids.dedup();
    assert_eq!(ids.len(), 1);
    assert_eq!(
        counts(&fixture.database.admin_client().await).await?,
        (1, 1, 2, 0)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn distinct_candidates_import_independently() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    for slug in ["one", "two"] {
        let outcome = import(&fixture, token(&fixture, slug, slug)?).await?;
        assert_eq!(outcome.status, ImportStatus::CreatedJob);
    }
    assert_eq!(
        counts(&fixture.database.admin_client().await).await?,
        (2, 2, 4, 0)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_provenance_insert_rolls_back_the_whole_import() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let admin = fixture.database.admin_client().await;
    admin
        .batch_execute(
            "ALTER TABLE job_schema.job_link_ingestions ADD CONSTRAINT never_ok CHECK (false)",
        )
        .await?;
    let refused = import(&fixture, token(&fixture, "test-job", "Platform Engineer")?).await;
    let error = refused.err().ok_or("the import must fail")?;
    let text = error.to_string();
    assert!(
        !text.contains("never_ok") && !text.contains("INSERT"),
        "{text}"
    );
    assert_eq!(counts(&admin).await?, (0, 0, 0, 0));
    assert!(
        fixture
            .repository
            .find_by_normalized_url(URL)
            .await?
            .is_none()
    );
    Ok(())
}

/// A fake board with one synthetic upstream job: the scan never reaches the network.
#[derive(Debug)]
struct SyntheticBoard;

impl ArbeitnowBoard for SyntheticBoard {
    fn fetch(&self, _page: u32, _visa: Option<bool>) -> Result<Page, BoardFailure> {
        Ok(Page {
            jobs: vec![UpstreamJob {
                slug: "test-job".to_owned(),
                company: "Example Works GmbH".to_owned(),
                title: "Platform Engineer".to_owned(),
                html_description: "<p>Build things.</p>".to_owned(),
                remote: Some(true),
                url: "https://www.arbeitnow.com/view/test-job".to_owned(),
                tags: vec!["Python".to_owned(), "SQL".to_owned()],
                job_types: vec!["full_time".to_owned()],
                location: Some("Berlin".to_owned()),
                created_at: Some(1_760_000_000),
            }],
            another_page_may_remain: false,
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scan_then_import_through_real_services_without_network() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    // The real scan service over the synthetic board, issuing tokens with the same key as the import.
    let scan = ArbeitnowScanService::new(SyntheticBoard, CursorKey::new(&[7u8; 32])?, codec()?);
    let result = tokio::task::spawn_blocking(move || scan.scan(&ScanRequest::default())).await??;
    let candidate = result
        .jobs
        .first()
        .ok_or("the scan must return the synthetic job")?;
    assert_eq!(candidate.canonical_url, URL);
    let token = candidate.candidate_token.clone();

    let first = import(&fixture, token.clone()).await?;
    assert_eq!(first.status, ImportStatus::CreatedJob);
    assert_eq!(first.job.job.source_label.as_deref(), Some("arbeitnow"));

    // A second import of the same candidate reuses the stored job by its canonical URL.
    let again = import(&fixture, token).await?;
    assert_eq!(again.status, ImportStatus::ReusedExistingJob);
    assert_eq!(again.job.job.id, first.job.job.id);
    assert_eq!(
        counts(&fixture.database.admin_client().await).await?,
        (1, 1, 2, 0)
    );
    Ok(())
}
