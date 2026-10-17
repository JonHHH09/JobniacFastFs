//! Outbound address policy: the address cases and the comparison of addresses and resolver record
//! admission (`golden/outbound-policy.json`). Host name rows are compared in `outbound_idna.rs`.

use std::net::IpAddr;
use std::sync::Mutex;

use async_trait::async_trait;
use jobniac_core::outbound::address::{compressed_ipv6, parse_ip};
use jobniac_core::outbound::{
    AddressLookup, Endpoint, Family, LookupFailed, MAX_RECORDS, PinnedResolver, PolicyFailure,
    RecordProtocol, ResolverRecord, SocketKind, admit_records, is_public_address,
    normalize_hostname,
};
use serde_json::Value;

const GOLDEN: &str = include_str!("golden/outbound-policy.json");

fn golden() -> Value {
    match serde_json::from_str(GOLDEN) {
        Ok(value) => value,
        Err(error) => panic!("outbound-policy.json does not parse: {error}"),
    }
}

fn rows<'a>(document: &'a Value, key: &str) -> &'a [Value] {
    match document[key].as_array() {
        Some(items) => items,
        None => panic!("missing array {key}"),
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    match value[key].as_str() {
        Some(found) => found,
        None => panic!("missing string {key} in {value}"),
    }
}

fn record(value: &Value) -> ResolverRecord {
    let family = match text(value, "family") {
        "inet" => Family::Inet,
        "inet6" => Family::Inet6,
        _ => Family::Other,
    };
    let kind = if text(value, "kind") == "stream" {
        SocketKind::Stream
    } else {
        SocketKind::Other
    };
    let protocol = if text(value, "protocol") == "tcp" {
        RecordProtocol::Tcp
    } else {
        RecordProtocol::Other
    };
    let ipv6_extra = value["extra"]
        .as_array()
        .map(|pair| (pair[0].as_i64().unwrap_or(0), pair[1].as_i64().unwrap_or(0)));
    ResolverRecord {
        family,
        kind,
        protocol,
        endpoint: Endpoint {
            host: text(value, "host").to_owned(),
            port: value["port"].as_i64().unwrap_or(-1),
            ipv6_extra,
        },
    }
}

fn record_of(host: &str, port: i64) -> ResolverRecord {
    let family = if host.contains(':') {
        Family::Inet6
    } else {
        Family::Inet
    };
    let ipv6_extra = if family == Family::Inet6 {
        Some((0, 0))
    } else {
        None
    };
    ResolverRecord {
        family,
        kind: SocketKind::Stream,
        protocol: RecordProtocol::Tcp,
        endpoint: Endpoint {
            host: host.to_owned(),
            port,
            ipv6_extra,
        },
    }
}

fn admitted_texts(records: &[ResolverRecord], port: i64) -> Result<Vec<String>, PolicyFailure> {
    admit_records(records, port).map_err(|error| error.failure())
}

const PUBLIC: [&str; 8] = [
    "8.8.8.8",
    "1.1.1.1",
    "93.184.216.34",
    "2606:4700:4700::1111",
    "2001:4860:4860::8888",
    "::ffff:8.8.8.8",
    "64:ff9b::808:808",
    "2002:808:808::1",
];

