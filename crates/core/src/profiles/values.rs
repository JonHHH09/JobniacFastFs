//! JSON admission helpers shared by the profile, project and personal-details use cases.
//!
//! Each helper keeps its error field, reason and message. Requests arrive as `serde_json::Value`, so
//! the type rules are checked explicitly: a JSON boolean is not an integer, and JSON `null` is not
//! text.
//!
//! `serde_json` without the `arbitrary_precision` feature stores an integer outside the `u64` range
//! as a float, and a negative integer below `i64::MIN` likewise. The MCP boundary refuses such
//! values as `int_type` before these helpers run, so the "must be an integer" branch is a defensive
//! fallback for direct core callers, who see it where a range message might be expected. The `i64`
//! and `u64` ranges are exact, and the unit tests at the end of this file pin the fallback.

use jobniac_text::str_rules::strip_text;
use serde_json::{Map, Value};
use time::{Date, Month};
use uuid::Uuid;

use crate::errors::{AppError, validation_error};
use crate::ids::admit_uuid;

/// A JSON object request.
pub(crate) type Object = Map<String, Value>;

/// Message of every write-request validation error (`Invalid profile write request`).
pub(crate) const WRITE_MESSAGE: &str = "Invalid profile write request";
/// Message of identifier, revision and project-patch errors (`Invalid profile request`).
pub(crate) const REQUEST_MESSAGE: &str = "Invalid profile request";
/// Largest revision the aggregate accepts (PostgreSQL `bigint` maximum).
pub const MAX_REVISION: i64 = i64::MAX;
/// Largest `displayOrder` accepted (PostgreSQL `integer` maximum).
pub const MAX_DISPLAY_ORDER: i64 = 2_147_483_647;

/// A validation error with the given message, field and reason.
pub(crate) fn invalid(message: &str, field: &str, reason: &str) -> AppError {
    AppError::from(validation_error(message, Some(field), Some(reason)))
}

/// A write-request validation error (the default write message).
pub(crate) fn write_invalid(field: &str, reason: &str) -> AppError {
    invalid(WRITE_MESSAGE, field, reason)
}

/// The value must be a JSON object.
pub(crate) fn mapping<'a>(value: &'a Value, field: &str) -> Result<&'a Object, AppError> {
    match value {
        Value::Object(map) => Ok(map),
        Value::Null => Err(write_invalid(field, "must not be null")),
        _ => Err(write_invalid(field, "must be an object")),
    }
}

/// An absent or null value is an empty array; anything but a JSON array is refused.
pub(crate) fn array<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a [Value], AppError> {
    match value {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(write_invalid(field, "must be an array")),
    }
}

/// The value unless it is absent or JSON null.
pub(crate) fn present(value: Option<&Value>) -> Option<&Value> {
    match value {
        None | Some(Value::Null) => None,
        Some(found) => Some(found),
    }
}

/// Text that is not blank after `strip_text`. Returns the raw text.
pub(crate) fn required_text<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<&'a str, AppError> {
    match value {
        Some(Value::String(text)) if !strip_text(text).is_empty() => Ok(text),
        _ => Err(write_invalid(field, "must not be blank")),
    }
}

/// Null is absent, a non-text value is refused, and blank text is absent.
pub(crate) fn optional_text(
    value: Option<&Value>,
    field: &str,
) -> Result<Option<String>, AppError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            let cleaned = strip_text(text);
            Ok((!cleaned.is_empty()).then(|| cleaned.to_owned()))
        }
        Some(_) => Err(write_invalid(field, "must be text or null")),
    }
}

/// An absent normalized value falls back to the default (already
/// stripped), otherwise it must be non-blank text. The result is lower-cased.
pub(crate) fn normalized_text(
    value: Option<&Value>,
    default: &str,
    field: &str,
) -> Result<String, AppError> {
    match value {
        None | Some(Value::Null) => Ok(lowercase_text(strip_text(default))),
        Some(_) => Ok(lowercase_text(strip_text(required_text(value, field)?))),
    }
}

/// Null is absent; anything else must be a canonical UUID.
pub(crate) fn optional_uuid(value: Option<&Value>, field: &str) -> Result<Option<Uuid>, AppError> {
    match present(value) {
        None => Ok(None),
        Some(found) => parse_uuid(Some(found), field).map(Some),
    }
}

/// A canonical 36-character hyphenated UUID text.
pub(crate) fn parse_uuid(value: Option<&Value>, field: &str) -> Result<Uuid, AppError> {
    match value {
        Some(Value::String(text)) => uuid_text(text, field),
        _ => Err(invalid(REQUEST_MESSAGE, field, "must be a valid UUID")),
    }
}

/// Admits identifier text (see [`parse_uuid`]).
pub(crate) fn uuid_text(text: &str, field: &str) -> Result<Uuid, AppError> {
    admit_uuid(text).map_err(|refusal| refusal.validation(REQUEST_MESSAGE, field))
}

