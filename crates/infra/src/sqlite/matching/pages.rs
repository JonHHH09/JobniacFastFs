//! Keyset reads of the matching adapter: the profile and one page of its jobs,
//! and the report history with its four filter shapes.
//!
//! The first page fixes a snapshot instant that later pages reuse. Each query binds the snapshot,
//! the cursor's created-at and identifier, and the limit plus one, in that order. The row-value
//! comparison `(created_at, id) < (?2, ?3)` is the keyset position.

use jobniac_core::matching::{JobGraph, MatchView, ProfileGraph};
use jobniac_core::pagination::{Page, PageRequest};
use rusqlite::params;
use uuid::Uuid;

use super::store::{
    RawJobRoot, RawView, attach_skills, load_profile, read_job_root, read_view, report_columns,
};
use super::value::{Failure, fetch_limit, run, select_page, snapshot_at};
use crate::sqlite::codec::{instant_text, uuid_text};
use crate::sqlite::unit_of_work::SqliteTransaction;

/// One bounded page of jobs, newest first, from one snapshot. Binds: snapshot, cursor created-at,
/// cursor id, fetch limit.
const JOB_PAGE_SQL: &str = "
SELECT job.id, job.updated_at, job.created_at, job.experience_requirement, job.seniority
FROM job_jobs AS job
WHERE job.created_at <= ?1
  AND (?2 IS NULL
       OR job.created_at < ?2
       OR (job.created_at = ?2 AND job.id > ?3))
ORDER BY job.created_at DESC, job.id ASC
LIMIT ?4
";

/// A report page with the current revisions of its sources. The filter fragment is spliced in at
/// the end of the `WHERE` clause; its binds follow the four fixed ones.
macro_rules! report_page_sql {
    ($filter:literal) => {
        concat!(
            "SELECT ",
            report_columns!("report"),
            ", profile.updated_at AS current_profile_revision,
       job.updated_at AS current_job_revision
FROM match_reports AS report
LEFT JOIN profile_profiles AS profile ON profile.id = report.profile_id
LEFT JOIN job_jobs AS job ON job.id = report.job_id
WHERE report.created_at <= ?1
  AND (?2 IS NULL OR (report.created_at, report.id) < (?2, ?3))",
            $filter,
            "
ORDER BY report.created_at DESC, report.id DESC
LIMIT ?4"
        )
    };
}

/// The unfiltered report page. Binds: snapshot, cursor created-at, cursor id, fetch limit.
const UNFILTERED_SQL: &str = report_page_sql!("");

/// The report page filtered by profile. Binds as [`UNFILTERED_SQL`], then the profile identifier.
const PROFILE_SQL: &str = report_page_sql!(" AND report.profile_id = ?5");

/// The report page filtered by job. Binds as [`UNFILTERED_SQL`], then the job identifier.
const JOB_HISTORY_SQL: &str = report_page_sql!(" AND report.job_id = ?5");

/// The report page filtered by profile and job. Binds as [`UNFILTERED_SQL`], then the profile and
/// job identifiers. The pair reads the composite `(profile_id, job_id, created_at, id)` index.
const PROFILE_JOB_SQL: &str =
    report_page_sql!(" AND report.profile_id = ?5 AND report.job_id = ?6");

/// The profile, and one page of its candidate jobs from one snapshot, with
/// their skills attached to the page items only.
pub(crate) fn load_batch(
    tx: &SqliteTransaction,
    profile_id: Uuid,
    request: &PageRequest<'_>,
) -> Result<(ProfileGraph, Page<JobGraph>), Failure> {
    let anchor = request.cursor();
    let snapshot = snapshot_at(request);
    let created = anchor.map(|cursor| instant_text(cursor.created_at));
    let anchor_id = anchor.map(|cursor| uuid_text(cursor.id));
    let fetch = fetch_limit(request)?;
    run(tx, |connection| {
        let profile = load_profile(connection, profile_id)?;
        let roots = connection
            .prepare_cached(JOB_PAGE_SQL)?
            .query_map(
                params![instant_text(snapshot), created, anchor_id, fetch],
                read_job_root,
            )?
            .collect::<rusqlite::Result<Vec<RawJobRoot>>>()?;
        let page = select_page(request, snapshot, roots, RawJobRoot::key, |root| {
            root.graph()
        })?;
        let mut graphs = page.items;
        attach_skills(connection, &mut graphs)?;
        Ok((
            profile,
            Page {
                items: graphs,
                next_cursor: page.next_cursor,
            },
        ))
    })
}

/// One page of stored reports, newest first, filtered by profile and by job
/// when given. The query shape follows the filters.
pub(crate) fn list_reports(
    tx: &SqliteTransaction,
    profile_id: Option<Uuid>,
    job_id: Option<Uuid>,
    request: &PageRequest<'_>,
) -> Result<Page<MatchView>, Failure> {
    let anchor = request.cursor();
    let snapshot = snapshot_at(request);
    let created = anchor.map(|cursor| instant_text(cursor.created_at));
    let anchor_id = anchor.map(|cursor| uuid_text(cursor.id));
    let fetch = fetch_limit(request)?;
    let snapshot_text = instant_text(snapshot);
    let rows = run(tx, |connection| {
        let (sql, extra): (&str, Vec<String>) = match (profile_id, job_id) {
            (Some(profile), Some(job)) => {
                (PROFILE_JOB_SQL, vec![uuid_text(profile), uuid_text(job)])
            }
            (Some(profile), None) => (PROFILE_SQL, vec![uuid_text(profile)]),
            (None, Some(job)) => (JOB_HISTORY_SQL, vec![uuid_text(job)]),
            (None, None) => (UNFILTERED_SQL, Vec::new()),
        };
        let mut binds: Vec<&dyn rusqlite::ToSql> =
            vec![&snapshot_text, &created, &anchor_id, &fetch];
        binds.extend(extra.iter().map(|value| value as &dyn rusqlite::ToSql));
        let rows = connection
            .prepare_cached(sql)?
            .query_map(binds.as_slice(), read_view)?
            .collect::<rusqlite::Result<Vec<RawView>>>()?;
        Ok(rows)
    })?;
    select_page(request, snapshot, rows, RawView::key, RawView::into_view)
}
