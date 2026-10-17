//! Job page HTML extraction and content admission (pure parts of the link fetch): the pinned
//! Java fixture cases, the synthetic pages, the final-header admission and the raw header guard,
//! compared with `golden/job-link-html.json`.

use jobniac_core::job_links::JobLinkFetchFailure;
use jobniac_core::job_links::html::{
    MAX_BODY_BYTES, MAX_DESCRIPTION_UNITS, MAX_HEADER_BYTES, NETWORK_READ_BYTES, RawHeaderGuard,
    admit_response_headers, admit_response_headers_with_limit, parse_job_html,
};
use serde_json::Value;

const GOLDEN: &str = include_str!("golden/job-link-html.json");

fn golden() -> Value {
    match serde_json::from_str(GOLDEN) {
        Ok(value) => value,
        Err(error) => panic!("job-link-html.json does not parse: {error}"),
    }
}

fn rows<'a>(document: &'a Value, key: &str) -> &'a [Value] {
    match document[key].as_array() {
        Some(items) => items,
        None => panic!("missing array {key}"),
    }
}

/// The bytes of a stored value encoded in `golden/job-link-html.json`: a UTF-8
/// string, or `parts` of strings, `{"base64": ...}` and `{"repeat": [byte, count]}`.
fn stored_bytes(value: &Value) -> Vec<u8> {
    match value {
        Value::String(text) => text.as_bytes().to_vec(),
        Value::Object(object) => {
            if let Some(parts) = object.get("parts").and_then(Value::as_array) {
                parts.iter().flat_map(stored_bytes).collect()
            } else if let Some(text) = object.get("base64").and_then(Value::as_str) {
                base64_decode(text)
            } else if let Some(repeat) = object.get("repeat").and_then(Value::as_array) {
                match (repeat.first(), repeat.get(1)) {
                    (Some(byte), Some(count)) => {
                        let byte = u8::try_from(byte.as_u64().unwrap_or(0)).unwrap_or(0);
                        let count = usize::try_from(count.as_u64().unwrap_or(0)).unwrap_or(0);
                        vec![byte; count]
                    }
                    _ => panic!("malformed repeat {value}"),
                }
            } else {
                panic!("unknown stored value {value}")
            }
        }
        other => panic!("unexpected stored value {other}"),
    }
}

/// Standard base64 with padding, as the vectors write it.
fn base64_decode(text: &str) -> Vec<u8> {
    let mut output = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(u8::try_from(buffer >> bits).unwrap_or(0));
            buffer &= (1 << bits) - 1;
        }
    }
    output
}

fn assert_html(row: &Value, context: &str) {
    let body = stored_bytes(&row["body"]);
    let status = row["status"]
        .as_i64()
        .unwrap_or_else(|| row["httpStatus"].as_i64().unwrap_or(200));
    let actual = parse_job_html(&body, status);
    if let Some(failure) = row.get("failure").and_then(Value::as_str) {
        match actual {
            Ok(result) => {
                panic!("{context}: admitted {result:?}, the vector refuses with {failure}")
            }
            Err(actual) => assert_eq!(actual.name(), failure, "{context}"),
        }
        return;
    }
    let result = match actual {
        Ok(result) => result,
        Err(failure) => panic!("{context}: refused with {}", failure.name()),
    };
    assert_eq!(
        result.title.as_deref(),
        row["title"].as_str(),
        "{context}: title"
    );
    assert_eq!(
        result.description.as_deref(),
        row["description"].as_str(),
        "{context}: description"
    );
    assert_eq!(
        Some(result.http_status),
        row["httpStatus"].as_i64(),
        "{context}: status"
    );
}

#[test]
fn pinned_java_fixture_cases_match_the_golden_vectors() {
    let document = golden();
    let fixture = rows(&document, "fixture");
    assert_eq!(fixture.len(), 40);
    for row in fixture {
        assert_html(row, row["name"].as_str().unwrap_or_default());
    }
}

#[test]
fn synthetic_pages_match_the_golden_vectors() {
    let document = golden();
    let pages = rows(&document, "html");
    assert!(pages.len() > 900, "only {} pages", pages.len());
    for (index, row) in pages.iter().enumerate() {
        assert_html(row, &format!("page {index}"));
    }
}

