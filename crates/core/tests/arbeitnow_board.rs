//! Arbeitnow board admission and page parsing: the pure admission cases, the response-admission
//! cases, and the golden vectors in `golden/arbeitnow-board.json`.
//!
//! Bodies are synthetic. Header-structure cases (mixed bytes and text, lists instead of tuples)
//! have no Rust form: headers are a slice of byte pairs.

use jobniac_core::arbeitnow::board::{
    ARBEITNOW_TIMEOUT_SECONDS, ENDPOINT_PATH, HOST, MAX_BODY_BYTES, MAX_JOBS_PER_PAGE,
    MAX_LIST_ELEMENTS, REQUEST_HEADERS, admit_response, parse_page, validate_request,
};
use jobniac_core::arbeitnow::failure::{BoardCategory, BoardFailure};
use jobniac_core::arbeitnow::instant::ArbeitnowInstant;
use serde_json::{Value, json};

type Headers = Vec<(Vec<u8>, Vec<u8>)>;

fn pair(name: &str, value: &str) -> (Vec<u8>, Vec<u8>) {
    (name.as_bytes().to_vec(), value.as_bytes().to_vec())
}

fn json_headers() -> Headers {
    vec![pair("Content-Type", "application/json")]
}

fn instant(seconds: i64, nanos: u32) -> ArbeitnowInstant {
    ArbeitnowInstant::new(seconds, nanos).expect("in range")
}

fn at_100() -> ArbeitnowInstant {
    instant(100, 0)
}

/// 2026-10-04T12:00:00Z.
const BASE_SECONDS: i64 = 1_791_115_200;

fn envelope(jobs: &[Value], page: u64, last: Option<u64>, next: Value) -> Vec<u8> {
    let mut meta = json!({ "current_page": page });
    if let Some(last) = last {
        meta["last_page"] = json!(last);
    }
    serde_json::to_vec(&json!({"data": jobs, "links": {"next": next}, "meta": meta}))
        .expect("serialize")
}

fn job(slug: &str) -> Value {
    json!({
        "slug": slug,
        "company_name": "Acme",
        "title": "Java Engineer",
        "description": "<p>Java description</p>",
        "url": "https://ignored.invalid",
        "remote": true,
        "tags": ["Java"],
        "job_types": ["Full Time"],
        "location": "Berlin",
    })
}

fn with(mut value: Value, key: &str, replacement: Value) -> Value {
    value[key] = replacement;
    value
}

fn link(page: &str) -> Value {
    json!(format!(
        "https://www.arbeitnow.com/api/job-board-api?page={page}"
    ))
}

fn category(result: Result<impl std::fmt::Debug, BoardFailure>) -> BoardCategory {
    result.expect_err("must fail").category()
}

fn retry_seconds(value: &str, now: ArbeitnowInstant) -> Option<u32> {
    let failure = admit_response(429, &[pair("Retry-After", value)], now).expect_err("429");
    assert_eq!(failure.category(), BoardCategory::RateLimited);
    assert!(failure.retryable());
    failure.retry_after_seconds()
}

fn utf16_hex(value: &str) -> String {
    value
        .encode_utf16()
        .map(|unit| format!("{unit:04x}"))
        .collect()
}

fn parse_ok(body: &[u8]) -> jobniac_core::arbeitnow::board::Page {
    parse_page(body, 1, None).expect("page")
}

#[test]
fn constants_match_the_approved_values() {
    assert_eq!(MAX_BODY_BYTES, 4 * 1024 * 1024);
    assert_eq!(MAX_JOBS_PER_PAGE, 500);
    assert_eq!(MAX_LIST_ELEMENTS, 64);
    assert_eq!(ENDPOINT_PATH, "/api/job-board-api");
    assert_eq!(HOST, "www.arbeitnow.com");
    assert_eq!(ARBEITNOW_TIMEOUT_SECONDS, 10);
    assert_eq!(
        REQUEST_HEADERS,
        [
            ("Accept", "application/json"),
            (
                "User-Agent",
                "Jobniac (+https://github.com/JonHHH09/JobniacFast)"
            ),
        ]
    );
}

#[test]
fn validate_request_bounds_the_page() {
    assert_eq!(validate_request(1, None), Ok(1));
    assert_eq!(
        validate_request(2_147_483_647, Some(true)),
        Ok(2_147_483_647)
    );
    for page in [0, -1, 2_147_483_648, i64::MIN, i64::MAX] {
        assert_eq!(
            validate_request(page, None),
            Err(BoardFailure::HttpStatus { retryable: false })
        );
    }
}

#[test]
fn status_mapping() {
    let headers = json_headers();
    assert_eq!(admit_response(200, &headers, at_100()), Ok(None));
    assert_eq!(
        admit_response(500, &headers, at_100()),
        Err(BoardFailure::HttpStatus { retryable: true })
    );
    assert_eq!(
        admit_response(599, &headers, at_100()),
        Err(BoardFailure::HttpStatus { retryable: true })
    );
    for status in [100, 199, 201, 204, 301, 404, 499, 600, 999] {
        assert_eq!(
            admit_response(status, &headers, at_100()),
            Err(BoardFailure::HttpStatus { retryable: false }),
            "{status}"
        );
    }
    // A 429 never reads the content type or the length.
    let failure = admit_response(429, &[pair("content-length", "PRIVATE")], at_100());
    assert_eq!(
        failure,
        Err(BoardFailure::RateLimited { retry_after: None })
    );
}

