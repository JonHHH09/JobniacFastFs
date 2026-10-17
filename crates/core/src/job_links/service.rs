//! Link ingestion use case (`add_job_from_link`).
//!
//! The order is: the request is a mapping, `url` is a non-blank string, the display and canonical
//! forms are derived (their errors never echo the URL), and the canonical URL is looked up before anything else. A reuse returns the stored aggregate before any optional field is
//! validated and before any fetch. Otherwise the optional fields are validated, the page is fetched
//! through [`JobLinkPageFetcher`], the fetched content is admitted, and the new aggregate is written.
//!
//! The fetch port is asynchronous so that the blocking child call can run off the async runtime (the
//! parent side of the worker refuses to run inside a runtime). The core crate has no runtime
//! dependency: the composition root wraps the blocking fetcher.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_text::java::java_strip;
use serde_json::Value;

use crate::clock::Clock;
use crate::errors::{AppError, validation_error};
use crate::jobs::model::{
    CaptureStatus, IdSource, JobAggregate, JobCaptureResult, JobRecord, JobSkillRecord,
    LinkIngestionRecord, RandomIds,
};
use crate::jobs::request::{explicit_skills, instant_field, job_invalid, mapping, optional_text};
use crate::jobs::text::{
    extract_experience, extract_skills, fingerprint, merge_skills, normalized_key,
};
use crate::profiles::repository::{READ_ONLY, READ_WRITE, UnitOfWork};

use super::urls::{canonical_source_url, safe_display_url};
use super::{JobLinkFetchFailure, JobLinkFetchResult};

/// Lowercase markers of blocked or security-check page text.
const BLOCKED_MARKERS: [&str; 9] = [
    "security check",
    "additional verification required",
    "request blocked",
    "blocked - indeed",
    "cloudflare",
    "enable javascript",
    "javascript and cookies",
    "javascript is required",
    "app shell",
];

/// The asynchronous fetch port of the link use case.
///
/// The implementation runs the fetch off the async runtime and returns only the classifications of
/// [`JobLinkFetchFailure`]; no retrieval URL, header or body text reaches an error.
#[async_trait]
pub trait JobLinkPageFetcher: Send + Sync {
    /// Fetches and extracts one job page for an already validated retrieval URL.
    async fn fetch(&self, retrieval_url: &str) -> Result<JobLinkFetchResult, JobLinkFetchFailure>;
}

/// Persistence port of the link use case.
#[async_trait]
pub trait JobLinkRepository: Send + Sync {
    /// The transaction handle of the adapter.
    type Tx: Send;

    /// The aggregate whose link provenance has `normalized_url`, or `None`.
    async fn find_by_normalized_url(
        &self,
        tx: &mut Self::Tx,
        normalized_url: &str,
    ) -> Result<Option<JobAggregate>, AppError>;

    /// Writes a new link job. Returns the stored aggregate and `true`, or, when the canonical URL or
    /// the fingerprint already exists, that job and `false` (nothing is written).
    async fn create_link_job(
        &self,
        tx: &mut Self::Tx,
        graph: &JobAggregate,
    ) -> Result<(JobAggregate, bool), AppError>;
}

/// The link ingestion use case over a unit of work, a repository, a fetch port and a clock.
pub struct JobLinkService<U, R> {
    uow: U,
    repo: R,
    fetcher: Arc<dyn JobLinkPageFetcher>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdSource>,
}

