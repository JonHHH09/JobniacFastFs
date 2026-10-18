//! Keyset history of the advisory reviews and of the disagreements.
//!
//! Each query has an unfiltered form and a report-filtered form. The binds are the snapshot, the
//! cursor's created-at and identifier, the limit plus one, and the report identifier for the
//! filtered form.

use jobniac_core::pagination::{Page, PageRequest};
use jobniac_core::reviews::{MatchDisagreement, MatchReview};
use uuid::Uuid;

use super::store::{
    RawDisagreement, RawReview, disagreement_columns, read_disagreement, read_review,
    review_columns,
};
use crate::sqlite::jobs::codec::{instant_text, uuid_text};
use crate::sqlite::matching::value::{Failure, fetch_limit, run, select_page, snapshot_at};
use crate::sqlite::unit_of_work::SqliteTransaction;

/// Review page, unfiltered. Binds: snapshot, cursor created-at, cursor id, fetch limit.
const REVIEW_UNFILTERED_SQL: &str = concat!(
    "SELECT ",
    review_columns!("review"),
    "
FROM match_reviews AS review
WHERE review.created_at <= ?1
  AND (?2 IS NULL OR (review.created_at, review.id) < (?2, ?3))
ORDER BY review.created_at DESC, review.id DESC
LIMIT ?4"
);

/// Review page, filtered by report. Binds as above, then the report identifier.
const REVIEW_REPORT_SQL: &str = concat!(
    "SELECT ",
    review_columns!("review"),
    "
FROM match_reviews AS review
WHERE review.report_id = ?5
  AND review.created_at <= ?1
  AND (?2 IS NULL OR (review.created_at, review.id) < (?2, ?3))
ORDER BY review.created_at DESC, review.id DESC
LIMIT ?4"
);

/// Disagreement page, unfiltered. Binds as [`REVIEW_UNFILTERED_SQL`].
const DISAGREEMENT_UNFILTERED_SQL: &str = concat!(
    "SELECT ",
    disagreement_columns!("disagreement"),
    "
FROM match_disagreements AS disagreement
WHERE disagreement.created_at <= ?1
  AND (?2 IS NULL OR (disagreement.created_at, disagreement.id) < (?2, ?3))
ORDER BY disagreement.created_at DESC, disagreement.id DESC
LIMIT ?4"
);

/// Disagreement page, filtered by report. Binds as [`REVIEW_REPORT_SQL`].
const DISAGREEMENT_REPORT_SQL: &str = concat!(
    "SELECT ",
    disagreement_columns!("disagreement"),
    "
FROM match_disagreements AS disagreement
WHERE disagreement.report_id = ?5
  AND disagreement.created_at <= ?1
  AND (?2 IS NULL OR (disagreement.created_at, disagreement.id) < (?2, ?3))
ORDER BY disagreement.created_at DESC, disagreement.id DESC
LIMIT ?4"
);

/// One page of reviews, newest first, filtered by report when given.
pub(crate) fn list_reviews(
    tx: &SqliteTransaction,
    report_id: Option<Uuid>,
    request: &PageRequest<'_>,
) -> Result<Page<MatchReview>, Failure> {
    let anchor = request.cursor();
    let snapshot = snapshot_at(request);
    let created = anchor.map(|cursor| instant_text(cursor.created_at));
    let anchor_id = anchor.map(|cursor| uuid_text(cursor.id));
    let fetch = fetch_limit(request)?;
    let snapshot_text = instant_text(snapshot);
    let rows = run(tx, |connection| {
        let report = report_id.map(uuid_text);
        let mut statement = connection.prepare_cached(if report.is_some() {
            REVIEW_REPORT_SQL
        } else {
            REVIEW_UNFILTERED_SQL
        })?;
        let rows = match &report {
            None => statement.query_map(
                rusqlite::params![snapshot_text, created, anchor_id, fetch],
                read_review,
            )?,
            Some(report) => statement.query_map(
                rusqlite::params![snapshot_text, created, anchor_id, fetch, report],
                read_review,
            )?,
        };
        Ok(rows.collect::<rusqlite::Result<Vec<RawReview>>>()?)
    })?;
    select_page(
        request,
        snapshot,
        rows,
        RawReview::key,
        RawReview::into_review,
    )
}

/// One page of disagreements, newest first, filtered by report when given.
pub(crate) fn list_disagreements(
    tx: &SqliteTransaction,
    report_id: Option<Uuid>,
    request: &PageRequest<'_>,
) -> Result<Page<MatchDisagreement>, Failure> {
    let anchor = request.cursor();
    let snapshot = snapshot_at(request);
    let created = anchor.map(|cursor| instant_text(cursor.created_at));
    let anchor_id = anchor.map(|cursor| uuid_text(cursor.id));
    let fetch = fetch_limit(request)?;
    let snapshot_text = instant_text(snapshot);
    let rows = run(tx, |connection| {
        let report = report_id.map(uuid_text);
        let mut statement = connection.prepare_cached(if report.is_some() {
            DISAGREEMENT_REPORT_SQL
        } else {
            DISAGREEMENT_UNFILTERED_SQL
        })?;
        let rows = match &report {
            None => statement.query_map(
                rusqlite::params![snapshot_text, created, anchor_id, fetch],
                read_disagreement,
            )?,
            Some(report) => statement.query_map(
                rusqlite::params![snapshot_text, created, anchor_id, fetch, report],
                read_disagreement,
            )?,
        };
        Ok(rows.collect::<rusqlite::Result<Vec<RawDisagreement>>>()?)
    })?;
    select_page(
        request,
        snapshot,
        rows,
        RawDisagreement::key,
        RawDisagreement::into_disagreement,
    )
}
