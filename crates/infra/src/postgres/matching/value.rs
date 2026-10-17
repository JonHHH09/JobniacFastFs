//! Row values shared by the matching and review adapters.
//!
//! * [`Failure`] is the error of a transaction body. It keeps a serialization failure (SQLSTATE
//!   40001) apart from the public errors, so the snapshot operations can retry it. The unit of work converts its own errors into it through [`PgError`].
//! * `jsonb` columns are read and bound as [`serde_json::Value`] directly (the driver's
//!   `with-serde_json-1` feature), so the stored value is `jsonb`.
//! * [`select_page`] builds a keyset page from `limit + 1` rows.

use jobniac_core::errors::AppError;
use jobniac_core::matching::{MatchComponent, MatchEvidence, Outcome};
use jobniac_core::pagination::{Page, PageRequest};
use serde_json::{Value, json};
use tokio_postgres::Row;
use tokio_postgres::types::FromSql;

use crate::postgres::sqlstate::is_serialization_failure;
use crate::postgres::unit_of_work::{PgError, details_app_error};

/// Why a transaction body stopped.
#[derive(Debug)]
pub(crate) enum Failure {
    /// SQLSTATE 40001 from the server. Only snapshot operations retry it.
    Serialization,
    /// Any other failure, already in its public form.
    App(AppError),
}

impl From<AppError> for Failure {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<PgError> for Failure {
    /// The transaction policy: an integrity violation is a persistence constraint, and every other
    /// database failure is "Local database is unavailable". The not-configured message is kept. A
    /// serialization failure, also when raised by `COMMIT`, stays a [`Failure::Serialization`] so
    /// the snapshot operations retry it.
    fn from(error: PgError) -> Self {
        match error {
            PgError::Serialization => Self::Serialization,
            other => Self::App(details_app_error(other)),
        }
    }
}

impl From<tokio_postgres::Error> for Failure {
    fn from(error: tokio_postgres::Error) -> Self {
        if is_serialization_failure(&error) {
            Self::Serialization
        } else {
            Self::from(PgError::from(error))
        }
    }
}

impl From<Failure> for AppError {
    /// A serialization failure that is not retried reports the database as unavailable.
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Serialization => unavailable(),
            Failure::App(error) => error,
        }
    }
}

/// The "Local database is unavailable" envelope.
pub(crate) fn unavailable() -> AppError {
    AppError::from(PgError::Unavailable)
}

/// The same envelope as a [`Failure`], for a stored value that does not match its record.
pub(crate) fn corrupted() -> Failure {
    Failure::App(unavailable())
}

/// Reads one named column. A decode failure maps like any driver error.
pub(crate) fn column<'a, T: FromSql<'a>>(row: &'a Row, name: &str) -> Result<T, Failure> {
    Ok(row.try_get::<_, T>(name)?)
}

/// A `jsonb` array column. A value that is not an array is a corrupted record.
pub(crate) fn json_array(row: &Row, name: &str) -> Result<Vec<Value>, Failure> {
    let value: Value = column(row, name)?;
    match value {
        Value::Array(items) => Ok(items),
        _ => Err(corrupted()),
    }
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
    let limit = i64::try_from(request.limit()).map_err(|_| Failure::App(AppError::Internal))?;
    Ok(limit + 1)
}

/// Builds one keyset page from the rows of a `limit + 1` query.
///
/// Only the first `limit` rows are hydrated. The cursor is issued only when a row beyond the limit
/// exists, and it carries the snapshot, created-at and identifier of the last row returned.
pub(crate) fn select_page<T>(
    request: &PageRequest<'_>,
    rows: &[Row],
    hydrate: impl Fn(&Row) -> Result<T, Failure>,
) -> Result<Page<T>, Failure> {
    let limit = request.limit();
    let selected = &rows[..rows.len().min(limit)];
    let items = selected
        .iter()
        .map(hydrate)
        .collect::<Result<Vec<T>, Failure>>()?;
    let next_cursor = match selected.last() {
        Some(last) if rows.len() > limit => Some(request.next_cursor(
            column(last, "snapshot_at")?,
            column(last, "created_at")?,
            column(last, "id")?,
        )?),
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
    fn serialization_failure_is_unavailable_when_it_is_not_retried() {
        assert_eq!(AppError::from(Failure::Serialization), unavailable());
    }

    #[test]
    fn a_commit_time_serialization_failure_is_retryable() {
        assert!(matches!(
            Failure::from(PgError::Serialization),
            Failure::Serialization
        ));
        assert!(matches!(
            Failure::from(PgError::Unavailable),
            Failure::App(_)
        ));
    }
}
