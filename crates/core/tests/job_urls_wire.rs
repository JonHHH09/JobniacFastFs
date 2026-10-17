//! The URL check before any fetch: the pure cases of hostname and link fetch admission, and the
//! comparison with `golden/job-urls.json`, section `wire`. No network and no child process are
//! involved.

use jobniac_core::job_links::JobLinkFetchFailure;
use jobniac_core::job_links::urls::wire_url;
use serde_json::Value;

const GOLDEN: &str = include_str!("golden/job-urls.json");

fn golden() -> Value {
    match serde_json::from_str(GOLDEN) {
        Ok(value) => value,
        Err(error) => panic!("job-urls.json does not parse: {error}"),
    }
}

#[test]
fn wire_outcomes_match_the_golden_vectors() {
    // The wire outcome of each URL is stored in its identity row (`wire`: `{"value"}` or
    // `{"failure"}`), so every URL is written once in the fixture.
    let document = golden();
    let rows = match document["identity"].as_array() {
        Some(rows) => rows,
        None => panic!("missing identity rows"),
    };
    assert!(rows.len() > 1000, "only {} wire rows", rows.len());
    let mut admitted = 0usize;
    for row in rows {
        let url = row["url"].as_str().unwrap_or_default();
        match (
            row["wire"]["value"].as_str(),
            row["wire"]["failure"].as_str(),
        ) {
            (Some(wire), _) => {
                assert_eq!(wire_url(url).ok().as_deref(), Some(wire), "{url:?}");
                admitted += 1;
            }
            (_, Some(name)) => match wire_url(url) {
                Ok(wire) => panic!("{url:?} admitted as {wire}, the vector refuses with {name}"),
                Err(failure) => assert_eq!(failure.name(), name, "{url:?}"),
            },
            _ => panic!("row without outcome: {row}"),
        }
    }
    assert!(admitted > 100, "only {admitted} admitted URLs checked");
}

#[test]
fn odd_schemes_are_refused_as_invalid_url() {
    // A scheme is one run of `[A-Za-z][A-Za-z0-9+.-]*` before `://`: a colon inside it refuses the
    // URL, and the text after the colon is never kept (the scheme must match in full).
    for url in [
        "https:x://8.8.8.8/",
        "http:x://8.8.8.8/",
        "ht:tp://8.8.8.8/",
        "https:x://93.184.216.34/",
        "a:b://8.8.8.8/",
        "https::://8.8.8.8/",
        "http:/x://8.8.8.8/",
        "https:://8.8.8.8/",
        "1http://8.8.8.8/",
        "h.t+t-p://8.8.8.8/",
        "http.x://8.8.8.8/",
        "https:x//8.8.8.8/",
    ] {
        assert_eq!(
            wire_url(url).err(),
            Some(JobLinkFetchFailure::InvalidUrl),
            "{url}"
        );
    }
    // A well-formed scheme that is not http or https is refused as well; http is admitted in capitals.
    assert_eq!(
        wire_url("h2.x-y+z://8.8.8.8/").err(),
        Some(JobLinkFetchFailure::InvalidUrl)
    );
    assert_eq!(
        wire_url("HTTPS://8.8.8.8/x").ok().as_deref(),
        Some("https://8.8.8.8/x")
    );
}

#[test]
fn ip_literal_http_is_admitted_and_names_need_https() {
    assert_eq!(
        wire_url("http://8.8.8.8/x?y=1").ok().as_deref(),
        Some("http://8.8.8.8/x?y=1")
    );
    assert_eq!(
        wire_url("https://8.8.8.8:8443/x").ok().as_deref(),
        Some("https://8.8.8.8:8443/x")
    );
    assert_eq!(
        wire_url("http://example.com/x").err(),
        Some(JobLinkFetchFailure::IpLiteralRequired)
    );
}

#[test]
fn https_names_are_admitted_in_normal_form() {
    assert_eq!(
        wire_url("https://Jobs.Example.COM./a/b?c=d#frag")
            .ok()
            .as_deref(),
        Some("https://jobs.example.com/a/b?c=d")
    );
    assert_eq!(
        wire_url("https://bücher.example.com/x").ok().as_deref(),
        Some("https://xn--bcher-kva.example.com/x")
    );
    assert_eq!(
        wire_url("https://example.com:0443/x").ok().as_deref(),
        Some("https://example.com:443/x")
    );
}

#[test]
fn unsafe_names_keep_the_generic_failure() {
    for url in [
        "https://127.0.0.1/x",
        "https://10.0.0.1/x",
        "https://[::1]/x",
        "https://[::ffff:127.0.0.1]/x",
        "https://169.254.169.254/latest",
    ] {
        assert_eq!(
            wire_url(url).err(),
            Some(JobLinkFetchFailure::UnsafeAddress),
            "{url}"
        );
    }
    // A name the normalizer refuses keeps the classification set before the name check.
    for url in [
        "https://localhost/x",
        "https://printer.local/x",
        "https://2130706433/x",
        "https://0x7f.0.0.1/x",
    ] {
        assert_eq!(
            wire_url(url).err(),
            Some(JobLinkFetchFailure::IpLiteralRequired),
            "{url}"
        );
    }
    assert_eq!(
        wire_url("https://999.1.1.1/x").err(),
        Some(JobLinkFetchFailure::InvalidUrl)
    );
}

#[test]
fn padded_dotted_literals_are_canonicalized_before_admission() {
    assert_eq!(
        wire_url("https://008.008.008.008/x").ok().as_deref(),
        Some("https://8.8.8.8/x")
    );
    assert_eq!(
        wire_url("https://[::ffff:008.008.008.008]/x")
            .ok()
            .as_deref(),
        Some("https://[::ffff:8.8.8.8]/x")
    );
    // Four-digit octets are not canonicalized (the pattern takes one to three digits).
    assert_eq!(
        wire_url("https://[::ffff:0008.0008.0008.0008]/x").err(),
        Some(JobLinkFetchFailure::InvalidUrl)
    );
}

#[test]
fn userinfo_percent_and_overlong_urls_are_refused_as_invalid() {
    for url in [
        "https://user:secret@8.8.8.8/x",
        "https://8.8.8.8/%zz",
        "https://8.8.8.8/a\\b",
        "https://8.8.8.8/a b",
        "https://8.8.8.8:0/x",
        "https://8.8.8.8:65536/x",
        "https://8.8.8.8:/x",
        "ftp://8.8.8.8/x",
        "/relative",
    ] {
        assert!(wire_url(url).is_err(), "{url}");
    }
    let long = format!("https://8.8.8.8/{}", "a".repeat(8200));
    assert_eq!(wire_url(&long).err(), Some(JobLinkFetchFailure::InvalidUrl));
}

#[test]
fn failures_never_echo_the_url() {
    let secret = "https://user:private-token-sentinel@10.0.0.1/x";
    let failure = wire_url(secret).err();
    assert!(failure.is_some());
    let text = format!(
        "{failure:?} {}",
        failure.map_or("", |value| value.message())
    );
    assert!(!text.contains("private-token-sentinel"));
}

#[test]
fn a_bracketed_dns_name_is_refused_as_an_ip_literal_requirement() {
    for url in ["https://[example.com]/x", "https://[example.com]:8443/"] {
        assert_eq!(
            wire_url(url).err(),
            Some(JobLinkFetchFailure::IpLiteralRequired),
            "{url}"
        );
    }
}
