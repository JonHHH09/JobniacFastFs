//! Keyset reads of the matching adapter: the profile and one page of its jobs,
//! and the report history with its four filter shapes.
//!
//! The first page fixes a snapshot instant that later pages reuse. Each query binds the cursor's
//! snapshot, created-at and identifier, and the limit plus one, in that order.

use jobniac_core::matching::{JobGraph, MatchView, ProfileGraph};
use jobniac_core::pagination::{Page, PageRequest, keyset_page};
use time::OffsetDateTime;
use tokio_postgres::Client;
use uuid::Uuid;

use super::store::{attach_skills, job_graph, load_profile, view_from_row};
use super::value::{Failure, column, fetch_limit, select_page};

/// One bounded page of jobs, newest first, from one snapshot.
const JOB_PAGE_SQL: &str = "
WITH bounds AS (SELECT COALESCE($1::timestamptz, CURRENT_TIMESTAMP) AS snapshot_at)
SELECT job.id, job.updated_at, job.created_at, job.experience_requirement, job.seniority,
       bounds.snapshot_at
FROM job_schema.jobs AS job
CROSS JOIN bounds
WHERE job.created_at <= bounds.snapshot_at
  AND ($2::timestamptz IS NULL
       OR job.created_at < $3::timestamptz
       OR (job.created_at = $4::timestamptz AND job.id > $5::uuid))
ORDER BY job.created_at DESC, job.id ASC
LIMIT $6
";

/// The unfiltered report page. Binds: snapshot, cursor created-at, cursor id, fetch limit.
const UNFILTERED_SQL: &str = "
WITH bounds AS (
    SELECT COALESCE(CAST($1 AS timestamptz), CURRENT_TIMESTAMP) AS snapshot_at
), page AS MATERIALIZED (
    SELECT report.*, bounds.snapshot_at
    FROM match.reports report
    CROSS JOIN bounds
    WHERE report.created_at <= bounds.snapshot_at
      AND (CAST($2 AS timestamptz) IS NULL
           OR (report.created_at, report.id) < ($2, $3))
    ORDER BY report.created_at DESC, report.id DESC
    LIMIT $4
)
SELECT page.*, profile.updated_at AS current_profile_revision,
       job.updated_at AS current_job_revision
FROM page
LEFT JOIN profile.profiles profile ON profile.id = page.profile_id
LEFT JOIN job_schema.jobs job ON job.id = page.job_id
ORDER BY page.created_at DESC, page.id DESC
";

/// The report page filtered by profile. Binds as [`UNFILTERED_SQL`], then the profile identifier.
const PROFILE_SQL: &str = "
WITH bounds AS (
    SELECT COALESCE(CAST($1 AS timestamptz), CURRENT_TIMESTAMP) AS snapshot_at
), page AS MATERIALIZED (
    SELECT report.*, bounds.snapshot_at
    FROM match.reports report
    CROSS JOIN bounds
    WHERE report.profile_id = $5
      AND (report.profile_id, report.created_at, report.id)
          <= ($5, bounds.snapshot_at,
              CAST('ffffffff-ffff-ffff-ffff-ffffffffffff' AS uuid))
      AND (CAST($2 AS timestamptz) IS NULL
           OR (report.created_at, report.id) < ($2, $3))
    ORDER BY report.profile_id, report.created_at DESC, report.id DESC
    LIMIT $4
)
SELECT page.*, profile.updated_at AS current_profile_revision,
       job.updated_at AS current_job_revision
FROM page
LEFT JOIN profile.profiles profile ON profile.id = page.profile_id
LEFT JOIN job_schema.jobs job ON job.id = page.job_id
ORDER BY page.created_at DESC, page.id DESC
";

/// The report page filtered by job. Binds as [`UNFILTERED_SQL`], then the job identifier.
const JOB_HISTORY_SQL: &str = "
WITH bounds AS (
    SELECT COALESCE(CAST($1 AS timestamptz), CURRENT_TIMESTAMP) AS snapshot_at
), page AS MATERIALIZED (
    SELECT report.*, bounds.snapshot_at
    FROM match.reports report
    CROSS JOIN bounds
    WHERE report.job_id = $5
      AND (report.job_id, report.created_at, report.id)
          <= ($5, bounds.snapshot_at,
              CAST('ffffffff-ffff-ffff-ffff-ffffffffffff' AS uuid))
      AND (CAST($2 AS timestamptz) IS NULL
           OR (report.created_at, report.id) < ($2, $3))
    ORDER BY report.job_id, report.created_at DESC, report.id DESC
    LIMIT $4
)
SELECT page.*, profile.updated_at AS current_profile_revision,
       job.updated_at AS current_job_revision
FROM page
LEFT JOIN profile.profiles profile ON profile.id = page.profile_id
LEFT JOIN job_schema.jobs job ON job.id = page.job_id
ORDER BY page.created_at DESC, page.id DESC
";

