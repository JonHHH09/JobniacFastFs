//! Link ingestion persistence and the link use case against a disposable PostgreSQL 18 server.
//!
//! Covered: `find_by_normalized_url`,
//! `create_link_job`, the forged-graph refusals and the prepared link service. The server-backed
//! tests skip with a notice when `JOBNIAC_TEST_PG_PORT` is unset, and panic instead when `CI` is set.
//! The forged-graph refusals need no server. All data is synthetic.

mod job_support;

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use job_support::{Fixture, TestResult, count, fixed_service, start, text_graph};
use jobniac_core::clock::FixedClock;
use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::job_links::service::{JobLinkPageFetcher, JobLinkRepository, JobLinkService};
use jobniac_core::job_links::{JobLinkFetchFailure, JobLinkFetchResult};
use jobniac_core::jobs::{CaptureStatus, JobAggregate, JobRepository, LinkIngestionRecord};
use jobniac_core::profiles::repository::{READ_ONLY, READ_WRITE};
use jobniac_infra::postgres::jobs::PgJobRepository;
use jobniac_infra::postgres::jobs::graph::check_link_graph;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use serde_json::{Map, Value, json};
use time::OffsetDateTime;
use time::macros::datetime;
use tokio_postgres::Client;
use uuid::Uuid;

const CREATED_AT: OffsetDateTime = datetime!(2026-10-04 12:30 UTC);
const SECRET: &str = "PRIVATE_GRAPH_SECRET";

/// A fetch port that counts its calls and answers with fixed content.
#[derive(Debug, Default)]
struct CountingFetcher {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl JobLinkPageFetcher for CountingFetcher {
    async fn fetch(&self, _retrieval_url: &str) -> Result<JobLinkFetchResult, JobLinkFetchFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(JobLinkFetchResult {
            title: Some("Fetched title".to_owned()),
            description: Some("Skills: Python, SQL".to_owned()),
            http_status: 200,
        })
    }
}

/// A link-sourced aggregate: built from the text
/// fixture, with the link provenance of `normalized_url` (or `?id=identifier`) and the fingerprint
/// of `fingerprint` (or the fixture fingerprint).
fn link_graph(
    identifier: u128,
    normalized_url: Option<&str>,
    fingerprint: Option<&str>,
) -> JobAggregate {
    let mut graph = text_graph(identifier, CREATED_AT);
    graph.job.source_method = "link".to_owned();
    graph.text_ingestion = None;
    if let Some(fingerprint) = fingerprint {
        graph.job.canonical_fingerprint = fingerprint.to_owned();
    }
    let job_id = graph.job.id;
    let mut fields = Map::new();
    fields.insert("url".to_owned(), json!("https://example.com/jobs"));
    fields.insert("fetchedAt".to_owned(), json!("2026-10-04T12:30:00Z"));
    fields.insert("httpStatus".to_owned(), json!(200));
    fields.insert("sourceTitle".to_owned(), json!("Fetched title"));
    fields.insert("createdAt".to_owned(), json!("2026-10-04T12:30:00Z"));
    graph.link_ingestion = Some(LinkIngestionRecord {
        id: Uuid::from_u128(identifier + 3000),
        job_id,
        normalized_url: normalized_url.map_or_else(
            || format!("https://example.com/jobs?id={identifier}"),
            str::to_owned,
        ),
        fields,
    });
    graph
}

/// Writes one graph through the repository in a read-write scope.
async fn create(uow: &PgUnitOfWork, graph: JobAggregate) -> Result<(JobAggregate, bool), AppError> {
    uow.transaction(READ_WRITE, async |tx| {
        PgJobRepository.create_link_job(tx, &graph).await
    })
    .await
}

/// Reads the job that owns `normalized_url` through the repository in a read-only scope.
async fn find(uow: &PgUnitOfWork, normalized_url: &str) -> Result<Option<JobAggregate>, AppError> {
    uow.transaction(READ_ONLY, async |tx| {
        PgJobRepository
            .find_by_normalized_url(tx, normalized_url)
            .await
    })
    .await
}

/// A started fixture, or `None` (with a skip notice) when no server is configured.
async fn fixture() -> Result<Option<Fixture>, Box<dyn Error>> {
    if std::env::var_os("JOBNIAC_TEST_PG_PORT").is_none() {
        assert!(
            std::env::var_os("CI").is_none(),
            "JOBNIAC_TEST_PG_PORT must be set in CI so that the link PostgreSQL tests run"
        );
    }
    start().await
}

/// The error envelope text of a refusal, for the no-echo checks.
fn wire(error: &AppError) -> String {
    serde_json::to_string(&error_envelope(error)).unwrap_or_default()
}