#[test]
fn content_type_rule() {
    let accepted = [
        "application/json",
        "application/json; charset=utf-8",
        "APPLICATION/JSON",
        "  application/json  ;x=y",
        "\u{1}\tapplication/json\u{1f} ",
        "application/vnd.test+json",
        "application/+json",
        "Application/Problem+JSON; charset=utf-8",
    ];
    for value in accepted {
        assert_eq!(
            admit_response(200, &[pair("Content-Type", value)], at_100()),
            Ok(None),
            "{value:?}"
        );
    }
    let refused = [
        "text/json",
        "text/plain",
        "application/",
        "application/jsonx",
        "application/xml",
        "application+json",
        "",
        ";application/json",
        "application /json",
        "\u{a0}application/json",
        "text/html; type=application/json",
    ];
    for value in refused {
        assert_eq!(
            admit_response(200, &[pair("Content-Type", value)], at_100()),
            Err(BoardFailure::ContentType),
            "{value:?}"
        );
    }
    assert_eq!(
        admit_response(200, &[], at_100()),
        Err(BoardFailure::ContentType)
    );
}

#[test]
fn first_header_wins_and_names_are_case_insensitive() {
    let headers = vec![
        pair("content-type", "application/json"),
        pair("CONTENT-TYPE", "text/plain"),
        pair("content-length", "+00073"),
    ];
    assert_eq!(admit_response(200, &headers, at_100()), Ok(Some(73)));
    let headers = vec![
        pair("CONTENT-TYPE", "text/plain"),
        pair("content-type", "application/json"),
    ];
    assert_eq!(
        admit_response(200, &headers, at_100()),
        Err(BoardFailure::ContentType)
    );
    let headers = vec![
        pair("Content-Type", "application/json"),
        pair("Content-Length", "10"),
        pair("content-length", "99999999"),
    ];
    assert_eq!(admit_response(200, &headers, at_100()), Ok(Some(10)));
}

#[test]
fn content_length_rule() {
    let check = |value: &str| {
        let mut headers = json_headers();
        headers.push(pair("Content-Length", value));
        admit_response(200, &headers, at_100())
    };
    assert_eq!(check("0"), Ok(Some(0)));
    assert_eq!(check("+0"), Ok(Some(0)));
    assert_eq!(check("-0"), Ok(Some(0)));
    assert_eq!(check("4194304"), Ok(Some(4_194_304)));
    for value in [
        "4194305",
        "-1",
        "",
        "+",
        "-",
        "abc",
        "1a",
        " 1",
        "1 ",
        "1.5",
        "9223372036854775808",
        "-9223372036854775809",
        "99999999999999999999999999",
    ] {
        assert_eq!(check(value), Err(BoardFailure::BodySize), "{value:?}");
    }
    assert_eq!(check("-9223372036854775808"), Err(BoardFailure::BodySize));
}

#[test]
fn retry_after_integer_semantics_do_not_depend_on_the_clock() {
    let now = at_100();
    assert_eq!(retry_seconds("0", now), Some(0));
    assert_eq!(retry_seconds("5", now), Some(5));
    assert_eq!(retry_seconds("86400", now), Some(86_400));
    for value in [
        "86401",
        "-1",
        "+1",
        " 1",
        "1 ",
        "99999999999999999999",
        "1e3",
        "abc",
        "",
    ] {
        assert_eq!(retry_seconds(value, now), None, "{value:?}");
    }
    // Absent header.
    assert_eq!(
        admit_response(429, &[], now),
        Err(BoardFailure::RateLimited { retry_after: None })
    );
}

#[test]
fn retry_after_http_date_forms() {
    let now = instant(BASE_SECONDS, 0);
    // 2026-10-04 is a Sunday.
    assert_eq!(retry_seconds("Sun, 04 Oct 2026 12:00:05 GMT", now), Some(5));
    assert_eq!(retry_seconds("04 Oct 2026 12:00:05 GMT", now), Some(5));
    assert_eq!(retry_seconds("SUN, 04 OCT 2026 12:00:05 gmt", now), Some(5));
    // Seconds are optional; narrow and wide widths are admitted.
    assert_eq!(retry_seconds("Sun, 04 Oct 2026 12:01 GMT", now), Some(60));
    assert_eq!(retry_seconds("Sun, 4 Oct 2026 12:0:5 GMT", now), Some(5));
    assert_eq!(
        retry_seconds("Sun, 0000004 Oct 000002026 012:000:0005 GMT", now),
        Some(5)
    );
    // A wrong weekday is refused.
    assert_eq!(retry_seconds("Mon, 04 Oct 2026 12:00:05 GMT", now), None);
    // Past dates and dates beyond a day are refused.
    assert_eq!(retry_seconds("Sun, 04 Oct 2026 11:59:59 GMT", now), None);
    assert_eq!(retry_seconds("Sun, 04 Oct 2026 12:00:00 GMT", now), Some(0));
    assert_eq!(retry_seconds("05 Oct 2026 12:00:00 GMT", now), Some(86_400));
    assert_eq!(retry_seconds("05 Oct 2026 12:00:01 GMT", now), None);
    // A fractional clock second counts as a whole elapsed second.
    let fractional = instant(BASE_SECONDS, 250_000_000);
    assert_eq!(
        retry_seconds("04 Oct 2026 12:00:05 GMT", fractional),
        Some(4)
    );
    assert_eq!(retry_seconds("04 Oct 2026 12:00:00 GMT", fractional), None);
    // Zones: GMT or +/-HH[MM], hours up to 18 and at 18 no minutes.
    assert_eq!(retry_seconds("04 Oct 2026 14:00:05 +02", now), Some(5));
    assert_eq!(retry_seconds("04 Oct 2026 14:00:05 +0200", now), Some(5));
    assert_eq!(retry_seconds("04 Oct 2026 10:00:05 -0200", now), Some(5));
    assert_eq!(retry_seconds("04 Oct 2026 12:10:00 +0010", now), Some(0));
    assert_eq!(retry_seconds("04 Oct 2026 12:00:05 +0000", now), Some(5));
    assert_eq!(retry_seconds("04 Oct 2026 12:00:05 -00", now), Some(5));
    assert_eq!(retry_seconds("05 Oct 2026 06:00:00 +18", now), Some(0));
    for zone in [
        "+1801", "+19", "+0060", "+053", "+05:30", "UTC", "Z", "+5", "",
    ] {
        assert_eq!(
            retry_seconds(&format!("04 Oct 2026 12:00:05 {zone}"), now),
            None,
            "{zone:?}"
        );
    }
    // Malformed shapes.
    for value in [
        "Sun,04 Oct 2026 12:00:05 GMT",
        "Sun, 04  Oct 2026 12:00:05 GMT",
        "Sun, 04 Oct 2026 12:00:05",
        "Sun, 04 Oct 2026 12:00:05 GMT ",
        " Sun, 04 Oct 2026 12:00:05 GMT",
        "Sun, 04 Oct 2026 12:00:05:01 GMT",
        "Sun, 04 Oct 2026 12 GMT",
        "Sun, 04 Octo 2026 12:00:05 GMT",
        "Sun, 04 Oct 2026 12:00:60 GMT",
        "Sun, 04 Oct 2026 12:60:00 GMT",
        "Sun, 04 Oct 2026 25:00:00 GMT",
        "Sun, 04 Oct 2026 24:00:01 GMT",
        "Sun, 04 Oct 2026 24:01:00 GMT",
        "Sun, 1234567890 Oct 2026 12:00:05 GMT",
        "Sun, 04 Oct 1234567890 12:00:05 GMT",
        "Sun, 00 Oct 2026 12:00:05 GMT",
        "Sun, 32 Oct 2026 12:00:05 GMT",
        "Sun, 04 Oct 2026 -1:00:05 GMT",
    ] {
        assert_eq!(retry_seconds(value, now), None, "{value:?}");
    }
}

