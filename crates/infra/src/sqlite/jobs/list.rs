//! Keyset listing of jobs.
//!
//! The first page fixes a created-at watermark (`snapshotAt`) that later pages reuse, so jobs created
//! after the first page do not shift the listing. The page is built by the core `keyset_page` from
//! `limit + 1` rows. Ingestion text is never selected.

use jobniac_core::errors::AppError;
use jobniac_core::jobs::JobRecord;
use jobniac_core::pagination::{Page, PageRequest, keyset_page};
use rusqlite::params;
use time::OffsetDateTime;

use super::codec::{instant_text, parse_instant_text, uuid_text};
use super::graph::read_job;
use super::{db, invalid_shape, run};
use crate::sqlite::unit_of_work::SqliteTransaction;

/// The job listing query. The binds are the snapshot, the anchor's created-at, its identifier and
/// the fetch limit, in that order. The anchor binds are NULL on the first page.
const LIST_JOBS_SQL: &str = r#"
SELECT job.id, job.source_method, job.source_label, job.title, job.company, job.location,
       job.description, job.experience_requirement, job.employment_type, job.seniority,
       job.posted_at, job.canonical_fingerprint, job.created_at, job.updated_at, job.revision
FROM job_jobs AS job
WHERE job.created_at <= ?1
  AND (
      ?2 IS NULL
      OR job.created_at < ?2
      OR (job.created_at = ?2 AND job.id > ?3)
  )
ORDER BY job.created_at DESC, job.id ASC
LIMIT ?4
"#;

/// Reads one bounded page of jobs for `request`, without ingestion text.
pub(crate) fn list_jobs(
    tx: &SqliteTransaction,
    request: &PageRequest<'_>,
) -> Result<Page<JobRecord>, AppError> {
    let anchor = request.cursor();
    // The first page's watermark is the current instant; later pages reuse the anchor's. It is
    // read back through its stored text, so a cursor carries the instant the query compared against.
    let snapshot_at = anchor
        .map(|cursor| cursor.snapshot_at)
        .unwrap_or_else(OffsetDateTime::now_utc);
    let snapshot_at = parse_instant_text(&instant_text(snapshot_at)).ok_or_else(invalid_shape)?;
    let anchor_created_at = anchor.map(|cursor| instant_text(cursor.created_at));
    let anchor_id = anchor.map(|cursor| uuid_text(cursor.id));
    let fetch = i64::try_from(request.limit())
        .map_err(|_| AppError::Internal)?
        .saturating_add(1);
    let rows = run(tx, |connection| {
        db(connection
            .prepare_cached(LIST_JOBS_SQL)
            .and_then(|mut statement| {
                statement
                    .query_map(
                        params![
                            instant_text(snapshot_at),
                            anchor_created_at,
                            anchor_id,
                            fetch
                        ],
                        read_job,
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()
            }))
    })?;
    let mut jobs = Vec::with_capacity(rows.len());
    for row in rows {
        jobs.push(row.into_record().map_err(|_| invalid_shape())?);
    }
    keyset_page(
        request,
        jobs,
        snapshot_at,
        |job| job.created_at,
        |job| job.id,
    )
}
