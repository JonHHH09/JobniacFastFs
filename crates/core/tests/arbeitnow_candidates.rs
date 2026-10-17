//! Arbeitnow signed candidate tokens: the cases the Rust types can express and the byte-level check
//! against `golden/arbeitnow-candidates.json`, which uses the TEST-ONLY key `bytes(range(32))`.
//!
//! Not tested, because Rust types make them unrepresentable: wrong-typed arguments, caller-owned
//! mutable lists, a clock returning a wrong type, monkeypatched internals. Lone surrogates are
//! refused; see `lone_surrogate_escape_is_refused`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{KeyInit, Mac, SimpleHmac};
use jobniac_core::arbeitnow::candidates::{
    ArbeitnowCandidate, CandidateCodec, MAX_PAYLOAD_BYTES, MAX_TOKEN_CHARS,
};
use jobniac_core::arbeitnow::instant::{ArbeitnowInstant, MAX_EPOCH_SECOND, MIN_EPOCH_SECOND};
use jobniac_core::errors::{ApplicationError, ErrorCode};
use serde_json::{Value, json};
use sha2::Sha256;

const GOLDEN: &str = include_str!("golden/arbeitnow-candidates.json");
const MESSAGE: &str = "Invalid Arbeitnow candidate token";

fn key() -> Vec<u8> {
    (0u8..32).collect()
}

fn instant(second: i64, nano: u32) -> ArbeitnowInstant {
    ArbeitnowInstant::new(second, nano).expect("in range")
}

fn codec_at(second: i64, nano: u32) -> CandidateCodec {
    let now = instant(second, nano);
    CandidateCodec::with_instant_source(&key(), move || Ok(now)).expect("codec")
}

fn codec() -> CandidateCodec {
    codec_at(100, 0)
}

fn candidate() -> ArbeitnowCandidate {
    ArbeitnowCandidate {
        issued_at: instant(100, 0),
        expires_at: instant(1000, 0),
        slug: "test-job".to_owned(),
        canonical_url: "https://arbeitnow.com/view/test-job".to_owned(),
        company: "Company".to_owned(),
        title: "Title".to_owned(),
        location: String::new(),
        remote: false,
        posted_at: None,
        tags: Vec::new(),
        job_types: Vec::new(),
        description_excerpt: "Description".to_owned(),
    }
}

fn payload() -> Value {
    json!({
        "v": 1,
        "provider": "arbeitnow",
        "issuedAt": 100,
        "expiresAt": 1000,
        "slug": "test-job",
        "canonicalUrl": "https://arbeitnow.com/view/test-job",
        "company": "Company",
        "title": "Title",
        "location": "",
        "remote": false,
        "postedAt": null,
        "tags": [],
        "jobTypes": [],
        "descriptionExcerpt": "",
    })
}

fn with(changes: &[(&str, Value)]) -> Value {
    let mut document = payload();
    for (name, value) in changes {
        document[*name] = value.clone();
    }
    document
}

fn sign(body: &[u8]) -> Vec<u8> {
    let mut mac = <SimpleHmac<Sha256> as KeyInit>::new_from_slice(&key()).expect("key");
    mac.update(body);
    mac.finalize().into_bytes().to_vec()
}

fn encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn signed_bytes(body: &[u8]) -> String {
    format!("{}.{}", encode(body), encode(&sign(body)))
}

fn signed(document: &Value) -> String {
    signed_bytes(&serde_json::to_vec(document).expect("json"))
}

fn assert_invalid(result: Result<impl std::fmt::Debug, ApplicationError>) {
    let error = result.expect_err("must be refused");
    assert_eq!(error.code(), ErrorCode::ValidationError, "{error:?}");
    assert_eq!(error.message(), MESSAGE);
    assert!(error.details().is_empty());
}

fn accepts(token: &str) -> bool {
    codec().verify(token).is_ok()
}