#[test]
fn retry_after_day_clamp_and_midnight_quirks() {
    // 2026-02-28 11:59:59.25: Feb 29 and later days clamp to Feb 28, 12:00:00 is one second later.
    let now = instant(1_772_279_999, 250_000_000);
    assert_eq!(retry_seconds("28 Feb 2026 12:00:00 GMT", now), Some(0));
    assert_eq!(retry_seconds("29 Feb 2026 12:00:00 GMT", now), Some(0));
    assert_eq!(retry_seconds("31 Feb 2026 12:00:00 GMT", now), Some(0));
    assert_eq!(retry_seconds("32 Feb 2026 12:00:00 GMT", now), None);
    // The clamp precedes the weekday check: 31 Apr is 30 Apr 2026, a Thursday.
    let april = instant(1_777_550_400, 0); // 2026-04-30 12:00:00Z
    assert_eq!(
        retry_seconds("Thu, 31 Apr 2026 12:00:05 GMT", april),
        Some(5)
    );
    assert_eq!(retry_seconds("Fri, 31 Apr 2026 12:00:05 GMT", april), None);
    // Leap years: 2024-02-29 exists, 2100-02-29 clamps to 28.
    let leap = instant(1_709_208_000, 0); // 2024-02-29 12:00:00Z
    assert_eq!(
        retry_seconds("Thu, 29 Feb 2024 12:00:05 GMT", leap),
        Some(5)
    );
    // Hour 24:00:00 is the next midnight.
    let evening = instant(1_791_158_400 - 10, 0); // 2026-10-04 23:59:50Z
    assert_eq!(
        retry_seconds("Sun, 04 Oct 2026 24:00:00 GMT", evening),
        Some(10)
    );
    assert_eq!(
        retry_seconds("Mon, 05 Oct 2026 00:00:00 GMT", evening),
        Some(10)
    );
    assert_eq!(
        retry_seconds("Sun, 04 Oct 2026 24:00 GMT", evening),
        Some(10)
    );
    // Year 0 and years beyond the common datetime range stay valid inputs.
    let year_zero = instant(-62_167_219_200, 250_000_000);
    assert_eq!(
        retry_seconds("01 Jan 0000 00:00:05 GMT", year_zero),
        Some(4)
    );
    let year_10000 = instant(253_402_300_800, 250_000_000);
    assert_eq!(
        retry_seconds("01 Jan 10000 00:00:05 GMT", year_10000),
        Some(4)
    );
}

#[test]
fn body_bound_is_exact() {
    let base = envelope(&[job("a")], 1, None, Value::Null);
    let mut exact = base.clone();
    exact.resize(MAX_BODY_BYTES, b' ');
    assert_eq!(parse_ok(&exact).jobs.len(), 1);
    exact.push(b' ');
    assert_eq!(
        category(parse_page(&exact, 1, None)),
        BoardCategory::BodySize
    );
    let failure = parse_page(&exact, 1, None).expect_err("size");
    assert!(!failure.retryable());
}

#[test]
fn job_count_bound() {
    let jobs: Vec<Value> = (0..325).map(|n| job(&format!("job-{n}"))).collect();
    assert_eq!(
        parse_ok(&envelope(&jobs, 1, None, Value::Null)).jobs.len(),
        325
    );
    let jobs: Vec<Value> = (0..500).map(|n| job(&format!("job-{n}"))).collect();
    assert_eq!(
        parse_ok(&envelope(&jobs, 1, None, Value::Null)).jobs.len(),
        500
    );
    let jobs: Vec<Value> = (0..501).map(|n| job(&format!("job-{n}"))).collect();
    assert_eq!(
        category(parse_page(&envelope(&jobs, 1, None, Value::Null), 1, None)),
        BoardCategory::Schema
    );
}

