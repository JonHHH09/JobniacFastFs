//! Column codecs of the SQLite profile and document adapters.
//!
//! The baseline schema stores a uuid as lowercase hyphenated `TEXT`, an instant as fixed-width
//! RFC 3339 UTC `TEXT` (`2026-01-01T00:00:00.000000Z`, so string order is time order) and a date
//! as `YYYY-MM-DD`. The newtypes below bind and read those forms, so a statement binds
//! `&Id(uuid)` or `&Instant(instant)` and a row reads `col::<Id>(row, "id")?.0`.

use jobniac_core::clock::{parse_instant, truncate_to_micros};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, Value, ValueRef};
use time::{Date, Month, OffsetDateTime};
use uuid::Uuid;

/// A uuid column.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Id(pub Uuid);

impl ToSql for Id {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(Value::Text(
            self.0.hyphenated().to_string(),
        )))
    }
}

impl FromSql for Id {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        Uuid::parse_str(text)
            .map(Id)
            .map_err(|error| FromSqlError::Other(Box::new(error)))
    }
}

/// An instant column.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Instant(pub OffsetDateTime);

/// Formats an instant as fixed-width RFC 3339 UTC with six fraction digits.
pub(crate) fn format_timestamp(value: OffsetDateTime) -> String {
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

impl ToSql for Instant {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(Value::Text(format_timestamp(self.0))))
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
pub(crate) struct Day(pub Date);

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
    use time::macros::{date, datetime};

    use super::*;

    #[test]
    fn instants_are_fixed_width_utc() {
        assert_eq!(
            format_timestamp(datetime!(2026-01-02 03:04:05 UTC)),
            "2026-01-02T03:04:05.000000Z"
        );
        assert_eq!(
            format_timestamp(datetime!(2026-01-02 03:04:05.5 +01:00)),
            "2026-01-02T02:04:05.500000Z"
        );
    }

    #[test]
    fn values_round_trip_through_the_column_forms() {
        let id = Uuid::from_u128(0x1234);
        let text = Id(id).to_sql().map(|out| match out {
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
}
