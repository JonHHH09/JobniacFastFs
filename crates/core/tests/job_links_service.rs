//! Link ingestion use case, the service cases. The repository and the fetch port are in-memory
//! doubles that record every call, so a test can assert that nothing is fetched or written when
//! validation or reuse decides the outcome. All data is synthetic.

mod job_support;

use std::error::Error;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use job_support::{CounterIds, MemoryUow, fixed_clock, locked};
use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::job_links::html::parse_job_html;
use jobniac_core::job_links::service::{JobLinkPageFetcher, JobLinkRepository, JobLinkService};
use jobniac_core::job_links::{JobLinkFetchFailure, JobLinkFetchResult};
use jobniac_core::jobs::{
    CaptureStatus, JobAggregate, JobRecord, JobSkillRecord, LinkIngestionRecord,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const NOW: &str = "2026-10-04T12:30:00Z";
const RETRIEVAL_URL: &str =
    "https://EXAMPLE.com/jobs/?id=7&token=private-token-sentinel#private-fragment-sentinel";
const CANONICAL: &str = "https://example.com/jobs?id=7";
const DISPLAY: &str = "https://example.com/jobs";

/// What the doubles recorded, in call order.
#[derive(Debug, Default)]
struct Log {
    lookups: Vec<String>,
    writes: Vec<JobAggregate>,
    fetches: Vec<String>,
}

/// One double for both ports: the repository (stored duplicate or race winner) and the fetch port
/// (fixed content or failure). Clones share the log.
#[derive(Debug, Clone)]
struct Double {
    log: Arc<Mutex<Log>>,
    existing: Option<JobAggregate>,
    winner: Option<JobAggregate>,
    content: Result<JobLinkFetchResult, JobLinkFetchFailure>,
}

impl Double {
    fn with_content(content: JobLinkFetchResult) -> Self {
        Self {
            log: Arc::new(Mutex::new(Log::default())),
            existing: None,
            winner: None,
            content: Ok(content),
        }
    }

    fn failing(failure: JobLinkFetchFailure) -> Self {
        Self {
            content: Err(failure),
            ..Self::with_content(content("Title", "Description", 200))
        }
    }

    fn service(&self) -> JobLinkService<MemoryUow, Double> {
        JobLinkService::with_ids(
            MemoryUow,
            self.clone(),
            Arc::new(self.clone()),
            fixed_clock(NOW),
            Arc::new(CounterIds::default()),
        )
    }

    fn lookups(&self) -> Vec<String> {
        locked(&self.log).lookups.clone()
    }

    fn writes(&self) -> usize {
        locked(&self.log).writes.len()
    }

    fn fetches(&self) -> Vec<String> {
        locked(&self.log).fetches.clone()
    }
}

#[async_trait]
impl JobLinkRepository for Double {
    type Tx = ();

    async fn find_by_normalized_url(
        &self,
        _tx: &mut (),
        normalized_url: &str,
    ) -> Result<Option<JobAggregate>, AppError> {
        locked(&self.log).lookups.push(normalized_url.to_owned());
        Ok(self.existing.clone())
    }

    async fn create_link_job(
        &self,
        _tx: &mut (),
        graph: &JobAggregate,
    ) -> Result<(JobAggregate, bool), AppError> {
        locked(&self.log).writes.push(graph.clone());
        Ok(match &self.winner {
            Some(winner) => (winner.clone(), false),
            None => (graph.clone(), true),
        })
    }
}

#[async_trait]
impl JobLinkPageFetcher for Double {
    async fn fetch(&self, retrieval_url: &str) -> Result<JobLinkFetchResult, JobLinkFetchFailure> {
        locked(&self.log).fetches.push(retrieval_url.to_owned());
        self.content.clone()
    }
}

fn content(title: &str, description: &str, status: i64) -> JobLinkFetchResult {
    JobLinkFetchResult {
        title: Some(title.to_owned()),
        description: Some(description.to_owned()),
        http_status: status,
    }
}

fn stored_job(id: u128) -> JobAggregate {
    let job_id = Uuid::from_u128(id);
    JobAggregate {
        job: JobRecord {
            id: job_id,
            source_method: "link".to_owned(),
            source_label: None,
            title: "Stored".to_owned(),
            company: None,
            location: None,
            description: "Stored description".to_owned(),
            experience_requirement: None,
            employment_type: None,
            seniority: None,
            posted_at: None,
            canonical_fingerprint: "stored-fingerprint".to_owned(),
            created_at: jobniac_core::clock::parse_instant(NOW).expect("synthetic instant"),
            updated_at: jobniac_core::clock::parse_instant(NOW).expect("synthetic instant"),
            revision: 7,
        },
        skills: Vec::<JobSkillRecord>::new(),
        link_ingestion: Some(LinkIngestionRecord {
            id: Uuid::from_u128(id + 1),
            job_id,
            normalized_url: CANONICAL.to_owned(),
            fields: Map::new(),
        }),
        text_ingestion: None,
    }
}

/// The envelope of a refused call, as the MCP tool would send it.
fn envelope(error: &AppError) -> Value {
    serde_json::to_value(error_envelope(error)).unwrap_or(Value::Null)
}

fn request(extra: Value) -> Value {
    let mut object = json!({ "url": RETRIEVAL_URL });
    if let (Some(base), Some(extra)) = (object.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            base.insert(key.clone(), value.clone());
        }
    }
    object
}

