//! Arbeitnow instant range and ISO instant text.

use jobniac_core::arbeitnow::instant::{
    ArbeitnowInstant, MAX_EPOCH_SECOND, MIN_EPOCH_SECOND, format_java_instant,
};
use jobniac_core::errors::ErrorCode;

fn text(epoch: i64, nano: u32) -> String {
    format_java_instant(ArbeitnowInstant::new(epoch, nano).expect("in range"))
}

#[test]
fn instants_use_java_iso_instant_text() {
    let cases: [(i64, u32, &str); 14] = [
        (0, 0, "1970-01-01T00:00:00Z"),
        (0, 500_000_000, "1970-01-01T00:00:00.500Z"),
        (1, 123_456_000, "1970-01-01T00:00:01.123456Z"),
        (1, 123_456_789, "1970-01-01T00:00:01.123456789Z"),
        (100, 0, "1970-01-01T00:01:40Z"),
        (-1, 0, "1969-12-31T23:59:59Z"),
        (951_782_400, 0, "2000-02-29T00:00:00Z"),
        (1_760_000_000, 0, "2025-10-09T08:53:20Z"),
        (253_402_300_799, 0, "9999-12-31T23:59:59Z"),
        (253_402_300_800, 0, "+10000-01-01T00:00:00Z"),
        (-62_167_219_200, 0, "0000-01-01T00:00:00Z"),
        (-62_167_219_201, 0, "-0001-12-31T23:59:59Z"),
        (
            31_556_889_864_403_199,
            999_999_999,
            "+1000000000-12-31T23:59:59.999999999Z",
        ),
        (-31_557_014_167_219_200, 0, "-1000000000-01-01T00:00:00Z"),
    ];
    for (epoch, nano, expected) in cases {
        assert_eq!(text(epoch, nano), expected, "epoch {epoch} nano {nano}");
    }
}

#[test]
fn fraction_width_follows_trailing_zeros() {
    assert_eq!(text(0, 1), "1970-01-01T00:00:00.000000001Z");
    assert_eq!(text(0, 1000), "1970-01-01T00:00:00.000001Z");
    assert_eq!(text(0, 1_000_000), "1970-01-01T00:00:00.001Z");
    assert_eq!(text(0, 999_000_000), "1970-01-01T00:00:00.999Z");
}

#[test]
fn range_boundaries_are_checked() {
    assert!(ArbeitnowInstant::new(MIN_EPOCH_SECOND, 0).is_ok());
    assert!(ArbeitnowInstant::new(MAX_EPOCH_SECOND, 999_999_999).is_ok());
    for (second, nano) in [
        (MIN_EPOCH_SECOND - 1, 0),
        (MAX_EPOCH_SECOND + 1, 0),
        (0, 1_000_000_000),
        (i64::MIN, 0),
        (i64::MAX, 0),
    ] {
        let error = ArbeitnowInstant::new(second, nano).expect_err("out of range");
        assert_eq!(error.code(), ErrorCode::ValidationError);
        assert_eq!(error.message(), "Invalid Arbeitnow candidate token");
        assert!(error.details().is_empty());
    }
}

#[test]
fn ordering_is_second_then_nanosecond() {
    let a = ArbeitnowInstant::new(5, 999).unwrap();
    let b = ArbeitnowInstant::new(6, 0).unwrap();
    let c = ArbeitnowInstant::new(6, 1).unwrap();
    assert!(a < b && b < c);
    assert_eq!(a.epoch_second(), 5);
    assert_eq!(a.nanosecond(), 999);
}

#[test]
fn checked_add_seconds_stays_in_range() {
    let base = ArbeitnowInstant::new(100, 7).unwrap();
    let shifted = base.checked_add_seconds(900).unwrap();
    assert_eq!((shifted.epoch_second(), shifted.nanosecond()), (1000, 7));
    assert_eq!(base.checked_add_seconds(-200).unwrap().epoch_second(), -100);
    let max = ArbeitnowInstant::new(MAX_EPOCH_SECOND, 0).unwrap();
    assert!(max.checked_add_seconds(1).is_none());
    assert!(max.checked_add_seconds(0).is_some());
    assert!(max.checked_add_seconds(i64::MAX).is_none());
    let min = ArbeitnowInstant::new(MIN_EPOCH_SECOND, 0).unwrap();
    assert!(min.checked_add_seconds(-1).is_none());
    assert!(min.checked_add_seconds(i64::MIN).is_none());
}