impl<U, R> JobLinkService<U, R>
where
    U: UnitOfWork,
    R: JobLinkRepository<Tx = U::Tx>,
{
    /// A service that draws identifiers at random.
    pub fn new(
        uow: U,
        repo: R,
        fetcher: Arc<dyn JobLinkPageFetcher>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self::with_ids(uow, repo, fetcher, clock, Arc::new(RandomIds))
    }

    /// A service that draws identifiers from `ids` (tests substitute a counter).
    pub fn with_ids(
        uow: U,
        repo: R,
        fetcher: Arc<dyn JobLinkPageFetcher>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdSource>,
    ) -> Self {
        Self {
            uow,
            repo,
            fetcher,
            clock,
            ids,
        }
    }

    /// Ingests the job page at `url`, or reuses the job that the canonical URL identifies.
    ///
    /// # Errors
    ///
    /// `Invalid job request` for a request that is not an object or a blank `url`; the URL
    /// validation errors of [`canonical_source_url`] (which never echo the URL); the validation
    /// error of the first invalid optional field; `Invalid job ingestion request` (field `url`) for a
    /// classified fetch failure; `Invalid job request` (field `fetchedContent`) for content that the
    /// page admission refuses; or a repository error.
    pub async fn add_job_from_link(&self, request: &Value) -> Result<JobCaptureResult, AppError> {
        let object = mapping(request)?;
        let url = match object.get("url") {
            Some(Value::String(url)) if !java_strip(url).is_empty() => java_strip(url).to_owned(),
            _ => return Err(job_invalid("url", "must not be blank")),
        };
        let display_url = safe_display_url(&url).map_err(AppError::from)?;
        let normalized_url = canonical_source_url(&url).map_err(AppError::from)?;

        let lookup = normalized_url.clone();
        let existing = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo.find_by_normalized_url(tx, &lookup).await
            })
            .await?;
        if let Some(job) = existing {
            return Ok(JobCaptureResult {
                status: CaptureStatus::ReusedExistingJob,
                job,
            });
        }

        let source_label = optional_text(object, "sourceLabel")?;
        let title_override = optional_text(object, "title")?;
        let company = optional_text(object, "company")?;
        let location = optional_text(object, "location")?;
        let explicit_description = optional_text(object, "description")?;
        let experience_override = optional_text(object, "experienceRequirement")?;
        let employment_type = optional_text(object, "employmentType")?;
        let seniority = optional_text(object, "seniority")?;
        let explicit = explicit_skills(object)?;
        let posted_at = instant_field(object.get("postedAt"), "postedAt")?;

        let fetched = self.fetch_content(&url).await?;
        let source_title = stripped_or_none(fetched.title.as_deref());
        let fetched_description = stripped_or_none(fetched.description.as_deref());
        if explicit_description.is_none() {
            validate_content(
                source_title.as_deref(),
                fetched_description.as_deref(),
                fetched.http_status,
            )?;
        }
        let title = title_override
            .or_else(|| source_title.clone())
            .unwrap_or_else(|| normalized_url.clone());
        let description = explicit_description
            .or(fetched_description)
            .unwrap_or_else(|| title.clone());
        let experience = experience_override.or_else(|| extract_experience(&description));
        let skills = merge_skills(&explicit, &extract_skills(&description));
        let canonical = fingerprint(
            &title,
            company.as_deref(),
            location.as_deref(),
            &description,
            Some(&normalized_url),
        );

        let now = self.clock.now();
        let job_id = self.ids.next_id();
        let skill_rows = skills
            .iter()
            .enumerate()
            .map(|(index, skill)| JobSkillRecord {
                id: self.ids.next_id(),
                job_id,
                skill: skill.clone(),
                normalized_skill: normalized_key(Some(skill)),
                required: true,
                display_order: i64::try_from(index).unwrap_or(i64::MAX),
                created_at: now,
            })
            .collect();
        let link = LinkIngestionRecord {
            id: self.ids.next_id(),
            job_id,
            normalized_url: normalized_url.clone(),
            fields: link_fields(&display_url, now, fetched.http_status, source_title),
        };
        let job = JobRecord {
            id: job_id,
            source_method: "link".to_owned(),
            source_label,
            title,
            company,
            location,
            description,
            experience_requirement: experience,
            employment_type,
            seniority,
            posted_at,
            canonical_fingerprint: canonical,
            created_at: now,
            updated_at: now,
            revision: 0,
        };
        let graph = JobAggregate {
            job,
            skills: skill_rows,
            link_ingestion: Some(link),
            text_ingestion: None,
        };
        let (saved, created) = self
            .uow
            .transaction(READ_WRITE, async |tx| {
                self.repo.create_link_job(tx, &graph).await
            })
            .await?;
        Ok(JobCaptureResult {
            status: if created {
                CaptureStatus::CreatedJob
            } else {
                CaptureStatus::ReusedExistingJob
            },
            job: saved,
        })
    }

    /// Runs the fetch port and admits its result.
    async fn fetch_content(&self, retrieval_url: &str) -> Result<JobLinkFetchResult, AppError> {
        let fetched = self.fetcher.fetch(retrieval_url).await.map_err(|failure| {
            AppError::from(validation_error(
                "Invalid job link ingestion request",
                Some("url"),
                Some(failure.message()),
            ))
        })?;
        if !(100..=599).contains(&fetched.http_status) {
            return Err(job_invalid(
                "fetchedContent",
                "job page fetch returned invalid content",
            ));
        }
        Ok(fetched)
    }
}

impl<U, R> std::fmt::Debug for JobLinkService<U, R> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JobLinkService")
            .finish_non_exhaustive()
    }
}

/// The stripped text, or `None` when absent or blank.
fn stripped_or_none(value: Option<&str>) -> Option<String> {
    value
        .map(java_strip)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// An HTTP error status, or blocked or security-check text, is refused
/// unless the caller supplied an explicit description.
fn validate_content(
    title: Option<&str>,
    description: Option<&str>,
    http_status: i64,
) -> Result<(), AppError> {
    if http_status >= 400 {
        return Err(job_invalid(
            "fetchedContent",
            &format!(
                "job page fetch returned HTTP {http_status}; \
                 provide pasted job text or a usable description"
            ),
        ));
    }
    let combined = format!("{} {}", title.unwrap_or(""), description.unwrap_or("")).to_lowercase();
    if BLOCKED_MARKERS
        .iter()
        .any(|marker| combined.contains(marker))
    {
        return Err(job_invalid(
            "fetchedContent",
            "job page fetch returned blocked/security-check content; \
             provide pasted job text or a usable description",
        ));
    }
    Ok(())
}

/// The provenance columns that the link row keeps in `fields` under their wire keys (the same
/// layout the PostgreSQL adapter reads back).
fn link_fields(
    display_url: &str,
    now: time::OffsetDateTime,
    http_status: i64,
    source_title: Option<String>,
) -> serde_json::Map<String, Value> {
    let mut fields = serde_json::Map::new();
    fields.insert("url".to_owned(), Value::String(display_url.to_owned()));
    fields.insert(
        "fetchedAt".to_owned(),
        Value::String(crate::clock::format_instant(now)),
    );
    fields.insert("httpStatus".to_owned(), Value::from(http_status));
    fields.insert(
        "sourceTitle".to_owned(),
        source_title.map_or(Value::Null, Value::String),
    );
    fields.insert(
        "createdAt".to_owned(),
        Value::String(crate::clock::format_instant(now)),
    );
    fields
}
