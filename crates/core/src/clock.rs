//! Clock port and `java.time.Instant` wire formatting.
//!
//! Instants are UTC and carry microsecond
//! precision (the precision of the stored instants).
//!
//! `parse_instant` accepts ASCII digits only, not other Unicode decimal digits.

use time::{Date, Month, OffsetDateTime, Time, UtcOffset};

use crate::errors::AppError;

const INSTANT_MESSAGE: &str = "instant must be an ISO-8601 UTC timestamp";

/// Source of the current instant (UTC, truncated to microseconds).
pub trait Clock: Send + Sync + std::fmt::Debug {
    fn now(&self) -> OffsetDateTime;
}

/// The system clock, truncated to microseconds.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        truncate_to_micros(OffsetDateTime::now_utc())
    }
}

/// A deterministic clock for tests and reproducible runs.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(OffsetDateTime);

impl FixedClock {
    pub fn new(instant: OffsetDateTime) -> Self {
        Self(truncate_to_micros(instant.to_offset(UtcOffset::UTC)))
    }
}

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.0
    }
}

/// Drop sub-microsecond precision and normalize to UTC.
pub fn truncate_to_micros(value: OffsetDateTime) -> OffsetDateTime {
    let utc = value.to_offset(UtcOffset::UTC);
    let micros = utc.nanosecond() / 1_000;
    utc.replace_nanosecond(micros * 1_000).unwrap_or(utc)
}

/// Format like `Instant.toString()`: UTC `Z` and 0, 3 or 6 fraction digits.
///
/// `2026-09-22T19:26:28Z`, `2026-09-22T19:26:28.500Z`, `2026-09-22T19:26:28.717138Z`.
pub fn format_instant(value: OffsetDateTime) -> String {
    let utc = truncate_to_micros(value);
    let base = format_base(utc);
    let micros = utc.nanosecond() / 1_000;
    if micros == 0 {
        format!("{base}Z")
    } else if micros.is_multiple_of(1_000) {
        format!("{base}.{:03}Z", micros / 1_000)
    } else {
        format!("{base}.{micros:06}Z")
    }
}

/// ISO-8601 for an aware UTC value with `+00:00` replaced by `Z`:
/// six fraction digits when microseconds are non-zero, none otherwise.
pub fn format_isoformat_utc(value: OffsetDateTime) -> String {
    let utc = truncate_to_micros(value);
    let base = format_base(utc);
    let micros = utc.nanosecond() / 1_000;
    if micros == 0 {
        format!("{base}Z")
    } else {
        format!("{base}.{micros:06}Z")
    }
}

fn format_base(utc: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        utc.year(),
        u8::from(utc.month()),
        utc.day(),
        utc.hour(),
        utc.minute(),
        utc.second()
    )
}

