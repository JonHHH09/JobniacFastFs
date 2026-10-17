//! Admission of the stdin lines of the MCP server.
//!
//! rmcp parses every stdin line with `serde_json`. A line that holds a lone surrogate escape (a
//! high surrogate without a low partner, or a lone low surrogate) or that nests deeper than
//! `serde_json`'s limit of 128 fails to parse, and rmcp drops it without an answer. [`admit_line`]
//! repairs such a line before rmcp reads it. A `tools/call` line also gets the marker
//! [`REJECTED_INPUT_KEY`] in its `params._meta`, so the server answers that call with the normal
//! argument rejection of its tool and never runs the tool (see `server.rs`).

use serde_json::{Map, Value};

use crate::args::IssueKind;

/// The `_meta` key that marks a `tools/call` whose input was repaired at admission.
pub const REJECTED_INPUT_KEY: &str = "io.jobniac/rejected-input";

/// The deepest container nesting that admission keeps. A container that would open deeper is
/// replaced by `null`.
pub const MAX_DEPTH: usize = 100;

/// Why a line was repaired at admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// The line held a lone surrogate escape, which a Rust string cannot hold.
    LoneSurrogate,
    /// The line nested deeper than [`MAX_DEPTH`].
    TooDeep,
}

impl Rejection {
    /// The value of [`REJECTED_INPUT_KEY`] for this rejection.
    pub const fn meta_value(self) -> &'static str {
        match self {
            Self::LoneSurrogate => "lone_surrogate",
            Self::TooDeep => "too_deep",
        }
    }

    /// The rejection that a [`REJECTED_INPUT_KEY`] value names, or `None` for any other text.
    pub fn from_meta_value(text: &str) -> Option<Self> {
        match text {
            "lone_surrogate" => Some(Self::LoneSurrogate),
            "too_deep" => Some(Self::TooDeep),
            _ => None,
        }
    }

    /// The argument issue that reports this rejection.
    pub const fn issue_kind(self) -> IssueKind {
        match self {
            Self::LoneSurrogate => IssueKind::StringUnicode,
            Self::TooDeep => IssueKind::JsonInvalid,
        }
    }
}

/// One admitted stdin line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admitted {
    /// The text to hand to rmcp, without its line terminator.
    pub text: String,
    /// The rejection the line was repaired for, if any.
    pub rejection: Option<Rejection>,
}

/// Admit one stdin line (without its terminator).
///
/// The line is scanned first, whether or not it parses, so that nesting past [`MAX_DEPTH`] is
/// caught even where `serde_json` would accept it. A line that needed no repair is returned
/// unchanged, except that a `tools/call` loses any client-supplied [`REJECTED_INPUT_KEY`] in its
/// `params._meta`: only admission sets that marker. Otherwise the line is repaired: each lone
/// surrogate escape becomes `U+FFFD`, and each container nested deeper than [`MAX_DEPTH`] becomes
/// `null`. If the repaired text is a `tools/call` with an object `params`, the marker is set in
/// `params._meta` and the text is re-serialized compactly. Any other method gets the repaired text
/// with no rejection. A line that still does not parse is returned unchanged, so rmcp drops it as
/// before.
pub fn admit_line(line: &str) -> Admitted {
    admit_changed(line).unwrap_or_else(|| unchanged(line))
}

/// Like [`admit_line`], but `None` when the line goes through unchanged, so a caller that holds
/// the line already (the HTTP body, a stdin line) need not copy it.
///
/// A line that cannot need repair is recognised by one allocation-free scan: no `\u` escape, no
/// text `rejected` (the marker key can only be spelled differently through a `\u` escape) and no
/// container nested past [`MAX_DEPTH`]. Such a line is never parsed here.
pub fn admit_changed(line: &str) -> Option<Admitted> {
    if !may_need_admission(line) {
        return None;
    }
    let repaired = repair(line);
    let Ok(mut value) = serde_json::from_str::<Value>(&repaired.text) else {
        return None;
    };
    let Some(rejection) = repaired.rejection else {
        // Nothing was repaired, so the text is the line itself.
        if !strip_marker(&mut value) {
            return None;
        }
        return serde_json::to_string(&value).ok().map(|text| Admitted {
            text,
            rejection: None,
        });
    };
    if !mark_tools_call(&mut value, rejection) {
        return Some(Admitted {
            text: repaired.text,
            rejection: None,
        });
    }
    serde_json::to_string(&value).ok().map(|text| Admitted {
        text,
        rejection: Some(rejection),
    })
}

