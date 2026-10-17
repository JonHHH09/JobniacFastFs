//! JSON text in the three wire layouts of tool results, produced byte for byte.
//!
//! Tool-result text comes in three layouts, and clients see the bytes:
//!
//! - [`Layout::Compact`]: `json.dumps(envelope, separators=(',', ':'))`, the error text of the
//!   profile, job, deletion, link and Arbeitnow modules and of `list_profiles`.
//! - [`Layout::Default`]: `json.dumps(envelope)`, separators `", "` and `": "`, the error text of
//!   the match, review, document and generation modules.
//! - [`Layout::Indented`]: the text of a structured success, which the SDK renders with
//!   `to_json(indent=2)`. It does not escape non-ASCII characters.
//!
//! The error layouts use `ensure_ascii`: every character outside `' '..='~'` becomes a `\uXXXX`
//! escape in lowercase hexadecimal, and astral characters become surrogate pairs. `serde_json`
//! does not do either, which is why this module exists. Floats use the shortest round-trip form
//! with a `.0` suffix on whole numbers.

use serde_json::{Map, Value};

/// The layout of a JSON text (see the module documentation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `separators=(',', ':')`.
    Compact,
    /// The default separators `", "` and `": "`.
    Default,
    /// `indent=2`, one item per line.
    Indented,
}

/// Render `value` in `layout`. With `ascii`, the output is pure ASCII (`ensure_ascii`).
pub fn dumps(value: &Value, layout: Layout, ascii: bool) -> String {
    let mut out = String::new();
    write_value(value, layout, ascii, 0, &mut out);
    out
}

/// The `repr` form of a finite float, as `json.dumps` writes it (`NaN`, `Infinity` and `-Infinity`
/// for the non-finite values).
pub fn compact_float(value: f64) -> String {
    float_text(value, -4, true)
}

/// The text `to_json` gives a float in the structured-success layout. It uses positional
/// notation for decimal exponents -5 to 15 (the compact layouts start at -4) and scientific
/// notation otherwise, with an unpadded exponent: `0.00001` and `2.5e-7` where `json.dumps` gives `1e-05`
/// and `2.5e-07`.
pub fn indented_float(value: f64) -> String {
    float_text(value, -5, false)
}

/// Shared float text: positional for decimal exponents from `lowest` to 15, scientific otherwise.
/// `padded` gives the two-digit exponent of the compact layouts.
fn float_text(value: f64, lowest: i32, padded: bool) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .to_owned();
    }
    let scientific = format!("{value:e}");
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return scientific;
    };
    let Ok(exponent) = exponent.parse::<i32>() else {
        return scientific;
    };
    let (negative, unsigned) = match mantissa.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, mantissa),
    };
    let digits: String = unsigned.chars().filter(char::is_ascii_digit).collect();
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    // Positional notation for decimal exponents from `lowest` to 15, scientific otherwise.
    if (lowest..16).contains(&exponent) {
        if exponent >= 0 {
            let integer_digits = usize::try_from(exponent).unwrap_or(0) + 1;
            if digits.len() <= integer_digits {
                out.push_str(&digits);
                out.push_str(&"0".repeat(integer_digits - digits.len()));
                out.push_str(".0");
            } else {
                out.push_str(&digits[..integer_digits]);
                out.push('.');
                out.push_str(&digits[integer_digits..]);
            }
        } else {
            out.push_str("0.");
            out.push_str(&"0".repeat(usize::try_from(-exponent - 1).unwrap_or(0)));
            out.push_str(&digits);
        }
    } else {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exponent < 0 { '-' } else { '+' });
        if padded {
            out.push_str(&format!("{:02}", exponent.unsigned_abs()));
        } else {
            out.push_str(&exponent.unsigned_abs().to_string());
        }
    }
    out
}

fn write_value(value: &Value, layout: Layout, ascii: bool, depth: usize, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => out.push_str(&number_text(number, layout)),
        Value::String(text) => write_string(text, ascii, out),
        Value::Array(items) => write_array(items, layout, ascii, depth, out),
        Value::Object(entries) => write_object(entries, layout, ascii, depth, out),
    }
}

fn number_text(number: &serde_json::Number, layout: Layout) -> String {
    if let Some(integer) = number.as_i64() {
        integer.to_string()
    } else if let Some(integer) = number.as_u64() {
        integer.to_string()
    } else {
        number.as_f64().map_or_else(
            || "null".to_owned(),
            |float| match layout {
                Layout::Indented => indented_float(float),
                Layout::Compact | Layout::Default => compact_float(float),
            },
        )
    }
}

fn write_array(items: &[Value], layout: Layout, ascii: bool, depth: usize, out: &mut String) {
    if items.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push('[');
    for (index, item) in items.iter().enumerate() {
        separate(layout, index, depth + 1, out);
        write_value(item, layout, ascii, depth + 1, out);
    }
    close(layout, depth, out);
    out.push(']');
}

