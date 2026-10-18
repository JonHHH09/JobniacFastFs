//! Column codecs shared by every SQLite adapter.
//!
//! * A uuid is its lowercase hyphenated text.
//! * An instant is fixed-width RFC 3339 UTC with six fraction digits
//!   (`2026-09-29T10:00:00.000000Z`), truncated to microseconds, the precision of the PostgreSQL
//!   column it replaces. Keyset queries, CHECK constraints and indexes compare instants as text,
//!   so string order must be time order.
//! * A date is `YYYY-MM-DD`.
//! * A JSON column is the compact text of a [`serde_json::Value`].
//!
//! Three forms of access exist: free functions that return the text to bind ([`uuid_text`],
//! [`instant_text`]), `Option`-returning parsers for row mappers that report their own error
//! ([`parse_uuid`], [`parse_instant_text`]), and [`SqliteError`]-returning readers of a named
//! column ([`get_id`], [`get_ts`]). The [`Id`], [`Instant`] and [`Day`] newtypes bind and read the
//! same forms through `ToSql` and `FromSql`.

use jobniac_core::clock::{parse_instant, truncate_to_micros};
use rusqlite::Row;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, Value, ValueRef};
use serde_json::Value as Json;
use time::{Date, Month, OffsetDateTime};
use uuid::Uuid;

use crate::sqlite::error::SqliteError;

/// The text of a uuid column.
#[must_use]
pub fn uuid_text(id: Uuid) -> String {
    id.hyphenated().to_string()
}

/// The uuid of a stored text, or `None` when it does not parse.
#[must_use]
pub fn parse_uuid(text: &str) -> Option<Uuid> {
    Uuid::parse_str(text).ok()
}

/// The fixed-width text of an instant column, truncated to microseconds.
#[must_use]
pub fn instant_text(value: OffsetDateTime) -> String {
    let utc = truncate_to_micros(value);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
        utc.year(),
        u8::from(utc.month()),
        utc.day(),
        utc.hour(),
        utc.minute(),
        utc.second(),
        utc.nanosecond() / 1_000
    )
}

/// The instant of a stored text, or `None` when it does not parse.
#[must_use]
pub fn parse_instant_text(text: &str) -> Option<OffsetDateTime> {
    parse_instant(text).ok()
}

/// The text of a nullable instant column.
#[must_use]
pub fn optional_instant_text(value: Option<OffsetDateTime>) -> Option<String> {
    value.map(instant_text)
}

/// The text of a JSON column.
#[must_use]
pub fn json_text(value: &Json) -> String {
    value.to_string()
}

/// A JSON array of uuid texts, the bind of a `json_each` membership test.
#[must_use]
pub fn uuid_array_text(ids: &[Uuid]) -> String {
    Json::Array(ids.iter().map(|id| Json::String(uuid_text(*id))).collect()).to_string()
}

/// Parses a stored instant. Text that is not an instant is corrupted data.
///
/// # Errors
///
/// Fails with [`SqliteError::Internal`] for text that is not a stored instant.
pub fn parse_ts(text: &str) -> Result<OffsetDateTime, SqliteError> {
    parse_instant(text).map_err(|_| SqliteError::Internal)
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

/// A uuid column.
#[derive(Debug, Clone, Copy)]
pub struct Id(pub Uuid);

impl ToSql for Id {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(Value::Text(uuid_text(self.0))))
    }
}

impl FromSql for Id {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Uuid::parse_str(value.as_str()?)
            .map(Id)
            .map_err(|error| FromSqlError::Other(Box::new(error)))
    }
}

/// An instant column.
#[derive(Debug, Clone, Copy)]
pub struct Instant(pub OffsetDateTime);

impl ToSql for Instant {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(Value::Text(instant_text(self.0))))
    }
}

impl FromSql for Instant {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        parse_instant(value.as_str()?)
            .map(Instant)
            .map_err(|_| FromSqlError::InvalidType)
    }
}

