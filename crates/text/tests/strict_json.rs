//! Strict JSON admission: golden vectors for the board's tree parser and the board cases, plus
//! one test per refused input class.

use jobniac_text::json::{
    JsonAdmissionError, JsonValue, KeyMeasure, StrictLimits, decode_body, parse_body, parse_strict,
};
use serde_json::{Value, json};
use std::path::PathBuf;

const BASE: &[u8] = br#"{"data":[],"links":{"next":null},"meta":{"current_page":1,"last_page":1}"#;

fn body(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn err(bytes: &[u8]) -> JsonAdmissionError {
    match parse_body(bytes) {
        Err(error) => error,
        Ok(_) => panic!("body was admitted"),
    }
}

fn ok(bytes: &[u8]) -> JsonValue {
    match parse_body(bytes) {
        Ok(Some(value)) => value,
        other => panic!("body was not a value: {other:?}"),
    }
}

/// Renders a tree in the tagged form of the golden vectors.
fn tagged(value: &JsonValue) -> Value {
    match value {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(flag) => json!(flag),
        JsonValue::Integer(digits) => json!({ "i": digits }),
        JsonValue::Float(literal) => json!({ "f": literal }),
        JsonValue::String(text) => json!(text),
        JsonValue::Array(items) => Value::Array(items.iter().map(tagged).collect()),
        JsonValue::Object(members) => {
            let pairs = members
                .iter()
                .map(|(key, item)| json!([key, tagged(item)]))
                .collect();
            json!({ "o": Value::Array(pairs) })
        }
    }
}

/// Floats compare by value (the vectors write the shortest form, Rust the literal).
fn normalised(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(normalised).collect()),
        Value::Object(map) if map.len() == 1 && map.contains_key("f") => {
            let parsed: f64 = map["f"]
                .as_str()
                .and_then(|text| text.parse().ok())
                .expect("float literal");
            json!({ "f": parsed.to_bits().to_string() })
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), normalised(item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex"))
        .collect()
}

#[test]
fn golden_vectors_match() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../core/tests/golden/arbeitnow-strict-json.json");
    let text = std::fs::read_to_string(path).expect("vector file");
    let document: Value = serde_json::from_str(&text).expect("vector json");
    let cases = document["cases"].as_array().expect("cases");
    assert!(cases.len() >= 100);
    let mut deviations = 0;
    for case in cases {
        let id = case["id"].as_str().expect("id");
        let count = usize::try_from(case["count"].as_u64().expect("count")).expect("count");
        let bytes = body(&[
            &hex(case["prefixHex"].as_str().expect("prefix")),
            &hex(case["unitHex"].as_str().expect("unit")).repeat(count),
            &hex(case["suffixHex"].as_str().expect("suffix")),
        ]);
        let result = parse_body(&bytes);
        let expected = &case["rust"];
        match expected["result"].as_str().expect("result") {
            "error" => {
                let want = expected["error"].as_str().expect("error");
                match result {
                    Err(error) => assert_eq!(error.code(), want, "{id}"),
                    Ok(_) => panic!("{id}: admitted, expected {want}"),
                }
            }
            "empty" => assert_eq!(result, Ok(None), "{id}"),
            "value" => {
                let value = result.unwrap_or_else(|e| panic!("{id}: {e:?}")).expect(id);
                if case["recorded"].get("value").is_some() {
                    assert_eq!(
                        normalised(&tagged(&value)),
                        normalised(&case["recorded"]["value"]),
                        "{id}"
                    );
                }
            }
            other => panic!("{id}: unknown result {other}"),
        }
        if case.get("deviation").is_some() {
            deviations += 1;
            assert_eq!(expected["result"], "error", "{id}");
            assert_ne!(case["recorded"]["result"], "error", "{id}");
        }
    }
    assert!(deviations >= 10);
}

// Invalid JSON text fails without leaking any context.