/// Whether the line could hold something admission acts on. `false` is certain: the line has no
/// `\u` escape, no `rejected` text and no container deeper than [`MAX_DEPTH`], so repair and the
/// marker strip would both leave it alone. The scan tracks strings and depth exactly as [`repair`]
/// does and allocates nothing.
fn may_need_admission(line: &str) -> bool {
    if line.contains("rejected") {
        return true;
    }
    let bytes = line.as_bytes();
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut at = 0_usize;
    while let Some(&byte) = bytes.get(at) {
        if in_string {
            match byte {
                b'"' => in_string = false,
                b'\\' => {
                    if bytes.get(at + 1) == Some(&b'u') {
                        return true;
                    }
                    at += 1;
                }
                _ => {}
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'[' | b'{' => {
                    if depth >= MAX_DEPTH {
                        return true;
                    }
                    depth += 1;
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        at += 1;
    }
    false
}

fn unchanged(line: &str) -> Admitted {
    Admitted {
        text: line.to_owned(),
        rejection: None,
    }
}

/// The result of the byte scan: the repaired text and the rejection it needs.
struct Repaired {
    text: String,
    rejection: Option<Rejection>,
}

/// Replace lone surrogate escapes and over-deep containers, byte by byte. Only ASCII bytes are
/// ever inserted or removed, so the output stays valid UTF-8.
fn repair(line: &str) -> Repaired {
    let bytes = line.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut lone_surrogate = false;
    let mut too_deep = false;
    let mut depth = 0_usize;
    let mut at = 0_usize;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'"' => at = copy_string(bytes, at, &mut out, &mut lone_surrogate),
            b'[' | b'{' if depth >= MAX_DEPTH => {
                out.extend_from_slice(b"null");
                too_deep = true;
                at = skip_container(bytes, at);
            }
            b'[' | b'{' => {
                depth += 1;
                out.push(byte);
                at += 1;
            }
            b']' | b'}' => {
                depth = depth.saturating_sub(1);
                out.push(byte);
                at += 1;
            }
            _ => {
                // A run of ordinary bytes up to the next structural one is copied at once.
                let end = bytes
                    .get(at..)
                    .and_then(|rest| {
                        rest.iter()
                            .position(|b| matches!(b, b'"' | b'[' | b'{' | b']' | b'}'))
                    })
                    .map_or(bytes.len(), |offset| at + offset);
                push_range(&mut out, bytes, at, end);
                at = end;
            }
        }
    }
    let rejection = if lone_surrogate {
        Some(Rejection::LoneSurrogate)
    } else if too_deep {
        Some(Rejection::TooDeep)
    } else {
        None
    };
    let text = String::from_utf8(out).unwrap_or_else(|_| line.to_owned());
    Repaired { text, rejection }
}

/// Copy the string that opens at `start` (a quote), repairing its surrogate escapes. A high
/// surrogate escape followed by a low one is a pair and is copied unchanged. Returns the index
/// just past the closing quote, or the end of the line.
fn copy_string(bytes: &[u8], start: usize, out: &mut Vec<u8>, lone_surrogate: &mut bool) -> usize {
    out.push(b'"');
    let mut at = start + 1;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'"' => {
                out.push(b'"');
                return at + 1;
            }
            b'\\' => match unicode_escape(bytes, at) {
                Some(unit)
                    if is_high(unit) && unicode_escape(bytes, at + 6).is_some_and(is_low) =>
                {
                    push_range(out, bytes, at, at + 12);
                    at += 12;
                }
                Some(unit) if is_high(unit) || is_low(unit) => {
                    out.extend_from_slice(b"\\ufffd");
                    *lone_surrogate = true;
                    at += 6;
                }
                Some(_) => {
                    push_range(out, bytes, at, at + 6);
                    at += 6;
                }
                None => {
                    // Any other escape, or a malformed one: copy the backslash and the next byte
                    // together, so an escaped backslash is never read as the start of an escape.
                    push_range(out, bytes, at, at + 2);
                    at += 2;
                }
            },
            _ => {
                // A run of ordinary bytes up to the next quote or backslash is copied at once.
                let end = bytes
                    .get(at..)
                    .and_then(|rest| rest.iter().position(|b| matches!(b, b'"' | b'\\')))
                    .map_or(bytes.len(), |offset| at + offset);
                push_range(out, bytes, at, end);
                at = end;
            }
        }
    }
    bytes.len()
}