#[test]
fn forged_link_graphs_are_refused_before_any_sql() -> TestResult {
    let valid = link_graph(600, None, None);
    check_link_graph(&valid)?;

    let mut source = valid.clone();
    source.job.source_method = "text".to_owned();
    let mut second = valid.clone();
    second.text_ingestion = Some(text_graph(600, CREATED_AT).text_ingestion.ok_or("text")?);
    let mut link_owner = valid.clone();
    if let Some(link) = link_owner.link_ingestion.as_mut() {
        link.job_id = Uuid::from_u128(9999);
    }
    let mut skill_owner = valid.clone();
    if let Some(skill) = skill_owner.skills.first_mut() {
        skill.job_id = Uuid::from_u128(9999);
    }
    let mut display_secret = valid.clone();
    set_field(
        &mut display_secret,
        "url",
        json!(format!("https://example.com/jobs?token={SECRET}")),
    );
    let mut identity_secret = valid.clone();
    if let Some(link) = identity_secret.link_ingestion.as_mut() {
        link.normalized_url = format!("https://example.com/jobs?id=600&token={SECRET}");
    }
    let mut path_mismatch = valid.clone();
    set_field(
        &mut path_mismatch,
        "url",
        json!("https://example.com/jobs/different"),
    );
    let mut status_bool = valid.clone();
    set_field(&mut status_bool, "httpStatus", json!(true));
    let mut status_low = valid.clone();
    set_field(&mut status_low, "httpStatus", json!(99));
    let mut status_high = valid.clone();
    set_field(&mut status_high, "httpStatus", json!(600));
    let mut status_text = valid.clone();
    set_field(&mut status_text, "httpStatus", json!(SECRET));
    let mut title_number = valid.clone();
    set_field(&mut title_number, "sourceTitle", json!(7));
    let mut no_url = valid.clone();
    if let Some(link) = no_url.link_ingestion.as_mut() {
        link.fields.remove("url");
    }
    let mut blank_fingerprint = valid.clone();
    blank_fingerprint.job.canonical_fingerprint = "   ".to_owned();

    let forged = [
        ("source", source),
        ("second-provenance", second),
        ("link-owner", link_owner),
        ("skill-owner", skill_owner),
        ("display-secret", display_secret),
        ("identity-secret", identity_secret),
        ("path-mismatch", path_mismatch),
        ("status-bool", status_bool),
        ("status-low", status_low),
        ("status-high", status_high),
        ("status-text", status_text),
        ("source-title", title_number),
        ("missing-url", no_url),
        ("blank-fingerprint", blank_fingerprint),
    ];
    for (name, graph) in forged {
        let error = match check_link_graph(&graph) {
            Err(error) => error,
            Ok(()) => return Err(format!("{name} must be refused").into()),
        };
        assert!(!wire(&error).contains(SECRET), "{name} echoed the secret");
    }
    Ok(())
}

fn set_field(graph: &mut JobAggregate, key: &str, value: Value) {
    if let Some(link) = graph.link_ingestion.as_mut() {
        link.fields.insert(key.to_owned(), value);
    }
}

#[tokio::test]
async fn an_unconfigured_database_refuses_link_writes_without_connecting() -> TestResult {
    let uow = job_support::unconfigured_uow();
    let error = create(&uow, link_graph(610, None, None))
        .await
        .err()
        .ok_or("an unconfigured database must refuse the write")?;
    assert!(wire(&error).contains("not configured"));
    Ok(())
}

#[tokio::test]
async fn a_link_round_trips_and_is_found_by_its_canonical_url() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let graph = link_graph(620, Some("https://example.com/jobs?id=1"), None);
    assert_eq!(
        create(&fixture.uow, graph.clone()).await?,
        (graph.clone(), true)
    );
    assert_eq!(
        find(&fixture.uow, "https://example.com/jobs?id=1").await?,
        Some(graph.clone())
    );
    let stored = uow_get(&fixture.uow, graph.job.id).await?;
    assert_eq!(stored, Some(graph));
    Ok(())
}

#[tokio::test]
async fn a_duplicate_canonical_url_reuses_the_first_job() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let first = link_graph(630, Some("https://example.com/jobs?id=630"), None);
    let second = link_graph(631, Some("https://example.com/jobs?id=630"), None);
    assert_eq!(
        create(&fixture.uow, first.clone()).await?,
        (first.clone(), true)
    );
    assert_eq!(create(&fixture.uow, second).await?, (first, false));
    let client = fixture.client().await;
    let jobs: i64 = client
        .query_one("SELECT count(*) FROM job_schema.jobs", &[])
        .await?
        .get(0);
    assert_eq!(jobs, 1);
    Ok(())
}

#[tokio::test]
async fn a_duplicate_fingerprint_reuses_the_first_job() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let first = link_graph(640, None, Some(&"f".repeat(64)));
    let second = link_graph(641, None, Some(&"f".repeat(64)));
    assert_eq!(
        create(&fixture.uow, first.clone()).await?,
        (first.clone(), true)
    );
    assert_eq!(create(&fixture.uow, second).await?, (first, false));
    Ok(())
}

