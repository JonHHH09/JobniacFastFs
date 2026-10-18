//! Column codecs of the job, matching and review adapters.
//!
//! * A uuid is its lowercase hyphenated text.
//! * An instant is fixed-width RFC 3339 UTC with six fraction digits
//!   (`2026-09-29T10:00:00.000000Z`), because the keyset queries, the CHECK constraints and the
//!   indexes compare timestamps as text. The instant is truncated to microseconds, the precision of
//!   the PostgreSQL column it replaces.
//! * A JSON column is the compact text of a [`serde_json::Value`].

use jobniac_core::clock::parse_instant;
use serde_json::Value;
use time::{OffsetDateTime, UtcOffset};
use uuid::Uuid;

/// The text of a uuid column.
pub fn uuid_text(id: Uuid) -> String {
    id.hyphenated().to_string()
}

/// The uuid of a stored text, or `None` when it does not parse.
pub fn parse_uuid(text: &str) -> Option<Uuid> {
    Uuid::parse_str(text).ok()
}

/// The fixed-width text of an instant column.
pub fn instant_text(value: OffsetDateTime) -> String {
    let utc = value.to_offset(UtcOffset::UTC);
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
pub fn parse_instant_text(text: &str) -> Option<OffsetDateTime> {
    parse_instant(text).ok()
}

/// The text of a nullable instant column.
pub fn optional_instant_text(value: Option<OffsetDateTime>) -> Option<String> {
    value.map(instant_text)
}

/// The text of a JSON column.
pub fn json_text(value: &Value) -> String {
    value.to_string()
}

/// A JSON array of uuid texts, the bind of a `json_each` membership test.
pub fn uuid_array_text(ids: &[Uuid]) -> String {
    Value::Array(ids.iter().map(|id| Value::String(uuid_text(*id))).collect()).to_string()
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    #[test]
    fn instants_are_fixed_width_and_round_trip() {
        let whole = datetime!(2026-09-29 10:00 UTC);
        assert_eq!(instant_text(whole), "2026-09-29T10:00:00.000000Z");
        let micros = datetime!(2026-09-29 10:00:00.123456789 UTC);
        assert_eq!(instant_text(micros), "2026-09-29T10:00:00.123456Z");
        assert_eq!(
            parse_instant_text(&instant_text(micros)),
            parse_instant_text("2026-09-29T10:00:00.123456Z")
        );
        assert!(instant_text(whole) < instant_text(micros));
    }

    #[test]
    fn an_offset_instant_is_written_in_utc() {
        let offset = datetime!(2026-09-29 12:00 +02:00);
        assert_eq!(instant_text(offset), "2026-09-29T10:00:00.000000Z");
    }

    #[test]
    fn uuids_are_lowercase_hyphenated() {
        let id = Uuid::from_u128(0xABCDEF);
        assert_eq!(uuid_text(id), "00000000-0000-0000-0000-000000abcdef");
        assert_eq!(parse_uuid(&uuid_text(id)), Some(id));
        assert_eq!(parse_uuid("nope"), None);
    }
}