#[test]
fn issued_token_round_trips_and_is_deterministic() {
    let token = codec().issue(&candidate()).expect("issue");
    assert_eq!(token, codec().issue(&candidate()).expect("issue"));
    assert_eq!(codec().verify(&token).expect("verify"), candidate());
    let other = CandidateCodec::with_instant_source(&[42u8; 32], || Ok(instant(100, 0))).unwrap();
    assert_invalid(other.verify(&token));
    assert_ne!(other.issue(&candidate()).unwrap(), token);
}

#[test]
fn payload_bytes_match_golden_json_dumps() {
    let mut value = candidate();
    value.company = "a\"b\\c\u{8}\u{c}\n\r\t".to_owned();
    // C0 controls are refused by the text rules, so only the escapes of quote and backslash can
    // be issued; the byte contract for the others is covered by the golden vectors.
    assert_invalid(codec().issue(&value));
    value.company = "q\"b\\c \u{2028} \u{1F600} \u{e9}".to_owned();
    let token = codec().issue(&value).unwrap();
    let body = URL_SAFE_NO_PAD
        .decode(token.split_once('.').unwrap().0)
        .unwrap();
    assert_eq!(
        String::from_utf8(body).unwrap(),
        "{\"v\":1,\"provider\":\"arbeitnow\",\"issuedAt\":100,\"expiresAt\":1000,\"slug\":\"test-job\",\
         \"canonicalUrl\":\"https://arbeitnow.com/view/test-job\",\"company\":\"q\\\"b\\\\c \u{2028} \u{1F600} \u{e9}\",\
         \"title\":\"Title\",\"location\":\"\",\"remote\":false,\"postedAt\":null,\"tags\":[],\"jobTypes\":[],\
         \"descriptionExcerpt\":\"Description\"}"
    );
}

#[test]
fn expiry_uses_precise_now_without_flooring() {
    let token = codec().issue(&candidate()).unwrap();
    assert_invalid(codec_at(1000, 1).verify(&token));
    assert!(codec_at(1000, 0).verify(&token).is_ok());
    assert_invalid(codec_at(1001, 0).verify(&token));
}

#[test]
fn every_field_is_required() {
    for name in payload().as_object().unwrap().keys() {
        let mut document = payload();
        document.as_object_mut().unwrap().remove(name);
        assert_invalid(codec().verify(&signed(&document)));
    }
}

#[test]
fn exact_object_envelope() {
    for document in [
        json!([]),
        json!(null),
        json!("value"),
        json!(1),
        json!({"extra": 1}),
        with(&[("extra", json!(1))]),
    ] {
        assert_invalid(codec().verify(&signed(&document)));
    }
}

#[test]
fn duplicate_trailing_and_non_json_payloads() {
    let body = serde_json::to_vec(&payload()).unwrap();
    for ending in [&b",\"v\":1}"[..], b"} {}", b"}\x00", b",\"extra\":NaN}"] {
        let mut changed = body[..body.len() - 1].to_vec();
        changed.extend_from_slice(ending);
        assert_invalid(codec().verify(&signed_bytes(&changed)));
    }
}

#[test]
fn signed_numeric_fields_require_integral_signed_64bit_values() {
    let values = [
        json!(true),
        json!(false),
        json!(1.0),
        json!("100"),
        json!([]),
        json!({}),
        json!(9_223_372_036_854_775_808u64),
    ];
    for field in ["v", "issuedAt", "expiresAt", "postedAt"] {
        for value in &values {
            assert_invalid(codec().verify(&signed(&with(&[(field, value.clone())]))));
        }
        let mut raw = serde_json::to_string(&payload()).unwrap();
        raw = raw.replace(
            &format!("\"{field}\":{}", payload()[field]),
            &format!("\"{field}\":-9223372036854775809"),
        );
        assert_invalid(codec().verify(&signed_bytes(raw.as_bytes())));
    }
}