/// A non-negative JSON integer within the revision range.
///
/// # Errors
///
/// Returns the `Invalid profile request` validation error for null, booleans, non-integers,
/// negative values and values above [`MAX_REVISION`].
pub fn expected_revision(value: Option<&Value>) -> Result<i64, AppError> {
    let revision = match value {
        None | Some(Value::Null) => {
            return Err(invalid(
                REQUEST_MESSAGE,
                "expectedRevision",
                "must not be null",
            ));
        }
        Some(Value::Number(number)) => match (number.as_i64(), number.as_u64()) {
            (Some(found), _) => found,
            (None, Some(_)) => {
                return Err(invalid(
                    REQUEST_MESSAGE,
                    "expectedRevision",
                    "exceeds the supported revision range",
                ));
            }
            (None, None) => {
                return Err(invalid(
                    REQUEST_MESSAGE,
                    "expectedRevision",
                    "must be an integer",
                ));
            }
        },
        _ => {
            return Err(invalid(
                REQUEST_MESSAGE,
                "expectedRevision",
                "must be an integer",
            ));
        }
    };
    if revision < 0 {
        return Err(invalid(
            REQUEST_MESSAGE,
            "expectedRevision",
            "must not be negative",
        ));
    }
    Ok(revision)
}

/// An integer from 0 to [`MAX_DISPLAY_ORDER`].
pub(crate) fn non_negative_order(value: &Value, field: &str) -> Result<i64, AppError> {
    let Value::Number(number) = value else {
        return Err(write_invalid(field, "must be an integer"));
    };
    let order = match (number.as_i64(), number.as_u64()) {
        (Some(found), _) => found,
        (None, Some(_)) => {
            return Err(write_invalid(
                field,
                "must be less than or equal to 2147483647",
            ));
        }
        (None, None) => return Err(write_invalid(field, "must be an integer")),
    };
    if order < 0 {
        return Err(write_invalid(field, "must be greater than or equal to 0"));
    }
    if order > MAX_DISPLAY_ORDER {
        return Err(write_invalid(
            field,
            "must be less than or equal to 2147483647",
        ));
    }
    Ok(order)
}

/// The `displayOrder` of a child object: absent means 0, but an explicit null is refused.
pub(crate) fn display_order(item: &Object, field: &str) -> Result<i64, AppError> {
    match item.get("displayOrder") {
        None => Ok(0),
        Some(value) => non_negative_order(value, field),
    }
}

/// Null is absent; otherwise a canonical `YYYY-MM-DD` date.
pub(crate) fn optional_date(value: Option<&Value>, field: &str) -> Result<Option<Date>, AppError> {
    match present(value) {
        None => Ok(None),
        Some(Value::String(text)) => match parse_iso_date(text) {
            Some(date) => Ok(Some(date)),
            None => Err(write_invalid(field, "must be an ISO date or null")),
        },
        Some(_) => Err(write_invalid(field, "must be an ISO date or null")),
    }
}

/// Parses a canonical `YYYY-MM-DD` date with ASCII digits and a year from 1 to 9999.
pub(crate) fn parse_iso_date(text: &str) -> Option<Date> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        let slice = bytes.get(range)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(slice).ok()?.parse().ok()
    };
    let year = i32::try_from(digits(0..4)?).ok()?;
    if year < 1 {
        return None;
    }
    let month = Month::try_from(u8::try_from(digits(5..7)?).ok()?).ok()?;
    let day = u8::try_from(digits(8..10)?).ok()?;
    Date::from_calendar_date(year, month, day).ok()
}

/// Canonical `YYYY-MM-DD` text of a date.
pub(crate) fn iso_date(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// Unicode lower-casing for the text rules of the profile aggregate.
pub(crate) fn lowercase_text(text: &str) -> String {
    text.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::error_envelope;

    fn parsed(text: &str) -> Value {
        serde_json::from_str(text).unwrap_or(Value::Null)
    }

    /// The `reason` detail of the validation envelope, or an empty string.
    fn reason_of(result: Result<impl std::fmt::Debug, AppError>) -> String {
        match result {
            Ok(value) => panic!("expected a refusal, got {value:?}"),
            Err(error) => error_envelope(&error)
                .details
                .get("reason")
                .cloned()
                .unwrap_or_default(),
        }
    }

    #[test]
    fn revision_range_is_exact_up_to_the_i64_maximum() {
        let maximum = expected_revision(Some(&parsed("9223372036854775807")));
        assert_eq!(maximum.ok(), Some(i64::MAX));
        assert_eq!(
            reason_of(expected_revision(Some(&parsed("9223372036854775808")))),
            "exceeds the supported revision range"
        );
    }

    #[test]
    fn integers_beyond_u64_fall_back_to_must_be_an_integer() {
        let huge = parsed("100000000000000000000000000000");
        assert_eq!(
            reason_of(expected_revision(Some(&huge))),
            "must be an integer"
        );
        assert_eq!(
            reason_of(non_negative_order(&huge, "displayOrder")),
            "must be an integer"
        );
    }

    #[test]
    fn display_order_refuses_values_above_the_postgresql_integer_maximum() {
        assert_eq!(
            reason_of(non_negative_order(&parsed("2147483648"), "displayOrder")),
            "must be less than or equal to 2147483647"
        );
        assert_eq!(
            reason_of(non_negative_order(&parsed("-1"), "displayOrder")),
            "must be greater than or equal to 0"
        );
    }
}
