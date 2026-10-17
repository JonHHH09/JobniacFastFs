//! Trusted Arbeitnow candidate import.
//!
//! A signed candidate token is authenticated by the owned codec before anything else; its already
//! sanitized fields become a `link` job whose provenance is the candidate's canonical URL. The
//! service has no content-retrieval dependency by construction: it holds a codec, a repository
//! port, a clock and an identifier source, and nothing else. No key, clock or runtime wiring lives
//! here.
//!
//! A `postedAt` instant outside the years 0001 to 9999 is refused with the candidate-token message,
//! after the lookup by
//! URL and before the write.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_text::java::java_strip;
use serde_json::{Map, Value};
use time::{Duration, OffsetDateTime};

use crate::arbeitnow::candidates::{ArbeitnowCandidate, CandidateCodec, INVALID_MESSAGE};
use crate::arbeitnow::instant::ArbeitnowInstant;
use crate::clock::Clock;
use crate::errors::{ApplicationError, validation_error};
use crate::job_links::urls::canonical_source_url;
use crate::jobs::model::{
    IdSource, JobAggregate, JobRecord, JobSkillRecord, LinkIngestionRecord, RandomIds,
};
use crate::jobs::text::{fingerprint, merge_skills, normalized_key, split_skill_list};

/// Epoch second of `0001-01-01T00:00:00Z`, the smallest accepted posting instant.
const MIN_POSTED_SECOND: i64 = -62_135_596_800;
/// Epoch second of `9999-12-31T23:59:59Z`, the largest accepted posting instant.
const MAX_POSTED_SECOND: i64 = 253_402_300_799;

/// Persistence port of the trusted import.
#[async_trait]
pub trait TrustedImportRepository: Send + Sync {
    /// The aggregate whose link provenance has `url`, or `None`.
    async fn find_by_normalized_url(
        &self,
        url: &str,
    ) -> Result<Option<JobAggregate>, ApplicationError>;

    /// Atomically reuses by URL or fingerprint, else inserts. `true` means inserted.
    async fn create_link_job(
        &self,
        graph: JobAggregate,
    ) -> Result<(JobAggregate, bool), ApplicationError>;
}

/// The request of the import: the candidate token (absent and blank are the same refusal).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportRequest {
    /// The signed candidate token.
    pub candidate_token: Option<String>,
}

/// Whether the import wrote a job or found one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportStatus {
    /// A new job was written.
    CreatedJob,
    /// The URL or the fingerprint matched a stored job; nothing was written.
    ReusedExistingJob,
}

impl ImportStatus {
    /// The wire status (`created_job`, `reused_existing_job`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CreatedJob => "created_job",
            Self::ReusedExistingJob => "reused_existing_job",
        }
    }
}

/// The result of an import: a status and the job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOutcome {
    /// Created or reused.
    pub status: ImportStatus,
    /// The stored aggregate.
    pub job: JobAggregate,
}

/// Turns a verified candidate token into a stored `link` job.
#[derive(Debug)]
pub struct ArbeitnowJobImportService<R> {
    codec: CandidateCodec,
    repository: R,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdSource>,
}

impl<R: TrustedImportRepository> ArbeitnowJobImportService<R> {
    /// A service that draws identifiers at random.
    pub fn new(codec: CandidateCodec, repository: R, clock: Arc<dyn Clock>) -> Self {
        Self::with_ids(codec, repository, clock, Arc::new(RandomIds))
    }

    /// A service that draws identifiers from `ids` (tests substitute a counter).
    pub fn with_ids(
        codec: CandidateCodec,
        repository: R,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdSource>,
    ) -> Self {
        Self {
            codec,
            repository,
            clock,
            ids,
        }
    }