const BLOCKED: &[&str] = &[
    "0.0.0.0",
    "0.1.2.3",
    "10.0.0.1",
    "100.64.0.1",
    "100.127.255.254",
    "127.0.0.1",
    "127.255.255.255",
    "169.254.169.254",
    "172.16.0.1",
    "172.31.255.255",
    "192.0.0.8",
    "192.0.2.1",
    "192.88.99.1",
    "192.168.1.1",
    "198.18.0.1",
    "198.51.100.1",
    "203.0.113.1",
    "224.0.0.1",
    "239.255.255.255",
    "240.0.0.1",
    "255.255.255.255",
    "::",
    "::1",
    "::ffff:127.0.0.1",
    "::ffff:10.0.0.1",
    "::ffff:169.254.169.254",
    "::1.2.3.4",
    "64:ff9b::7f00:1",
    "64:ff9b::a00:1",
    "64:ff9b:1::1",
    "2002:7f00:1::1",
    "2002:a00:1::1",
    "2002:c0a8:101::1",
    "2001:0:4136:e378:8000:63bf:3fff:fdd2",
    "2001:db8::1",
    "2001:10::1",
    "100::1",
    "fc00::1",
    "fd12:3456::1",
    "fe80::1",
    "fec0::1",
    "ff02::1",
    "2606:4700::5efe:a00:1",
    "3fff::1",
    "5f00::1",
    "192.0.0.9",
    "192.0.0.10",
    "192.0.0.170",
    "255.255.255.254",
    "2001:20::1",
    "2001:2::1",
];

fn parse(text: &str) -> IpAddr {
    match parse_ip(text) {
        Some(address) => address,
        None => panic!("{text} is not an IP literal"),
    }
}

#[test]
fn public_addresses_are_admitted() {
    for text in PUBLIC {
        assert!(is_public_address(parse(text)), "{text}");
    }
}

#[test]
fn every_blocked_range_is_refused_without_echo() {
    for text in BLOCKED {
        assert!(!is_public_address(parse(text)), "{text}");
        let refusal = admit_records(&[record_of(text, 443)], 443);
        match refusal {
            Err(error) => {
                assert_eq!(error.reason(), "address", "{text}");
                assert!(!error.to_string().contains(text), "{text} echoed");
            }
            Ok(admitted) => panic!("{text} admitted as {admitted:?}"),
        }
    }
}

#[test]
fn one_non_public_record_rejects_a_mixed_answer() {
    let mixed = [record_of("8.8.8.8", 443), record_of("10.0.0.5", 443)];
    assert_eq!(admitted_texts(&mixed, 443), Err(PolicyFailure::Address));
    let v6 = [
        record_of("2606:4700:4700::1111", 443),
        record_of("::1", 443),
    ];
    assert_eq!(admitted_texts(&v6, 443), Err(PolicyFailure::Address));
}

#[test]
fn malformed_resolver_records_are_refused() {
    let base = record_of("8.8.8.8", 443);
    let cases: Vec<Vec<ResolverRecord>> = vec![
        Vec::new(),
        vec![record_of("8.8.8.8", 80)],
        vec![base.clone(); MAX_RECORDS + 1],
        vec![ResolverRecord {
            kind: SocketKind::Other,
            ..base.clone()
        }],
        vec![ResolverRecord {
            protocol: RecordProtocol::Other,
            ..base.clone()
        }],
        vec![record_of("8.8.8.8%eth0", 443)],
        vec![ResolverRecord {
            family: Family::Inet6,
            endpoint: Endpoint {
                host: "::1".into(),
                port: 443,
                ipv6_extra: Some((0, 1)),
            },
            ..base.clone()
        }],
        vec![ResolverRecord {
            family: Family::Inet6,
            endpoint: Endpoint {
                host: "::1".into(),
                port: 443,
                ipv6_extra: Some((1, 0)),
            },
            ..base.clone()
        }],
        vec![ResolverRecord {
            family: Family::Inet,
            endpoint: Endpoint {
                host: "8.8.8.8".into(),
                port: 443,
                ipv6_extra: Some((0, 0)),
            },
            ..base.clone()
        }],
        vec![record_of("example.com", 443)],
        vec![ResolverRecord {
            family: Family::Other,
            ..base.clone()
        }],
    ];
    for records in cases {
        assert_eq!(admitted_texts(&records, 443), Err(PolicyFailure::Address));
    }
}

#[test]
fn admission_deduplicates_and_keeps_order() {
    let records = [
        record_of("8.8.4.4", 443),
        record_of("8.8.8.8", 443),
        record_of("8.8.4.4", 443),
    ];
    assert_eq!(
        admitted_texts(&records, 443),
        Ok(vec!["8.8.4.4".into(), "8.8.8.8".into()])
    );
}

