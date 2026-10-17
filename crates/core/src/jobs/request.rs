//! JSON admission of job requests.
//!
//! Requests arrive as `serde_json::Value`, so the type rules are checked explicitly: a JSON
//! boolean is not an integer, JSON null is absent, and only a JSON string is
//! text. Every failure is `Invalid job request` with the field and the fixed reason of the
//! failed check; no input value is echoed.

use jobniac_text::java::java_strip;
use serde_json::{Map, Value};
use time::{Date, Duration, Month, OffsetDateTime, PrimitiveDateTime, Time};
use uuid::Uuid;

use crate::clock::parse_instant;
use crate::errors::{AppError, validation_error};
use crate::ids::admit_uuid;

use super::text::{merge_skills, split_skill_list};

/// Message of every job request validation error.
pub const JOB_REQUEST_MESSAGE: &str = "Invalid job request";

/// A JSON object request.
pub type JobObject = Map<String, Value>;

/// A job validation error for `field` with the fixed `reason`.
pub fn job_invalid(field: &str, reason: &str) -> AppError {
    AppError::from(validation_error(
        JOB_REQUEST_MESSAGE,
        Some(field),
        Some(reason),
    ))
}

/// The request must be a JSON object.
pub fn mapping(request: &Value) -> Result<&JobObject, AppError> {
    match request {
        Value::Object(map) => Ok(map),
        _ => Err(job_invalid("request", "must not be null")),
    }
}

/// Absent or null is `None`, a non-string is refused, and text that is
/// blank after the Java strip is `None`.
pub fn optional_text(request: &JobObject, field: &str) -> Result<Option<String>, AppError> {
    match request.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            let stripped = java_strip(text);
            Ok((!stripped.is_empty()).then(|| stripped.to_owned()))
        }
        Some(_) => Err(job_invalid(field, "must be a string")),
    }
}

/// Absent or null is no skills; the value must be a list of strings,
/// each split on the skill separators.
pub fn explicit_skills(request: &JobObject) -> Result<Vec<String>, AppError> {
    let items = match request.get("skills") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(job_invalid("skills", "must be a list of strings")),
    };
    let mut parts = Vec::new();
    for item in items {
        let Value::String(text) = item else {
            return Err(job_invalid("skills", "must contain only strings"));
        };
        parts.extend(split_skill_list(text));
    }
    Ok(parts)
}

/// Absent or null keeps the stored skills (`None`). A list may hold
/// nulls, which are skipped; any other non-string element is refused. The result is de-duplicated.
pub fn update_skill_names(request: &JobObject) -> Result<Option<Vec<String>>, AppError> {
    let items = match request.get("skills") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(job_invalid("skills", "must be a list of strings")),
    };
    let mut parts = Vec::new();
    for item in items {
        match item {
            Value::Null => {}
            Value::String(text) => parts.extend(split_skill_list(text)),
            _ => return Err(job_invalid("skills", "must contain only strings")),
        }
    }
    Ok(Some(merge_skills(&parts, &[])))
}

/// Admission of a required job identifier: null and absent are "must
/// not be null", and anything but a canonical UUID string is "must be a valid UUID".
pub fn job_id_field(value: Option<&Value>) -> Result<Uuid, AppError> {
    match value {
        None | Some(Value::Null) => Err(job_invalid("jobId", "must not be null")),
        Some(Value::String(text)) => {
            admit_uuid(text).map_err(|refusal| refusal.validation(JOB_REQUEST_MESSAGE, "jobId"))
        }
        Some(_) => Err(job_invalid("jobId", "must be a valid UUID")),
    }
}

/// The magnitude of `2^63` as an `f64`, which is exact.
const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;

/// A non-negative JSON integer no larger than the signed 64-bit
/// maximum. A negative integer in the i64 range is "must not be negative"; a float or a boolean is
/// "must be an integer".
///
/// Without `serde_json`'s `arbitrary_precision` feature an integer
/// literal outside the 64-bit range is stored as a float. An integral float of magnitude at least
/// `2^63` can only come from such a literal, so it is treated as an out-of-range
/// integer: "exceeds the supported revision range" when positive, "must not be negative" when
/// negative. A float literal such as `1e30` is indistinguishable from that and gets the same
/// outcome. The MCP layer refuses such integers before this point.
pub fn expected_revision(value: Option<&Value>) -> Result<i64, AppError> {
    let reason = match value {
        None | Some(Value::Null) => "must not be null",
        Some(Value::Number(number)) => match (number.as_i64(), number.as_u64()) {
            (Some(revision), _) if revision >= 0 => return Ok(revision),
            (Some(_), _) => "must not be negative",
            (None, Some(_)) => "exceeds the supported revision range",
            (None, None) => match number.as_f64() {
                Some(float) if float.fract() == 0.0 && float >= TWO_POW_63 => {
                    "exceeds the supported revision range"
                }
                Some(float) if float.fract() == 0.0 && float <= -TWO_POW_63 => {
                    "must not be negative"
                }
                _ => "must be an integer",
            },
        },
        Some(_) => "must be an integer",
    };
    Err(job_invalid("expectedRevision", reason))
}