#[tokio::test]
async fn a_stored_trailing_slash_is_kept_and_not_canonicalized_again() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let mut graph = link_graph(650, None, None);
    set_field(&mut graph, "url", json!("https://example.com/jobs/"));
    if let Some(link) = graph.link_ingestion.as_mut() {
        link.normalized_url = "https://example.com/jobs/?id=1".to_owned();
    }
    assert_eq!(
        create(&fixture.uow, graph.clone()).await?,
        (graph.clone(), true)
    );
    assert_eq!(
        find(&fixture.uow, "https://example.com/jobs/?id=1").await?,
        Some(graph)
    );
    Ok(())
}

#[tokio::test]
async fn the_service_reuses_by_canonical_url_before_fetching() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let service = JobLinkService::new(
        fixture.uow.clone(),
        PgJobRepository,
        Arc::new(CountingFetcher {
            calls: Arc::clone(&calls),
        }),
        Arc::new(FixedClock::new(CREATED_AT)),
    );
    let raw = "https://EXAMPLE.com/jobs/?id=7&token=PRIVATE_RETRIEVAL_SECRET#private-fragment";
    let created = service.add_job_from_link(&json!({ "url": raw })).await?;
    assert_eq!(created.status, CaptureStatus::CreatedJob);
    let link = created
        .job
        .link_ingestion
        .as_ref()
        .ok_or("missing provenance")?;
    assert_eq!(
        link.fields.get("url"),
        Some(&json!("https://example.com/jobs"))
    );
    assert_eq!(link.normalized_url, "https://example.com/jobs?id=7");
    assert!(!serde_json::to_string(&created.job)?.contains("PRIVATE_RETRIEVAL_SECRET"));

    let reused = service
        .add_job_from_link(&json!({ "url": raw, "title": "Ignored override" }))
        .await?;
    assert_eq!(reused.status, CaptureStatus::ReusedExistingJob);
    assert_eq!(reused.job, created.job);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn an_edit_keeps_the_link_provenance_and_the_postings_follow_the_skills() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let graph = link_graph(660, None, None);
    assert_eq!(
        create(&fixture.uow, graph.clone()).await?,
        (graph.clone(), true)
    );
    let edited = fixed_service(&fixture.uow, CREATED_AT)?
        .update_job(&json!({
            "jobId": graph.job.id.to_string(),
            "expectedRevision": 0,
            "title": "New title",
            "description": "New content",
            "skills": ["Docker"],
        }))
        .await?;
    assert_eq!(edited.job.revision, 1);
    assert_eq!(edited.link_ingestion, graph.link_ingestion);
    assert!(edited.text_ingestion.is_none());
    let search = job_support::search(&fixture.uow);
    let old = search.search_jobs(&json!({ "query": "Python" })).await?;
    assert!(old.jobs.is_empty());
    let new = search.search_jobs(&json!({ "query": "Docker" })).await?;
    assert_eq!(new.jobs.len(), 1);
    assert_eq!(new.jobs[0].job, edited.job);
    let client = fixture.client().await;
    let links: i64 = client
        .query_one("SELECT count(*) FROM job_schema.job_link_ingestions", &[])
        .await?
        .get(0);
    let texts: i64 = client
        .query_one("SELECT count(*) FROM job_schema.job_text_ingestions", &[])
        .await?
        .get(0);
    assert_eq!((links, texts), (1, 0));
    Ok(())
}

#[tokio::test]
async fn concurrent_creates_of_one_url_persist_one_complete_winner() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let first = link_graph(670, Some("https://example.com/jobs?id=670"), None);
    let second = link_graph(671, Some("https://example.com/jobs?id=670"), None);
    let (left, right) = tokio::join!(
        create(&fixture.uow, first.clone()),
        create(&fixture.uow, second)
    );
    let (left, left_created) = left?;
    let (right, right_created) = right?;
    assert_ne!(left_created, right_created);
    assert_eq!(left, right);
    assert!(left.link_ingestion.is_some() && left.skills.len() == 1);
    let client = fixture.client().await;
    let jobs: i64 = client
        .query_one("SELECT count(*) FROM job_schema.jobs", &[])
        .await?
        .get(0);
    assert_eq!(jobs, 1);
    Ok(())
}