/// The index just past the container that opens at `start` (`[` or `{`), or the end of the line.
/// Strings inside the container are skipped as whole tokens.
fn skip_container(bytes: &[u8], start: usize) -> usize {
    let mut depth = 0_usize;
    let mut at = start;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'"' => at = skip_string(bytes, at),
            b'[' | b'{' => {
                depth += 1;
                at += 1;
            }
            b']' | b'}' => {
                depth = depth.saturating_sub(1);
                at += 1;
                if depth == 0 {
                    return at;
                }
            }
            _ => at += 1,
        }
    }
    bytes.len()
}

/// The index just past the string that opens at `start`, or the end of the line.
fn skip_string(bytes: &[u8], start: usize) -> usize {
    let mut at = start + 1;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'\\' => at += 2,
            b'"' => return at + 1,
            _ => at += 1,
        }
    }
    bytes.len()
}

/// The code unit of a `\uXXXX` escape that starts at `at`, or `None` when no such escape starts
/// there.
fn unicode_escape(bytes: &[u8], at: usize) -> Option<u16> {
    if bytes.get(at) != Some(&b'\\') || bytes.get(at + 1) != Some(&b'u') {
        return None;
    }
    let digits = bytes.get(at + 2..at + 6)?;
    let mut unit = 0_u16;
    for &digit in digits {
        let value = u16::try_from(char::from(digit).to_digit(16)?).ok()?;
        unit = (unit << 4) | value;
    }
    Some(unit)
}

fn is_high(unit: u16) -> bool {
    (0xD800..=0xDBFF).contains(&unit)
}

fn is_low(unit: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&unit)
}

fn push_range(out: &mut Vec<u8>, bytes: &[u8], from: usize, to: usize) {
    let end = to.min(bytes.len());
    if let Some(part) = bytes.get(from..end) {
        out.extend_from_slice(part);
    }
}

/// The `params` object of a `tools/call` message, or `None` for any other message.
fn tools_call_params(value: &mut Value) -> Option<&mut Map<String, Value>> {
    let object = value.as_object_mut()?;
    if object.get("method").and_then(Value::as_str) != Some("tools/call") {
        return None;
    }
    match object.get_mut("params") {
        Some(Value::Object(params)) => Some(params),
        _ => None,
    }
}

/// Set the rejection marker in `params._meta` of a `tools/call` with an object `params`. Returns
/// `false`, leaving the value alone, for any other message.
fn mark_tools_call(value: &mut Value, rejection: Rejection) -> bool {
    let Some(params) = tools_call_params(value) else {
        return false;
    };
    let meta = params
        .entry("_meta")
        .or_insert_with(|| Value::Object(Map::new()));
    if !meta.is_object() {
        *meta = Value::Object(Map::new());
    }
    if let Value::Object(meta) = meta {
        meta.insert(
            REJECTED_INPUT_KEY.to_owned(),
            Value::String(rejection.meta_value().to_owned()),
        );
    }
    true
}