#[test]
fn golden_addresses_match_the_golden_vectors() {
    let document = golden();
    let mut checked = 0usize;
    for row in rows(&document, "addresses") {
        let spelling = text(row, "text");
        let parsed = parse_ip(spelling);
        assert_eq!(
            parsed.is_some(),
            row["parsed"] == Value::Bool(true),
            "{spelling:?}"
        );
        let Some(address) = parsed else {
            continue;
        };
        let version = if address.is_ipv4() { 4 } else { 6 };
        assert_eq!(row["version"].as_i64(), Some(version), "{spelling}");
        let compressed = match address {
            IpAddr::V4(address) => address.to_string(),
            IpAddr::V6(address) => compressed_ipv6(u128::from(address)),
        };
        assert_eq!(text(row, "compressed"), compressed, "{spelling}");
        assert_eq!(
            row["public"] == Value::Bool(true),
            is_public_address(address),
            "{spelling}"
        );
        checked += 1;
    }
    // The seeded random rows were cut to about 300 (the boundary and targeted rows remain), so the
    // minimum is the count of parsed rows that the trimmed fixture holds.
    assert!(checked > 1500, "only {checked} addresses checked");
}

#[test]
fn golden_records_match_the_golden_vectors() {
    let document = golden();
    for case in rows(&document, "records") {
        let name = text(case, "name");
        let records: Vec<ResolverRecord> = rows(case, "records").iter().map(record).collect();
        let port = case["port"].as_i64().unwrap_or(-1);
        let actual = admit_records(&records, port);
        match (&case["result"]["ok"], &case["result"]["refused"]) {
            (Value::Array(expected), _) => {
                let expected: Vec<String> = expected
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect();
                assert_eq!(actual.ok(), Some(expected), "{name}");
            }
            (_, Value::String(reason)) => {
                assert_eq!(
                    actual.err().map(|error| error.reason()),
                    Some(reason.as_str()),
                    "{name}"
                );
            }
            _ => panic!("{name}: no expected result"),
        }
    }
}

#[test]
fn golden_constants_match_the_golden_vectors() {
    let document = golden();
    assert_eq!(
        document["constants"]["maxRecords"].as_u64(),
        Some(MAX_RECORDS as u64)
    );
    assert_eq!(
        document["constants"]["maxHostnameLength"].as_u64(),
        Some(253)
    );
    assert_eq!(document["constants"]["maxLabelLength"].as_u64(), Some(63));
    assert_eq!(
        document["constants"]["userAgent"].as_str(),
        Some(jobniac_core::outbound::USER_AGENT)
    );
}

#[test]
fn hostname_normalization_matches_the_golden_cases() {
    let cases = [
        ("example.com", "example.com"),
        ("EXAMPLE.Com", "example.com"),
        ("jobs.example.com.", "jobs.example.com"),
        ("münchen.de", "xn--mnchen-3ya.de"),
        ("bücher.example.de", "xn--bcher-kva.example.de"),
        ("xn--mnchen-3ya.de", "xn--mnchen-3ya.de"),
        ("a-b.c1.io", "a-b.c1.io"),
    ];
    for (host, expected) in cases {
        assert_eq!(
            normalize_hostname(host).ok().as_deref(),
            Some(expected),
            "{host}"
        );
    }
}