/// Installs a deferred constraint trigger on the link table that refuses every link row at commit.
/// The insert then fails only when the transaction commits, after the job, skills and search terms
/// were written.
async fn reject_links_at_commit(client: &Client) -> TestResult {
    client
        .batch_execute(
            r"
            CREATE FUNCTION job_schema.reject_link_test() RETURNS trigger
            LANGUAGE plpgsql AS $body$ BEGIN
                RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'private link sentinel';
            END; $body$;
            CREATE CONSTRAINT TRIGGER reject_link_test AFTER INSERT
            ON job_schema.job_link_ingestions DEFERRABLE INITIALLY DEFERRED
            FOR EACH ROW EXECUTE FUNCTION job_schema.reject_link_test();
            ",
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn a_commit_failure_rolls_back_the_job_skills_search_terms_and_provenance() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let client = fixture.client().await;
    reject_links_at_commit(&client).await?;
    let error = create(&fixture.uow, link_graph(640, None, None))
        .await
        .err()
        .ok_or("the commit trigger must refuse the link graph")?;
    let text = wire(&error);
    assert!(
        text.contains("Request data violates a persistence constraint"),
        "{text}"
    );
    assert!(!text.contains("private link sentinel"), "{text}");
    for table in [
        "jobs",
        "job_skills",
        "search_terms",
        "job_link_ingestions",
        "job_text_ingestions",
    ] {
        let rows = count(&client, &format!("SELECT count(*) FROM job_schema.{table}")).await?;
        assert_eq!(rows, 0, "{table} must be rolled back");
    }
    Ok(())
}

#[tokio::test]
async fn a_duplicate_url_reuse_locks_a_coherent_graph_against_a_concurrent_edit() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let original = link_graph(630, Some("https://example.com/jobs?id=630"), None);
    create(&fixture.uow, original.clone()).await?;
    let duplicate = link_graph(631, Some("https://example.com/jobs?id=630"), None);
    reuse_waits_for_the_row_lock(&fixture, original, duplicate).await
}

#[tokio::test]
async fn a_duplicate_fingerprint_reuse_locks_a_coherent_graph_against_a_concurrent_edit()
-> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let original = link_graph(632, Some("https://example.com/jobs?id=632"), None);
    create(&fixture.uow, original.clone()).await?;
    let fingerprint = original.job.canonical_fingerprint.clone();
    let duplicate = link_graph(
        633,
        Some("https://example.com/jobs?id=633"),
        Some(&fingerprint),
    );
    reuse_waits_for_the_row_lock(&fixture, original, duplicate).await
}

/// The duplicate reuse must wait for a concurrent row lock on the stored job (a transaction holding
/// `FOR UPDATE` pauses the reader), then return the stored
/// aggregate unchanged.
async fn reuse_waits_for_the_row_lock(
    fixture: &Fixture,
    original: JobAggregate,
    duplicate: JobAggregate,
) -> TestResult {
    let holder = fixture.client().await;
    holder.batch_execute("BEGIN").await?;
    holder
        .query_one(
            "SELECT id FROM job_schema.jobs WHERE id = $1 FOR UPDATE",
            &[&original.job.id],
        )
        .await?;
    let observer = fixture.client().await;
    let reuse = create(&fixture.uow, duplicate);
    let release = async {
        let mut waited = false;
        for _ in 0..500 {
            let blocked: bool = observer
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity \
                     WHERE datname = current_database() AND wait_event_type = 'Lock')",
                    &[],
                )
                .await?
                .get(0);
            if blocked {
                waited = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        holder.batch_execute("COMMIT").await?;
        Ok::<bool, Box<dyn Error>>(waited)
    };
    let (result, released) = tokio::join!(reuse, release);
    assert!(released?, "the reuse must block on the concurrent row lock");
    assert_eq!(result?, (original.clone(), false));
    let normalized = original
        .link_ingestion
        .as_ref()
        .map(|link| link.normalized_url.clone())
        .ok_or("the original is a link graph")?;
    assert_eq!(find(&fixture.uow, &normalized).await?, Some(original));
    Ok(())
}

#[tokio::test]
async fn a_database_failure_in_the_link_repository_scrubs_the_private_context() -> TestResult {
    // The closed loopback port fails every connection without a server; the password and the
    // database name must never reach the error.
    let uow = job_support::closed_port_uow()?;
    let found = find(&uow, "https://example.com/jobs?id=1")
        .await
        .err()
        .ok_or("a closed port must fail the lookup")?;
    let created = create(&uow, link_graph(650, None, None))
        .await
        .err()
        .ok_or("a closed port must fail the write")?;
    for error in [&found, &created] {
        let text = wire(error);
        assert!(text.contains("Local database is unavailable"), "{text}");
        for secret in ["synthetic-password", "jobniac_unreachable", "app_user"] {
            assert!(!text.contains(secret), "{text}");
            assert!(!format!("{error:?}").contains(secret));
        }
    }
    Ok(())
}

/// Reads one aggregate through the repository in a read-only scope.
async fn uow_get(uow: &PgUnitOfWork, job_id: Uuid) -> Result<Option<JobAggregate>, AppError> {
    uow.transaction(READ_ONLY, async |tx| {
        PgJobRepository.get_job(tx, job_id).await
    })
    .await
}
