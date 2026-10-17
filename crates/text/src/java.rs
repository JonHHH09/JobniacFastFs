//! Java string and character semantics.
//!
//! These rules follow the Java 25 JDK: the `Character.isWhitespace` set, `String.strip` over that set, blank checks, `String.lines`,
//! UTF-16 lengths, UTF-16 code-unit ordering and the preparation budget. Rust's `str::trim`,
//! `char::is_whitespace`, `str::lines` and `str` ordering differ from them, so this module
//! reproduces the Java rules exactly. Inputs are valid Unicode: Rust strings cannot hold lone
//! surrogates.

use std::cmp::Ordering;
use std::fmt;

/// Most UTF-16 code units one preparation may count.
pub const MAX_PREPARATION_UNITS: usize = 1_000_000;

/// Most entries one preparation may count.
pub const MAX_PREPARATION_ENTRIES: usize = 10_000;

/// Returns true for the 25 members of the Java whitespace set (`Character.isWhitespace`).
///
/// The set is U+0009 to U+000D, U+001C to U+001F, U+0020, U+1680, U+2000 to U+2006, U+2008 to
/// U+200A, U+2028, U+2029, U+205F and U+3000. U+0085, U+00A0, U+2007 and U+202F are excluded.
pub fn java_is_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{09}'..='\u{0D}'
            | '\u{1C}'..='\u{1F}'
            | ' '
            | '\u{1680}'
            | '\u{2000}'..='\u{2006}'
            | '\u{2008}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{205F}'
            | '\u{3000}'
    )
}

/// Removes leading and trailing characters of the Java whitespace set.
pub fn java_strip(value: &str) -> &str {
    value.trim_matches(java_is_whitespace)
}

/// True for the empty string or a string made only of the Java whitespace set.
pub fn java_is_blank(value: &str) -> bool {
    value.chars().all(java_is_whitespace)
}

/// Splits like Java `String.lines`: CRLF, a lone CR and a lone LF are terminators.
///
/// A terminator at the end adds no empty element, and empty input gives no lines. Unlike
/// `str::lines`, a lone CR also splits, and a trailing CR is not kept.
pub fn java_lines(value: &str) -> Vec<&str> {
    if value.is_empty() {
        return Vec::new();
    }
    let bytes = value.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                lines.push(&value[start..index]);
                index += 1;
                start = index;
            }
            b'\r' => {
                lines.push(&value[start..index]);
                index += 1;
                if bytes.get(index) == Some(&b'\n') {
                    index += 1;
                }
                start = index;
            }
            _ => index += 1,
        }
    }
    if start < value.len() {
        lines.push(&value[start..]);
    }
    lines
}

/// The UTF-16BE bytes of `value`.
///
/// Bytewise order of these keys equals Java `String.compareTo`, which is UTF-16 code-unit order.
pub fn java_string_key(value: &str) -> Vec<u8> {
    value.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

/// Orders two strings by UTF-16 code units, like Java `String.compareTo`.
///
/// Rust's `str` ordering compares UTF-8 bytes (code point order), which differs for astral
/// characters against U+E000 to U+FFFF.
pub fn java_compare(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

/// The Java `String.length()` of `value`: its count of UTF-16 code units.
pub fn utf16_length(value: &str) -> usize {
    value.encode_utf16().count()
}

/// The longest prefix of `value` that holds at most `max_units` UTF-16 code units.
///
/// A surrogate pair is never split: a character whose units do not all fit is dropped whole, which
/// is what truncating the UTF-16LE bytes and decoding them leniently gives. The cap is unsigned.
pub fn truncate_utf16(value: &str, max_units: usize) -> &str {
    let mut units = 0;
    for (index, character) in value.char_indices() {
        units += character.len_utf16();
        if units > max_units {
            return &value[..index];
        }
    }
    value
}

/// The fixed field label of a preparation budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetField {
    /// Profile input (`field` is `profile`).
    Profile,
    /// Text input (`field` is `text`).
    Text,
}

impl BudgetField {
    /// The label used in the validation details (`profile` or `text`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Text => "text",
        }
    }
}