/// The report page filtered by profile and job. Binds as [`UNFILTERED_SQL`], then the profile and job
/// identifiers. The keys of the pair come from the composite index, then the rows are joined back.
const PROFILE_JOB_SQL: &str = "
WITH bounds AS (
    SELECT COALESCE(CAST($1 AS timestamptz), CURRENT_TIMESTAMP) AS snapshot_at
), keys AS MATERIALIZED (
    SELECT report.id, report.created_at, bounds.snapshot_at
    FROM match.reports report
    CROSS JOIN bounds
    WHERE report.profile_id = $5
      AND report.job_id = $6
      AND (ARRAY[report.profile_id, report.job_id], report.created_at, report.id)
          >= (ARRAY[CAST($5 AS uuid), CAST($6 AS uuid)],
              CAST('-infinity' AS timestamptz),
              CAST('00000000-0000-0000-0000-000000000000' AS uuid))
      AND (ARRAY[report.profile_id, report.job_id], report.created_at, report.id)
          < (ARRAY[CAST($5 AS uuid), CAST($6 AS uuid)],
             COALESCE(CAST($2 AS timestamptz),
                      bounds.snapshot_at + INTERVAL '1 microsecond'),
             COALESCE(CAST($3 AS uuid),
                      CAST('00000000-0000-0000-0000-000000000000' AS uuid)))
    ORDER BY ARRAY[report.profile_id, report.job_id], report.created_at DESC, report.id DESC
    LIMIT $4
), page AS MATERIALIZED (
    SELECT report.*, keys.snapshot_at
    FROM keys
    JOIN match.reports report ON report.id = keys.id
)
SELECT page.*, profile.updated_at AS current_profile_revision,
       job.updated_at AS current_job_revision
FROM page
LEFT JOIN profile.profiles profile ON profile.id = page.profile_id
LEFT JOIN job_schema.jobs job ON job.id = page.job_id
ORDER BY page.created_at DESC, page.id DESC
";

/// A job of a batch page with the fields the keyset policy reads.
struct SnapshotJob {
    graph: JobGraph,
    created_at: OffsetDateTime,
    id: Uuid,
}

/// The profile, and one page of its candidate jobs from one snapshot, with
/// their skills attached to the page items only.
pub(crate) async fn load_batch(
    client: &Client,
    profile_id: Uuid,
    request: &PageRequest<'_>,
) -> Result<(ProfileGraph, Page<JobGraph>), Failure> {
    let profile = load_profile(client, profile_id).await?;
    let anchor = request.cursor();
    let snapshot = anchor.map(|cursor| cursor.snapshot_at);
    let created = anchor.map(|cursor| cursor.created_at);
    let anchor_id = anchor.map(|cursor| cursor.id);
    let fetch = fetch_limit(request)?;
    let rows = client
        .query(
            JOB_PAGE_SQL,
            &[&snapshot, &created, &created, &created, &anchor_id, &fetch],
        )
        .await?;
    // The first row's snapshot, else the anchor's, else the current instant.
    let snapshot_at = match rows.first() {
        Some(row) => column::<OffsetDateTime>(row, "snapshot_at")?,
        None => snapshot.unwrap_or_else(OffsetDateTime::now_utc),
    };
    let mut snapshots = Vec::with_capacity(rows.len());
    for row in &rows {
        snapshots.push(SnapshotJob {
            graph: job_graph(row)?,
            created_at: column(row, "created_at")?,
            id: column(row, "id")?,
        });
    }
    let page = keyset_page(
        request,
        snapshots,
        snapshot_at,
        |job| job.created_at,
        |job| job.id,
    )?;
    let mut graphs: Vec<JobGraph> = page.items.into_iter().map(|job| job.graph).collect();
    attach_skills(client, &mut graphs).await?;
    Ok((
        profile,
        Page {
            items: graphs,
            next_cursor: page.next_cursor,
        },
    ))
}

/// One page of stored reports, newest first, filtered by profile and by job
/// when given. The query shape follows the filters.
pub(crate) async fn list_reports(
    client: &Client,
    profile_id: Option<Uuid>,
    job_id: Option<Uuid>,
    request: &PageRequest<'_>,
) -> Result<Page<MatchView>, Failure> {
    let anchor = request.cursor();
    let snapshot = anchor.map(|cursor| cursor.snapshot_at);
    let created = anchor.map(|cursor| cursor.created_at);
    let anchor_id = anchor.map(|cursor| cursor.id);
    let fetch = fetch_limit(request)?;
    let rows = match (profile_id, job_id) {
        (Some(profile), Some(job)) => {
            client
                .query(
                    PROFILE_JOB_SQL,
                    &[&snapshot, &created, &anchor_id, &fetch, &profile, &job],
                )
                .await?
        }
        (Some(profile), None) => {
            client
                .query(
                    PROFILE_SQL,
                    &[&snapshot, &created, &anchor_id, &fetch, &profile],
                )
                .await?
        }
        (None, Some(job)) => {
            client
                .query(
                    JOB_HISTORY_SQL,
                    &[&snapshot, &created, &anchor_id, &fetch, &job],
                )
                .await?
        }
        (None, None) => {
            client
                .query(UNFILTERED_SQL, &[&snapshot, &created, &anchor_id, &fetch])
                .await?
        }
    };
    select_page(request, &rows, view_from_row)
}