#[test]
fn required_instants_have_full_java_range() {
    for field in ["issuedAt", "expiresAt"] {
        for value in [
            json!(null),
            json!(MIN_EPOCH_SECOND - 1),
            json!(MAX_EPOCH_SECOND + 1),
        ] {
            assert_invalid(codec().verify(&signed(&with(&[(field, value)]))));
        }
    }
}

#[test]
fn nullable_posted_at_admits_negative_and_full_java_range() {
    for posted in [
        None,
        Some(-1),
        Some(MIN_EPOCH_SECOND),
        Some(MAX_EPOCH_SECOND),
    ] {
        let value = posted.map_or(Value::Null, |second| json!(second));
        let verified = codec()
            .verify(&signed(&with(&[("postedAt", value)])))
            .unwrap();
        assert_eq!(verified.posted_at, posted.map(|second| instant(second, 0)));
    }
}

#[test]
fn signed_fields_reject_wrong_types() {
    for field in [
        "provider",
        "slug",
        "canonicalUrl",
        "company",
        "title",
        "location",
        "descriptionExcerpt",
        "remote",
        "tags",
        "jobTypes",
    ] {
        for value in [json!(null), json!(1), json!({}), json!([])] {
            let list = matches!(field, "tags" | "jobTypes") && value == json!([]);
            let result = codec().verify(&signed(&with(&[(field, value)])));
            if list {
                assert!(result.is_ok(), "{field}");
            } else {
                assert_invalid(result);
            }
        }
    }
}

#[test]
fn slug_is_bounded_ascii_segments() {
    let long = "a".repeat(257);
    for slug in [
        "",
        "-a",
        "a-",
        "a--b",
        "a_b",
        "a/b",
        "\u{e4}",
        long.as_str(),
    ] {
        let url = format!("https://arbeitnow.com/view/{slug}");
        assert_invalid(codec().verify(&signed(&with(&[
            ("slug", json!(slug)),
            ("canonicalUrl", json!(url)),
        ]))));
    }
}

#[test]
fn slug_case_and_lists_are_preserved_without_normalization() {
    let document = with(&[
        ("slug", json!("ABC-09")),
        ("canonicalUrl", json!("https://arbeitnow.com/view/ABC-09")),
        ("tags", json!(["", " ", "Java", "Java"])),
        ("jobTypes", json!([" ", ""])),
    ]);
    let value = codec().verify(&signed(&document)).unwrap();
    assert_eq!(value.slug, "ABC-09");
    assert_eq!(value.tags, ["", " ", "Java", "Java"]);
    assert_eq!(value.job_types, [" ", ""]);
    let mut lower = document.clone();
    lower["canonicalUrl"] = json!("https://arbeitnow.com/view/abc-09");
    assert_invalid(codec().verify(&signed(&lower)));
}

#[test]
fn codepoint_text_boundaries_count_astral_characters_once() {
    for (field, maximum) in [
        ("company", 256),
        ("title", 256),
        ("location", 256),
        ("descriptionExcerpt", 1000),
    ] {
        for character in ["a", "\u{1F600}"] {
            let at = character.repeat(maximum);
            let over = character.repeat(maximum + 1);
            assert!(accepts(&signed(&with(&[(field, json!(at))]))), "{field}");
            assert_invalid(codec().verify(&signed(&with(&[(field, json!(over))]))));
        }
    }
}

#[test]
fn iso_controls_are_refused_and_line_separator_is_accepted() {
    for control in [
        '\0', '\t', '\n', '\r', '\u{1f}', '\u{7f}', '\u{85}', '\u{9f}',
    ] {
        for field in ["company", "title", "location", "descriptionExcerpt"] {
            let text = format!("before{control}after");
            assert_invalid(codec().verify(&signed(&with(&[(field, json!(text))]))));
        }
    }
    for allowed in ['\u{2028}', '\u{2029}', '\u{a0}', '\u{a1}'] {
        let text = format!("a{allowed}b");
        let verified = codec()
            .verify(&signed(&with(&[("company", json!(text))])))
            .unwrap();
        assert_eq!(verified.company, text);
    }
}

