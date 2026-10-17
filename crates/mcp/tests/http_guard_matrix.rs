//! Loopback guard matrix over real sockets: each refusal, with the precedence pairs (trust, header,
//! authentication, media type and framing cases). Every request is raw HTTP/1.1, so the header
//! bytes are exact.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "common/mod.rs"]
mod common;
#[path = "http_support/mod.rs"]
mod support;

use support::{
    ACCEPT, Harness, Reply, TOKEN, exchange, initialize_body, is_refusal, request_bytes,
    tools_list_body,
};

/// One request of the matrix.
struct Case {
    label: &'static str,
    /// The status is pinned, not the body: hyper answers these before the guard runs.
    status_only: bool,
    method: &'static str,
    path: String,
    host: Option<String>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn pair(name: &str, value: &str) -> (String, String) {
    (name.to_owned(), value.to_owned())
}

/// A valid authenticated POST of `tools/list` for `port`, with its exact Content-Length.
fn valid(label: &'static str, port: u16) -> Case {
    let body = tools_list_body(1);
    Case {
        label,
        status_only: false,
        method: "POST",
        path: "/mcp".into(),
        host: Some(format!("127.0.0.1:{port}")),
        headers: vec![
            pair("Accept", ACCEPT),
            pair("Authorization", &format!("Bearer {TOKEN}")),
            pair("Content-Type", "application/json"),
            pair("Content-Length", &body.len().to_string()),
        ],
        body,
    }
}

/// The request bytes of a case.
fn bytes_of(case: &Case) -> Vec<u8> {
    request_bytes(
        case.method,
        &case.path,
        case.host.as_deref(),
        &case.headers,
        &case.body,
    )
}

/// Replace the value of a header (or add it) and return the case.
fn set(mut case: Case, name: &str, value: &str) -> Case {
    case.headers
        .retain(|(key, _)| !key.eq_ignore_ascii_case(name));
    case.headers.push(pair(name, value));
    case
}

/// Drop every header of a name.
fn drop_header(mut case: Case, name: &str) -> Case {
    case.headers
        .retain(|(key, _)| !key.eq_ignore_ascii_case(name));
    case
}

/// Refused with exactly this status, by the guard.
async fn expect_refused(port: u16, case: &Case, status: u16) {
    let reply: Reply = exchange(port, bytes_of(case)).await;
    assert_eq!(
        reply.status,
        status,
        "{}: status (reply {:?} body {:?})",
        case.label,
        reply.headers,
        reply.text()
    );
    // A HEAD reply has no body. A status hyper writes itself (malformed framing or bytes) is not
    // the guard's refusal text; those cases pin the status only.
    if !case.status_only {
        assert_private(&reply, case.label);
        if case.method != "HEAD" {
            assert!(
                is_refusal(&reply),
                "{}: body must be the fixed refusal",
                case.label
            );
        }
    }
}

/// Every guard and SDK response carries the private headers.
fn assert_private(reply: &Reply, label: &str) {
    assert_eq!(
        reply.header("cache-control"),
        Some("no-store"),
        "{label}: cache-control"
    );
    assert_eq!(
        reply.header("x-content-type-options"),
        Some("nosniff"),
        "{label}: nosniff"
    );
    // The server and date headers are off, on every status.
    assert!(reply.header("date").is_none(), "{label}: date header");
    assert!(reply.header("server").is_none(), "{label}: server header");
}

/// Accepted by the guard: the answer, whatever it is, is not the refusal body.
async fn expect_admitted(port: u16, case: &Case) -> Reply {
    let reply = exchange(port, bytes_of(case)).await;
    assert!(
        !is_refusal(&reply),
        "{}: must reach the service (status {})",
        case.label,
        reply.status
    );
    assert_private(&reply, case.label);
    reply
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trust_path_and_method_refusals_precede_everything() {
    let harness = Harness::start().await;
    let port = harness.port;
    let mut cases: Vec<(Case, u16)> = Vec::new();
    // An empty query string is no query (uvicorn and the guard agree), so "/mcp?" is served.
    for path in ["/", "/mcp/", "/mcp?token=1", "/MCP", "/%6dcp", "/mcp/x"] {
        let mut case = valid("path", port);
        case.path = path.into();
        cases.push((case, 404));
    }
    for method in ["PUT", "PATCH", "OPTIONS", "HEAD", "CONNECT"] {
        // An unauthenticated request with the wrong method is 405 (405 before 401).
        let mut case = drop_header(valid("method", port), "Authorization");
        case.method = method;
        cases.push((case, 405));
        let mut authenticated = valid("method", port);
        authenticated.method = method;
        cases.push((authenticated, 405));
    }
    let mut empty_query = valid("empty-query", port);
    empty_query.path = "/mcp?".into();
    cases.push((empty_query, 200));
    // 404 before 405: a wrong path with a wrong method.
    let mut both = valid("path-and-method", port);
    both.path = "/x".into();
    both.method = "PUT";
    cases.push((both, 404));
    // 403 (Origin) before 401 (no credentials).
    cases.push((
        set(
            drop_header(valid("origin-before-auth", port), "Authorization"),
            "Origin",
            "http://evil.example",
        ),
        403,
    ));
    for (case, status) in &cases {
        if *status == 200 {
            expect_admitted(port, case).await;
        } else {
            expect_refused(port, case, *status).await;
        }
    }
    harness.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_origin_and_authentication_order_and_tampering() {
    let harness = Harness::start().await;
    let port = harness.port;
    let mut cases: Vec<(Case, u16)> = Vec::new();

    // Host: missing, wrong name, portless, other port, and before 401.
    for host in [
        None,
        Some("evil.example".to_owned()),
        Some("127.0.0.1".into()),
        Some(format!("localhost:{port}")),
        Some(format!("127.0.0.1:{}", port.wrapping_add(1))),
    ] {
        let mut case = valid("host", port);
        case.host = host;
        cases.push((case, 421));
    }
    let mut host_before_auth = drop_header(valid("host-before-auth", port), "Authorization");
    host_before_auth.host = Some("evil.example".into());
    cases.push((host_before_auth, 421));

    // Origin: any other origin, including the literal null, is 403.
    for origin in [
        "http://evil.example",
        "null",
        "http://127.0.0.1",
        "https://127.0.0.1:1",
        "http://localhost",
    ] {
        cases.push((set(valid("origin", port), "Origin", origin), 403));
    }

    // Authentication: missing, wrong scheme, tampered token (length, prefix, case, whitespace,
    // empty, non-ASCII).
    let token = TOKEN.to_owned();
    let tampered = [
        ("no authorization", None),
        ("basic scheme", Some(format!("Basic {token}"))),
        ("scheme without space", Some(format!("Bearer{token}"))),
        ("empty token", Some("Bearer ".to_owned())),
        ("scheme only", Some("Bearer".to_owned())),
        (
            "short by one",
            Some(format!("Bearer {}", &token[..token.len() - 1])),
        ),
        ("prefix only", Some(format!("Bearer {}", &token[..42]))),
        ("long by one", Some(format!("Bearer {token}A"))),
        (
            "uppercased",
            Some(format!("Bearer {}", token.to_uppercase())),
        ),
        ("leading space in token", Some(format!("Bearer  {token}"))),
        ("cookie only", None),
        (
            "wrong token",
            Some(format!("Bearer {}", "B".repeat(token.len()))),
        ),
    ];
    for (label, value) in tampered {
        let mut case = valid("auth", port);
        case.label = label;
        case.headers
            .retain(|(key, _)| !key.eq_ignore_ascii_case("authorization"));
        if let Some(value) = value {
            case.headers.push(pair("Authorization", &value));
        }
        if label == "cookie only" {
            case.headers.push(pair("Cookie", &format!("token={token}")));
        }
        cases.push((case, 401));
    }
    // A tab inside the value is outside the printable range: 400, before authentication.
    let tab = set(
        valid("tab", port),
        "Authorization",
        &format!("Bearer\t{token}"),
    );
    expect_refused(port, &tab, 400).await;
    // Non-ASCII in the header value is a syntax fault (400), which comes before authentication.
    let mut unicode = valid("unicode", port);
    unicode
        .headers
        .retain(|(key, _)| !key.eq_ignore_ascii_case("authorization"));
    let mut raw = bytes_of(&unicode);
    let needle = b"Content-Type:";
    let at = raw
        .windows(needle.len())
        .position(|window| window == needle)
        .unwrap();
    raw.splice(
        at..at,
        format!("Authorization: Bearer {}\r\n", "é".repeat(43)).into_bytes(),
    );
    let reply = exchange(port, raw).await;
    assert_eq!(reply.status, 400, "non-ASCII value");
    assert!(is_refusal(&reply));

    // Scheme matching is case-insensitive; the token is exact.
    for scheme in ["bearer", "BEARER", "bEaReR"] {
        let case = set(
            valid("scheme-case", port),
            "Authorization",
            &format!("{scheme} {token}"),
        );
        expect_admitted(port, &case).await;
    }

    for (case, status) in &cases {
        expect_refused(port, case, *status).await;
    }
    harness.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn header_syntax_proxy_duplicates_and_size() {
    let harness = Harness::start().await;
    let port = harness.port;
    let mut cases: Vec<(Case, u16)> = Vec::new();
    for (name, value) in [
        ("Forwarded", "for=192.0.2.1"),
        ("X-Forwarded-For", "192.0.2.1"),
        ("X-Forwarded-Host", "example.test"),
        ("X-Real-IP", "192.0.2.1"),
        ("Via", "1.1 proxy"),
        ("X-Original-URL", "/mcp"),
        ("X-Rewrite-URL", "/mcp"),
    ] {
        // Proxy headers are refused with 400, before Host (421) and authentication (401).
        let mut case = set(valid("proxy", port), name, value);
        case.host = Some("evil.example".into());
        case.headers
            .retain(|(key, _)| !key.eq_ignore_ascii_case("authorization"));
        cases.push((case, 400));
    }
    // Duplicates of critical headers, and of any mcp-* header, are 400 even when one copy is right.
    for name in [
        "Authorization",
        "Content-Type",
        "Accept",
        "Host",
        "Origin",
        "MCP-Session-Id",
        "mcp-protocol-version",
    ] {
        let mut case = valid("duplicate", port);
        // Two copies of the same field, whether or not the request already carried one.
        let value = if name == "Origin" {
            format!("http://127.0.0.1:{port}")
        } else {
            "x".to_owned()
        };
        case.headers.push(pair(name, &value));
        case.headers.push(pair(name, &value));
        if name == "Host" {
            case.host = Some(format!("127.0.0.1:{port}"));
            let mut raw = bytes_of(&case);
            // A second Host line, after the first.
            let needle = b"\r\nAccept:";
            let at = raw.windows(needle.len()).position(|w| w == needle).unwrap();
            raw.splice(at..at, format!("\r\nHost: 127.0.0.1:{port}").into_bytes());
            let reply = exchange(port, raw).await;
            assert_eq!(reply.status, 400, "duplicate Host");
            assert!(is_refusal(&reply));
            continue;
        }
        cases.push((case, 400));
    }
    // A non-critical duplicate is not refused by the guard.
    let mut joined = valid("joined", port);
    joined.headers.push(pair("X-Trace", "a"));
    joined.headers.push(pair("X-Trace", "b"));
    cases.push((joined, 200));
    // Header block over 16 KiB is 431, before Host and authentication.
    let mut big = drop_header(valid("big-header", port), "Authorization");
    big.host = Some("evil.example".into());
    big.headers.push(pair("X-Pad", &"a".repeat(17_000)));
    cases.push((big, 431));
    // Non-printable value bytes (a control character) are a syntax fault.
    let mut raw = bytes_of(&valid("control", port));
    let needle = b"Accept:";
    let at = raw.windows(needle.len()).position(|w| w == needle).unwrap();
    raw.splice(at..at, b"X-Ctl: a\x01b\r\n".iter().copied());
    let reply = exchange(port, raw).await;
    assert_eq!(reply.status, 400, "control byte in a value");

    for (case, status) in &cases {
        if *status == 200 {
            expect_admitted(port, case).await;
        } else {
            expect_refused(port, case, *status).await;
        }
    }
    harness.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn framing_and_media_type_refusals() {
    let harness = Harness::start().await;
    let port = harness.port;
    let mut cases: Vec<(Case, u16)> = Vec::new();

    // Transfer-Encoding: only chunked, only on POST, never with Content-Length.
    let mut te_gzip = set(valid("te-gzip", port), "Transfer-Encoding", "gzip");
    te_gzip.status_only = true;
    cases.push((te_gzip, 400));
    let mut te_get = valid("te-get", port);
    te_get.method = "GET";
    te_get.body = Vec::new();
    te_get.headers = vec![
        pair("Accept", ACCEPT),
        pair("Authorization", &format!("Bearer {TOKEN}")),
        pair("Transfer-Encoding", "chunked"),
    ];
    cases.push((te_get, 400));
    let mut te_delete = valid("te-delete", port);
    te_delete.method = "DELETE";
    te_delete.headers.push(pair("Transfer-Encoding", "chunked"));
    te_delete
        .headers
        .retain(|(key, _)| !key.eq_ignore_ascii_case("content-length"));
    cases.push((te_delete, 400));
    cases.push((
        set(valid("te-and-cl", port), "Transfer-Encoding", "chunked"),
        400,
    ));

    // Content-Length: digits only, at most 10 characters, within 4 MiB, zero on non-POST.
    for length in ["abc", "+5", "-5", "1e3", "00000000001", ""] {
        let mut case = set(valid("cl-syntax", port), "Content-Length", length);
        // hyper parses most malformed lengths before the guard; only the eleven-digit form
        // reaches the guard's own length rule.
        case.status_only = length != "00000000001";
        cases.push((case, 400));
    }
    cases.push((
        set(valid("cl-too-big", port), "Content-Length", "4194305"),
        413,
    ));
    cases.push((
        set(
            valid("cl-far-too-big", port),
            "Content-Length",
            "9999999999",
        ),
        413,
    ));
    let mut get_with_body = valid("get-cl", port);
    get_with_body.method = "GET";
    get_with_body.headers = vec![
        pair("Accept", ACCEPT),
        pair("Authorization", &format!("Bearer {TOKEN}")),
        pair("Content-Length", "5"),
    ];
    get_with_body.body = b"hello".to_vec();
    cases.push((get_with_body, 400));
    // Authentication precedes the length rules: a 401 wins over a 413.
    cases.push((
        set(
            drop_header(valid("auth-before-413", port), "Authorization"),
            "Content-Length",
            "4194305",
        ),
        401,
    ));

    // Content-Type on POST: application/json, optionally with charset=utf-8, nothing else.
    for content_type in [
        "text/plain",
        "application/jsonx",
        "application/json; charset=latin-1",
        "application/json; charset=utf-8; x=1",
        "application/json; foo=bar",
        "application/json; charset=\"utf-8",
        "application/json;;",
        "",
    ] {
        let case = if content_type.is_empty() {
            drop_header(valid("ct-missing", port), "Content-Type")
        } else {
            set(valid("ct", port), "Content-Type", content_type)
        };
        cases.push((case, 415));
    }
    // 401 before 415: an unauthenticated POST with the wrong media type.
    cases.push((
        set(
            drop_header(valid("ct-unauth", port), "Authorization"),
            "Content-Type",
            "text/plain",
        ),
        401,
    ));
    // A duplicate Content-Type is 400 before the media type is judged.
    let mut dup_ct = valid("ct-dup", port);
    dup_ct
        .headers
        .push(pair("Content-Type", "application/json"));
    cases.push((dup_ct, 400));

    for (case, status) in &cases {
        expect_refused(port, case, *status).await;
    }

    // Accepted media types and framings reach the service.
    let mut accepted: Vec<Case> = Vec::new();
    for content_type in [
        "application/json",
        "Application/JSON",
        "application/json; charset=utf-8",
        "application/json;charset=\"UTF-8\"",
        "application/json ;  charset = utf-8 ",
    ] {
        accepted.push(set(valid("ct-ok", port), "Content-Type", content_type));
    }
    for case in &accepted {
        expect_admitted(port, case).await;
    }
    // Chunked POST without Content-Length reaches the service.
    let body = initialize_body(7);
    let mut chunked = valid("chunked", port);
    chunked
        .headers
        .retain(|(key, _)| !key.eq_ignore_ascii_case("content-length"));
    chunked.headers.push(pair("Transfer-Encoding", "chunked"));
    chunked.body = format!("{:x}\r\n", body.len()).into_bytes();
    chunked.body.extend_from_slice(&body);
    chunked.body.extend_from_slice(b"\r\n0\r\n\r\n");
    let reply = expect_admitted(port, &chunked).await;
    assert_eq!(reply.status, 200, "chunked initialize is answered");
    assert!(reply.text().contains("protocolVersion"));

    harness.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn valid_requests_reach_the_service_for_each_method() {
    let harness = Harness::start().await;
    let port = harness.port;
    // Without a session id the service answers, but never with the guard's refusal body.
    let post = valid("post", port);
    let mut get = valid("get", port);
    get.method = "GET";
    get.body = Vec::new();
    get.headers.retain(|(key, _)| {
        !key.eq_ignore_ascii_case("content-type") && !key.eq_ignore_ascii_case("content-length")
    });
    let mut delete = valid("delete", port);
    delete.method = "DELETE";
    delete.body = Vec::new();
    delete.headers.retain(|(key, _)| {
        !key.eq_ignore_ascii_case("content-type") && !key.eq_ignore_ascii_case("content-length")
    });
    for case in [&post, &get, &delete] {
        expect_admitted(port, case).await;
    }
    harness.stop().await.unwrap();
}