#[test]
fn unsuitable_hostnames_are_refused() {
    let refused = [
        "",
        ".",
        "example.com..",
        ".example.com",
        "localhost",
        "localhost.",
        "a.localhost",
        "printer.local",
        "db.internal",
        "host.invalid",
        "x.test",
        "single",
        "2130706433",
        "0x7f000001",
        "0177.0.0.1",
        "127.0.0.1",
        "8.8.8.8",
        "1.2.3",
        "0x7f.0.0.1",
        "1.1.1.1.",
        "a_b.example.com",
        "-a.example.com",
        "a-.example.com",
        "exa mple.com",
        "xn--.com",
        "user@example.com",
    ];
    for host in refused {
        match normalize_hostname(host) {
            Err(error) => assert_eq!(error.reason(), "hostname", "{host}"),
            Ok(name) => panic!("{host} admitted as {name}"),
        }
    }
    let long_label = format!("{}.com", "x".repeat(64));
    let long_name = format!("{}.com", vec!["a".repeat(63); 4].join("."));
    assert!(normalize_hostname(&long_label).is_err());
    assert!(normalize_hostname(&long_name).is_err());
    let over_total = format!("{}.com", vec!["a".repeat(60); 5].join("."));
    assert!(normalize_hostname(&over_total).is_err());
}

/// A lookup adapter that answers from a script and counts calls.
struct Scripted {
    answers: Mutex<Vec<Result<Vec<ResolverRecord>, LookupFailed>>>,
    calls: Mutex<Vec<(String, i64)>>,
}

#[async_trait]
impl AddressLookup for Scripted {
    async fn lookup(&self, host: &str, port: i64) -> Result<Vec<ResolverRecord>, LookupFailed> {
        let mut calls = self
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        calls.push((host.to_owned(), port));
        let mut answers = self
            .answers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if answers.is_empty() {
            return Err(LookupFailed);
        }
        answers.remove(0)
    }
}

#[tokio::test]
async fn resolver_resolves_once_and_pins_the_first_validated_answer() {
    let lookup = Scripted {
        answers: Mutex::new(vec![
            Ok(vec![record_of("8.8.8.8", 443)]),
            Ok(vec![record_of("127.0.0.1", 443)]),
        ]),
        calls: Mutex::new(Vec::new()),
    };
    let mut resolver = PinnedResolver::new();
    let first = resolver.resolve(&lookup, "Example.COM.", 443).await;
    let second = resolver.resolve(&lookup, "example.com", 443).await;
    assert_eq!(first.ok(), Some(vec!["8.8.8.8".to_owned()]));
    assert_eq!(second.ok(), Some(vec!["8.8.8.8".to_owned()]));
    let calls = lookup
        .calls
        .lock()
        .map(|calls| calls.clone())
        .unwrap_or_default();
    assert_eq!(calls, vec![("example.com".to_owned(), 443)]);
}

#[tokio::test]
async fn resolver_failure_is_sanitized_and_not_pinned() {
    let lookup = Scripted {
        answers: Mutex::new(vec![Err(LookupFailed), Ok(vec![record_of("8.8.8.8", 443)])]),
        calls: Mutex::new(Vec::new()),
    };
    let mut resolver = PinnedResolver::new();
    let failed = resolver.resolve(&lookup, "example.com", 443).await;
    match failed {
        Err(error) => {
            assert_eq!(error.reason(), "resolution");
            assert!(!error.to_string().contains("10.1"));
        }
        Ok(addresses) => panic!("resolved {addresses:?}"),
    }
    let retried = resolver.resolve(&lookup, "example.com", 443).await;
    assert_eq!(retried.ok(), Some(vec!["8.8.8.8".to_owned()]));
}

#[tokio::test]
async fn resolver_rejects_ip_literal_and_numeric_names_without_resolving() {
    let lookup = Scripted {
        answers: Mutex::new(Vec::new()),
        calls: Mutex::new(Vec::new()),
    };
    let mut resolver = PinnedResolver::new();
    for host in ["127.0.0.1", "2130706433", "0x7f000001", "localhost"] {
        assert!(
            resolver.resolve(&lookup, host, 443).await.is_err(),
            "{host}"
        );
    }
    let calls = lookup
        .calls
        .lock()
        .map(|calls| calls.len())
        .unwrap_or(usize::MAX);
    assert_eq!(calls, 0);
}