#[test]
fn list_count_element_type_and_codepoint_bounds() {
    for field in ["tags", "jobTypes"] {
        let full = vec!["\u{1F600}".repeat(128); 20];
        assert!(accepts(&signed(&with(&[(field, json!(full))]))));
        let long = "a".repeat(129);
        let many = vec!["a"; 21];
        for values in [
            json!([null]),
            json!([false]),
            json!([long]),
            json!(["\0"]),
            json!(many),
        ] {
            assert_invalid(codec().verify(&signed(&with(&[(field, values)]))));
        }
    }
}

#[test]
fn issuance_precise_lifetime_and_future_boundary() {
    let now = (100, 123);
    let mut value = candidate();
    value.issued_at = instant(160, 123);
    value.expires_at = instant(1060, 123);
    value.posted_at = Some(instant(-1, 999_999_999));
    let token = codec_at(now.0, now.1).issue(&value).unwrap();
    let verified = codec_at(now.0, now.1).verify(&token).unwrap();
    assert_eq!(verified.issued_at, instant(160, 0));
    assert_eq!(verified.expires_at, instant(1060, 0));
    assert_eq!(verified.posted_at, Some(instant(-1, 0)));
    let mut later = candidate();
    later.issued_at = instant(160, 124);
    later.expires_at = instant(1060, 124);
    assert_invalid(codec_at(now.0, now.1).issue(&later));
    let mut long = candidate();
    long.issued_at = instant(100, 123);
    long.expires_at = instant(1000, 124);
    assert_invalid(codec_at(now.0, now.1).issue(&long));
}

#[test]
fn zero_lifetime_and_expired_issuance_preserve_source_rules() {
    let mut zero = candidate();
    zero.expires_at = instant(100, 0);
    let token = codec().issue(&zero).unwrap();
    assert!(accepts(&token));
    assert_invalid(codec_at(100, 1).verify(&token));
    let mut old = candidate();
    old.issued_at = instant(-900, 0);
    old.expires_at = instant(0, 0);
    let old_token = codec().issue(&old).unwrap();
    assert_invalid(codec().verify(&old_token));
    let mut backwards = candidate();
    backwards.expires_at = instant(99, 0);
    assert_invalid(codec().issue(&backwards));
}

fn assert_internal(result: Result<String, ApplicationError>) {
    let error = result.expect_err("must fail");
    assert_eq!(error.code(), ErrorCode::InternalError);
    assert_eq!(error.message(), ErrorCode::InternalError.default_message());
}

#[test]
fn full_java_minimum_and_maximum_arithmetic_boundaries() {
    let mut minimum = candidate();
    minimum.issued_at = instant(MIN_EPOCH_SECOND, 0);
    minimum.expires_at = instant(MIN_EPOCH_SECOND + 900, 0);
    let at_min = codec_at(MIN_EPOCH_SECOND, 0);
    assert!(at_min.verify(&at_min.issue(&minimum).unwrap()).is_ok());

    let mut maximum = candidate();
    maximum.issued_at = instant(MAX_EPOCH_SECOND, 0);
    maximum.expires_at = instant(MAX_EPOCH_SECOND, 0);
    let boundary = codec_at(MAX_EPOCH_SECOND - 60, 0);
    let token = boundary.issue(&maximum).unwrap();
    assert!(boundary.verify(&token).is_ok());
    assert_internal(codec_at(MAX_EPOCH_SECOND - 59, 0).issue(&maximum));
    assert_invalid(codec_at(MAX_EPOCH_SECOND - 59, 0).verify(&token));

    let mut underflow = candidate();
    underflow.issued_at = instant(MIN_EPOCH_SECOND, 0);
    underflow.expires_at = instant(MIN_EPOCH_SECOND + 899, 0);
    assert_internal(at_min.issue(&underflow));
    let document = with(&[
        ("issuedAt", json!(MIN_EPOCH_SECOND)),
        ("expiresAt", json!(MIN_EPOCH_SECOND + 899)),
    ]);
    assert_invalid(at_min.verify(&signed(&document)));
}