fn write_object(
    entries: &Map<String, Value>,
    layout: Layout,
    ascii: bool,
    depth: usize,
    out: &mut String,
) {
    if entries.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push('{');
    for (index, (key, value)) in entries.iter().enumerate() {
        separate(layout, index, depth + 1, out);
        write_string(key, ascii, out);
        out.push_str(if layout == Layout::Compact { ":" } else { ": " });
        write_value(value, layout, ascii, depth + 1, out);
    }
    close(layout, depth, out);
    out.push('}');
}

/// Write what precedes item `index` of a container at `depth` (the item separator, and the line
/// break and indentation of the indented layout).
fn separate(layout: Layout, index: usize, depth: usize, out: &mut String) {
    match layout {
        Layout::Compact => {
            if index > 0 {
                out.push(',');
            }
        }
        Layout::Default => {
            if index > 0 {
                out.push_str(", ");
            }
        }
        Layout::Indented => {
            if index > 0 {
                out.push(',');
            }
            out.push('\n');
            push_indent(depth, out);
        }
    }
}

/// Write what precedes the closing bracket of a non-empty container at `depth`.
fn close(layout: Layout, depth: usize, out: &mut String) {
    if layout == Layout::Indented {
        out.push('\n');
        push_indent(depth, out);
    }
}

fn push_indent(depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_string(text: &str, ascii: bool, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(character),
            control if (control as u32) < 0x20 => push_unit_escape(control as u32 as u16, out),
            other if ascii => {
                let mut units = [0_u16; 2];
                for unit in other.encode_utf16(&mut units) {
                    push_unit_escape(*unit, out);
                }
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

fn push_unit_escape(unit: u16, out: &mut String) {
    out.push_str(&format!("\\u{unit:04x}"));
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn compact_and_default_layouts_match_json_dumps() {
        let value = json!({"code": "conflict", "details": {"a": "1", "b": ["x", 2]}});
        assert_eq!(
            dumps(&value, Layout::Compact, true),
            r#"{"code":"conflict","details":{"a":"1","b":["x",2]}}"#
        );
        assert_eq!(
            dumps(&value, Layout::Default, true),
            r#"{"code": "conflict", "details": {"a": "1", "b": ["x", 2]}}"#
        );
    }

    #[test]
    fn ascii_mode_escapes_like_ensure_ascii() {
        let value = json!("M\u{fc}ller \u{1F642} \u{7f} \u{1} \"q\" \\");
        assert_eq!(
            dumps(&value, Layout::Compact, true),
            r#""M\u00fcller \ud83d\ude42 \u007f \u0001 \"q\" \\""#
        );
        assert_eq!(
            dumps(&value, Layout::Compact, false),
            "\"M\u{fc}ller \u{1F642} \u{7f} \\u0001 \\\"q\\\" \\\\\""
        );
    }

    #[test]
    fn indented_layout_uses_two_space_indent() {
        let value = json!({"profiles": [], "nextCursor": null, "n": {"a": [1, 2]}});
        assert_eq!(
            dumps(&value, Layout::Indented, false),
            "{\n  \"profiles\": [],\n  \"nextCursor\": null,\n  \"n\": {\n    \"a\": [\n      1,\n      2\n    ]\n  }\n}"
        );
        assert_eq!(dumps(&json!({}), Layout::Indented, false), "{}");
    }

    #[test]
    fn floats_follow_repr_form() {
        assert_eq!(compact_float(1.0), "1.0");
        assert_eq!(compact_float(-0.0), "-0.0");
        assert_eq!(compact_float(100.0), "100.0");
        assert_eq!(compact_float(1.5), "1.5");
        assert_eq!(compact_float(0.0001), "0.0001");
        assert_eq!(compact_float(1e-5), "1e-05");
        assert_eq!(compact_float(1e16), "1e+16");
        assert_eq!(compact_float(1.5e16), "1.5e+16");
        assert_eq!(compact_float(1234.5678), "1234.5678");
        assert_eq!(compact_float(-2.5e-7), "-2.5e-07");
    }

    #[test]
    fn floats_match_to_json_and_json_dumps_goldens() -> Result<(), Box<dyn std::error::Error>> {
        // tests/golden/indented-floats.json holds the expected texts. The indented layout is the
        // `to_json` text, the compact layout is `json.dumps` text (separators ",", ":", ASCII).
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../tests/golden/indented-floats.json"))?;
        assert_eq!(cases.len(), 28);
        for case in cases {
            let input = case["input"].as_str().ok_or("case without input")?;
            let value: f64 = input.parse()?;
            let object = json!({ "x": value });
            assert_eq!(
                dumps(&object, Layout::Indented, false),
                case["indented"].as_str().ok_or("no indented text")?,
                "indented text of {input}"
            );
            assert_eq!(
                dumps(&object, Layout::Compact, true),
                case["compact"].as_str().ok_or("no compact text")?,
                "compact text of {input}"
            );
        }
        Ok(())
    }
}
