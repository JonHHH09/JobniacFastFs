//! Row values shared by the matching and review adapters.
//!
//! * [`Failure`] is the error of a transaction body. It carries a public error already, converted
//!   from a database failure by the transaction policy of [`details_app_error`]. There is no
//!   serialization failure to retry: a write scope is `BEGIN IMMEDIATE` on the single writer, and
//!   a busy lock is retried by the unit of work.
//! * JSON columns are stored as the compact text of a [`serde_json::Value`] and read back with
//!   [`json_array`].
//! * [`select_page`] builds a keyset page from `limit + 1` rows.

use jobniac_core::errors::{AppError, ApplicationError};
use jobniac_core::matching::{MatchComponent, MatchEvidence, Outcome};
use jobniac_core::pagination::{Page, PageRequest};
use rusqlite::Connection;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::sqlite::error::{SqliteError, details_app_error};
use crate::sqlite::jobs::codec::{instant_text, parse_instant_text, parse_uuid};
use crate::sqlite::unit_of_work::SqliteTransaction;

/// Why a transaction body stopped: a public error.
#[derive(Debug)]
pub(crate) struct Failure(pub(crate) AppError);

impl From<AppError> for Failure {
    fn from(error: AppError) -> Self {
        Self(error)
    }
}

impl From<SqliteError> for Failure {
    /// The transaction policy: an integrity violation is a persistence constraint, and every other
    /// database failure is "Local database is unavailable". The not-configured message is kept.
    fn from(error: SqliteError) -> Self {
        Self(details_app_error(error))
    }
}

impl From<rusqlite::Error> for Failure {
    fn from(error: rusqlite::Error) -> Self {
        Self::from(SqliteError::from(error))
    }
}

impl From<Failure> for AppError {
    fn from(failure: Failure) -> Self {
        failure.0
    }
}

/// The "Local database is unavailable" envelope.
pub(crate) fn unavailable() -> AppError {
    ApplicationError::database_unavailable().into()
}

/// The same envelope as a [`Failure`], for a stored value that does not match its record.
pub(crate) fn corrupted() -> Failure {
    Failure(unavailable())
}

/// Runs `work` on the transaction's connection. A database failure becomes a [`Failure`] under the
/// transaction policy.
pub(crate) fn run<T>(
    tx: &SqliteTransaction,
    work: impl FnOnce(&Connection) -> Result<T, Failure>,
) -> Result<T, Failure> {
    tx.run(|connection| Ok(work(connection)))
        .map_err(Failure::from)?
}

/// The uuid of a stored text, or a corrupted record.
pub(crate) fn uuid_column(text: &str) -> Result<Uuid, Failure> {
    parse_uuid(text).ok_or_else(corrupted)
}

/// The instant of a stored text, or a corrupted record.
pub(crate) fn instant_column(text: &str) -> Result<OffsetDateTime, Failure> {
    parse_instant_text(text).ok_or_else(corrupted)
}

/// A stored JSON array column. A value that is not an array is a corrupted record.
pub(crate) fn json_array(text: &str) -> Result<Vec<Value>, Failure> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(items)) => Ok(items),
        _ => Err(corrupted()),
    }
}

/// A stored JSON column, unvalidated (the record's own validator does that).
pub(crate) fn raw_json(text: &str) -> Result<Value, Failure> {
    serde_json::from_str(text).map_err(|_| corrupted())
}

/// The stored JSON array of a report's or review's components.
pub(crate) fn components_json(components: &[MatchComponent]) -> Value {
    Value::Array(
        components
            .iter()
            .map(|component| {
                json!({
                    "component": component.component.as_str(),
                    "earnedPoints": component.earned_points,
                    "availablePoints": component.available_points,
                    "status": component.status.as_str(),
                })
            })
            .collect(),
    )
}

/// The stored JSON array of a report's or review's evidence.
pub(crate) fn evidence_json(evidence: &[MatchEvidence]) -> Value {
    Value::Array(
        evidence
            .iter()
            .map(|item| {
                json!({
                    "component": item.component.as_str(),
                    "status": item.status.as_str(),
                    "sourceType": item.source_type,
                    "fact": item.fact,
                    "outcomeChangingDefect": item.outcome_changing_defect,
                })
            })
            .collect(),
    )
}

/// The stored wire name of an outcome.
pub(crate) const fn outcome_text(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::StrongMatch => "STRONG_MATCH",
        Outcome::PartialMatch => "PARTIAL_MATCH",
        Outcome::WeakMatch => "WEAK_MATCH",
        Outcome::InsufficientEvidence => "INSUFFICIENT_EVIDENCE",
    }
}

/// The outcome named by its stored wire name, or `None`.
pub(crate) fn outcome_from_text(text: &str) -> Option<Outcome> {
    [
        Outcome::StrongMatch,
        Outcome::PartialMatch,
        Outcome::WeakMatch,
        Outcome::InsufficientEvidence,
    ]
    .into_iter()
    .find(|outcome| outcome_text(*outcome) == text)
}

/// The `limit + 1` bound of a keyset query.
pub(crate) fn fetch_limit(request: &PageRequest<'_>) -> Result<i64, Failure> {
    let limit = i64::try_from(request.limit()).map_err(|_| Failure(AppError::Internal))?;
    Ok(limit + 1)
}

/// The watermark of a keyset page: the cursor's snapshot, else the current instant. It is read
/// back through its stored text, so the cursor carries the instant the query compared against.
pub(crate) fn snapshot_at(request: &PageRequest<'_>) -> OffsetDateTime {
    let instant = request
        .cursor()
        .map(|cursor| cursor.snapshot_at)
        .unwrap_or_else(OffsetDateTime::now_utc);
    parse_instant_text(&instant_text(instant)).unwrap_or(instant)
}

/// Builds one keyset page from the rows of a `limit + 1` query.
///
/// Only the first `limit` rows are hydrated. The cursor is issued only when a row beyond the limit
/// exists, and it carries the snapshot, created-at and identifier of the last row returned.
/// `key` reads the created-at and identifier of a row.
pub(crate) fn select_page<R, T>(
    request: &PageRequest<'_>,
    snapshot: OffsetDateTime,
    rows: Vec<R>,
    key: impl Fn(&R) -> Result<(OffsetDateTime, Uuid), Failure>,
    hydrate: impl Fn(R) -> Result<T, Failure>,
) -> Result<Page<T>, Failure> {
    let limit = request.limit();
    let more = rows.len() > limit;
    let selected: Vec<R> = rows.into_iter().take(limit).collect();
    let last_key = selected.last().map(&key).transpose()?;
    let items = selected
        .into_iter()
        .map(hydrate)
        .collect::<Result<Vec<T>, Failure>>()?;
    let next_cursor = match last_key {
        Some((created_at, id)) if more => Some(request.next_cursor(snapshot, created_at, id)?),
        _ => None,
    };
    Ok(Page { items, next_cursor })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_names_round_trip() {
        for outcome in [
            Outcome::StrongMatch,
            Outcome::PartialMatch,
            Outcome::WeakMatch,
            Outcome::InsufficientEvidence,
        ] {
            assert_eq!(outcome_from_text(outcome_text(outcome)), Some(outcome));
        }
        assert_eq!(outcome_from_text("strong_match"), None);
    }

    #[test]
    fn a_busy_database_is_unavailable_and_a_constraint_is_kept() {
        assert_eq!(
            AppError::from(Failure::from(SqliteError::Busy)),
            unavailable()
        );
        assert!(matches!(
            AppError::from(Failure::from(SqliteError::constraint("x"))),
            AppError::PersistenceConstraint { .. }
        ));
    }
}
