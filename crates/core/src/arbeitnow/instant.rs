//! Java `Instant` range and text for Arbeitnow candidates.

use crate::errors::{ApplicationError, validation_error};

/// Smallest Java `Instant` epoch second.
pub const MIN_EPOCH_SECOND: i64 = -31_557_014_167_219_200;
/// Largest Java `Instant` epoch second.
pub const MAX_EPOCH_SECOND: i64 = 31_556_889_864_403_199;

const INVALID_MESSAGE: &str = "Invalid Arbeitnow candidate token";
const SECONDS_PER_DAY: i64 = 86_400;

/// Java Instant's full epoch-second range with an exact nanosecond fraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArbeitnowInstant {
    epoch_second: i64,
    nanosecond: u32,
}

impl ArbeitnowInstant {
    /// Build an instant; the second must be in the Java range and the nanosecond below one second.
    pub fn new(epoch_second: i64, nanosecond: u32) -> Result<Self, ApplicationError> {
        if !(MIN_EPOCH_SECOND..=MAX_EPOCH_SECOND).contains(&epoch_second)
            || nanosecond > 999_999_999
        {
            return Err(validation_error(INVALID_MESSAGE, None, None));
        }
        Ok(Self {
            epoch_second,
            nanosecond,
        })
    }

    /// Seconds since the Unix epoch.
    pub const fn epoch_second(&self) -> i64 {
        self.epoch_second
    }

    /// Nanosecond of the second.
    pub const fn nanosecond(&self) -> u32 {
        self.nanosecond
    }

    /// Shift by whole seconds; `None` when the result leaves the Java range.
    pub fn checked_add_seconds(&self, seconds: i64) -> Option<Self> {
        let shifted = self.epoch_second.checked_add(seconds)?;
        if (MIN_EPOCH_SECOND..=MAX_EPOCH_SECOND).contains(&shifted) {
            Some(Self {
                epoch_second: shifted,
                nanosecond: self.nanosecond,
            })
        } else {
            None
        }
    }
}

/// Render like Jackson / `Instant.toString` over the full Java Instant range.
pub fn format_java_instant(instant: ArbeitnowInstant) -> String {
    let days = instant.epoch_second.div_euclid(SECONDS_PER_DAY);
    let seconds = instant.epoch_second.rem_euclid(SECONDS_PER_DAY);
    // Howard Hinnant's civil-from-days algorithm with a proleptic Gregorian calendar.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let year_text = if year > 9999 {
        format!("+{year}")
    } else if year >= 0 {
        format!("{year:04}")
    } else {
        format!("-{:04}", -year)
    };
    let hour = seconds / 3600;
    let minute = seconds % 3600 / 60;
    let second = seconds % 60;
    let nano = instant.nanosecond;
    let fraction = if nano == 0 {
        String::new()
    } else if nano.is_multiple_of(1_000_000) {
        format!(".{:03}", nano / 1_000_000)
    } else if nano.is_multiple_of(1000) {
        format!(".{:06}", nano / 1000)
    } else {
        format!(".{nano:09}")
    };
    format!("{year_text}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}{fraction}Z")
}