#[test]
fn limits_match_the_golden_vectors() {
    let document = golden();
    let limits = &document["limits"];
    assert_eq!(limits["maxBodyBytes"].as_u64(), Some(MAX_BODY_BYTES));
    assert_eq!(
        limits["maxHeaderBytes"].as_u64(),
        Some(MAX_HEADER_BYTES as u64)
    );
    assert_eq!(
        limits["networkReadBytes"].as_u64(),
        Some(NETWORK_READ_BYTES as u64)
    );
    assert_eq!(
        limits["maxDescriptionUnits"].as_u64(),
        Some(MAX_DESCRIPTION_UNITS as u64)
    );
}

#[test]
fn body_bound_and_status_range_are_enforced() {
    let body = vec![b'a'; (MAX_BODY_BYTES as usize) + 1];
    assert_eq!(
        parse_job_html(&body, 200).err(),
        Some(JobLinkFetchFailure::Unavailable)
    );
    assert!(parse_job_html(b"<p>x</p>", 99).is_err());
    assert!(parse_job_html(b"<p>x</p>", 600).is_err());
    assert!(parse_job_html(b"<p>x</p>", 100).is_ok());
    assert!(parse_job_html(b"<p>x</p>", 599).is_ok());
}

/// A body of the largest admitted size made by repeating `unit`.
fn full_body(unit: &str) -> Vec<u8> {
    let limit = MAX_BODY_BYTES as usize;
    let mut body = unit.repeat(limit / unit.len() + 1).into_bytes();
    body.truncate(limit);
    body
}

/// Parses a body of the largest admitted size and checks that the search is not quadratic: the
/// bound is generous so that a slow CI machine does not make it flaky.
fn assert_parses_quickly(body: &[u8], context: &str) {
    let started = std::time::Instant::now();
    let result = parse_job_html(body, 200);
    let elapsed = started.elapsed();
    assert!(result.is_ok(), "{context}: refused");
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "{context}: took {elapsed:?}"
    );
}

#[test]
fn two_mebibytes_of_unmatched_angle_brackets_parse_quickly() {
    assert_parses_quickly(&full_body("<"), "'<'");
}

#[test]
fn two_mebibytes_of_unclosed_script_tags_parse_quickly() {
    assert_parses_quickly(&full_body("<script"), "'<script'");
}

#[test]
fn two_mebibytes_of_unclosed_tags_with_attributes_parse_quickly() {
    assert_parses_quickly(&full_body("<a "), "'<a '");
}

#[test]
fn angle_brackets_before_one_closing_bracket_parse_quickly() {
    let mut body = full_body("<script");
    let last = body.len() - 1;
    body[last] = b'>';
    assert_parses_quickly(&body, "'<script' repeated, then '>'");
}

#[test]
fn description_is_truncated_at_twenty_thousand_utf16_units() {
    let text = "a".repeat(19_999) + "\u{1F600}" + "b";
    let result = parse_job_html(text.as_bytes(), 200);
    let description = result
        .ok()
        .and_then(|value| value.description)
        .unwrap_or_default();
    assert_eq!(description, "a".repeat(19_999));
}