#[test]
fn source_short_circuit_order_precedes_underflow_and_text_validation() {
    let at_min = codec_at(MIN_EPOCH_SECOND, 0);
    // Expiry before issue is a validation error even at the minimum instant.
    let mut value = candidate();
    value.issued_at = instant(MIN_EPOCH_SECOND + 1, 0);
    value.expires_at = instant(MIN_EPOCH_SECOND, 0);
    assert_invalid(at_min.issue(&value));
    // Underflow of expiry minus 900 is internal and precedes text validation.
    let mut bad_slug = candidate();
    bad_slug.issued_at = instant(MIN_EPOCH_SECOND, 0);
    bad_slug.expires_at = instant(MIN_EPOCH_SECOND, 0);
    bad_slug.slug = "bad_slug".to_owned();
    assert_internal(at_min.issue(&bad_slug));
}

#[test]
fn utf8_bom_is_authenticated_and_alternate_encodings_refused() {
    let text = serde_json::to_string(&payload()).unwrap();
    let mut with_bom = vec![0xEF, 0xBB, 0xBF];
    with_bom.extend_from_slice(text.as_bytes());
    assert!(accepts(&signed_bytes(&with_bom)));
    let mut twice = vec![0xEF, 0xBB, 0xBF];
    twice.extend_from_slice(&with_bom);
    assert_invalid(codec().verify(&signed_bytes(&twice)));
    let utf16_le: Vec<u8> = std::iter::once(0xFEFFu16)
        .chain(text.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    let utf16_be: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let utf32_be: Vec<u8> = text
        .chars()
        .flat_map(|c| u32::from(c).to_be_bytes())
        .collect();
    for body in [utf16_le, utf16_be, utf32_be] {
        assert_invalid(codec().verify(&signed_bytes(&body)));
    }
    let placeholder = text.replace("Company", "PLACEHOLDER");
    for malformed in [
        &[0xFFu8][..],
        &[0xC2],
        &[0xED, 0xA0, 0x80],
        &[0xC0, 0xAF],
        &[0xE0, 0x80, 0xAF],
        &[0xF0, 0x80, 0x80, 0xAF],
        &[0xF4, 0x90, 0x80, 0x80],
    ] {
        let mut body = Vec::new();
        let (head, tail) = placeholder.split_once("PLACEHOLDER").unwrap();
        body.extend_from_slice(head.as_bytes());
        body.extend_from_slice(malformed);
        body.extend_from_slice(tail.as_bytes());
        assert_invalid(codec().verify(&signed_bytes(&body)));
    }
}

#[test]
fn authentication_precedes_json_parsing() {
    // A payload that is valid JSON with a valid candidate shape, but signed with another key,
    // is refused; so is non-JSON signed with a wrong key. Neither can reach the parser.
    let valid = serde_json::to_vec(&payload()).unwrap();
    for body in [&valid[..], b"not even JSON"] {
        let mut mac = <SimpleHmac<Sha256> as KeyInit>::new_from_slice(&[1u8; 32]).unwrap();
        mac.update(body);
        let token = format!("{}.{}", encode(body), encode(&mac.finalize().into_bytes()));
        assert_invalid(codec().verify(&token));
    }
}

#[test]
fn byte_and_token_bounds() {
    let body = serde_json::to_vec(&payload()).unwrap();
    let pad = |size: usize| {
        let mut padded = body.clone();
        padded.resize(size, b' ');
        padded
    };
    assert!(accepts(&signed_bytes(&pad(MAX_PAYLOAD_BYTES))));
    assert_invalid(codec().verify(&signed_bytes(&pad(MAX_PAYLOAD_BYTES + 1))));
    assert_invalid(codec().verify(&"a".repeat(MAX_TOKEN_CHARS + 1)));
}

#[test]
fn token_grammar_and_wrong_mac_size() {
    for token in [
        "", ".", "..", "a.b.c", "a=.a", "a.a=", "a+.b", "a/b.b", "a.\u{e4}", "a.a",
    ] {
        assert_invalid(codec().verify(token));
    }
}

#[test]
fn noncanonical_unused_base64_bits_and_payload_tampering() {
    let token = signed(&payload());
    let (body, mac) = token.split_once('.').unwrap();
    let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let last = mac.chars().last().unwrap();
    let next = alphabet
        .chars()
        .nth(alphabet.find(last).unwrap() + 1)
        .unwrap();
    let changed = format!("{}{next}", &mac[..mac.len() - 1]);
    // Both spellings decode to the same MAC bytes under a lenient decoder.
    let lenient = base64::engine::GeneralPurpose::new(
        &base64::alphabet::URL_SAFE,
        base64::engine::GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );
    assert_eq!(
        lenient.decode(mac).unwrap(),
        lenient.decode(&changed).unwrap()
    );
    assert_invalid(codec().verify(&format!("{body}.{changed}")));
    assert_invalid(codec().verify(&format!("{}.{mac}", encode(b"{}"))));
    for size in [0usize, 1, 31, 33, 64, 65] {
        assert_invalid(codec().verify(&format!("{body}.{}", encode(&vec![0u8; size]))));
    }
    // Padded segments are refused.
    assert_invalid(codec().verify(&format!("{body}=.{mac}")));
    assert_invalid(codec().verify(&format!("{body}.{mac}=")));
}

#[test]
fn flipped_bits_in_payload_and_signature_are_refused() {
    let body = serde_json::to_vec(&payload()).unwrap();
    let mac = sign(&body);
    for index in [0usize, 10, body.len() - 1] {
        let mut flipped = body.clone();
        flipped[index] ^= 1;
        assert_invalid(codec().verify(&format!("{}.{}", encode(&flipped), encode(&mac))));
    }
    for index in [0usize, 15, 31] {
        let mut flipped = mac.clone();
        flipped[index] ^= 0x80;
        assert_invalid(codec().verify(&format!("{}.{}", encode(&body), encode(&flipped))));
    }
}

#[test]
fn configuration_is_explicit_and_safe() {
    for key in [&[][..], &[0u8; 31]] {
        let error = CandidateCodec::with_instant_source(key, || Ok(instant(100, 0))).unwrap_err();
        assert_eq!(
            error.message(),
            "Invalid Arbeitnow candidate codec configuration"
        );
        assert!(error.details().is_empty());
    }
    assert!(CandidateCodec::with_instant_source(&[0u8; 32], || Ok(instant(100, 0))).is_ok());
    assert!(CandidateCodec::with_instant_source(&[0u8; 64], || Ok(instant(100, 0))).is_ok());
}

#[test]
fn private_clock_failures_are_never_exposed() {
    let failing = || {
        Err(ApplicationError::with_message(
            ErrorCode::NotFound,
            "PRIVATE-CLOCK",
        ))
    };
    let authority = CandidateCodec::with_instant_source(&key(), failing).unwrap();
    assert_internal(authority.issue(&candidate()));
    assert_invalid(authority.verify(&signed(&payload())));
    let error = authority.now().unwrap_err();
    assert_eq!(error.code(), ErrorCode::InternalError);
    assert_eq!(error.message(), ErrorCode::InternalError.default_message());
    assert!(!format!("{authority:?}").contains("PRIVATE"));
}

#[test]
fn public_now_returns_the_exact_precise_instant() {
    let exact = instant(MIN_EPOCH_SECOND, 999_999_999);
    let authority = CandidateCodec::with_instant_source(&key(), move || Ok(exact)).unwrap();
    assert_eq!(authority.now().unwrap(), exact);
}

#[test]
fn issuance_runs_common_validation() {
    type Change = Box<dyn Fn(&mut ArbeitnowCandidate)>;
    let cases: Vec<Change> = vec![
        Box::new(|c| c.title = "\0".to_owned()),
        Box::new(|c| c.location = "x".repeat(257)),
        Box::new(|c| c.description_excerpt = "x".repeat(1001)),
        Box::new(|c| c.slug = "bad_slug".to_owned()),
        Box::new(|c| c.canonical_url = "https://arbeitnow.com/view/TEST-JOB".to_owned()),
        Box::new(|c| c.tags = vec!["x".repeat(129)]),
        Box::new(|c| c.tags = vec!["a".to_owned(); 21]),
        Box::new(|c| c.job_types = vec!["\u{9f}".to_owned()]),
        Box::new(|c| c.job_types = vec!["a".to_owned(); 21]),
        Box::new(|c| c.company = "x".repeat(1_000_000)),
    ];
    for change in cases {
        let mut value = candidate();
        change(&mut value);
        assert_invalid(codec().issue(&value));
    }
}

#[test]
fn largest_admissible_candidate_stays_inside_the_size_limits() {
    // The field caps keep the payload under 32 KiB and the token under 44,000 characters, so the
    // issuer's size checks are a backstop that valid candidates cannot reach.
    let mut value = candidate();
    value.company = "\u{1F600}".repeat(256);
    value.title = "\"".repeat(256);
    value.location = "\u{1F600}".repeat(256);
    value.tags = vec!["\u{1F600}".repeat(128); 20];
    value.job_types = vec!["\"".repeat(128); 20];
    value.description_excerpt = "\u{1F600}".repeat(1000);
    let token = codec().issue(&value).unwrap();
    assert!(token.len() <= MAX_TOKEN_CHARS);
    assert_eq!(codec().verify(&token).unwrap(), value);
}

#[test]
fn authenticated_semantically_wrong_version_provider_and_times() {
    for changes in [
        vec![("v", json!(2))],
        vec![("provider", json!("other"))],
        vec![("issuedAt", json!(161)), ("expiresAt", json!(1061))],
        vec![("expiresAt", json!(99))],
        vec![("expiresAt", json!(1001))],
    ] {
        assert_invalid(codec().verify(&signed(&with(&changes))));
    }
}

#[test]
fn authenticated_sixty_second_future_boundary_is_inclusive() {
    let token = signed(&with(&[
        ("issuedAt", json!(160)),
        ("expiresAt", json!(1060)),
    ]));
    assert!(accepts(&token));
    assert!(codec_at(100, 1).verify(&token).is_ok());
    assert_invalid(codec_at(99, 999_999_999).verify(&token));
}

#[test]
fn lone_surrogate_escape_is_refused() {
    // A lone surrogate escape (`\ud800`) is refused on verify and on issue.
    let raw = serde_json::to_string(&payload())
        .unwrap()
        .replace("\"company\":\"Company\"", "\"company\":\"\\ud800\"");
    assert_invalid(codec().verify(&signed_bytes(raw.as_bytes())));
    let pair = serde_json::to_string(&payload())
        .unwrap()
        .replace("\"company\":\"Company\"", "\"company\":\"\\ud83d\\ude00\"");
    assert_eq!(
        codec()
            .verify(&signed_bytes(pair.as_bytes()))
            .unwrap()
            .company,
        "\u{1F600}"
    );
}

#[test]
fn clock_adapter_reads_the_system_style_clock() {
    use jobniac_core::clock::FixedClock;
    use std::sync::Arc;
    use time::macros::datetime;
    let clock = Arc::new(FixedClock::new(datetime!(1970-01-01 00:01:40.5 UTC)));
    let authority = CandidateCodec::new(&key(), clock).unwrap();
    assert_eq!(authority.now().unwrap(), instant(100, 500_000_000));
    assert!(
        CandidateCodec::new(
            &[0u8; 8],
            Arc::new(FixedClock::new(datetime!(1970-01-01 0:00 UTC)))
        )
        .is_err()
    );
}

// ---- Byte-level check against the golden vectors ----

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden json")
}

