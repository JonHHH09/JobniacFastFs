//! Replays the recorded guard status vectors (`tests/golden/http-guard.json`) through the guard
//! decision. No socket is opened.
//!
//! Every vector must answer its recorded status: 200 means the guard admits the request (the
//! service then answers), any other value is the refusal status.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{IpAddr, SocketAddr};

use http::Request;
use jobniac_mcp::http::{LoopbackHttpConfig, check_request};
use serde_json::Value;

const VECTORS: &str = include_str!("golden/http-guard.json");

/// Named differences: vectors where the guard answers another status than the one recorded, as
/// `(label, recorded status, guard status)`.
///
/// The recording walked the header fields in wire order. The guard reads an `http::HeaderMap`,
/// which yields all values of a repeated name together, so a repeat that follows an oversize
/// field is seen before the size check: 400 where the recording answers 431. The header order is
/// not recoverable from the parsed parts, so this is kept as an explicit difference. Both answers
/// are refusals before the SDK.
const EXPECTED_DIFFERENCES: [(&str, u16, u16); 2] = [
    ("order interleaved repeat after oversize", 431, 400),
    ("order interleaved authorization after oversize", 431, 400),
];

#[test]
fn guard_matches_the_recorded_status_on_every_vector() {
    let document: Value = serde_json::from_str(VECTORS).unwrap();
    let port = u16::try_from(document["port"].as_u64().unwrap()).unwrap();
    let token = document["token"].as_str().unwrap();
    let config = LoopbackHttpConfig::new(port, token).unwrap();
    let vectors = document["vectors"].as_array().unwrap();
    assert!(vectors.len() >= 90, "the golden vectors are present");

    let mut mismatches = Vec::new();
    let mut differences_seen = Vec::new();
    for vector in vectors {
        let label = vector["label"].as_str().unwrap();
        let expected = u16::try_from(vector["status"].as_u64().unwrap()).unwrap();
        let named = EXPECTED_DIFFERENCES
            .iter()
            .find(|(name, _, _)| *name == label);
        let expected = match named {
            Some((_, recorded, _)) => {
                assert_eq!(*recorded, expected, "{label}: the recorded status");
                differences_seen.push(label);
                // The named difference is the guard's answer, asserted below.
                named.map_or(expected, |(_, _, rust)| *rust)
            }
            None => expected,
        };
        let method = vector["method"].as_str().unwrap();
        let path = vector["path"].as_str().unwrap();
        let query = vector["query"].as_str().unwrap();
        let uri = if query.is_empty() {
            path.to_owned()
        } else {
            format!("{path}?{query}")
        };
        let mut builder = Request::builder().method(method).uri(uri);
        for pair in vector["headers"].as_array().unwrap() {
            let name = pair[0].as_str().unwrap();
            let value = pair[1].as_str().unwrap();
            builder = builder.header(name, value);
        }
        let (parts, ()) = builder.body(()).unwrap().into_parts();
        let peer = socket(&vector["peer"]);
        let local = socket(&vector["local"]);
        let actual = match check_request(&config, &parts, peer, local) {
            Ok(()) => 200,
            Err(status) => status.as_u16(),
        };
        if actual != expected {
            mismatches.push(format!("{label}: recorded {expected}, guard {actual}"));
        }
    }
    assert_eq!(
        differences_seen.len(),
        EXPECTED_DIFFERENCES.len(),
        "every named difference is present in the vectors"
    );
    assert!(
        mismatches.is_empty(),
        "guard disagrees with the golden vectors:\n{}",
        mismatches.join("\n")
    );
}

/// The named differences are real: the guard's answer differs from the recorded one on exactly those vectors.
#[test]
fn named_header_order_differences_are_the_only_ones() {
    for (label, recorded, guard) in EXPECTED_DIFFERENCES {
        assert_ne!(recorded, guard, "{label} must be a real difference");
        assert!(
            matches!(recorded, 431) && matches!(guard, 400),
            "{label}: only the 431-versus-400 order difference is named"
        );
    }
}

/// The `[ip, port]` pair of a vector as a socket address.
fn socket(pair: &Value) -> SocketAddr {
    let ip: IpAddr = pair[0].as_str().unwrap().parse().unwrap();
    let port = u16::try_from(pair[1].as_u64().unwrap()).unwrap();
    SocketAddr::new(ip, port)
}
