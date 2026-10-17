//! Keyset listing of jobs.
//!
//! The first page fixes a created-at watermark (`snapshotAt`) that later pages reuse, so jobs created
//! after the first page do not shift the listing. The page is built by the core `keyset_page` from
//! `limit + 1` rows. Ingestion text is never selected.

use jobniac_core::errors::AppError;
use jobniac_core::jobs::JobRecord;
use jobniac_core::pagination::{Page, PageRequest, keyset_page};
use time::OffsetDateTime;
use tokio_postgres::Client;

use super::graph::job_from_row;
use super::{invalid_shape, job_col};
use crate::postgres::profiles::db;

/// The job listing query. The six binds are the anchor's snapshot, its created-at (three times),
/// its identifier and the fetch limit, in that order.
const LIST_JOBS_SQL: &str = r#"
WITH bounds AS (
    SELECT COALESCE($1::timestamptz, CURRENT_TIMESTAMP) AS snapshot_at
)
SELECT job.id, job.source_method AS "sourceMethod", job.source_label AS "sourceLabel",
       job.title, job.company, job.location, job.description,
       job.experience_requirement AS "experienceRequirement",
       job.employment_type AS "employmentType", job.seniority,
       job.posted_at AS "postedAt", job.canonical_fingerprint AS "canonicalFingerprint",
       job.created_at AS "createdAt", job.updated_at AS "updatedAt", job.revision,
       bounds.snapshot_at AS "snapshotAt"
FROM job_schema.jobs AS job
CROSS JOIN bounds
WHERE job.created_at <= bounds.snapshot_at
  AND (
      $2::timestamptz IS NULL
      OR job.created_at < $3::timestamptz
      OR (job.created_at = $4::timestamptz AND job.id > $5::uuid)
  )
ORDER BY job.created_at DESC, job.id ASC
LIMIT $6
"#;

/// Reads one bounded page of jobs for `request`, without ingestion text.
pub(crate) async fn list_jobs(
    client: &Client,
    request: &PageRequest<'_>,
) -> Result<Page<JobRecord>, AppError> {
    let anchor = request.cursor();
    let snapshot = anchor.map(|cursor| cursor.snapshot_at);
    let anchor_created_at = anchor.map(|cursor| cursor.created_at);
    let anchor_id = anchor.map(|cursor| cursor.id);
    let fetch = i64::try_from(request.limit())
        .map_err(|_| AppError::Internal)?
        .saturating_add(1);
    let rows = db(client
        .query(
            LIST_JOBS_SQL,
            &[
                &snapshot,
                &anchor_created_at,
                &anchor_created_at,
                &anchor_created_at,
                &anchor_id,
                &fetch,
            ],
        )
        .await)?;
    let mut jobs = Vec::with_capacity(rows.len());
    let mut snapshot_at = None;
    for row in &rows {
        if snapshot_at.is_none() {
            snapshot_at = Some(job_col::<OffsetDateTime>(row, "snapshotAt")?);
        }
        jobs.push(job_from_row(row).map_err(|_| invalid_shape())?);
    }
    // The first row's watermark, else the anchor's, else the current instant. Without rows
    // and without an anchor no cursor is issued, so the last fallback is never signed.
    let snapshot_at = snapshot_at
        .or(snapshot)
        .unwrap_or_else(OffsetDateTime::now_utc);
    keyset_page(
        request,
        jobs,
        snapshot_at,
        |job| job.created_at,
        |job| job.id,
    )
}
