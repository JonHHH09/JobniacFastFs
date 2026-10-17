//! Job URL identity (214 pinned JDK cases): `safe_display_url`, `canonical_source_url` and
//! `validate_persisted_job_urls` against `golden/job-urls.json`. Errors are compared by code, fixed
//! message, field and reason; the rejected URL is never echoed and must not appear in any envelope.

use jobniac_core::errors::ApplicationError;
use jobniac_core::job_links::urls::{
    canonical_source_url, safe_display_url, validate_persisted_job_urls,
};
use serde_json::Value;

const GOLDEN: &str = include_str!("golden/job-urls.json");

fn golden() -> Value {
    match serde_json::from_str(GOLDEN) {
        Ok(value) => value,
        Err(error) => panic!("job-urls.json does not parse: {error}"),
    }
}

fn rows<'a>(document: &'a Value, key: &str) -> &'a [Value] {
    match document[key].as_array() {
        Some(items) => items,
        None => panic!("missing array {key}"),
    }
}

/// Checks a Rust refusal against the recorded envelope (code, fixed message, field and reason).
fn assert_envelope(error: &ApplicationError, expected: &Value, context: &str) {
    assert_eq!(
        error.code().code(),
        expected["code"].as_str().unwrap_or_default(),
        "{context}"
    );
    assert_eq!(
        error.message(),
        expected["message"].as_str().unwrap_or_default(),
        "{context}"
    );
    let details = error.details();
    assert_eq!(
        details.get("field").map(String::as_str),
        expected["field"].as_str(),
        "{context}"
    );
    assert_eq!(
        details.get("reason").map(String::as_str),
        expected["reason"].as_str(),
        "{context}"
    );
}

#[test]
fn java_pinned_cases_match() {
    let document = golden();
    assert_eq!(
        document["java"]["sourceRevision"],
        "c601f885f89ceb649fdbea5a5874ae27d8f14f94"
    );
    let cases = rows(&document["java"], "cases");
    assert_eq!(cases.len(), 214);
    assert_eq!(
        cases
            .iter()
            .filter(|case| case["invalid"] == Value::Bool(true))
            .count(),
        76
    );
    for case in cases {
        let url = case["url"].as_str().unwrap_or_default();
        if case["invalid"] == Value::Bool(true) {
            for error in [canonical_source_url(url).err(), safe_display_url(url).err()] {
                let Some(error) = error else {
                    panic!("{url:?} admitted but Java refuses it");
                };
                assert_eq!(error.code().code(), "validation_error", "{url:?}");
                assert_eq!(
                    error.details().get("field").map(String::as_str),
                    Some("url")
                );
                assert!(!format!("{:?}", error.to_envelope()).contains("private_token"));
            }
        } else {
            assert_eq!(
                canonical_source_url(url).ok().as_deref(),
                case["canonicalUrl"].as_str(),
                "{url:?}"
            );
            assert_eq!(
                safe_display_url(url).ok().as_deref(),
                case["displayUrl"].as_str(),
                "{url:?}"
            );
        }
    }
}

#[test]
fn java_invalid_authorities_are_refused_by_provenance_too() {
    for url in [
        "https://127.0.0.1./jobs",
        "https://127.000.000.001./jobs",
        "https://[fe80::1%a-b]/jobs",
        "https://[fe80::1%25a-b]/jobs",
    ] {
        assert!(canonical_source_url(url).is_err(), "{url}");
        assert!(safe_display_url(url).is_err(), "{url}");
        assert!(validate_persisted_job_urls(url, url).is_err(), "{url}");
    }
}

#[test]
fn java_padded_embedded_ipv4_and_scope_spelling_is_preserved() {
    for url in [
        "https://[::ffff:192.168.000.001]/jobs",
        "https://[::ffff:000192.000168.000000000.000000001]/jobs",
        "https://[::ffff:000000000000000000000192.168.0.1]/jobs",
        "https://[::ffff:192.168.0.000000000000000000000000000000000001]/jobs",
        "https://[fe80::1%a_b]/jobs",
        "https://[fe80::1%a.b]/jobs",
        "https://[fe80::1%a.b_c]/jobs",
    ] {
        assert_eq!(
            canonical_source_url(url).ok().as_deref(),
            Some(url),
            "{url}"
        );
        assert_eq!(safe_display_url(url).ok().as_deref(), Some(url), "{url}");
        assert!(validate_persisted_job_urls(url, url).is_ok(), "{url}");
    }
}

#[test]
fn identity_outcomes_match_the_golden_vectors() {
    let document = golden();
    let mut checked = 0usize;
    for row in rows(&document, "identity") {
        let url = row["url"].as_str().unwrap_or_default();
        for (key, result) in [
            ("display", safe_display_url(url)),
            ("canonical", canonical_source_url(url)),
        ] {
            let expected = &row[key];
            match (expected.get("value"), expected.get("error"), result) {
                (Some(value), _, Ok(actual)) => {
                    assert_eq!(Some(actual.as_str()), value.as_str(), "{key} of {url:?}");
                }
                (_, Some(error), Err(actual)) => {
                    assert_envelope(&actual, error, &format!("{key} of {url:?}"))
                }
                (value, error, result) => panic!(
                    "{key} of {url:?}: recorded {value:?} / {error:?}, rust {:?}",
                    result.map_err(|error| error.message().to_owned())
                ),
            }
            checked += 1;
        }
    }
    assert!(checked > 2000, "only {checked} identity outcomes checked");
}

#[test]
fn persisted_provenance_matches_golden() {
    let document = golden();
    let rows = rows(&document, "persisted");
    assert!(rows.len() > 400, "only {} provenance rows", rows.len());
    for row in rows {
        let display = row["display"].as_str().unwrap_or_default();
        let canonical = row["canonical"].as_str().unwrap_or_default();
        let result = validate_persisted_job_urls(display, canonical);
        if row["ok"] == Value::Bool(true) {
            assert!(result.is_ok(), "{display:?} / {canonical:?} refused");
        } else {
            match result {
                Ok(()) => panic!("{display:?} / {canonical:?} admitted"),
                Err(error) => {
                    let expected = &row["error"];
                    assert_eq!(
                        error.code().code(),
                        expected["code"].as_str().unwrap_or_default()
                    );
                    assert_eq!(
                        error.message(),
                        expected["message"].as_str().unwrap_or_default()
                    );
                    assert_eq!(
                        error.details().get("field").map(String::as_str),
                        expected["details"]["field"].as_str()
                    );
                    assert_eq!(
                        error.details().get("reason").map(String::as_str),
                        expected["details"]["reason"].as_str()
                    );
                    assert!(
                        !format!("{:?}", error.to_envelope()).contains("private-token-sentinel")
                    );
                }
            }
        }
    }
}