#[test]
fn nan_infinity_and_bad_exponent_are_refused() {
    assert_eq!(
        err(&body(&[BASE, br#","x":NaN}"#])),
        JsonAdmissionError::NonFinite
    );
    assert_eq!(
        err(&body(&[BASE, br#","x":Infinity}"#])),
        JsonAdmissionError::NonFinite
    );
    assert_eq!(
        err(&body(&[BASE, br#","x":-Infinity}"#])),
        JsonAdmissionError::NonFinite
    );
    assert_eq!(
        err(&body(&[BASE, br#","x":1e}"#])),
        JsonAdmissionError::Syntax
    );
}

#[test]
fn bom_whitespace_and_empty_bodies() {
    assert_eq!(err(b"\xef\xbb\xbf"), JsonAdmissionError::BomOnly);
    assert_eq!(parse_body(b"\xef\xbb\xbf \n"), Ok(None));
    assert_eq!(parse_body(b""), Ok(None));
    assert_eq!(parse_body(b" \t\r\n"), Ok(None));
    assert_eq!(
        ok(b"\xef\xbb\xbf{\"a\":1}")
            .get("a")
            .and_then(JsonValue::as_i64),
        Some(1)
    );
    // An empty body and a literal `null` stay apart (a `null` page is a schema failure in the
    // caller, as an empty body is).
    assert_eq!(parse_body(b"null"), Ok(Some(JsonValue::Null)));
    // Form feed is not JSON whitespace.
    assert_eq!(err(b"\x0c1"), JsonAdmissionError::Syntax);
}

#[test]
fn encodings_are_detected_by_mark_and_zero_pattern() {
    let units: Vec<u16> = r#"{"k":"é😀"}"#.encode_utf16().collect();
    let le: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
    let be: Vec<u8> = units.iter().flat_map(|u| u.to_be_bytes()).collect();
    let chars: Vec<u32> = r#"{"k":"é😀"}"#.chars().map(u32::from).collect();
    let le32: Vec<u8> = chars.iter().flat_map(|u| u.to_le_bytes()).collect();
    let be32: Vec<u8> = chars.iter().flat_map(|u| u.to_be_bytes()).collect();
    let utf8 = body(&[br#"{"k":"#, b"\"", "é😀".as_bytes(), b"\"}"]);
    let want = ok(&utf8);
    let inputs = [
        body(&[b"\xff\xfe", &le]),
        body(&[b"\xfe\xff", &be]),
        le.clone(),
        be.clone(),
        body(&[b"\xff\xfe\x00\x00", &le32]),
        body(&[b"\x00\x00\xfe\xff", &be32]),
        le32.clone(),
        be32.clone(),
    ];
    for input in &inputs {
        assert_eq!(ok(input), want);
    }
    assert!(decode_body(&le).is_ok_and(|d| !d.utf8));
    assert!(decode_body(&utf8).is_ok_and(|d| d.utf8));
    // Odd length, partial code unit, and above U+10FFFF.
    assert_eq!(
        err(&body(&[b"\xff\xfe", &le, b"\x00"])),
        JsonAdmissionError::InvalidEncoding
    );
    assert_eq!(
        err(&body(&[b"\xff\xfe\x00\x00", &le32, b"\x00"])),
        JsonAdmissionError::InvalidEncoding
    );
    assert_eq!(
        err(b"\xff\xfe\x00\x00\x00\x00\x11\x00"),
        JsonAdmissionError::InvalidEncoding
    );
}

#[test]
fn utf16_lone_surrogates_become_replacement_characters() {
    // A lone low surrogate, and a high surrogate that swallows the unit after it.
    let low = [0x22u16, 0xDC00, 0x22];
    let bytes: Vec<u8> = [0xFEFFu16]
        .iter()
        .chain(low.iter())
        .flat_map(|u| u.to_le_bytes())
        .collect();
    assert_eq!(ok(&bytes), JsonValue::String("\u{FFFD}".into()));
    let high = [0x22u16, 0xD800, 0x61, 0x62, 0x22];
    let bytes: Vec<u8> = [0xFEFFu16]
        .iter()
        .chain(high.iter())
        .flat_map(|u| u.to_le_bytes())
        .collect();
    assert_eq!(ok(&bytes), JsonValue::String("\u{FFFD}b".into()));
}

#[test]
fn duplicate_keys_are_refused_after_unescaping() {
    for text in [
        &br#"{"a":1,"a":2}"#[..],
        br#"{"a":1,"\u0061":2}"#,
        br#"{"a":{"b":1,"b":2}}"#,
        br#"[{"a":1,"a":1}]"#,
    ] {
        assert_eq!(err(text), JsonAdmissionError::DuplicateKey);
    }
    let pair = "{\"\u{1F600}\":1,\"\\ud83d\\ude00\":2}";
    assert_eq!(err(pair.as_bytes()), JsonAdmissionError::DuplicateKey);
    ok(br#"{"a":{"a":1},"b":{"a":2},"A":3}"#);
}

#[test]
fn depth_bomb_is_bounded_without_stack_overflow() {
    // The default thread stack of a spawned thread, in a debug build.
    let handle = std::thread::spawn(|| {
        let at = |depth: usize| body(&[&b"[".repeat(depth), &b"]".repeat(depth)]);
        assert!(parse_body(&at(500)).is_ok_and(|value| value.is_some()));
        assert_eq!(parse_body(&at(501)), Err(JsonAdmissionError::TooDeep));
        let objects = |depth: usize| body(&[&b"{\"a\":".repeat(depth), b"1", &b"}".repeat(depth)]);
        assert!(parse_body(&objects(500)).is_ok());
        assert_eq!(parse_body(&objects(501)), Err(JsonAdmissionError::TooDeep));
        // A very deep body is refused at the bound, never walked.
        assert_eq!(
            parse_body(&b"[".repeat(1_000_000)),
            Err(JsonAdmissionError::TooDeep)
        );
        // Brackets inside strings do not count.
        ok(&body(&[b"[\"", &b"[{".repeat(600), b"\"]"]));
    });
    handle.join().expect("no stack overflow");
}

#[test]
fn key_length_bound_counts_utf8_bytes_or_utf16_units() {
    let key = |unit: &[u8], count: usize| body(&[b"{\"", &unit.repeat(count), b"\":1}"]);
    ok(&key(b"a", 50_000));
    assert_eq!(err(&key(b"a", 50_001)), JsonAdmissionError::KeyTooLong);
    ok(&key(b"\\u0061", 50_000));
    assert_eq!(
        err(&key(b"\\u0061", 50_001)),
        JsonAdmissionError::KeyTooLong
    );
    ok(&key("é".as_bytes(), 25_000));
    assert_eq!(
        err(&key("é".as_bytes(), 25_001)),
        JsonAdmissionError::KeyTooLong
    );
    // Values and array elements are not limited.
    ok(&body(&[b"{\"k\":\"", &b"a".repeat(60_000), b"\"}"]));
    // UTF-16 bodies count units: 25,000 astral characters are 50,000 units.
    let utf16 = |count: usize| -> Vec<u8> {
        let text = format!("{{\"{}\":1}}", "\u{1F600}".repeat(count));
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        bytes
    };
    assert!(parse_body(&utf16(25_000)).is_ok());
    assert_eq!(
        parse_body(&utf16(25_001)),
        Err(JsonAdmissionError::KeyTooLong)
    );
    // The same key in UTF-8 is 100,000 bytes.
    assert_eq!(
        err(&key("\u{1F600}".as_bytes(), 25_000)),
        JsonAdmissionError::KeyTooLong
    );
    let limits = StrictLimits {
        key_measure: KeyMeasure::Utf16Units,
        ..StrictLimits::default()
    };
    let text = format!("{{\"{}\":1}}", "\u{1F600}".repeat(25_000));
    assert!(parse_strict(&text, limits).is_ok());
}

#[test]
fn number_digit_bound() {
    let number = |prefix: &[u8], zeros: usize| body(&[prefix, &b"0".repeat(zeros)]);
    ok(&number(b"1", 999));
    assert_eq!(err(&number(b"1", 1000)), JsonAdmissionError::NumberTooLong);
    ok(&number(b"-1", 999));
    assert_eq!(err(&number(b"-1", 1000)), JsonAdmissionError::NumberTooLong);
    ok(&number(b"0.1", 998));
    assert_eq!(err(&number(b"0.1", 999)), JsonAdmissionError::NumberTooLong);
    assert_eq!(err(&number(b"1e", 1000)), JsonAdmissionError::NumberTooLong);
    // Out-of-range exponents stay finite literals that read as infinity.
    assert_eq!(ok(b"1e999").as_f64(), Some(f64::INFINITY));
    assert_eq!(err(b"01"), JsonAdmissionError::TrailingData);
    assert_eq!(ok(b"10000000000000000000000").as_i64(), None);
}

#[test]
fn trailing_data_is_refused() {
    for text in [&b"{} x"[..], b"{}{}", b"[1]]", b"1 2"] {
        assert_eq!(err(text), JsonAdmissionError::TrailingData);
    }
    ok(b"{} \r\n\t");
}

#[test]
fn malformed_json_is_a_syntax_error() {
    for text in [
        &b"{"[..],
        b"[1,]",
        b"{\"a\":1,}",
        b"{'a':1}",
        b"\"a\tb\"",
        b"\"\\x41\"",
        b"\"\\u12\"",
        b"\"\\u+041\"",
        b"tru",
        b"{\"a\" 1}",
        b"{a:1}",
        b"\"abc",
        b"-",
        b"[1.]",
    ] {
        assert_eq!(err(text), JsonAdmissionError::Syntax, "{text:?}");
    }
}

#[test]
fn errors_have_fixed_messages_without_input() {
    let message = err(b"{\"secret-token\":1,\"secret-token\":2}").to_string();
    assert_eq!(message, "JSON object repeats a key");
    assert!(!message.contains("secret"));
}

// Refused input classes.

#[test]
fn malformed_utf8_is_refused() {
    // An overlong two-byte lead, an overlong three-byte form and leads F5 to F7 are refused.
    for text in [
        &b"\"\xc0\x80\""[..],
        b"\"\xc1\xaf\"",
        b"\"\xe0\x80\x80\"",
        b"\"\xf5\x80\x80\x80\"",
        b"\"\xf4\x90\x80\x80\"",
        b"\"\xed\xa0\x80\"",
        b"\"\xc3",
        b"\"\x80\"",
    ] {
        assert_eq!(err(text), JsonAdmissionError::InvalidEncoding, "{text:?}");
    }
    // Non-ASCII outside a string, or directly after a backslash, is refused.
    assert_eq!(
        err("{\"a\":1}é".as_bytes()),
        JsonAdmissionError::InvalidEncoding
    );
    assert_eq!(
        err("\"\\é\"".as_bytes()),
        JsonAdmissionError::InvalidEncoding
    );
    // Valid multi-byte text inside strings is admitted.
    assert_eq!(ok("\"é😀\"".as_bytes()), JsonValue::String("é😀".into()));
}

#[test]
fn lone_surrogate_escape_is_refused() {
    for text in [
        &br#""\ud800""#[..],
        br#""\udc00""#,
        br#""\ud800\u0041""#,
        br#""\ud800x""#,
        br#""\udc00\ud800""#,
        br#"{"\ud800":1}"#,
    ] {
        assert_eq!(err(text), JsonAdmissionError::LoneSurrogate, "{text:?}");
    }
    // A well-formed pair, in either hex case, is one scalar.
    assert_eq!(
        ok(br#""\uD83D\uDE00""#),
        JsonValue::String("\u{1F600}".into())
    );
    assert_eq!(
        ok(br#""\ud83d\ude00""#),
        JsonValue::String("\u{1F600}".into())
    );
}

#[test]
fn utf32_surrogate_code_point_is_refused() {
    let bytes = [
        &b"\xff\xfe\x00\x00"[..],
        &[0x22, 0, 0, 0],
        &[0x00, 0xD8, 0, 0],
        &[0x22, 0, 0, 0],
    ]
    .concat();
    assert_eq!(err(&bytes), JsonAdmissionError::LoneSurrogate);
}