/// A preparation budget was exceeded. The message is caller-safe and names no input value.
///
/// The core maps it to the `validation_error` envelope with `field` and `reason` details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparationLimitExceeded {
    field: BudgetField,
}

impl PreparationLimitExceeded {
    /// The validation message.
    pub const MESSAGE: &'static str = "Resume preparation limit exceeded";

    /// The validation reason.
    pub const REASON: &'static str = "exceeds preparation safety budget";

    /// The field whose budget was exceeded.
    pub fn field(&self) -> BudgetField {
        self.field
    }

    /// The validation message.
    pub fn message(&self) -> &'static str {
        Self::MESSAGE
    }

    /// The validation reason.
    pub fn reason(&self) -> &'static str {
        Self::REASON
    }
}

impl fmt::Display for PreparationLimitExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(Self::MESSAGE)
    }
}

impl std::error::Error for PreparationLimitExceeded {}

/// Per-call safety counters for resume preparation.
///
/// The counters count UTF-16 units (cap [`MAX_PREPARATION_UNITS`]) and entries (cap
/// [`MAX_PREPARATION_ENTRIES`]). A refused add leaves both counters unchanged. Unrelated profile
/// metadata is outside this budget, and no partial result is authorized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparationBudget {
    field: BudgetField,
    units: usize,
    entries: usize,
}

impl PreparationBudget {
    /// A new budget with both counters at zero.
    pub fn new(field: BudgetField) -> Self {
        Self {
            field,
            units: 0,
            entries: 0,
        }
    }

    /// The field this budget reports in its errors.
    pub fn field(&self) -> BudgetField {
        self.field
    }

    /// The UTF-16 units counted so far.
    pub fn units(&self) -> usize {
        self.units
    }

    /// The entries counted so far.
    pub fn entries(&self) -> usize {
        self.entries
    }

    /// Counts the UTF-16 units of `text`. `None` is a no-op.
    pub fn add_text(&mut self, text: Option<&str>) -> Result<(), PreparationLimitExceeded> {
        match text {
            None => Ok(()),
            Some(text) => self.add_units(utf16_length(text)),
        }
    }

    /// Counts `count` UTF-16 units, for validated context totals that are not re-encoded.
    pub fn add_units(&mut self, count: usize) -> Result<(), PreparationLimitExceeded> {
        if count > MAX_PREPARATION_UNITS - self.units {
            return Err(self.exceeded());
        }
        self.units += count;
        Ok(())
    }

    /// Counts `count` entries.
    pub fn add_entries(&mut self, count: usize) -> Result<(), PreparationLimitExceeded> {
        if count > MAX_PREPARATION_ENTRIES - self.entries {
            return Err(self.exceeded());
        }
        self.entries += count;
        Ok(())
    }

    fn exceeded(&self) -> PreparationLimitExceeded {
        PreparationLimitExceeded { field: self.field }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::Value;

    const TEXT_VECTORS: &str = include_str!("../tests/golden/java-text.json");
    const BUDGET_VECTORS: &str = include_str!("../tests/golden/java-budget.json");

    fn parse(source: &str) -> Value {
        serde_json::from_str(source).expect("golden JSON parses")
    }

    fn string_field<'a>(value: &'a Value, key: &str) -> &'a str {
        value[key].as_str().expect("string field")
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Parses inclusive hexadecimal ranges such as "0009-000D" or "0020".
    fn parse_ranges(items: &[Value]) -> Vec<(u32, u32)> {
        items
            .iter()
            .map(|item| {
                let text = item.as_str().expect("range string");
                let (start, end) = text.split_once('-').unwrap_or((text, text));
                (
                    u32::from_str_radix(start, 16).expect("range start"),
                    u32::from_str_radix(end, 16).expect("range end"),
                )
            })
            .collect()
    }