/// A calendar date column.
#[derive(Debug, Clone, Copy)]
pub struct Day(pub Date);

impl ToSql for Day {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(Value::Text(format!(
            "{:04}-{:02}-{:02}",
            self.0.year(),
            u8::from(self.0.month()),
            self.0.day()
        ))))
    }
}

impl FromSql for Day {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        let bytes = text.as_bytes();
        let shaped = bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit());
        let parsed = shaped
            .then(|| {
                let year = text[0..4].parse::<i32>().ok()?;
                let month = Month::try_from(text[5..7].parse::<u8>().ok()?).ok()?;
                let day = text[8..10].parse::<u8>().ok()?;
                Date::from_calendar_date(year, month, day).ok()
            })
            .flatten();
        parsed.map(Day).ok_or(FromSqlError::InvalidType)
    }
}

#[cfg(test)]
mod tests {
    use time::Duration;
    use time::macros::{date, datetime};

    use super::*;

    #[test]
    fn instants_are_fixed_width_utc_truncated_to_microseconds_and_round_trip() {
        let whole = datetime!(2026-09-29 10:00 UTC);
        assert_eq!(instant_text(whole), "2026-09-29T10:00:00.000000Z");
        let micros = datetime!(2026-09-29 10:00:00.123456789 UTC);
        assert_eq!(instant_text(micros), "2026-09-29T10:00:00.123456Z");
        assert_eq!(
            parse_instant_text(&instant_text(micros)),
            parse_instant_text("2026-09-29T10:00:00.123456Z")
        );
        assert!(instant_text(whole) < instant_text(micros));
        let epoch = OffsetDateTime::UNIX_EPOCH;
        let micro = epoch + Duration::microseconds(1_234_567);
        assert_eq!(parse_ts(&instant_text(micro)), Ok(micro));
        assert_eq!(
            instant_text(epoch + Duration::nanoseconds(1_999)),
            "1970-01-01T00:00:00.000001Z"
        );
    }

    #[test]
    fn an_offset_instant_is_written_in_utc() {
        assert_eq!(
            instant_text(datetime!(2026-01-02 03:04:05.5 +01:00)),
            "2026-01-02T02:04:05.500000Z"
        );
    }

    #[test]
    fn uuids_are_lowercase_hyphenated() {
        let id = Uuid::from_u128(0xABCDEF);
        assert_eq!(uuid_text(id), "00000000-0000-0000-0000-000000abcdef");
        assert_eq!(parse_uuid(&uuid_text(id)), Some(id));
        assert_eq!(parse_uuid("nope"), None);
        assert_eq!(parse_id("not-a-uuid"), Err(SqliteError::Internal));
        assert_eq!(parse_ts("not a time"), Err(SqliteError::Internal));
    }

    #[test]
    fn values_round_trip_through_the_column_forms() {
        let text = Id(Uuid::from_u128(0x1234)).to_sql().map(|out| match out {
            ToSqlOutput::Owned(Value::Text(text)) => text,
            _ => String::new(),
        });
        assert_eq!(text.as_deref(), Ok("00000000-0000-0000-0000-000000001234"));
        assert!(matches!(
            Day::column_result(ValueRef::Text(b"1934-01-01")),
            Ok(Day(day)) if day == date!(1934 - 01 - 01)
        ));
        assert!(Day::column_result(ValueRef::Text(b"1934-13-01")).is_err());
        assert!(Instant::column_result(ValueRef::Text(b"not a time")).is_err());
    }

    #[test]
    fn json_and_uuid_arrays_are_compact() {
        assert_eq!(
            json_text(&serde_json::json!({"a": [1, 2]})),
            r#"{"a":[1,2]}"#
        );
        assert_eq!(
            uuid_array_text(&[Uuid::from_u128(1)]),
            r#"["00000000-0000-0000-0000-000000000001"]"#
        );
    }
}