#[test]
fn header_admission_matches_golden() {
    let document = golden();
    for row in rows(&document, "headers") {
        let headers: Vec<(String, Vec<u8>)> = row["headers"]
            .as_array()
            .map(|pairs| {
                pairs
                    .iter()
                    .map(|pair| {
                        (
                            pair[0].as_str().unwrap_or_default().to_owned(),
                            stored_bytes(&pair[1]),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let raw: Vec<(&[u8], &[u8])> = headers
            .iter()
            .map(|(name, value)| (name.as_bytes(), value.as_slice()))
            .collect();
        let actual = admit_response_headers(&raw);
        match (row.get("declared"), row.get("failure")) {
            (Some(declared), _) if declared.is_null() => {
                assert_eq!(actual, Ok(None), "{headers:?}")
            }
            (Some(declared), _) => assert_eq!(actual, Ok(declared.as_u64()), "{headers:?}"),
            (_, Some(failure)) => match actual {
                Ok(value) => {
                    panic!("{headers:?}: admitted {value:?}, the vector refuses with {failure}")
                }
                Err(error) => assert_eq!(
                    error.name(),
                    failure.as_str().unwrap_or_default(),
                    "{headers:?}"
                ),
            },
            _ => panic!("row without outcome: {row}"),
        }
    }
}

const MIB: u64 = 1024 * 1024;

/// Admits a final header block that declares `declared` bytes of body, with the given limit.
fn admit_declared(declared: u64, limit: Option<u64>) -> Result<Option<u64>, JobLinkFetchFailure> {
    let digits = declared.to_string().into_bytes();
    let headers: [(&[u8], &[u8]); 1] = [(b"Content-Length", digits.as_slice())];
    match limit {
        Some(limit) => admit_response_headers_with_limit(&headers, limit),
        None => admit_response_headers(&headers),
    }
}

/// Feeds one header block that declares `declared` bytes of body to `guard`; returns whether the
/// guard completed on it.
fn guard_admits(mut guard: RawHeaderGuard, declared: u64) -> Result<bool, JobLinkFetchFailure> {
    let block = format!("HTTP/1.1 200 OK\r\nContent-Length: {declared}\r\n\r\n");
    guard.check(block.as_bytes())?;
    Ok(guard.is_complete())
}

#[test]
fn custom_body_limit_admits_a_declared_length_up_to_the_limit() {
    let limit = 4 * MIB;
    assert_eq!(admit_declared(3 * MIB, Some(limit)), Ok(Some(3 * MIB)));
    assert_eq!(admit_declared(limit - 1, Some(limit)), Ok(Some(limit - 1)));
    assert_eq!(admit_declared(limit, Some(limit)), Ok(Some(limit)));
    assert_eq!(
        admit_declared(limit + 1, Some(limit)),
        Err(JobLinkFetchFailure::Unavailable)
    );
}

#[test]
fn default_limit_is_two_mebibytes_and_refuses_over_it() {
    assert_eq!(
        admit_declared(MAX_BODY_BYTES - 1, None),
        Ok(Some(MAX_BODY_BYTES - 1))
    );
    assert_eq!(
        admit_declared(MAX_BODY_BYTES, None),
        Ok(Some(MAX_BODY_BYTES))
    );
    assert_eq!(
        admit_declared(MAX_BODY_BYTES + 1, None),
        Err(JobLinkFetchFailure::Unavailable)
    );
    assert_eq!(
        admit_declared(3 * MIB, None),
        Err(JobLinkFetchFailure::Unavailable)
    );
    // The explicit default limit is the same rule.
    assert_eq!(
        admit_declared(MAX_BODY_BYTES + 1, Some(MAX_BODY_BYTES)),
        Err(JobLinkFetchFailure::Unavailable)
    );
}

#[test]
fn header_guard_body_limit_is_per_guard() {
    let limit = 4 * MIB;
    assert_eq!(
        guard_admits(RawHeaderGuard::with_body_limit(limit), 3 * MIB),
        Ok(true)
    );
    assert_eq!(
        guard_admits(RawHeaderGuard::with_body_limit(limit), limit - 1),
        Ok(true)
    );
    assert_eq!(
        guard_admits(RawHeaderGuard::with_body_limit(limit), limit),
        Ok(true)
    );
    assert_eq!(
        guard_admits(RawHeaderGuard::with_body_limit(limit), limit + 1),
        Err(JobLinkFetchFailure::Unavailable)
    );
    // The default guard keeps the two mebibyte job page limit.
    assert_eq!(
        guard_admits(RawHeaderGuard::new(), 3 * MIB),
        Err(JobLinkFetchFailure::Unavailable)
    );
    assert_eq!(
        guard_admits(RawHeaderGuard::new(), MAX_BODY_BYTES),
        Ok(true)
    );
    assert_eq!(
        guard_admits(RawHeaderGuard::default(), MAX_BODY_BYTES + 1),
        Err(JobLinkFetchFailure::Unavailable)
    );
}

#[test]
fn header_guard_matches_golden_step_by_step() {
    let document = golden();
    let scenarios = rows(&document, "guard");
    assert!(scenarios.len() > 25);
    for (index, scenario) in scenarios.iter().enumerate() {
        let mut guard = RawHeaderGuard::new();
        for step in rows(scenario, "steps") {
            let limit = guard.read_limit(NETWORK_READ_BYTES);
            assert_eq!(
                Some(limit as u64),
                step["readLimit"].as_u64(),
                "scenario {index}: read limit"
            );
            let chunk = stored_bytes(&step["chunk"]);
            let outcome = match guard.check(&chunk) {
                Ok(()) => "ok".to_owned(),
                Err(failure) => failure.name().to_owned(),
            };
            assert_eq!(
                Some(outcome.as_str()),
                step["result"].as_str(),
                "scenario {index}"
            );
            assert_eq!(
                Some(guard.is_complete()),
                step["complete"].as_bool(),
                "scenario {index}: complete"
            );
            if outcome != "ok" {
                break;
            }
        }
    }
}
