//! Column codecs of the generated-asset and cleanup tables: uuids and instants as `TEXT`.
//!
//! Every instant is written as fixed-width RFC 3339 UTC with six fraction digits
//! (`2026-01-01T00:00:00.000000Z`), because the CHECK constraints and indexes of the schema compare
//! instants as strings. Uuids are the lowercase hyphenated form the schema shape checks require.

use rusqlite::Row;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;

use crate::sqlite::error::SqliteError;

const INSTANT: &[time::format_description::BorrowedFormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z");

/// The stored text of an instant, truncated to microseconds.
#[must_use]
pub fn ts(value: OffsetDateTime) -> String {
    let utc = jobniac_core::clock::truncate_to_micros(value);
    // Formatting a valid UTC instant with this description cannot fail.
    utc.format(&INSTANT).unwrap_or_default()
}

/// The stored text of an optional instant.
#[must_use]
pub fn opt_ts(value: Option<OffsetDateTime>) -> Option<String> {
    value.map(ts)
}

/// The stored text of a uuid.
#[must_use]
pub fn id(value: Uuid) -> String {
    value.to_string()
}

/// Parses a stored instant. Text that is not the stored format is corrupted data.
///
/// # Errors
///
/// Fails with [`SqliteError::Internal`] for text that is not a stored instant.
pub fn parse_ts(text: &str) -> Result<OffsetDateTime, SqliteError> {
    PrimitiveDateTime::parse(text, &INSTANT)
        .map(PrimitiveDateTime::assume_utc)
        .map_err(|_| SqliteError::Internal)
}

/// Parses a stored uuid.
///
/// # Errors
///
/// Fails with [`SqliteError::Internal`] for text that is not a uuid.
pub fn parse_id(text: &str) -> Result<Uuid, SqliteError> {
    Uuid::parse_str(text).map_err(|_| SqliteError::Internal)
}

/// Reads a `NOT NULL` uuid column.
///
/// # Errors
///
/// Fails when the column is missing, `NULL`, or not a uuid.
pub fn get_id(row: &Row<'_>, column: &str) -> Result<Uuid, SqliteError> {
    parse_id(&row.get::<_, String>(column)?)
}

/// Reads a nullable uuid column.
///
/// # Errors
///
/// Fails when the column is missing or holds text that is not a uuid.
pub fn get_opt_id(row: &Row<'_>, column: &str) -> Result<Option<Uuid>, SqliteError> {
    row.get::<_, Option<String>>(column)?
        .map(|text| parse_id(&text))
        .transpose()
}

/// Reads a `NOT NULL` instant column.
///
/// # Errors
///
/// Fails when the column is missing, `NULL`, or not a stored instant.
pub fn get_ts(row: &Row<'_>, column: &str) -> Result<OffsetDateTime, SqliteError> {
    parse_ts(&row.get::<_, String>(column)?)
}

/// Reads a nullable instant column.
///
/// # Errors
///
/// Fails when the column is missing or holds text that is not a stored instant.
pub fn get_opt_ts(row: &Row<'_>, column: &str) -> Result<Option<OffsetDateTime>, SqliteError> {
    row.get::<_, Option<String>>(column)?
        .map(|text| parse_ts(&text))
        .transpose()
}

#[cfg(test)]
mod tests {
    use time::Duration;

    use super::*;

    #[test]
    fn instants_are_fixed_width_and_round_trip() {
        let whole = OffsetDateTime::UNIX_EPOCH;
        assert_eq!(ts(whole), "1970-01-01T00:00:00.000000Z");
        let micro = whole + Duration::microseconds(1_234_567);
        assert_eq!(ts(micro), "1970-01-01T00:00:01.234567Z");
        assert_eq!(parse_ts(&ts(micro)), Ok(micro));
        // Nanoseconds below the stored precision are dropped.
        assert_eq!(
            ts(whole + Duration::nanoseconds(1_999)),
            "1970-01-01T00:00:00.000001Z"
        );
    }

    #[test]
    fn text_in_another_shape_is_corrupted_data() {
        assert_eq!(parse_ts("1970-01-01T00:00:00Z"), Err(SqliteError::Internal));
        assert_eq!(parse_id("not-a-uuid"), Err(SqliteError::Internal));
    }
}