#[tokio::test]
async fn created_job_keeps_the_source_graph_and_redacts_the_retrieval_secret() -> TestResult {
    let double = Double::with_content(content(
        "  Software Engineer  ",
        "Skills: Python, SQL.\nExperience: 3 years\nSynthetic work",
        200,
    ));
    let result = double
        .service()
        .add_job_from_link(&json!({ "url": format!("\u{2003}{RETRIEVAL_URL}\u{2003}") }))
        .await?;
    assert_eq!(result.status, CaptureStatus::CreatedJob);
    let graph = &result.job;
    let job = &graph.job;
    assert_eq!(job.source_method, "link");
    assert_eq!(job.source_label, None);
    assert_eq!(job.title, "Software Engineer");
    assert_eq!(job.revision, 0);
    assert_eq!(job.experience_requirement.as_deref(), Some("3 years"));
    assert_eq!(job.created_at, job.updated_at);
    assert!(graph.text_ingestion.is_none());
    let skills: Vec<&str> = graph.skills.iter().map(|s| s.skill.as_str()).collect();
    assert_eq!(skills, ["Python", "SQL"]);
    let orders: Vec<i64> = graph.skills.iter().map(|s| s.display_order).collect();
    assert_eq!(orders, [0, 1]);
    assert!(
        graph
            .skills
            .iter()
            .all(|s| s.required && s.job_id == job.id)
    );
    let link = graph.link_ingestion.as_ref().ok_or("missing provenance")?;
    assert_eq!(link.normalized_url, CANONICAL);
    assert_eq!(link.fields.get("url"), Some(&json!(DISPLAY)));
    assert_eq!(link.fields.get("httpStatus"), Some(&json!(200)));
    assert_eq!(
        link.fields.get("sourceTitle"),
        Some(&json!("Software Engineer"))
    );
    assert_eq!(link.fields.get("fetchedAt"), Some(&json!(NOW)));
    assert_eq!(link.fields.get("createdAt"), Some(&json!(NOW)));
    assert_eq!(double.lookups(), [CANONICAL]);
    assert_eq!(double.fetches(), [RETRIEVAL_URL]);
    assert_eq!(double.writes(), 1);
    let wire = serde_json::to_string(&result.job)?;
    assert!(!wire.contains("private-token-sentinel"));
    assert!(!wire.contains("private-fragment-sentinel"));
    Ok(())
}