#[test]
fn list_element_bound_is_64_and_entries_are_cut_at_128_units() {
    for name in ["tags", "job_types"] {
        let ok = with(job("a"), name, json!(vec!["t"; 64]));
        assert_eq!(
            parse_ok(&envelope(&[ok], 1, None, Value::Null)).jobs.len(),
            1
        );
        let too_many = with(job("a"), name, json!(vec!["t"; 65]));
        assert_eq!(
            category(parse_page(
                &envelope(&[too_many], 1, None, Value::Null),
                1,
                None
            )),
            BoardCategory::Schema
        );
        let long = with(job("a"), name, json!(["x".repeat(500)]));
        let page = parse_ok(&envelope(&[long], 1, None, Value::Null));
        let entries = if name == "tags" {
            &page.jobs[0].tags
        } else {
            &page.jobs[0].job_types
        };
        assert_eq!(entries[0].len(), 128);
        let not_text = with(job("a"), name, json!(["a", 1]));
        assert_eq!(
            category(parse_page(
                &envelope(&[not_text], 1, None, Value::Null),
                1,
                None
            )),
            BoardCategory::Schema
        );
        let not_list = with(job("a"), name, json!("a"));
        assert_eq!(
            category(parse_page(
                &envelope(&[not_list], 1, None, Value::Null),
                1,
                None
            )),
            BoardCategory::Schema
        );
    }
    // null and missing lists are empty.
    let mut missing = job("a");
    missing.as_object_mut().expect("object").remove("tags");
    let null = with(job("a"), "job_types", Value::Null);
    let page = parse_ok(&envelope(&[missing, null], 1, None, Value::Null));
    assert!(page.jobs[0].tags.is_empty() && page.jobs[1].job_types.is_empty());
}

#[test]
fn utf16_truncation_never_leaves_a_lone_surrogate_and_slug_counts_code_points() {
    let emoji = "\u{1F600}";
    let title = format!("{}{emoji}", "A".repeat(255));
    let body = envelope(
        &[with(job(&emoji.repeat(256)), "title", json!(title))],
        1,
        None,
        Value::Null,
    );
    let page = parse_ok(&body);
    assert_eq!(page.jobs[0].slug, emoji.repeat(256));
    // A dangling high surrogate left by the cut is dropped.
    assert_eq!(page.jobs[0].title, "A".repeat(255));
    let exact = with(
        job("a"),
        "title",
        json!(format!("{}{emoji}", "A".repeat(254))),
    );
    let page = parse_ok(&envelope(&[exact], 1, None, Value::Null));
    assert_eq!(page.jobs[0].title.encode_utf16().count(), 256);
    assert_eq!(
        category(parse_page(
            &envelope(&[job(&emoji.repeat(257))], 1, None, Value::Null),
            1,
            None
        )),
        BoardCategory::Schema
    );
    // Caps in UTF-16 units: 256, 256, 4096, 256, 128.
    let long = "é".repeat(5000);
    let mut value = job("a");
    value["company_name"] = json!(long);
    value["description"] = json!(long);
    value["location"] = json!(long);
    value["tags"] = json!([long]);
    let page = parse_ok(&envelope(&[value], 1, None, Value::Null));
    assert_eq!(page.jobs[0].company.chars().count(), 256);
    assert_eq!(page.jobs[0].html_description.chars().count(), 4096);
    assert_eq!(
        page.jobs[0].location.as_deref().map(|l| l.chars().count()),
        Some(256)
    );
    assert_eq!(page.jobs[0].tags[0].chars().count(), 128);
}

#[test]
fn job_fields_and_types() {
    let page = parse_ok(&envelope(&[job("first")], 1, None, Value::Null));
    let row = &page.jobs[0];
    assert_eq!(row.slug, "first");
    assert_eq!(row.company, "Acme");
    assert_eq!(row.title, "Java Engineer");
    assert_eq!(row.html_description, "<p>Java description</p>");
    assert_eq!(row.url, "https://ignored.invalid");
    assert_eq!(row.remote, Some(true));
    assert_eq!(row.tags, ["Java"]);
    assert_eq!(row.job_types, ["Full Time"]);
    assert_eq!(row.location.as_deref(), Some("Berlin"));
    assert_eq!(row.created_at, None);
    // Optional fields: null and missing.
    let mut minimal = job("m");
    for key in ["remote", "location", "created_at", "tags", "job_types"] {
        minimal.as_object_mut().expect("object").remove(key);
    }
    let nulls = with(
        with(job("n"), "remote", Value::Null),
        "location",
        Value::Null,
    );
    let page = parse_ok(&envelope(&[minimal, nulls], 1, None, Value::Null));
    for row in &page.jobs {
        assert_eq!(
            (row.remote, row.location.as_deref(), row.created_at),
            (None, None, None)
        );
    }
    // created_at bounds.
    let epoch = |value: Value| {
        let body = serde_json::to_vec(&json!({
            "data": [with(job("e"), "created_at", value)],
            "links": {"next": null},
            "meta": {"current_page": 1},
        }))
        .expect("serialize");
        parse_page(&body, 1, None)
    };
    assert_eq!(epoch(json!(0)).expect("zero").jobs[0].created_at, Some(0));
    let raw = |literal: &str| {
        let text = format!(
            r#"{{"data":[{{"slug":"e","company_name":"A","title":"T","description":"D","url":"u","created_at":{literal}}}],"links":{{"next":null}},"meta":{{"current_page":1}}}}"#
        );
        parse_page(text.as_bytes(), 1, None)
    };
    assert_eq!(
        raw("9223372036854775807").expect("max").jobs[0].created_at,
        Some(i64::MAX)
    );
    for literal in [
        "-1",
        "9223372036854775808",
        "1.5",
        "1e3",
        "true",
        "\"1\"",
        "[]",
    ] {
        assert_eq!(category(raw(literal)), BoardCategory::Schema, "{literal}");
    }
    // Wrong types and missing required keys.
    for broken in [
        with(job("a"), "slug", json!(1)),
        with(job("a"), "slug", Value::Null),
        with(job("a"), "company_name", Value::Null),
        with(job("a"), "title", json!(1)),
        with(job("a"), "description", Value::Null),
        with(job("a"), "url", Value::Null),
        with(job("a"), "remote", json!("yes")),
        with(job("a"), "location", json!(1)),
        json!("not an object"),
        json!([]),
        Value::Null,
    ] {
        assert_eq!(
            category(parse_page(
                &envelope(&[broken], 1, None, Value::Null),
                1,
                None
            )),
            BoardCategory::Schema
        );
    }
    for key in ["slug", "company_name", "title", "description", "url"] {
        let mut missing = job("a");
        missing.as_object_mut().expect("object").remove(key);
        assert_eq!(
            category(parse_page(
                &envelope(&[missing], 1, None, Value::Null),
                1,
                None
            )),
            BoardCategory::Schema,
            "{key}"
        );
    }
}