/// Parse an ISO-8601 UTC instant (`...Z`); sub-microsecond digits are truncated.
///
/// Accepted shape (after stripping surrounding Unicode whitespace):
/// `YYYY-MM-DDTHH:MM[:SS[.F{1,9}]]Z`, with a year range of 1 to 9999.
pub fn parse_instant(text: &str) -> Result<OffsetDateTime, AppError> {
    let invalid = || AppError::DomainValue(INSTANT_MESSAGE.to_owned());
    let text = text.trim_matches(is_unicode_whitespace);
    let bytes = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        let slice = bytes.get(range)?;
        if slice.is_empty() || !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(slice).ok()?.parse().ok()
    };
    if bytes.len() < 17 || bytes.last() != Some(&b'Z') {
        return Err(invalid());
    }
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' || bytes[13] != b':' {
        return Err(invalid());
    }
    let year = digits(0..4).ok_or_else(invalid)?;
    let month = digits(5..7).ok_or_else(invalid)?;
    let day = digits(8..10).ok_or_else(invalid)?;
    let hour = digits(11..13).ok_or_else(invalid)?;
    let minute = digits(14..16).ok_or_else(invalid)?;
    let rest = &bytes[16..bytes.len() - 1];
    let (second, micros) = match rest {
        [] => (0, 0),
        [b':', s1, s2, tail @ ..] => {
            let second = digits_of(&[*s1, *s2]).ok_or_else(invalid)?;
            let micros = match tail {
                [] => 0,
                [b'.', fraction @ ..] if (1..=9).contains(&fraction.len()) => {
                    if !fraction.iter().all(u8::is_ascii_digit) {
                        return Err(invalid());
                    }
                    let mut padded = [b'0'; 6];
                    for (slot, digit) in padded.iter_mut().zip(fraction.iter()) {
                        *slot = *digit;
                    }
                    digits_of(&padded).ok_or_else(invalid)?
                }
                _ => return Err(invalid()),
            };
            (second, micros)
        }
        _ => return Err(invalid()),
    };
    if !(1..=9999).contains(&year) {
        return Err(invalid());
    }
    let month = u8::try_from(month)
        .ok()
        .and_then(|m| Month::try_from(m).ok())
        .ok_or_else(invalid)?;
    let date = Date::from_calendar_date(
        i32::try_from(year).map_err(|_| invalid())?,
        month,
        u8::try_from(day).map_err(|_| invalid())?,
    )
    .map_err(|_| invalid())?;
    let time = Time::from_hms_micro(
        u8::try_from(hour).map_err(|_| invalid())?,
        u8::try_from(minute).map_err(|_| invalid())?,
        u8::try_from(second).map_err(|_| invalid())?,
        micros,
    )
    .map_err(|_| invalid())?;
    Ok(date.with_time(time).assume_utc())
}

fn digits_of(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn is_unicode_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{09}'..='\u{0D}'
            | '\u{1C}'..='\u{20}'
            | '\u{85}'
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn formats_zero_three_or_six_fraction_digits() {
        assert_eq!(
            format_instant(datetime!(2026-09-22 19:26:28 UTC)),
            "2026-09-22T19:26:28Z"
        );
        assert_eq!(
            format_instant(datetime!(2026-09-22 19:26:28.5 UTC)),
            "2026-09-22T19:26:28.500Z"
        );
        assert_eq!(
            format_instant(datetime!(2026-09-22 19:26:28.717138 UTC)),
            "2026-09-22T19:26:28.717138Z"
        );
        assert_eq!(
            format_instant(datetime!(2026-09-22 19:26:28.717138999 UTC)),
            "2026-09-22T19:26:28.717138Z"
        );
    }

    #[test]
    fn isoformat_uses_six_digits_or_none() {
        assert_eq!(
            format_isoformat_utc(datetime!(2026-01-02 03:04:05 UTC)),
            "2026-01-02T03:04:05Z"
        );
        assert_eq!(
            format_isoformat_utc(datetime!(2026-01-02 03:04:05.5 UTC)),
            "2026-01-02T03:04:05.500000Z"
        );
    }

    #[test]
    fn parses_accepted_forms() {
        let parse = |text: &str| parse_instant(text).map(format_instant).unwrap_or_default();
        assert_eq!(parse("2026-09-22T19:26Z"), "2026-09-22T19:26:00Z");
        assert_eq!(parse(" 2026-09-22T19:26:28Z\n"), "2026-09-22T19:26:28Z");
        assert_eq!(parse("2026-09-22T19:26:28.1Z"), "2026-09-22T19:26:28.100Z");
        assert_eq!(
            parse("2026-09-22T19:26:28.123456789Z"),
            "2026-09-22T19:26:28.123456Z"
        );
    }

    #[test]
    fn rejects_invalid_forms() {
        for text in [
            "",
            "2026-09-22T19:26:28",
            "2026-09-22 19:26:28Z",
            "2026-09-22T19:26:28.Z",
            "2026-09-22T19:26:28.1234567890Z",
            "2026-02-30T00:00:00Z",
            "0000-01-01T00:00:00Z",
            "2026-09-22T24:00:00Z",
            "2026-09-22T19:26:60Z",
            "2026-09-22T19:26:28+00:00",
        ] {
            assert!(parse_instant(text).is_err(), "{text:?} should be rejected");
        }
    }
}