    fn in_ranges(ranges: &[(u32, u32)], code_point: u32) -> bool {
        ranges
            .binary_search_by(|&(start, end)| {
                if end < code_point {
                    Ordering::Less
                } else if start > code_point {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            })
            .is_ok()
    }

    #[test]
    fn whitespace_set_matches_golden_for_every_code_point() {
        let root = parse(TEXT_VECTORS);
        let ranges = parse_ranges(root["whitespace"]["ranges"].as_array().expect("ranges"));
        let mut mismatches = Vec::new();
        let mut checked = 0_u32;
        for code_point in 0..=0x10FFFF_u32 {
            let Some(character) = char::from_u32(code_point) else {
                continue;
            };
            checked += 1;
            if java_is_whitespace(character) != in_ranges(&ranges, code_point) {
                mismatches.push(code_point);
            }
        }
        assert_eq!(checked, 1_112_064);
        assert!(
            mismatches.is_empty(),
            "whitespace mismatches: {mismatches:?}"
        );
        assert_eq!(
            ranges
                .iter()
                .map(|(start, end)| end - start + 1)
                .sum::<u32>(),
            25
        );
    }

    #[test]
    fn strip_matches_golden() {
        let root = parse(TEXT_VECTORS);
        for case in root["strip"].as_array().expect("strip") {
            let input = string_field(case, "input");
            assert_eq!(java_strip(input), string_field(case, "output"), "{input:?}");
        }
    }

    #[test]
    fn blank_matches_golden() {
        let root = parse(TEXT_VECTORS);
        for case in root["is_blank"].as_array().expect("is_blank") {
            let input = string_field(case, "input");
            assert_eq!(
                java_is_blank(input),
                case["result"].as_bool().expect("bool"),
                "{input:?}"
            );
        }
    }

    #[test]
    fn lines_match_golden() {
        let root = parse(TEXT_VECTORS);
        for case in root["lines"].as_array().expect("lines") {
            let input = string_field(case, "input");
            let expected: Vec<&str> = case["lines"]
                .as_array()
                .expect("lines array")
                .iter()
                .map(|line| line.as_str().expect("line"))
                .collect();
            assert_eq!(java_lines(input), expected, "{input:?}");
        }
    }

    #[test]
    fn utf16_length_matches_golden() {
        let root = parse(TEXT_VECTORS);
        for case in root["utf16_length"].as_array().expect("utf16_length") {
            let input = string_field(case, "input");
            let units = case["units"].as_u64().expect("units") as usize;
            assert_eq!(utf16_length(input), units, "{input:?}");
        }
    }

    #[test]
    fn truncate_utf16_matches_golden() {
        let root = parse(TEXT_VECTORS);
        let cases = root["truncate_utf16"].as_array().expect("truncate_utf16");
        assert!(
            cases.len() >= 20,
            "truncate_utf16 has {} cases",
            cases.len()
        );
        for case in cases {
            let input = string_field(case, "input");
            let maximum = case["maximum"].as_u64().expect("maximum") as usize;
            assert_eq!(
                truncate_utf16(input, maximum),
                string_field(case, "output"),
                "{input:?} at {maximum}"
            );
        }
    }

    #[test]
    fn truncate_utf16_never_splits_a_surrogate_pair() {
        let astral = "a\u{1F600}b";
        assert_eq!(truncate_utf16(astral, 0), "");
        assert_eq!(truncate_utf16(astral, 1), "a");
        assert_eq!(truncate_utf16(astral, 2), "a");
        assert_eq!(truncate_utf16(astral, 3), "a\u{1F600}");
        assert_eq!(truncate_utf16(astral, 4), "a\u{1F600}b");
        assert_eq!(truncate_utf16("\u{1F600}\u{1F600}", 3), "\u{1F600}");
        assert_eq!(truncate_utf16("", 0), "");
    }

    #[test]
    fn string_key_matches_golden() {
        let root = parse(TEXT_VECTORS);
        for case in root["string_key"].as_array().expect("string_key") {
            let input = string_field(case, "input");
            assert_eq!(
                hex(&java_string_key(input)),
                string_field(case, "utf16be_hex"),
                "{input:?}"
            );
        }
    }

    #[test]
    fn order_matches_java_code_unit_order() {
        let root = parse(TEXT_VECTORS);
        let mut inputs: Vec<&str> = root["string_key"]
            .as_array()
            .expect("string_key")
            .iter()
            .map(|case| string_field(case, "input"))
            .collect();
        inputs.sort_by(|left, right| java_compare(left, right));
        let expected: Vec<&str> = root["order"]["sorted"]
            .as_array()
            .expect("sorted")
            .iter()
            .map(|item| item.as_str().expect("sorted item"))
            .collect();
        assert_eq!(inputs, expected);
        assert_eq!(
            java_string_key("\u{1F600}").cmp(&java_string_key("\u{E000}")),
            Ordering::Less,
            "astral sorts before U+E000 in UTF-16 order"
        );
    }

    #[test]
    fn budget_matches_golden_steps_and_counters() {
        let root = parse(BUDGET_VECTORS);
        assert_eq!(
            root["limits"]["units"].as_u64(),
            Some(MAX_PREPARATION_UNITS as u64)
        );
        assert_eq!(
            root["limits"]["entries"].as_u64(),
            Some(MAX_PREPARATION_ENTRIES as u64)
        );
        for case in root["cases"].as_array().expect("cases") {
            let name = string_field(case, "name");
            let field = match string_field(case, "field") {
                "profile" => BudgetField::Profile,
                _ => BudgetField::Text,
            };
            let mut budget = PreparationBudget::new(field);
            let steps = case["steps"].as_array().expect("steps");
            let expected = case["expected"].as_array().expect("expected");
            assert_eq!(steps.len(), expected.len(), "{name}");
            for (step, outcome) in steps.iter().zip(expected) {
                let result = match string_field(step, "op") {
                    "units" => budget.add_units(step["count"].as_u64().expect("count") as usize),
                    "entries" => {
                        budget.add_entries(step["count"].as_u64().expect("count") as usize)
                    }
                    "text" => budget.add_text(Some(string_field(step, "value"))),
                    "text_none" => budget.add_text(None),
                    "text_repeat" => {
                        let count = step["count"].as_u64().expect("count") as usize;
                        let text = string_field(step, "value").repeat(count);
                        budget.add_text(Some(&text))
                    }
                    other => panic!("unknown op {other}"),
                };
                match string_field(outcome, "result") {
                    "ok" => assert!(result.is_ok(), "{name}: expected ok"),
                    _ => {
                        let error = result.expect_err("expected exceeded");
                        let envelope = &outcome["envelope"];
                        assert_eq!(error.message(), string_field(envelope, "message"), "{name}");
                        assert_eq!(
                            error.field().as_str(),
                            envelope["details"]["field"].as_str().expect("field"),
                            "{name}"
                        );
                        assert_eq!(
                            error.reason(),
                            envelope["details"]["reason"].as_str().expect("reason"),
                            "{name}"
                        );
                        assert_eq!(envelope["code"].as_str(), Some("validation_error"));
                    }
                }
                assert_eq!(
                    budget.units() as u64,
                    outcome["units"].as_u64().expect("units"),
                    "{name}: units"
                );
                assert_eq!(
                    budget.entries() as u64,
                    outcome["entries"].as_u64().expect("entries"),
                    "{name}: entries"
                );
            }
        }
    }

    #[test]
    fn budget_error_is_displayed_with_its_message() {
        let mut budget = PreparationBudget::new(BudgetField::Text);
        let error = budget
            .add_units(MAX_PREPARATION_UNITS + 1)
            .expect_err("over cap");
        assert_eq!(error.to_string(), PreparationLimitExceeded::MESSAGE);
        assert_eq!(budget.units(), 0);
    }

    #[test]
    fn excluded_non_breaking_characters_are_not_java_whitespace() {
        assert!(java_is_whitespace(' '));
        assert!(!java_is_whitespace('\u{A0}'));
        assert!(!java_is_whitespace('\u{85}'));
        assert!(!java_is_whitespace('\u{2007}'));
        assert!(!java_is_whitespace('\u{202F}'));
        assert!(java_is_whitespace('\u{1C}'));
    }
}
