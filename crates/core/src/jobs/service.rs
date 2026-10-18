//! The job use cases: paste-first capture, get, keyset list and the
//! revision-checked update.
//!
//! Validation runs before any transaction opens. Each repository call runs in its own transaction
//! scope. Writes are compare-and-set on the job revision: a stale revision, a
//! revision at the maximum, or a lost race at write time is a `conflict` and nothing is written.

use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use crate::clock::Clock;
use crate::errors::{AppError, conflict, not_found, validation_error};
use crate::ids::admit_uuid;
use crate::pagination::CursorCodec;
use crate::profiles::repository::{READ_ONLY, READ_WRITE, UnitOfWork};

use super::capture::{assemble_text_job, capture_hash, prepare_text_job};
use super::model::{
    CaptureStatus, IdSource, JobAggregate, JobCaptureResult, JobListPage, RandomIds,
};
use super::repository::JobRepository;
use super::request::JOB_REQUEST_MESSAGE;
use super::update::{build_update, parse_update, planned_fingerprint};

/// The largest revision the aggregate accepts (`i64::MAX`).
const MAX_REVISION: i64 = i64::MAX;

/// The job use cases over a unit of work, a repository, a clock and a cursor codec.
#[derive(Debug)]
pub struct JobService<U, R> {
    uow: U,
    repo: R,
    clock: Arc<dyn Clock>,
    codec: CursorCodec,
    ids: Arc<dyn IdSource>,
}

impl<U, R> JobService<U, R>
where
    U: UnitOfWork,
    R: JobRepository<Tx = U::Tx>,
{
    /// A service that draws identifiers at random.
    pub fn new(uow: U, repo: R, clock: Arc<dyn Clock>, codec: CursorCodec) -> Self {
        Self::with_ids(uow, repo, clock, codec, Arc::new(RandomIds))
    }

    /// A service that draws identifiers from `ids` (tests substitute a counter).
    pub fn with_ids(
        uow: U,
        repo: R,
        clock: Arc<dyn Clock>,
        codec: CursorCodec,
        ids: Arc<dyn IdSource>,
    ) -> Self {
        Self {
            uow,
            repo,
            clock,
            codec,
            ids,
        }
    }

    /// One bounded page of job rows, scoped to the `jobs` cursor scope and the `all` filter.
    ///
    /// # Errors
    ///
    /// The cursor error for a malformed, tampered or foreign cursor, the limit error for a limit
    /// outside 1 to 100 or a boolean, or a repository error.
    pub async fn list_jobs(
        &self,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<JobListPage, AppError> {
        let request = self.codec.page_request(limit, cursor, "jobs", "all")?;
        let page = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo.list_jobs(tx, &request).await
            })
            .await?;
        Ok(JobListPage {
            jobs: page.items,
            next_cursor: page.next_cursor,
        })
    }

    /// The complete aggregate of a job, by canonical UUID text.
    ///
    /// # Errors
    ///
    /// `validation_error` (field `jobId`) for malformed text, `not_found` for a missing job.
    pub async fn get_job(&self, job_id: &str) -> Result<JobAggregate, AppError> {
        let parsed = admit_uuid(job_id)
            .map_err(|refusal| refusal.validation(JOB_REQUEST_MESSAGE, "jobId"))?;
        self.read(parsed).await
    }

    /// Captures pasted text as a job, or reuses the job that the text hash or the canonical
    /// fingerprint identifies. A reuse returns the existing aggregate and writes nothing.
    ///
    /// # Errors
    ///
    /// The validation error of the first invalid field, after the duplicate lookup, or a repository
    /// error.
    pub async fn add_job_from_text(&self, request: &Value) -> Result<JobCaptureResult, AppError> {
        let capture = capture_hash(request)?;
        let hash = capture.input_text_hash.clone();
        let existing = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo.find_by_text_hash(tx, &hash).await
            })
            .await?;
        if let Some(job) = existing {
            return Ok(JobCaptureResult {
                status: CaptureStatus::ReusedExistingJob,
                job,
            });
        }
        let fields = prepare_text_job(request, &capture)?;
        let now = self.clock.now();
        let graph = assemble_text_job(fields, now, self.ids.as_ref());
        let (job, created) = self
            .uow
            .transaction(READ_WRITE, async |tx| {
                self.repo.create_text_job(tx, &graph).await
            })
            .await?;
        Ok(JobCaptureResult {
            status: if created {
                CaptureStatus::CreatedJob
            } else {
                CaptureStatus::ReusedExistingJob
            },
            job,
        })
    }

    /// Patches the editable fields of a job under the revision check. See [`super::update`] for the
    /// clearing rules and the order of the checks.
    ///
    /// # Errors
    ///
    /// `Invalid job request` for an invalid request (before any repository call), `not_found` for a
    /// missing job, `conflict` for a stale or maximal revision or a lost race, or `validation_error`
    /// (field `canonicalFingerprint`) when another job has the same fingerprint.
    pub async fn update_job(&self, request: &Value) -> Result<JobAggregate, AppError> {
        let update = parse_update(request)?;
        let job_id = update.job_id;
        let expected = update.expected_revision;
        let existing = self.read(job_id).await?;
        if existing.job.revision != expected {
            return Err(job_update_conflict(job_id, expected));
        }
        let fingerprint = planned_fingerprint(&existing, &update);
        let duplicate = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo
                    .find_by_canonical_fingerprint(tx, &fingerprint)
                    .await
            })
            .await?;
        if let Some(duplicate) = duplicate
            && duplicate.job.id != job_id
        {
            let reason = format!("duplicates existing job {}", duplicate.job.id);
            return Err(AppError::from(validation_error(
                JOB_REQUEST_MESSAGE,
                Some("canonicalFingerprint"),
                Some(reason.as_str()),
            )));
        }
        if expected == MAX_REVISION {
            return Err(job_update_conflict(job_id, expected));
        }
        let now = self.clock.now();
        let graph = build_update(&existing, &update, &fingerprint, now, self.ids.as_ref())?;
        let saved = self
            .uow
            .transaction(READ_WRITE, async |tx| {
                self.repo.update_job_aggregate(tx, &graph, expected).await
            })
            .await?;
        saved.ok_or_else(|| job_update_conflict(job_id, expected))
    }

    /// Reads the aggregate in a read-only transaction, or reports the missing job.
    async fn read(&self, job_id: Uuid) -> Result<JobAggregate, AppError> {
        let found = self
            .uow
            .transaction(READ_ONLY, async |tx| self.repo.get_job(tx, job_id).await)
            .await?;
        found.ok_or_else(|| job_not_found(job_id))
    }
}

/// `Job not found: <id>` with the `resource` and `jobId` details.
pub(crate) fn job_not_found(job_id: Uuid) -> AppError {
    AppError::from(not_found(
        &format!("Job not found: {job_id}"),
        [
            ("resource", "job".to_owned()),
            ("jobId", job_id.to_string()),
        ],
    ))
}

/// `Job revision conflict` with the `resource`, `jobId` and `expectedRevision` details.
pub(crate) fn job_update_conflict(job_id: Uuid, expected_revision: i64) -> AppError {
    AppError::from(conflict(
        "Job revision conflict",
        [
            ("resource", "job".to_owned()),
            ("jobId", job_id.to_string()),
            ("expectedRevision", expected_revision.to_string()),
        ],
    ))
}