fn instant_of(value: &Value) -> Option<ArbeitnowInstant> {
    value.as_i64().map(|second| instant(second, 0))
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry.as_str().unwrap().to_owned())
        .collect()
}

fn candidate_of(spec: &Value) -> ArbeitnowCandidate {
    ArbeitnowCandidate {
        issued_at: instant_of(&spec["issuedAt"]).unwrap(),
        expires_at: instant_of(&spec["expiresAt"]).unwrap(),
        slug: spec["slug"].as_str().unwrap().to_owned(),
        canonical_url: spec["canonicalUrl"].as_str().unwrap().to_owned(),
        company: spec["company"].as_str().unwrap().to_owned(),
        title: spec["title"].as_str().unwrap().to_owned(),
        location: spec["location"].as_str().unwrap().to_owned(),
        remote: spec["remote"].as_bool().unwrap(),
        posted_at: instant_of(&spec["postedAt"]),
        tags: strings(&spec["tags"]),
        job_types: strings(&spec["jobTypes"]),
        description_excerpt: spec["descriptionExcerpt"].as_str().unwrap().to_owned(),
    }
}

#[test]
fn golden_key_is_the_documented_test_key() {
    assert_eq!(
        golden()["metadata"]["test_only_key_hex"].as_str().unwrap(),
        hex(&key())
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn rust_issues_the_golden_token_byte_for_byte() {
    let golden = golden();
    let cases = golden["issued"].as_array().unwrap();
    assert!(cases.len() >= 10);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let value = candidate_of(&case["candidate"]);
        let token = case["token"].as_str().unwrap();
        assert_eq!(codec().issue(&value).unwrap(), token, "{name}");
        let verified = codec_at(100, 0)
            .verify(token)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut expected = value.clone();
        // The token carries whole seconds only.
        expected.issued_at = instant(expected.issued_at.epoch_second(), 0);
        expected.expires_at = instant(expected.expires_at.epoch_second(), 0);
        assert_eq!(verified, expected, "{name}");
    }
}

#[test]
fn rust_refuses_what_golden_refuses_to_issue() {
    let golden = golden();
    let cases = golden["refused_issue"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let value = candidate_of(&case["candidate"]);
        let error = codec()
            .issue(&value)
            .expect_err(case["name"].as_str().unwrap());
        assert_eq!(error.code().code(), case["code"].as_str().unwrap());
        assert_eq!(error.message(), MESSAGE);
    }
}

#[test]
fn rust_verifies_tamper_cases_like_the_golden_vectors() {
    let golden = golden();
    let cases = golden["tamper"].as_array().unwrap();
    assert!(cases.len() >= 40);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let result = codec().verify(case["token"].as_str().unwrap());
        if case["valid"].as_bool().unwrap() {
            assert!(result.is_ok(), "{name}");
        } else {
            assert_invalid(result);
        }
    }
}

#[test]
fn rust_matches_golden_on_clock_boundaries() {
    let golden = golden();
    let cases = golden["boundary"].as_array().unwrap();
    assert!(cases.len() >= 9);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let now = &case["now"];
        let second = now["second"].as_i64().unwrap();
        let nano = u32::try_from(now["nano"].as_u64().unwrap()).unwrap();
        let result = codec_at(second, nano).verify(case["token"].as_str().unwrap());
        if case["valid"].as_bool().unwrap() {
            assert!(result.is_ok(), "{name}");
        } else {
            assert_invalid(result);
        }
    }
}