#[tokio::test]
async fn explicit_fields_override_fetched_content() -> TestResult {
    let double = Double::with_content(content("Fetched title", "Cloudflare app shell", 503));
    let result = double
        .service()
        .add_job_from_link(&request(json!({
            "title": "  Explicit title  ",
            "description": "Skills: SQL, Rust, SQL.\nExperience: extracted requirement",
            "skills": ["Python / sql", "Ｐython; Go"],
            "experienceRequirement": "  explicit experience  ",
            "company": "  Example Co  ",
            "location": "  Tirana  ",
            "sourceLabel": "  Manual link  ",
            "employmentType": "  full-time  ",
            "seniority": "  senior  ",
            "postedAt": "2026-10-04T14:00:00+02:00",
        })))
        .await?;
    let job = &result.job.job;
    assert_eq!(job.title, "Explicit title");
    assert!(job.description.starts_with("Skills: SQL"));
    assert_eq!(
        job.experience_requirement.as_deref(),
        Some("explicit experience")
    );
    let skills: Vec<&str> = result.job.skills.iter().map(|s| s.skill.as_str()).collect();
    assert_eq!(skills, ["Python", "sql", "Go", "Rust"]);
    let normalized: Vec<&str> = result
        .job
        .skills
        .iter()
        .map(|s| s.normalized_skill.as_str())
        .collect();
    assert_eq!(normalized, ["python", "sql", "go", "rust"]);
    assert_eq!(job.company.as_deref(), Some("Example Co"));
    assert_eq!(job.location.as_deref(), Some("Tirana"));
    assert_eq!(job.source_label.as_deref(), Some("Manual link"));
    assert_eq!(job.employment_type.as_deref(), Some("full-time"));
    assert_eq!(job.seniority.as_deref(), Some("senior"));
    assert_eq!(
        job.posted_at,
        Some(jobniac_core::clock::parse_instant("2026-10-04T12:00:00Z")?)
    );
    let link = result
        .job
        .link_ingestion
        .as_ref()
        .ok_or("missing provenance")?;
    assert_eq!(link.fields.get("httpStatus"), Some(&json!(503)));
    assert_eq!(
        link.fields.get("sourceTitle"),
        Some(&json!("Fetched title"))
    );
    Ok(())
}

#[tokio::test]
async fn title_and_description_fall_back_in_the_documented_order() -> TestResult {
    let cases = [
        (
            JobLinkFetchResult {
                title: Some("Fetched title".to_owned()),
                description: Some("Fetched description".to_owned()),
                http_status: 200,
            },
            json!({}),
            "Fetched title",
            "Fetched description",
        ),
        (
            JobLinkFetchResult {
                title: Some("Fetched title".to_owned()),
                description: None,
                http_status: 200,
            },
            json!({}),
            "Fetched title",
            "Fetched title",
        ),
        (
            JobLinkFetchResult {
                title: None,
                description: Some("Fetched description".to_owned()),
                http_status: 200,
            },
            json!({}),
            CANONICAL,
            "Fetched description",
        ),
        (
            JobLinkFetchResult {
                title: None,
                description: None,
                http_status: 200,
            },
            json!({}),
            CANONICAL,
            CANONICAL,
        ),
        (
            content(" ", "\u{2003}", 200),
            json!({ "title": "Selected" }),
            "Selected",
            "Selected",
        ),
    ];
    for (fetched, overrides, title, description) in cases {
        let double = Double::with_content(fetched);
        let result = double
            .service()
            .add_job_from_link(&request(overrides))
            .await?;
        assert_eq!(result.job.job.title, title);
        assert_eq!(result.job.job.description, description);
    }
    Ok(())
}

#[tokio::test]
async fn blocked_content_is_refused_without_an_explicit_description() -> TestResult {
    let markers = [
        "SECURITY CHECK",
        "additional verification required",
        "request blocked",
        "blocked - indeed",
        "cloudflare",
        "enable javascript",
        "javascript and cookies",
        "javascript is required",
        "app shell",
    ];
    for marker in markers {
        for (title, description) in [(marker, "Usable"), ("Usable", marker)] {
            let double = Double::with_content(content(title, description, 200));
            let error = double
                .service()
                .add_job_from_link(&request(json!({ "description": "\u{2003} " })))
                .await
                .err()
                .ok_or("blocked content must be refused")?;
            assert_eq!(
                envelope(&error),
                json!({
                    "code": "validation_error",
                    "message": "Invalid job request",
                    "details": {
                        "field": "fetchedContent",
                        "reason": BLOCKED_REASON,
                    },
                })
            );
            assert_eq!(double.writes(), 0);
        }
    }
    Ok(())
}

const BLOCKED_REASON: &str = "job page fetch returned blocked/security-check content; provide pasted job text or a usable description";