    /// Verifies the token, then reuses the job of the canonical URL or creates a new one.
    ///
    /// # Errors
    ///
    /// `candidateToken is required` for a missing or blank token; every verification failure of the
    /// codec unchanged; `Invalid Arbeitnow candidate token` for a `postedAt` outside years 1 to
    /// 9999;
    /// the repository errors unchanged. A refused token never reaches the
    /// repository.
    pub async fn import(&self, request: ImportRequest) -> Result<ImportOutcome, ApplicationError> {
        let token = match request.candidate_token.as_deref() {
            Some(token) if !java_strip(token).is_empty() => token,
            _ => {
                return Err(validation_error(
                    "candidateToken is required",
                    Some("candidateToken"),
                    Some("must not be blank"),
                ));
            }
        };
        let candidate = self.codec.verify(token)?;
        self.import_candidate(&candidate).await
    }

    async fn import_candidate(
        &self,
        candidate: &ArbeitnowCandidate,
    ) -> Result<ImportOutcome, ApplicationError> {
        let normalized_url = canonical_source_url(&candidate.canonical_url)?;
        if let Some(existing) = self
            .repository
            .find_by_normalized_url(&normalized_url)
            .await?
        {
            return Ok(ImportOutcome {
                status: ImportStatus::ReusedExistingJob,
                job: existing,
            });
        }

        let title = java_strip(&candidate.title).to_owned();
        let company = clean_to_none(Some(&candidate.company));
        let location = clean_to_none(Some(&candidate.location));
        let description = java_strip(&candidate.description_excerpt).to_owned();
        let employment_type = clean_to_none(candidate.job_types.first().map(String::as_str));
        let tags: Vec<String> = candidate
            .tags
            .iter()
            .flat_map(|tag| split_skill_list(tag))
            .collect();
        let skills = merge_skills(&tags, &[]);
        let remote = candidate.remote;
        let now = self.clock.now();
        let job_id = self.ids.next_id();
        let posted_at = posted_at(candidate.posted_at)?;
        let skill_rows: Vec<JobSkillRecord> = skills
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
            fields: link_fields(&normalized_url, now, remote),
        };
        let job = JobRecord {
            id: job_id,
            source_method: "link".to_owned(),
            source_label: Some("arbeitnow".to_owned()),
            canonical_fingerprint: fingerprint(
                &title,
                company.as_deref(),
                location.as_deref(),
                &description,
                Some(&normalized_url),
            ),
            title,
            company,
            location,
            description,
            experience_requirement: None,
            employment_type,
            seniority: None,
            posted_at,
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
        let (saved, created) = self.repository.create_link_job(graph).await?;
        Ok(ImportOutcome {
            status: if created {
                ImportStatus::CreatedJob
            } else {
                ImportStatus::ReusedExistingJob
            },
            job: saved,
        })
    }
}

/// Java strip; blank becomes `None`.
fn clean_to_none(value: Option<&str>) -> Option<String> {
    let stripped = java_strip(value?);
    (!stripped.is_empty()).then(|| stripped.to_owned())
}

/// The instant truncated to microseconds, refused outside years 1 to 9999.
fn posted_at(
    instant: Option<ArbeitnowInstant>,
) -> Result<Option<OffsetDateTime>, ApplicationError> {
    let Some(instant) = instant else {
        return Ok(None);
    };
    let refused = || validation_error(INVALID_MESSAGE, None, None);
    let second = instant.epoch_second();
    if !(MIN_POSTED_SECOND..=MAX_POSTED_SECOND).contains(&second) {
        return Err(refused());
    }
    let base = OffsetDateTime::from_unix_timestamp(second).map_err(|_| refused())?;
    Ok(Some(
        base + Duration::microseconds(i64::from(instant.nanosecond() / 1000)),
    ))
}

/// The provenance columns of the link row.
fn link_fields(url: &str, now: OffsetDateTime, remote: bool) -> Map<String, Value> {
    let instant = crate::clock::format_instant(now);
    let mut fields = Map::new();
    fields.insert("url".to_owned(), Value::String(url.to_owned()));
    fields.insert("fetchedAt".to_owned(), Value::String(instant.clone()));
    fields.insert("httpStatus".to_owned(), Value::Null);
    fields.insert(
        "sourceTitle".to_owned(),
        Value::String(format!("Arbeitnow scan candidate (remote={remote})")),
    );
    fields.insert("createdAt".to_owned(), Value::String(instant));
    fields
}
