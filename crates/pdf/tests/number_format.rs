//! PDF number formatting against the golden vectors.
//!
//! Inputs are IEEE-754 double bit patterns recorded in `tests/golden/pdf-generic.json`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use jobniac_pdf::layout::number::format_number;
use serde_json::Value;

const GOLDEN: &str = include_str!("golden/pdf-generic.json");

#[test]
fn format_matches_the_golden_vectors_for_every_recorded_value() {
    let golden: Value = serde_json::from_str(GOLDEN).expect("golden JSON parses");
    let rows = golden["numbers"].as_array().expect("numbers array");
    assert!(rows.len() >= 20);
    for row in rows {
        let bits = u64::from_str_radix(row["bits"].as_str().unwrap(), 16).unwrap();
        let value = f64::from_bits(bits);
        let wanted = row["output"].as_str().unwrap();
        assert_eq!(format_number(value), wanted, "value bits {bits:016x}");
    }
}

#[test]
fn negative_zero_and_rounding_edges() {
    assert_eq!(format_number(-0.0), "-0");
    assert_eq!(format_number(0.0), "0");
    // Exact binary ties round to even.
    assert_eq!(format_number(0.0078125), "0.007812");
}