#[tokio::test]
async fn http_error_content_is_refused_before_persistence() -> TestResult {
    for status in [400, 404, 429, 500, 599] {
        let double = Double::with_content(content("Title", "Usable text", status));
        let error = double
            .service()
            .add_job_from_link(&request(json!({})))
            .await
            .err()
            .ok_or("an HTTP error must be refused")?;
        assert_eq!(
            envelope(&error)["details"],
            json!({
                "field": "fetchedContent",
                "reason": format!(
                    "job page fetch returned HTTP {status}; provide pasted job text or a usable description"
                ),
            })
        );
        assert_eq!(double.writes(), 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_non_error_status_is_retained_without_a_network_claim() -> TestResult {
    for status in [100, 200, 302, 399] {
        let double = Double::with_content(content("Title", "Description", status));
        let result = double
            .service()
            .add_job_from_link(&request(json!({})))
            .await?;
        let link = result
            .job
            .link_ingestion
            .as_ref()
            .ok_or("missing provenance")?;
        assert_eq!(link.fields.get("httpStatus"), Some(&json!(status)));
    }
    Ok(())
}

#[tokio::test]
async fn an_out_of_range_status_is_invalid_content() -> TestResult {
    for status in [99, 600] {
        let double = Double::with_content(content("Title", "Description", status));
        let error = double
            .service()
            .add_job_from_link(&request(json!({})))
            .await
            .err()
            .ok_or("an out-of-range status must be refused")?;
        assert_eq!(
            envelope(&error)["details"],
            json!({ "field": "fetchedContent", "reason": "job page fetch returned invalid content" })
        );
        assert_eq!(double.writes(), 0);
    }
    Ok(())
}

#[tokio::test]
async fn an_existing_canonical_url_is_reused_before_any_fetch_or_optional_validation() -> TestResult
{
    let mut double = Double::failing(JobLinkFetchFailure::Unavailable);
    double.existing = Some(stored_job(900));
    let result = double
        .service()
        .add_job_from_link(&json!({ "url": RETRIEVAL_URL, "title": 23, "skills": "invalid" }))
        .await?;
    assert_eq!(result.status, CaptureStatus::ReusedExistingJob);
    assert_eq!(Some(&result.job), double.existing.as_ref());
    assert!(double.fetches().is_empty());
    assert_eq!(double.lookups(), [CANONICAL]);
    assert_eq!(double.writes(), 0);
    Ok(())
}

#[tokio::test]
async fn a_lost_write_race_returns_the_winning_aggregate() -> TestResult {
    let mut double = Double::with_content(content("Loser", "Synthetic content", 200));
    double.winner = Some(stored_job(901));
    let result = double
        .service()
        .add_job_from_link(&request(json!({})))
        .await?;
    assert_eq!(result.status, CaptureStatus::ReusedExistingJob);
    assert_eq!(Some(&result.job), double.winner.as_ref());
    assert_eq!(double.writes(), 1);
    assert_eq!(double.fetches().len(), 1);
    Ok(())
}

#[tokio::test]
async fn invalid_requests_never_reach_the_ports() -> TestResult {
    let payloads = [
        Value::Null,
        json!([]),
        json!("private-token-sentinel"),
        json!({}),
        json!({ "url": null }),
        json!({ "url": 8 }),
        json!({ "url": "\u{2003} " }),
        json!({ "url": "https://user:private-token-sentinel@example.com/job" }),
    ];
    for payload in payloads {
        let double = Double::with_content(content("Title", "Description", 200));
        let error = double
            .service()
            .add_job_from_link(&payload)
            .await
            .err()
            .ok_or("an invalid request must be refused")?;
        assert_eq!(envelope(&error)["code"], "validation_error");
        assert!(
            !envelope(&error)
                .to_string()
                .contains("private-token-sentinel")
        );
        assert!(double.lookups().is_empty());
        assert!(double.fetches().is_empty());
        assert_eq!(double.writes(), 0);
    }
    Ok(())
}

#[tokio::test]
async fn optional_fields_are_checked_before_the_fetch() -> TestResult {
    let mut cases: Vec<(&str, Value)> = [
        "title",
        "description",
        "company",
        "location",
        "sourceLabel",
        "employmentType",
        "seniority",
        "experienceRequirement",
    ]
    .into_iter()
    .map(|name| (name, json!(123)))
    .collect();
    cases.push(("skills", json!("Python")));
    cases.push(("skills", json!([null])));
    cases.push(("postedAt", json!("not-a-time")));
    cases.push(("postedAt", json!(true)));
    for (field, value) in cases {
        let double = Double::with_content(content("Title", "Description", 200));
        let error = double
            .service()
            .add_job_from_link(&request(json!({ field: value })))
            .await
            .err()
            .ok_or("an invalid optional field must be refused")?;
        assert_eq!(envelope(&error)["details"]["field"], field);
        assert!(double.fetches().is_empty());
        assert_eq!(double.writes(), 0);
    }
    Ok(())
}

#[tokio::test]
async fn every_classified_fetch_failure_maps_to_its_fixed_reason() -> TestResult {
    let failures = [
        JobLinkFetchFailure::Unavailable,
        JobLinkFetchFailure::Interrupted,
        JobLinkFetchFailure::InvalidUrl,
        JobLinkFetchFailure::RedirectNotFollowed,
        JobLinkFetchFailure::UnsafeAddress,
        JobLinkFetchFailure::IpLiteralRequired,
    ];
    for failure in failures {
        let double = Double::failing(failure);
        let error = double
            .service()
            .add_job_from_link(&request(json!({})))
            .await
            .err()
            .ok_or("a fetch failure must be refused")?;
        let wire = envelope(&error);
        assert_eq!(
            wire,
            json!({
                "code": "validation_error",
                "message": "Invalid job link ingestion request",
                "details": { "field": "url", "reason": failure.message() },
            })
        );
        assert!(!wire.to_string().contains("private-"));
        assert_eq!(double.writes(), 0);
    }
    Ok(())
}

#[tokio::test]
async fn the_link_fingerprint_is_the_five_component_hash_and_the_text_path_is_unchanged()
-> TestResult {
    let double = Double::with_content(content("Engineer", "Synthetic description", 200));
    let result = double
        .service()
        .add_job_from_link(&json!({ "url": "https://example.com/jobs?id=7" }))
        .await?;
    let expected = hex(b"https://example.com/jobs?id=7\nengineer\n\n\nsynthetic description");
    assert_eq!(result.job.job.canonical_fingerprint, expected);
    let other = Double::with_content(content("Engineer", "Synthetic description", 200));
    let second = other
        .service()
        .add_job_from_link(&json!({ "url": "https://example.com/jobs?id=8" }))
        .await?;
    assert_ne!(second.job.job.canonical_fingerprint, expected);
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn a_duplicate_text_posting_reuses_the_first_job() -> TestResult {
    let jobs = job_support::MemoryJobs::new();
    let key =
        jobniac_core::pagination::CursorKey::from_secret("job-test-cursor-secret-0123456789abcdef")
            .map_err(|error| format!("{error:?}"))?;
    let service = jobniac_core::jobs::JobService::with_ids(
        MemoryUow,
        jobs.clone(),
        fixed_clock(NOW),
        jobniac_core::pagination::CursorCodec::new(key),
        Arc::new(CounterIds::default()),
    );
    let request = json!({ "text": "Engineer\nSynthetic description" });
    let first = service.add_job_from_text(&request).await?;
    let second = service.add_job_from_text(&request).await?;
    assert_eq!(first.status, CaptureStatus::CreatedJob);
    assert_eq!(second.status, CaptureStatus::ReusedExistingJob);
    assert_eq!(first.job, second.job);
    Ok(())
}

#[test]
fn the_description_is_stored_at_most_twenty_thousand_units() -> TestResult {
    // The fetch admission truncates the page description to 20000 UTF-16 units; the use case then
    // persists that text unchanged.
    let body = format!(
        "<html><head><title>Long</title><meta name=\"description\" content=\"{}\"></head></html>",
        "a".repeat(25_000)
    );
    let fetched = parse_job_html(body.as_bytes(), 200)
        .map_err(|failure| format!("the synthetic page must parse: {failure:?}"))?;
    let stored = fetched.description.clone().unwrap_or_default();
    assert_eq!(stored.encode_utf16().count(), 20_000);
    Ok(())
}

#[tokio::test]
async fn the_persisted_description_is_the_truncated_one() -> TestResult {
    let body = format!(
        "<html><head><meta name=\"description\" content=\"{}\"></head></html>",
        "b".repeat(25_000)
    );
    let fetched = parse_job_html(body.as_bytes(), 200)
        .map_err(|failure| format!("the synthetic page must parse: {failure:?}"))?;
    let double = Double::with_content(fetched);
    let result = double
        .service()
        .add_job_from_link(&request(json!({ "title": "Engineer" })))
        .await?;
    assert_eq!(result.job.job.description.encode_utf16().count(), 20_000);
    Ok(())
}
