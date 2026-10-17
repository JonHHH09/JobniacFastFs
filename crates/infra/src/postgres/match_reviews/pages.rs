//! Keyset history of the advisory reviews and of the disagreements.
//!
//! Each query has an unfiltered form and a report-filtered form. The binds are the cursor's snapshot,
//! created-at and identifier, the limit plus one, and the report identifier for the filtered form.

use jobniac_core::pagination::{Page, PageRequest};
use jobniac_core::reviews::{MatchDisagreement, MatchReview};
use tokio_postgres::Client;
use uuid::Uuid;

use super::store::{disagreement_from_row, review_from_row};
use crate::postgres::matching::value::{Failure, fetch_limit, select_page};

/// Review page, unfiltered. Binds: snapshot, cursor created-at, cursor id, fetch limit.
const REVIEW_UNFILTERED_SQL: &str = "
WITH bounds AS (
    SELECT COALESCE(CAST($1 AS timestamptz), CURRENT_TIMESTAMP) AS snapshot_at
)
SELECT review.*, bounds.snapshot_at
FROM match.reviews review
CROSS JOIN bounds
WHERE review.created_at <= bounds.snapshot_at
  AND (CAST($2 AS timestamptz) IS NULL
       OR (review.created_at, review.id) < ($2, $3))
ORDER BY review.created_at DESC, review.id DESC
LIMIT $4
";

/// Review page, filtered by report. Binds as above, then the report identifier.
const REVIEW_REPORT_SQL: &str = "
WITH bounds AS (
    SELECT COALESCE(CAST($1 AS timestamptz), CURRENT_TIMESTAMP) AS snapshot_at
)
SELECT review.*, bounds.snapshot_at
FROM match.reviews review
CROSS JOIN bounds
WHERE review.report_id = $5
  AND review.created_at <= bounds.snapshot_at
  AND (review.report_id, review.created_at, review.id)
      <= ($5, bounds.snapshot_at,
          CAST('ffffffff-ffff-ffff-ffff-ffffffffffff' AS uuid))
  AND (CAST($2 AS timestamptz) IS NULL
       OR (review.created_at, review.id) < ($2, $3))
ORDER BY review.report_id, review.created_at DESC, review.id DESC
LIMIT $4
";

/// Disagreement page, unfiltered. Binds as [`REVIEW_UNFILTERED_SQL`].
const DISAGREEMENT_UNFILTERED_SQL: &str = "
WITH bounds AS (
    SELECT COALESCE(CAST($1 AS timestamptz), CURRENT_TIMESTAMP) AS snapshot_at
)
SELECT disagreement.*, bounds.snapshot_at
FROM match.disagreements disagreement
CROSS JOIN bounds
WHERE disagreement.created_at <= bounds.snapshot_at
  AND (CAST($2 AS timestamptz) IS NULL
       OR (disagreement.created_at, disagreement.id) < ($2, $3))
ORDER BY disagreement.created_at DESC, disagreement.id DESC
LIMIT $4
";

/// Disagreement page, filtered by report. Binds as [`REVIEW_REPORT_SQL`].
const DISAGREEMENT_REPORT_SQL: &str = "
WITH bounds AS (
    SELECT COALESCE(CAST($1 AS timestamptz), CURRENT_TIMESTAMP) AS snapshot_at
)
SELECT disagreement.*, bounds.snapshot_at
FROM match.disagreements disagreement
CROSS JOIN bounds
WHERE disagreement.report_id = $5
  AND disagreement.created_at <= bounds.snapshot_at
  AND (disagreement.report_id, disagreement.created_at, disagreement.id)
      <= ($5, bounds.snapshot_at,
          CAST('ffffffff-ffff-ffff-ffff-ffffffffffff' AS uuid))
  AND (CAST($2 AS timestamptz) IS NULL
       OR (disagreement.created_at, disagreement.id) < ($2, $3))
ORDER BY disagreement.report_id, disagreement.created_at DESC, disagreement.id DESC
LIMIT $4
";

/// One page of reviews, newest first, filtered by report when given.
pub(crate) async fn list_reviews(
    client: &Client,
    report_id: Option<Uuid>,
    request: &PageRequest<'_>,
) -> Result<Page<MatchReview>, Failure> {
    let anchor = request.cursor();
    let snapshot = anchor.map(|cursor| cursor.snapshot_at);
    let created = anchor.map(|cursor| cursor.created_at);
    let anchor_id = anchor.map(|cursor| cursor.id);
    let fetch = fetch_limit(request)?;
    let rows = match report_id {
        None => {
            client
                .query(
                    REVIEW_UNFILTERED_SQL,
                    &[&snapshot, &created, &anchor_id, &fetch],
                )
                .await?
        }
        Some(report) => {
            client
                .query(
                    REVIEW_REPORT_SQL,
                    &[&snapshot, &created, &anchor_id, &fetch, &report],
                )
                .await?
        }
    };
    select_page(request, &rows, review_from_row)
}

/// One page of disagreements, newest first, filtered by report when given.
pub(crate) async fn list_disagreements(
    client: &Client,
    report_id: Option<Uuid>,
    request: &PageRequest<'_>,
) -> Result<Page<MatchDisagreement>, Failure> {
    let anchor = request.cursor();
    let snapshot = anchor.map(|cursor| cursor.snapshot_at);
    let created = anchor.map(|cursor| cursor.created_at);
    let anchor_id = anchor.map(|cursor| cursor.id);
    let fetch = fetch_limit(request)?;
    let rows = match report_id {
        None => {
            client
                .query(
                    DISAGREEMENT_UNFILTERED_SQL,
                    &[&snapshot, &created, &anchor_id, &fetch],
                )
                .await?
        }
        Some(report) => {
            client
                .query(
                    DISAGREEMENT_REPORT_SQL,
                    &[&snapshot, &created, &anchor_id, &fetch, &report],
                )
                .await?
        }
    };
    select_page(request, &rows, disagreement_from_row)
}