/// Remove a client-supplied marker from `params._meta` of a `tools/call`. Returns `true` when a
/// marker was removed.
fn strip_marker(value: &mut Value) -> bool {
    let Some(params) = tools_call_params(value) else {
        return false;
    };
    match params.get_mut("_meta") {
        Some(Value::Object(meta)) => meta.remove(REJECTED_INPUT_KEY).is_some(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parsed(text: &str) -> Value {
        match serde_json::from_str::<Value>(text) {
            Ok(value) => value,
            Err(error) => panic!("admitted text must parse: {error}"),
        }
    }

    fn nested(open: &str, close: &str, levels: usize) -> String {
        format!("{}{}{}", open.repeat(levels), "1", close.repeat(levels))
    }

    fn call(arguments: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{{"name":"get_profile","arguments":{arguments}}}}}"#
        )
    }

    #[test]
    fn a_valid_line_is_untouched() {
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        assert_eq!(admit_line(line), unchanged(line));
    }

    #[test]
    fn a_client_supplied_marker_on_a_valid_tools_call_is_dropped() {
        let line = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_profile","arguments":{"profileId":"x"},"_meta":{"io.jobniac/rejected-input":"lone_surrogate","keep":1}}}"#;
        let admitted = admit_line(line);
        assert_eq!(admitted.rejection, None);
        let value = parsed(&admitted.text);
        assert_eq!(value["params"]["_meta"], json!({"keep": 1}));
        assert_eq!(value["params"]["arguments"]["profileId"], json!("x"));
    }

    #[test]
    fn a_client_supplied_marker_on_another_method_is_kept() {
        let line = r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"_meta":{"io.jobniac/rejected-input":"too_deep"}}}"#;
        assert_eq!(admit_line(line), unchanged(line));
    }

    #[test]
    fn an_array_of_97_levels_is_the_deepest_kept_below_the_limit() {
        // Message object, params, arguments take three levels; 97 arrays reach MAX_DEPTH.
        let line = call(&format!(r#"{{"profileId":{}}}"#, nested("[", "]", 97)));
        assert_eq!(admit_line(&line), unchanged(&line));
    }

    #[test]
    fn one_level_past_the_limit_is_rejected_for_depth() {
        let line = call(&format!(r#"{{"profileId":{}}}"#, nested("[", "]", 98)));
        let admitted = admit_line(&line);
        assert_eq!(admitted.rejection, Some(Rejection::TooDeep));
    }

    #[test]
    fn nesting_that_serde_json_accepts_is_still_rejected() {
        // serde_json parses up to 128 levels (the line adds three), so these parse but exceed
        // MAX_DEPTH.
        for levels in [101, 110, 120] {
            let arrays = call(&format!(r#"{{"profileId":{}}}"#, nested("[", "]", levels)));
            assert!(serde_json::from_str::<Value>(&arrays).is_ok());
            let admitted = admit_line(&arrays);
            assert_eq!(
                admitted.rejection,
                Some(Rejection::TooDeep),
                "arrays {levels}"
            );
            assert_eq!(
                parsed(&admitted.text)["params"]["_meta"][REJECTED_INPUT_KEY],
                json!("too_deep")
            );

            let objects = call(&format!(
                r#"{{"profileId":{}}}"#,
                nested(r#"{"k":"#, "}", levels)
            ));
            let admitted = admit_line(&objects);
            assert_eq!(
                admitted.rejection,
                Some(Rejection::TooDeep),
                "objects {levels}"
            );
        }
    }

    #[test]
    fn a_valid_surrogate_pair_is_untouched() {
        let line = call(r#"{"profileId":"😀"}"#);
        assert_eq!(admit_line(&line), unchanged(&line));
    }

    #[test]
    fn a_high_surrogate_alone_is_rejected_and_replaced() {
        let line = call(r#"{"profileId":"a\ud800b"}"#);
        let admitted = admit_line(&line);
        assert_eq!(admitted.rejection, Some(Rejection::LoneSurrogate));
        let value = parsed(&admitted.text);
        assert_eq!(
            value["params"]["arguments"]["profileId"],
            json!("a\u{fffd}b")
        );
        assert_eq!(
            value["params"]["_meta"][REJECTED_INPUT_KEY],
            json!("lone_surrogate")
        );
        assert_eq!(value["id"], json!(7));
    }

    #[test]
    fn a_low_surrogate_alone_is_rejected_and_replaced() {
        let line = call(r#"{"profileId":"\udc00"}"#);
        let admitted = admit_line(&line);
        assert_eq!(admitted.rejection, Some(Rejection::LoneSurrogate));
        let value = parsed(&admitted.text);
        assert_eq!(value["params"]["arguments"]["profileId"], json!("\u{fffd}"));
    }

    #[test]
    fn a_low_surrogate_after_a_pair_is_rejected() {
        // The pair is kept; the trailing low surrogate has no high partner.
        let line = call(r#"{"profileId":"😀\ude00"}"#);
        let admitted = admit_line(&line);
        assert_eq!(admitted.rejection, Some(Rejection::LoneSurrogate));
        let value = parsed(&admitted.text);
        assert_eq!(
            value["params"]["arguments"]["profileId"],
            json!("\u{1f600}\u{fffd}")
        );
    }

    #[test]
    fn an_escaped_backslash_before_u_is_not_an_escape() {
        // The escaped backslash is literal text, so the only repair is the depth one.
        let deep = nested("[", "]", 200);
        let line = call(&format!(r#"{{"a":"\\ud800","b":{deep}}}"#));
        let admitted = admit_line(&line);
        assert_eq!(admitted.rejection, Some(Rejection::TooDeep));
        let value = parsed(&admitted.text);
        assert_eq!(value["params"]["arguments"]["a"], json!("\\ud800"));
    }

    #[test]
    fn a_pair_is_kept_when_the_line_is_repaired_for_depth() {
        let deep = nested("[", "]", 200);
        let line = call(&format!(r#"{{"a":"😀","b":{deep}}}"#));
        let admitted = admit_line(&line);
        assert_eq!(admitted.rejection, Some(Rejection::TooDeep));
        let value = parsed(&admitted.text);
        assert_eq!(value["params"]["arguments"]["a"], json!("\u{1f600}"));
    }

    #[test]
    fn an_array_nested_200_deep_is_replaced_below_the_limit() {
        let deep = nested("[", "]", 200);
        let line = call(&format!(r#"{{"profileId":{deep}}}"#));
        let admitted = admit_line(&line);
        assert_eq!(admitted.rejection, Some(Rejection::TooDeep));
        let value = parsed(&admitted.text);
        assert_eq!(
            value["params"]["_meta"][REJECTED_INPUT_KEY],
            json!("too_deep")
        );
        // The message object, `params`, `arguments` and `profileId` take four of the 100 levels.
        assert_eq!(
            depth_of(&value["params"]["arguments"]["profileId"]),
            MAX_DEPTH - 3
        );
    }

    #[test]
    fn an_object_nested_200_deep_is_replaced_below_the_limit() {
        let deep = nested(r#"{"k":"#, "}", 200);
        let line = call(&format!(r#"{{"profileId":{deep}}}"#));
        let admitted = admit_line(&line);
        assert_eq!(admitted.rejection, Some(Rejection::TooDeep));
        let value = parsed(&admitted.text);
        assert_eq!(
            value["params"]["_meta"][REJECTED_INPUT_KEY],
            json!("too_deep")
        );
    }

    #[test]
    fn brackets_inside_strings_do_not_count_toward_depth() {
        let text = "[".repeat(300);
        let line = call(&format!(r#"{{"profileId":"{text}"}}"#));
        assert_eq!(admit_line(&line), unchanged(&line));
    }

    #[test]
    fn a_skipped_container_respects_strings_inside_it() {
        let deep = format!("{}\"]]}}\"{}", "[".repeat(150), "]".repeat(150));
        let line = call(&format!(r#"{{"profileId":{deep},"after":"x"}}"#));
        let admitted = admit_line(&line);
        assert_eq!(admitted.rejection, Some(Rejection::TooDeep));
        let value = parsed(&admitted.text);
        assert_eq!(value["params"]["arguments"]["after"], json!("x"));
    }

    #[test]
    fn both_problems_report_lone_surrogate() {
        let deep = nested("[", "]", 200);
        let line = call(&format!(r#"{{"profileId":"\ud800","b":{deep}}}"#));
        let admitted = admit_line(&line);
        assert_eq!(admitted.rejection, Some(Rejection::LoneSurrogate));
        let value = parsed(&admitted.text);
        assert_eq!(
            value["params"]["_meta"][REJECTED_INPUT_KEY],
            json!("lone_surrogate")
        );
    }

    #[test]
    fn a_non_tools_call_is_sanitized_without_a_rejection() {
        let line = r#"{"jsonrpc":"2.0","id":5,"method":"tools/list","params":{"x":"\ud800"}}"#;
        let admitted = admit_line(line);
        assert_eq!(admitted.rejection, None);
        let value = parsed(&admitted.text);
        assert_eq!(value["params"]["x"], json!("\u{fffd}"));
        assert_eq!(value["params"].get("_meta"), None);
    }

    #[test]
    fn garbage_is_returned_unchanged() {
        let line = r#"{"jsonrpc":"2.0", \ud800 "#;
        assert_eq!(admit_line(line), unchanged(line));
        assert_eq!(admit_line("not json {"), unchanged("not json {"));
    }

    #[test]
    fn a_marker_key_spelled_with_an_escaped_slash_is_still_dropped() {
        // JSON allows `\/` for `/`: the key has no literal `io.jobniac/rejected-input` text, and
        // no `\u` escape either, so only the text `rejected` gives it away.
        let line = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_profile","arguments":{},"_meta":{"io.jobniac\/rejected-input":"too_deep"}}}"#;
        let admitted = admit_line(line);
        assert_eq!(admitted.rejection, None);
        assert_eq!(parsed(&admitted.text)["params"]["_meta"], json!({}));
        assert!(admit_changed(line).is_some());
    }

    #[test]
    fn a_marker_key_spelled_with_unicode_escapes_is_still_dropped() {
        let line = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_profile","arguments":{},"_meta":{"io.jobniac/rejected-input":"too_deep"}}}"#;
        let admitted = admit_line(line);
        assert_eq!(parsed(&admitted.text)["params"]["_meta"], json!({}));
    }

    #[test]
    fn a_line_that_needs_nothing_is_recognised_without_a_parse() {
        let plain = call(r#"{"profileId":"x","text":"a [bracket] and a {brace} and \\u in text"}"#);
        // The text `\\u` is an escaped backslash then `u`, not an escape, so it takes no slow path.
        assert!(!may_need_admission(&plain));
        assert!(admit_changed(&plain).is_none());
        assert!(!may_need_admission(&call(&format!(
            r#"{{"profileId":{}}}"#,
            nested("[", "]", 97)
        ))));
        assert!(may_need_admission(&call(&format!(
            r#"{{"profileId":{}}}"#,
            nested("[", "]", 98)
        ))));
        assert!(may_need_admission(&call(r#"{"profileId":"\ud800"}"#)));
        assert!(may_need_admission(&call(r#"{"profileId":"rejected"}"#)));
        // Brackets inside strings do not count toward depth.
        assert!(!may_need_admission(&call(&format!(
            r#"{{"profileId":"{}"}}"#,
            "[".repeat(300)
        ))));
    }

    #[test]
    fn admit_changed_agrees_with_admit_line() {
        let lines = [
            call(r#"{"profileId":"x"}"#),
            call(r#"{"profileId":"\ud800"}"#),
            call(&format!(
                r#"{{"profileId":{}}}"#,
                nested("{\"a\":", "}", 120)
            )),
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/list","params":{"x":"\ud800"}}"#.to_owned(),
            r#"{"jsonrpc":"2.0", \ud800 "#.to_owned(),
            "not json {".to_owned(),
        ];
        for line in lines {
            let expected = admit_line(&line);
            let changed = admit_changed(&line);
            assert_eq!(changed.unwrap_or_else(|| unchanged(&line)), expected);
        }
    }

    fn depth_of(value: &Value) -> usize {
        match value {
            Value::Array(items) => 1 + items.iter().map(depth_of).max().unwrap_or(0),
            _ => 0,
        }
    }
}
