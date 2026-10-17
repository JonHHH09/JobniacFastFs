//! PDF number formatting.

/// Formats a number as PDF content-stream text: six decimals with trailing zeros and the decimal
/// point removed. Negative values that round to zero keep their sign
/// (`-0.000000` becomes `-0`). An empty result becomes `0`.
pub fn format_number(value: f64) -> String {
    let fixed = format!("{value:.6}");
    let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::format_number;

    #[test]
    fn strips_trailing_zeros_and_point() {
        assert_eq!(format_number(54.0), "54");
        assert_eq!(format_number(13.5), "13.5");
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(64.0 / 255.0), "0.25098");
        assert_eq!(format_number(1.0), "1");
    }

    #[test]
    fn keeps_the_sign_of_negative_zero() {
        assert_eq!(format_number(-0.0), "-0");
        assert_eq!(format_number(-0.0000001), "-0");
    }
}