/// The optional instant of `field`. Absent or null is `None`.
///
/// Accepted: an ISO-8601 instant with a `Z` designator (surrounding whitespace is ignored, and
/// fractions of up to nine digits are truncated to microseconds), or a local time with an offset
/// from `+00:00` to `+17:59` or exactly `+18:00`, converted to UTC. The converted instant must lie in
/// years 1 to 9999. Anything else, and every non-string, is "must be an ISO-8601 UTC timestamp".
pub fn optional_instant(request: &Value, field: &str) -> Result<Option<OffsetDateTime>, AppError> {
    instant_field(request.get(field), field)
}

/// [`optional_instant`] for an already-looked-up field value.
pub(crate) fn instant_field(
    value: Option<&Value>,
    field: &str,
) -> Result<Option<OffsetDateTime>, AppError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => parse_offset_instant(text)
            .or_else(|| parse_instant(text).ok())
            .map(Some)
            .ok_or_else(|| job_invalid(field, "must be an ISO-8601 UTC timestamp")),
        Some(_) => Err(job_invalid(field, "must be an ISO-8601 UTC timestamp")),
    }
}

/// The offset form `YYYY-MM-DDTHH:MM[:SS[.F{1,9}]](+|-)HH:MM` . Returns `None` when the text is not of this form,
/// when a field is out of range, or when the UTC result is outside years 1 to 9999.
fn parse_offset_instant(text: &str) -> Option<OffsetDateTime> {
    let bytes = text.as_bytes();
    let number = |from: usize, length: usize| -> Option<u32> {
        let digits = bytes.get(from..from + length)?;
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(digits).ok()?.parse().ok()
    };
    let year = i32::try_from(number(0, 4)?).ok()?;
    if bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
    {
        return None;
    }
    let month = Month::try_from(u8::try_from(number(5, 2)?).ok()?).ok()?;
    let day = u8::try_from(number(8, 2)?).ok()?;
    let hour = u8::try_from(number(11, 2)?).ok()?;
    let minute = u8::try_from(number(14, 2)?).ok()?;
    let mut at = 16;
    let mut second = 0_u8;
    let mut microsecond = 0_u32;
    if bytes.get(at) == Some(&b':') {
        second = u8::try_from(number(at + 1, 2)?).ok()?;
        at += 3;
        if bytes.get(at) == Some(&b'.') {
            let start = at + 1;
            let mut end = start;
            while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            if !(1..=9).contains(&(end - start)) {
                return None;
            }
            for offset in 0..6 {
                let digit = match bytes.get(start + offset) {
                    Some(byte) if start + offset < end => u32::from(byte - b'0'),
                    _ => 0,
                };
                microsecond = microsecond * 10 + digit;
            }
            at = end;
        }
    }
    let sign = match bytes.get(at) {
        Some(b'+') => 1_i32,
        Some(b'-') => -1_i32,
        _ => return None,
    };
    if bytes.get(at + 3) != Some(&b':') || at + 6 != bytes.len() {
        return None;
    }
    let offset_hours = number(at + 1, 2)?;
    let offset_minutes = number(at + 4, 2)?;
    let offset_admitted =
        (offset_hours <= 17 && offset_minutes <= 59) || (offset_hours == 18 && offset_minutes == 0);
    if !offset_admitted || !(1..=9999).contains(&year) {
        return None;
    }
    let date = Date::from_calendar_date(year, month, day).ok()?;
    // `datetime.fromisoformat` takes `24:00[:00[.0+]]` as midnight of the next day; any other hour 24
    // value is refused.
    let local = if hour == 24 {
        if minute != 0 || second != 0 || microsecond != 0 {
            return None;
        }
        PrimitiveDateTime::new(date, Time::MIDNIGHT).checked_add(Duration::DAY)?
    } else {
        PrimitiveDateTime::new(
            date,
            Time::from_hms_micro(hour, minute, second, microsecond).ok()?,
        )
    };
    // UTC = local time minus the offset. The checked subtraction and the year bound together give
    // the overflow rule of the UTC conversion.
    let offset_seconds = i64::from(sign) * i64::from(offset_hours * 3600 + offset_minutes * 60);
    let utc = local.checked_sub(Duration::seconds(offset_seconds))?;
    (1..=9999).contains(&utc.year()).then(|| utc.assume_utc())
}