#[test]
fn schema_order_and_counters() {
    let ok = |text: &str| parse_page(text.as_bytes(), 1, None);
    assert_eq!(category(ok("[]")), BoardCategory::Schema);
    assert_eq!(category(ok("null")), BoardCategory::Schema);
    assert_eq!(category(ok("")), BoardCategory::Schema);
    assert_eq!(category(ok(" \t\r\n")), BoardCategory::Schema);
    assert_eq!(category(ok("{}")), BoardCategory::Schema);
    assert_eq!(
        category(ok(r#"{"data":[],"links":{},"meta":{"current_page":1}}"#)),
        BoardCategory::Schema
    );
    assert_eq!(
        category(ok(
            r#"{"data":{},"links":{"next":null},"meta":{"current_page":1}}"#
        )),
        BoardCategory::Schema
    );
    assert_eq!(
        category(ok(r#"{"data":[],"links":[],"meta":{"current_page":1}}"#)),
        BoardCategory::Schema
    );
    assert_eq!(
        category(ok(r#"{"data":[],"links":{"next":null},"meta":[]}"#)),
        BoardCategory::Schema
    );
    for current in ["0", "2", "1.0", "\"1\"", "true", "null", "2147483648", "-1"] {
        let text =
            format!(r#"{{"data":[],"links":{{"next":null}},"meta":{{"current_page":{current}}}}}"#);
        assert_eq!(category(ok(&text)), BoardCategory::Schema, "{current}");
    }
    for last in ["0", "null", "\"1\"", "2147483648", "1.0"] {
        let text = format!(
            r#"{{"data":[],"links":{{"next":null}},"meta":{{"current_page":1,"last_page":{last}}}}}"#
        );
        assert_eq!(category(ok(&text)), BoardCategory::Schema, "{last}");
    }
    // last_page before current_page.
    let body = envelope(&[], 3, Some(2), Value::Null);
    assert_eq!(category(parse_page(&body, 3, None)), BoardCategory::Schema);
    // Page mismatch.
    assert_eq!(
        category(parse_page(&envelope(&[], 2, Some(2), Value::Null), 1, None)),
        BoardCategory::Schema
    );
    // Missing links.next.
    // A job failure precedes the pagination check.
    let body = envelope(&[json!(1)], 1, Some(1), json!("bad"));
    assert_eq!(category(parse_page(&body, 1, None)), BoardCategory::Schema);
    // Request check first.
    assert_eq!(
        parse_page(&envelope(&[], 1, Some(1), Value::Null), 0, None),
        Err(BoardFailure::HttpStatus { retryable: false })
    );
}

#[test]
fn json_failures() {
    let base = r#"{"data":[],"links":{"next":null},"meta":{"current_page":1,"last_page":1}"#;
    for tail in [
        r#","x":NaN}"#,
        r#","x":Infinity}"#,
        r#","x":-Infinity}"#,
        r#","x":1e}"#,
        r#","x":1,"x":2}"#,
        r#","data":1}"#,
    ] {
        let text = format!("{base}{tail}");
        assert_eq!(
            category(parse_page(text.as_bytes(), 1, None)),
            BoardCategory::Json,
            "{tail}"
        );
    }
    assert_eq!(category(parse_page(b"{", 1, None)), BoardCategory::Json);
    assert_eq!(
        category(parse_page(b"\xEF\xBB\xBF", 1, None)),
        BoardCategory::Json
    );
    let trailing = format!("{base}}} x");
    assert_eq!(
        category(parse_page(trailing.as_bytes(), 1, None)),
        BoardCategory::Json
    );
    // Whitespace around a good document is fine; a UTF-8 BOM before it too.
    let good = format!("{base}}}");
    assert_eq!(parse_ok(format!("  \n{good}\r\n").as_bytes()).jobs.len(), 0);
    let mut with_bom = b"\xEF\xBB\xBF".to_vec();
    with_bom.extend_from_slice(good.as_bytes());
    assert_eq!(parse_ok(&with_bom).jobs.len(), 0);
    // Depth: 500 is fine, 501 is not.
    let nest = |depth: usize| {
        format!(
            r#"{{"data":[],"links":{{"next":null}},"meta":{{"current_page":1}},"x":{}0{}}}"#,
            "[".repeat(depth - 1),
            "]".repeat(depth - 1)
        )
    };
    assert_eq!(parse_ok(nest(500).as_bytes()).jobs.len(), 0);
    assert_eq!(
        category(parse_page(nest(501).as_bytes(), 1, None)),
        BoardCategory::Json
    );
}

fn utf16_body(text: &str, big: bool, bom: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if bom {
        bytes.extend_from_slice(if big { &[0xFE, 0xFF] } else { &[0xFF, 0xFE] });
    }
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&if big {
            unit.to_be_bytes()
        } else {
            unit.to_le_bytes()
        });
    }
    bytes
}

fn utf32_body(text: &str, big: bool, bom: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if bom {
        bytes.extend_from_slice(if big {
            &[0x00, 0x00, 0xFE, 0xFF]
        } else {
            &[0xFF, 0xFE, 0x00, 0x00]
        });
    }
    for ch in text.chars() {
        let code = u32::from(ch);
        bytes.extend_from_slice(&if big {
            code.to_be_bytes()
        } else {
            code.to_le_bytes()
        });
    }
    bytes
}

#[test]
fn bom_utf16_and_utf32_bodies() {
    let text = String::from_utf8(envelope(
        &[with(job("wide"), "title", json!("\u{e9}\u{1F600}"))],
        1,
        Some(1),
        Value::Null,
    ))
    .expect("utf8");
    // serde_json escapes nothing here: the text keeps its raw non-ASCII characters.
    assert!(text.contains('\u{1F600}'));
    let bodies = [
        utf16_body(&text, false, true),
        utf16_body(&text, true, true),
        utf16_body(&text, false, false),
        utf16_body(&text, true, false),
        utf32_body(&text, false, true),
        utf32_body(&text, true, true),
        utf32_body(&text, false, false),
        utf32_body(&text, true, false),
        [b"\xEF\xBB\xBF".as_slice(), text.as_bytes()].concat(),
        text.clone().into_bytes(),
    ];
    for (index, body) in bodies.iter().enumerate() {
        let page = parse_ok(body);
        assert_eq!(page.jobs[0].slug, "wide", "{index}");
        assert_eq!(page.jobs[0].title, "\u{e9}\u{1F600}", "{index}");
    }
    // Odd lengths, UTF-32 above U+10FFFF and non-ASCII outside a string are refused.
    let mut odd = utf16_body(&text, false, true);
    odd.push(0);
    assert_eq!(category(parse_page(&odd, 1, None)), BoardCategory::Json);
    let mut beyond = utf32_body("{}", false, true);
    beyond.extend_from_slice(&0x11_0000u32.to_le_bytes());
    assert_eq!(category(parse_page(&beyond, 1, None)), BoardCategory::Json);
    let outside = format!("{text}\u{e9}");
    assert_eq!(
        category(parse_page(outside.as_bytes(), 1, None)),
        BoardCategory::Json
    );
    // Lone surrogate escapes are refused.
    let escaped = String::from_utf8(envelope(&[job("a")], 1, Some(1), Value::Null))
        .expect("utf8")
        .replace("Java Engineer", "\\ud800");
    assert_eq!(
        category(parse_page(escaped.as_bytes(), 1, None)),
        BoardCategory::Json
    );
}

#[test]
fn six_rejected_next_link_shapes_are_pagination_failures() {
    let rejected = [
        "https://evil.example/api/job-board-api?page=2",
        "http://www.arbeitnow.com/api/job-board-api?page=2",
        "https://www.arbeitnow.com/other?page=2",
        "https://www.arbeitnow.com.evil.example/api/job-board-api?page=2",
        "https://www.arbeitnow.com/api/job-board-api?page=3",
        "https://www.arbeitnow.com/api/job-board-api?page=2&x=1",
    ];
    for next in rejected {
        for last in [None, Some(5)] {
            let body = envelope(&[job("a")], 1, last, json!(next));
            assert_eq!(
                category(parse_page(&body, 1, None)),
                BoardCategory::Pagination,
                "{next} {last:?}"
            );
        }
    }
}

#[test]
fn next_link_rules() {
    let valid = [
        "https://www.arbeitnow.com/api/job-board-api?page=2",
        "HTTPS://WWW.ARBEITNOW.COM/api/job-board-api?page=2",
        "https://arbeitnow.com/api/job-board-api?page=2",
        "https://www.arbeitnow.com:443/api/job-board-api?page=2",
        "https://www.arbeitnow.com:000443/api/job-board-api?page=2",
        // An empty port reads as absent.
        "https://www.arbeitnow.com:/api/job-board-api?page=2",
        "https://www.arbeitnow.com/api/job-board-api?page=2&visa_sponsorship=true",
        "https://www.arbeitnow.com/api/job-board-api?visa_sponsorship=true&page=2",
    ];
    for (index, next) in valid.iter().enumerate() {
        let visa = (index >= 6).then_some(true);
        let body = envelope(&[job("a")], 1, Some(2), json!(next));
        let page = parse_page(&body, 1, visa).unwrap_or_else(|f| panic!("{next}: {f:?}"));
        assert!(page.another_page_may_remain, "{next}");
    }
    let rejected = [
        "",
        "https://www.arbeitnow.com/api/job-board-api",
        "https://www.arbeitnow.com/api/job-board-api?",
        "https://www.arbeitnow.com/api/job-board-api?page=",
        "https://www.arbeitnow.com/api/job-board-api?page=2&page=2",
        "https://www.arbeitnow.com/api/job-board-api?page=02",
        "https://www.arbeitnow.com/api/job-board-api?page=%32",
        "https://www.arbeitnow.com/api/job-board-api?page=2&",
        "https://www.arbeitnow.com/api/job-board-api?page=2=3",
        "https://www.arbeitnow.com/api/job-board-api?page",
        "https://www.arbeitnow.com/api/job-board-api?page=2&visa_sponsorship=true",
        "https://www.arbeitnow.com/api/job-board-api?page=2#top",
        "https://www.arbeitnow.com/api/job-board-api/?page=2",
        "https://www.arbeitnow.com/API/job-board-api?page=2",
        "https://www.arbeitnow.com:444/api/job-board-api?page=2",
        "https://www.arbeitnow.com:44a/api/job-board-api?page=2",
        "https://www.arbeitnow.com:80443/api/job-board-api?page=2",
        "https://user@www.arbeitnow.com/api/job-board-api?page=2",
        "https://www.arbeitnow.com@evil.example/api/job-board-api?page=2",
        "https://[www.arbeitnow.com]/api/job-board-api?page=2",
        "https://sub.arbeitnow.com/api/job-board-api?page=2",
        "https://www.arbeitnow.com./api/job-board-api?page=2",
        "//www.arbeitnow.com/api/job-board-api?page=2",
        "/api/job-board-api?page=2",
        "https:www.arbeitnow.com/api/job-board-api?page=2",
        "https:/www.arbeitnow.com/api/job-board-api?page=2",
        "ftp://www.arbeitnow.com/api/job-board-api?page=2",
        "https://www.arbeitnow.com/api/job-board-api?page=2&visa_sponsorship=",
        "https://www.arbeitnow.com/api/job-board-api?visa=true&page=2",
        "https://www.arbeitnow.com/api/job-board-api?page=2 ",
        "https://www.arbeitnow.com/api/job-board-api?page=2&x=a b",
        "https://www.arbeitnow.com/api/job-board-api?page=2&x=%zz",
        "https://www.arbeitnow.com/api/job-board-api?page=2&x=%2",
        "https://www.arbeitnow.com/api/job-board-api%?page=2",
        "https://www.arbeitnow.com/api/job-board-api?page=2&x=<",
        "https://www.arbeitnow.com/api/job-board-api?page=2&x=>",
        "https://www.arbeitnow.com/api/job-board-api?page=2&x=\"",
        "https://www.arbeitnow.com/api/job-board-api?page=2&x={",
        "https://www.arbeitnow.com/api/job-board-api?page=2&x=}",
        "https://www.arbeitnow.com/api/job-board-api?page=2&x=|",
        "https://www.arbeitnow.com/api/job-board-api?page=2&x=\\",
        "https://www.arbeitnow.com/api/job-board-api?page=2&x=^",
        "https://www.arbeitnow.com/api/job-board-api?page=2&x=`",
        "https://www.arbeitnow.com/api/job-board-api?page=2\u{7f}",
        "https://www.arbeitnow.com/api/job-board-api?page=2\u{1}",
        "https://www.arbeitnow.com/api/job-board-api?page=2\t",
    ];
    for next in rejected {
        let body = envelope(&[job("a")], 1, Some(2), json!(next));
        assert_eq!(
            category(parse_page(&body, 1, None)),
            BoardCategory::Pagination,
            "{next:?}"
        );
    }
    // Valid percent escapes outside the key set are still refused by the key rule, not the escape.
    let escaped = "https://www.arbeitnow.com/api/job-board-api?page=2&x=%41";
    assert_eq!(
        category(parse_page(
            &envelope(&[job("a")], 1, Some(2), json!(escaped)),
            1,
            None
        )),
        BoardCategory::Pagination
    );
    // Length: 2048 UTF-16 units pass the length rule, 2049 do not.
    let prefix = "https://www.arbeitnow.com/api/job-board-api?page=2&visa_sponsorship=true";
    let over = format!("{prefix}&x={}", "a".repeat(2049 - prefix.len() - 3));
    assert_eq!(over.encode_utf16().count(), 2049);
    assert_eq!(
        category(parse_page(
            &envelope(&[job("a")], 1, Some(2), json!(over)),
            1,
            Some(true)
        )),
        BoardCategory::Pagination
    );
}

#[test]
fn next_link_visa_must_match_the_request() {
    let plain = "https://www.arbeitnow.com/api/job-board-api?page=2";
    let yes = format!("{plain}&visa_sponsorship=true");
    let no = format!("{plain}&visa_sponsorship=false");
    let accepts = |next: &str, visa: Option<bool>| {
        parse_page(&envelope(&[job("a")], 1, Some(2), json!(next)), 1, visa).is_ok()
    };
    assert!(accepts(plain, None));
    assert!(accepts(&yes, Some(true)));
    assert!(accepts(&no, Some(false)));
    assert!(!accepts(&yes, None));
    assert!(!accepts(&no, None));
    assert!(!accepts(plain, Some(true)));
    assert!(!accepts(plain, Some(false)));
    assert!(!accepts(&yes, Some(false)));
    assert!(!accepts(&no, Some(true)));
    assert!(!accepts(
        &format!("{plain}&visa_sponsorship=TRUE"),
        Some(true)
    ));
}

#[test]
fn last_page_rules() {
    // Present last_page stays strict: a null next with last > current is a pagination failure.
    assert_eq!(
        category(parse_page(
            &envelope(&[job("a")], 1, Some(2), Value::Null),
            1,
            None
        )),
        BoardCategory::Pagination
    );
    // A null next on the last page, or without last_page, is the final page.
    assert!(!parse_ok(&envelope(&[job("a")], 1, Some(1), Value::Null)).another_page_may_remain);
    assert!(!parse_ok(&envelope(&[job("a")], 1, None, Value::Null)).another_page_may_remain);
    // A next link beyond last_page is refused.
    assert_eq!(
        category(parse_page(
            &envelope(&[job("a")], 1, Some(1), link("2")),
            1,
            None
        )),
        BoardCategory::Pagination
    );
    let page = parse_ok(&envelope(&[job("a")], 1, Some(2), link("2")));
    assert!(page.another_page_may_remain);
    // Missing last_page: the next link is followed while the page holds jobs.
    let page = parse_ok(&envelope(&[job("a")], 1, None, link("2")));
    assert!(page.another_page_may_remain && page.jobs.len() == 1);
    // Missing last_page with an empty page stops even with a next link.
    let page = parse_ok(&envelope(&[], 1, None, link("2")));
    assert!(!page.another_page_may_remain && page.jobs.is_empty());
    // With last_page present an empty page with a next link may still continue.
    assert!(parse_ok(&envelope(&[], 1, Some(2), link("2"))).another_page_may_remain);
    // Other link types.
    for next in [json!(1), json!(true), json!([]), json!({})] {
        assert_eq!(
            category(parse_page(&envelope(&[job("a")], 1, None, next), 1, None)),
            BoardCategory::Pagination
        );
    }
    // A missing next key is a schema failure.
    let text = br#"{"data":[],"links":{},"meta":{"current_page":1}}"#;
    assert_eq!(category(parse_page(text, 1, None)), BoardCategory::Schema);
}

#[test]
fn page_numbers_follow_the_requested_page() {
    let body = envelope(&[job("a")], 7, Some(9), link("8"));
    assert!(
        parse_page(&body, 7, None)
            .expect("page")
            .another_page_may_remain
    );
    assert_eq!(category(parse_page(&body, 6, None)), BoardCategory::Schema);
    // The largest page without a next link is the last page.
    let max = 2_147_483_647u64;
    let body = envelope(&[job("a")], max, Some(max), Value::Null);
    assert!(
        !parse_page(&body, 2_147_483_647, None)
            .expect("page")
            .another_page_may_remain
    );
    // The expected page wraps to -2^31 for the largest request: a next link naming any
    // page above it fails.
    for next in [link("2147483648"), link("0"), link("2147483647")] {
        let body = envelope(&[job("a")], max, Some(max), next);
        assert_eq!(
            category(parse_page(&body, 2_147_483_647, None)),
            BoardCategory::Pagination
        );
    }
}

#[test]
fn utf16_hex_helper_matches_java_observations() {
    assert_eq!(utf16_hex("A\u{1F600}"), "0041d83dde00");
}

fn observation_of(result: &Result<jobniac_core::arbeitnow::board::Page, BoardFailure>) -> Value {
    match result {
        Ok(page) => json!({"ok": {
            "jobs": page.jobs.iter().map(|row| json!({
                "slug": utf16_hex(&row.slug),
                "company": utf16_hex(&row.company),
                "title": utf16_hex(&row.title),
                "htmlDescription": utf16_hex(&row.html_description),
                "remote": row.remote,
                "url": utf16_hex(&row.url),
                "tags": row.tags.iter().map(|tag| utf16_hex(tag)).collect::<Vec<_>>(),
                "jobTypes": row.job_types.iter().map(|tag| utf16_hex(tag)).collect::<Vec<_>>(),
                "location": row.location.as_deref().map(utf16_hex),
                "createdAt": row.created_at,
            })).collect::<Vec<_>>(),
            "anotherPageMayRemain": page.another_page_may_remain,
        }}),
        Err(failure) => json!({"error": {
            "category": failure.category().name(),
            "retryable": failure.retryable(),
            "retryAfter": failure.retry_after_seconds(),
        }}),
    }
}

fn hex_bytes(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex"))
        .collect()
}

/// The flow of `InjectedArbeitnowBoard.fetch` over a recorded response.
fn run_flow(case: &Value) -> Result<jobniac_core::arbeitnow::board::Page, BoardFailure> {
    use base64::Engine as _;
    let status = u16::try_from(case["status"].as_u64().expect("status")).expect("u16");
    let headers: Headers = case["headersHex"]
        .as_array()
        .expect("headers")
        .iter()
        .map(|entry| {
            (
                hex_bytes(entry[0].as_str().expect("name")),
                hex_bytes(entry[1].as_str().expect("value")),
            )
        })
        .collect();
    let body = base64::engine::general_purpose::STANDARD
        .decode(case["bodyBase64"].as_str().expect("body"))
        .expect("base64");
    let page = case["page"].as_i64().expect("page");
    let visa = case["visa"].as_bool();
    let now = instant(
        case["clockSeconds"].as_i64().expect("seconds"),
        u32::try_from(case["clockNanos"].as_u64().expect("nanos")).expect("nanos"),
    );
    let requested = validate_request(page, visa)?;
    admit_response(status, &headers, now)?;
    parse_page(&body, requested, visa)
}

const JSON_ERROR: &str = r#"{"error":{"category":"json","retryable":false,"retryAfter":null}}"#;

/// The UTF-16 hex text without its unpaired surrogate units.
fn without_lone_surrogates(hex: &str) -> String {
    let units: Vec<u16> = (0..hex.len())
        .step_by(4)
        .map(|index| u16::from_str_radix(&hex[index..index + 4], 16).expect("hex"))
        .collect();
    char::decode_utf16(units)
        .filter_map(Result::ok)
        .flat_map(|ch| {
            let mut buffer = [0u16; 2];
            ch.encode_utf16(&mut buffer).to_vec()
        })
        .map(|unit| format!("{unit:04x}"))
        .collect()
}

/// The recorded observation with every lone surrogate dropped from the job texts.
fn stripped(recorded: &Value) -> Value {
    fn strip(value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(without_lone_surrogates(text)),
            Value::Array(items) => Value::Array(items.iter().map(strip).collect()),
            other => other.clone(),
        }
    }
    let mut copy = recorded.clone();
    if let Some(jobs) = copy["ok"]["jobs"].as_array_mut() {
        for row in jobs {
            for key in [
                "slug",
                "company",
                "title",
                "htmlDescription",
                "url",
                "tags",
                "jobTypes",
                "location",
            ] {
                row[key] = strip(&row[key]);
            }
        }
    }
    copy
}

/// The documented deviations from the recorded observations, as the Rust observation they must
/// produce: Rust refuses the malformed UTF-8 sequences and the lone-surrogate escapes that the
/// recorded run admitted, and drops a lone high surrogate that a UTF-16 cut leaves in a text.
fn expected_deviation(id: &str, recorded: &Value) -> Vec<Value> {
    let json_error: Value = serde_json::from_str(JSON_ERROR).expect("json");
    if id.starts_with("utf8-overlong-") || id.starts_with("pagination-uri-lone-") {
        return vec![json_error];
    }
    // A lone surrogate from an escape or a UTF-32 code point is refused; one left by a UTF-16
    // cut is dropped.
    vec![json_error, stripped(recorded)]
}

#[test]
fn golden_golden_vectors() {
    let text = include_str!("golden/arbeitnow-board.json");
    let document: Value = serde_json::from_str(text).expect("vector file");
    let cases = document["cases"].as_array().expect("cases");
    assert!(cases.len() >= 250, "{} cases", cases.len());
    let mut mismatches = Vec::new();
    let mut deviations = 0;
    for case in cases {
        let id = case["id"].as_str().expect("id");
        let actual = observation_of(&run_flow(case));
        let recorded = &case["recorded"];
        if &actual == recorded {
            continue;
        }
        if expected_deviation(id, recorded).contains(&actual) {
            deviations += 1;
            continue;
        }
        mismatches.push(format!("{id}\n  recorded: {recorded}\n  rust:   {actual}"));
    }
    assert!(
        mismatches.is_empty(),
        "{} mismatches ({deviations} documented deviations):\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
    // Lone-surrogate escapes and code points, the UTF-16 cut and malformed UTF-8.
    assert_eq!(deviations, 49);
}
