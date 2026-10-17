//! Loopback Streamable HTTP protocol: the SDK session over `/mcp` answers the same catalog and
//! calls as the STDIO transport, with plain JSON replies (protocol,
//! session, capacity and protocol-version cases).

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "common/mod.rs"]
mod common;
#[path = "http_support/mod.rs"]
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jobniac_mcp::{JobniacMcpServer, serve_with};
use serde_json::{Value, json};
use support::{ACCEPT, Harness, Reply, TOKEN, counting_services, is_refusal, request_bytes};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// The JSON-RPC requests of one session, in order: initialize, the initialized notification,
/// tools/list and a health call.
fn session_messages() -> Vec<Value> {
    vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"health","arguments":{}}}),
    ]
}

/// The JSON-RPC message of a POST answer. A stateful answer is framed by the SDK as one SSE
/// event; the listener sends it as a plain `application/json` body, which is what is checked.
fn json_message(reply: &Reply) -> Value {
    assert_eq!(reply.status, 200, "status of the answer: {}", reply.text());
    assert_eq!(
        reply.header("content-type"),
        Some("application/json"),
        "a POST answer is plain JSON, not an event stream: {:?}",
        reply.text()
    );
    serde_json::from_slice(&reply.body)
        .unwrap_or_else(|error| panic!("the answer is not one JSON-RPC message: {error}"))
}

/// The result of request `id` from a list of JSON-RPC messages.
fn result_of(messages: &[Value], id: u64) -> Value {
    messages
        .iter()
        .find(|message| message.get("id") == Some(&json!(id)))
        .and_then(|message| message.get("result").cloned())
        .unwrap_or_else(|| panic!("no result for request {id}: {messages:?}"))
}

/// Run the session over STDIO and return the answers.
async fn over_stdio() -> Vec<Value> {
    let server = JobniacMcpServer::new(common::services()).unwrap();
    let (mut feed, input) = tokio::io::duplex(64 * 1024);
    let (output, answers) = tokio::io::duplex(256 * 1024);
    for message in session_messages() {
        feed.write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }
    drop(feed);
    tokio::time::timeout(Duration::from_secs(20), serve_with(server, input, output))
        .await
        .expect("stdio session ends at end of input")
        .expect("stdio session serves");
    let mut lines = BufReader::new(answers).lines();
    let mut out = Vec::new();
    while let Some(line) = lines.next_line().await.unwrap() {
        out.push(serde_json::from_str(&line).unwrap());
    }
    out
}

/// The headers of a POST carrying `message`, with an optional session id and protocol version.
fn post_bytes(port: u16, message: &Value, session: Option<&str>, version: Option<&str>) -> Vec<u8> {
    let body = message.to_string().into_bytes();
    let mut headers = vec![
        ("Accept".to_owned(), ACCEPT.to_owned()),
        ("Authorization".to_owned(), format!("Bearer {TOKEN}")),
        ("Content-Type".to_owned(), "application/json".to_owned()),
        ("Content-Length".to_owned(), body.len().to_string()),
    ];
    if let Some(version) = version {
        headers.push(("MCP-Protocol-Version".to_owned(), version.to_owned()));
    }
    if let Some(session) = session {
        headers.push(("Mcp-Session-Id".to_owned(), session.to_owned()));
    }
    request_bytes(
        "POST",
        "/mcp",
        Some(&format!("127.0.0.1:{port}")),
        &headers,
        &body,
    )
}

/// A stateless 2026-07-28 POST: the SEP-2243 `Mcp-Method` header, and `Mcp-Name` for a call.
fn modern_post_bytes(port: u16, message: &Value, method: &str, name: Option<&str>) -> Vec<u8> {
    let body = message.to_string().into_bytes();
    let mut headers = vec![
        ("Accept".to_owned(), ACCEPT.to_owned()),
        ("Authorization".to_owned(), format!("Bearer {TOKEN}")),
        ("Content-Type".to_owned(), "application/json".to_owned()),
        ("Content-Length".to_owned(), body.len().to_string()),
        ("MCP-Protocol-Version".to_owned(), "2026-07-28".to_owned()),
        ("Mcp-Method".to_owned(), method.to_owned()),
    ];
    if let Some(name) = name {
        headers.push(("Mcp-Name".to_owned(), name.to_owned()));
    }
    request_bytes(
        "POST",
        "/mcp",
        Some(&format!("127.0.0.1:{port}")),
        &headers,
        &body,
    )
}

/// Post one JSON-RPC message with the given session headers and return the raw reply.
fn post(port: u16, message: &Value, session: Option<&str>) -> Reply {
    support::send(
        port,
        &post_bytes(port, message, session, Some("2025-11-25")),
    )
}

/// `initialize` with the given request id.
fn initialize(id: u64) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"synthetic-http","version":"0"}}})
}

/// The session id issued by an initialize answer.
fn session_of(reply: &Reply) -> String {
    reply
        .header("mcp-session-id")
        .expect("the session id is issued at initialize")
        .to_owned()
}

/// Send a `DELETE` for one session.
fn delete(port: u16, session: &str) -> Reply {
    let bytes = request_bytes(
        "DELETE",
        "/mcp",
        Some(&format!("127.0.0.1:{port}")),
        &[
            ("Accept".to_owned(), ACCEPT.to_owned()),
            ("Authorization".to_owned(), format!("Bearer {TOKEN}")),
            ("Mcp-Session-Id".to_owned(), session.to_owned()),
            ("MCP-Protocol-Version".to_owned(), "2025-11-25".to_owned()),
        ],
        b"",
    );
    support::send(port, &bytes)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_session_answers_the_same_catalog_and_calls_as_stdio() {
    let stdio = over_stdio().await;
    let harness = Harness::start().await;
    let port = harness.port;
    let messages = session_messages();

    let (initialize, notification, list, call) = (
        messages[0].clone(),
        messages[1].clone(),
        messages[2].clone(),
        messages[3].clone(),
    );
    let http_answers = tokio::task::spawn_blocking(move || {
        let first = post(port, &initialize, None);
        assert!(!is_refusal(&first));
        let session = session_of(&first);
        let initialized = post(port, &notification, Some(&session));
        assert_eq!(initialized.status, 202, "the notification is accepted");
        let mut answers = vec![json_message(&first)];
        for message in [&list, &call] {
            answers.push(json_message(&post(port, message, Some(&session))));
        }
        answers
    })
    .await
    .unwrap();

    assert_eq!(
        result_of(&http_answers, 2),
        result_of(&stdio, 2),
        "tools/list equals STDIO"
    );
    assert_eq!(
        result_of(&http_answers, 3),
        result_of(&stdio, 3),
        "tools/call health equals STDIO"
    );
    assert_eq!(
        result_of(&http_answers, 1),
        result_of(&stdio, 1),
        "initialize result equals STDIO"
    );

    let tools = result_of(&http_answers, 2)["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(!tools.is_empty(), "the catalog lists tools");
    harness.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_session_is_answered_by_the_service_not_the_guard() {
    let harness = Harness::start().await;
    let port = harness.port;
    let reply = tokio::task::spawn_blocking(move || {
        post(
            port,
            &json!({"jsonrpc":"2.0","id":5,"method":"tools/list","params":{}}),
            Some("no-such-session"),
        )
    })
    .await
    .unwrap();
    assert!(!is_refusal(&reply), "a stale session is the SDK's answer");
    assert!(
        reply.status == 404 || reply.status == 400 || reply.status == 401,
        "status {}",
        reply.status
    );
    harness.stop().await.unwrap();
}

/// A deleted session answers 404 afterwards, the other client's session keeps working, and a new
/// initialize is served.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stateful_session_delete_requires_new_initialize_and_keeps_other_client() {
    let calls = Arc::new(AtomicUsize::new(0));
    let harness = Harness::start_with(counting_services(Arc::clone(&calls))).await;
    let port = harness.port;
    let health = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"health","arguments":{}}});
    tokio::task::spawn_blocking(move || {
        let first = post(port, &initialize(1), None);
        let second = post(port, &initialize(2), None);
        assert_eq!((first.status, second.status), (200, 200));
        let (one, two) = (session_of(&first), session_of(&second));
        assert_ne!(one, two);
        for session in [&one, &two] {
            let initialized = post(
                port,
                &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                Some(session),
            );
            assert_eq!(initialized.status, 202);
        }
        let removed = delete(port, &one);
        assert_eq!(removed.status, 200, "DELETE ends the session");
        let expired = post(port, &health, Some(&one));
        assert_eq!(expired.status, 404, "a deleted session is not found");
        let surviving = post(port, &health, Some(&two));
        assert_eq!(
            json_message(&surviving)["result"]["structuredContent"]["status"],
            json!("UP"),
            "the other client's session survives"
        );
        let restarted = post(port, &initialize(4), None);
        assert_eq!(restarted.status, 200, "a new initialize is served");
    })
    .await
    .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "only the surviving call ran"
    );
    harness.stop().await.unwrap();
}

/// Sixteen sessions open, the seventeenth opening is a 503 JSON-RPC error, and a DELETE frees one slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_capacity_refuses_and_frees_slot_after_delete() {
    let harness = Harness::start().await;
    let port = harness.port;
    tokio::task::spawn_blocking(move || {
        let mut sessions = Vec::new();
        for identifier in 1..=16 {
            let reply = post(port, &initialize(identifier), None);
            assert_eq!(reply.status, 200, "session {identifier} is opened");
            sessions.push(session_of(&reply));
        }
        let refused = post(port, &initialize(17), None);
        assert_eq!(refused.status, 503, "the seventeenth session is refused");
        assert_eq!(refused.header("content-type"), Some("application/json"));
        assert_eq!(refused.header("cache-control"), Some("no-store"));
        assert_eq!(refused.header("x-content-type-options"), Some("nosniff"));
        let body: Value = serde_json::from_slice(&refused.body).unwrap();
        assert_eq!(
            body,
            json!({"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"Too many open sessions"}})
        );
        assert_eq!(delete(port, &sessions[0]).status, 200);
        let admitted = post(port, &initialize(18), None);
        assert_eq!(admitted.status, 200, "the freed slot admits a session");
        assert!(!sessions.contains(&session_of(&admitted)));
        // A request on an existing session is never a new session, so it is not refused.
        let existing = post(
            port,
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
            Some(&sessions[1]),
        );
        assert_eq!(existing.status, 200, "existing sessions are unaffected");
    })
    .await
    .unwrap();
    harness.stop().await.unwrap();
}

/// An unknown protocol version and a `_meta` version that differs from the header are 400
/// refusals, and the service is never called.
///
/// The SDK answers the unknown `MCP-Protocol-Version` header with a `text/plain` body and the
/// `_meta` mismatch with a JSON-RPC error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sdk_rejects_unknown_or_mismatched_protocol_without_service_effect() {
    let calls = Arc::new(AtomicUsize::new(0));
    let harness = Harness::start_with(counting_services(Arc::clone(&calls))).await;
    let port = harness.port;
    let unknown = json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}});
    let mismatch = json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-11-25"}}});
    tokio::task::spawn_blocking(move || {
        let unknown_reply =
            support::send(port, &post_bytes(port, &unknown, None, Some("2099-01-01")));
        assert_eq!(unknown_reply.status, 400, "an unknown version is refused");
        assert!(
            unknown_reply
                .text()
                .contains("Unsupported MCP-Protocol-Version")
        );
        assert_eq!(unknown_reply.header("cache-control"), Some("no-store"));

        let mismatch_reply =
            support::send(port, &post_bytes(port, &mismatch, None, Some("2026-07-28")));
        assert_eq!(
            mismatch_reply.status, 400,
            "a mismatched _meta version is refused"
        );
        let parsed: Value = serde_json::from_slice(&mismatch_reply.body)
            .unwrap_or_else(|error| panic!("refusal is JSON: {error}: {}", mismatch_reply.text()));
        assert!(parsed.get("error").is_some(), "{parsed}");
    })
    .await
    .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the service was not called"
    );
    harness.stop().await.unwrap();
}

/// A chunked body with no Content-Length, and a lower-case `bearer` scheme, is served.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn streamed_json_initialize_accepts_unambiguous_chunked_body_and_bearer_scheme() {
    let harness = Harness::start().await;
    let port = harness.port;
    let body = initialize(1).to_string().into_bytes();
    let headers = vec![
        ("Accept".to_owned(), ACCEPT.to_owned()),
        ("Authorization".to_owned(), format!("bearer {TOKEN}")),
        ("Content-Type".to_owned(), "application/json".to_owned()),
        ("Transfer-Encoding".to_owned(), "chunked".to_owned()),
    ];
    let mut bytes = request_bytes(
        "POST",
        "/mcp",
        Some(&format!("127.0.0.1:{port}")),
        &headers,
        b"",
    );
    for piece in body.chunks(17) {
        bytes.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
        bytes.extend_from_slice(piece);
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(b"0\r\n\r\n");
    let reply = support::exchange(port, bytes).await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.header("content-type"), Some("application/json"));
    let parsed = json_message(&reply);
    assert_eq!(parsed["result"]["protocolVersion"], json!("2025-11-25"));
    harness.stop().await.unwrap();
}

/// The 2026-07-28 stateless protocol is not served: the catalog declares support only up to
/// 2025-11-25 (`JobniacMcpServer::supported_protocol_versions`, a deliberate ceiling). The SDK
/// therefore refuses a 2026-07-28 request with JSON-RPC error `-32022` ("Unsupported protocol
/// version") and the supported list, and the service is not called. A positive test waits for
/// that ceiling to be decided.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modern_protocol_2026_07_28_is_refused_as_unsupported() {
    let calls = Arc::new(AtomicUsize::new(0));
    let harness = Harness::start_with(counting_services(Arc::clone(&calls))).await;
    let port = harness.port;
    let meta = json!({"io.modelcontextprotocol/protocolVersion": "2026-07-28"});
    let list = json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":meta}});
    let reply = tokio::task::spawn_blocking(move || {
        support::send(port, &modern_post_bytes(port, &list, "tools/list", None))
    })
    .await
    .unwrap();
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(
        reply.header("mcp-session-id").is_none(),
        "no session is opened"
    );
    let error = json_error(&reply);
    assert_eq!(error["code"], json!(-32022));
    assert_eq!(error["message"], json!("Unsupported protocol version"));
    let supported = error["data"]["supported"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !supported.contains(&json!("2026-07-28")),
        "the Rust catalog does not declare 2026-07-28"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the service was not called"
    );
    harness.stop().await.unwrap();
}

/// The JSON-RPC error object of a refusal answered as a JSON body.
fn json_error(reply: &Reply) -> Value {
    let parsed: Value = serde_json::from_slice(&reply.body)
        .unwrap_or_else(|error| panic!("refusal is JSON: {error}: {}", reply.text()));
    parsed["error"].clone()
}

/// POST `body` exactly as written (so escapes and nesting stay as the test spells them), with the
/// given session id and extra headers.
fn raw_post(port: u16, body: &str, session: Option<&str>, extra: &[(&str, &str)]) -> Reply {
    let mut headers = vec![
        ("Accept".to_owned(), ACCEPT.to_owned()),
        ("Authorization".to_owned(), format!("Bearer {TOKEN}")),
        ("Content-Type".to_owned(), "application/json".to_owned()),
        ("Content-Length".to_owned(), body.len().to_string()),
    ];
    if let Some(session) = session {
        headers.push(("Mcp-Session-Id".to_owned(), session.to_owned()));
        headers.push(("MCP-Protocol-Version".to_owned(), "2025-11-25".to_owned()));
    }
    for (name, value) in extra {
        headers.push(((*name).to_owned(), (*value).to_owned()));
    }
    support::send(
        port,
        &request_bytes(
            "POST",
            "/mcp",
            Some(&format!("127.0.0.1:{port}")),
            &headers,
            body.as_bytes(),
        ),
    )
}

/// The text of the first content block of a `tools/call` answer.
fn call_text(reply: &Reply) -> String {
    json_message(reply)["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The HTTP body is admitted as a STDIO line is: a lone surrogate escape and nesting
/// past the limit get the argument rejection of the tool, which does not run, and a
/// client-supplied rejection marker is dropped instead of rejecting a valid call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_bodies_get_the_stdin_admission_and_a_client_marker_is_dropped() {
    let calls = Arc::new(AtomicUsize::new(0));
    let harness = Harness::start_with(counting_services(Arc::clone(&calls))).await;
    let port = harness.port;
    tokio::task::spawn_blocking(move || {
        let first = post(port, &initialize(1), None);
        let session = session_of(&first);
        let session = Some(session.as_str());
        let initialized = post(
            port,
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            session,
        );
        assert_eq!(initialized.status, 202);

        let call = |arguments: &str, meta: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{{"name":"health","arguments":{arguments}{meta}}}}}"#
            )
        };
        let lone = raw_post(port, &call(r#"{"note":"\ud800"}"#, ""), session, &[]);
        let text = call_text(&lone);
        assert!(text.contains("[type=string_unicode]"), "{text}");

        let deep = format!("{}1{}", "[".repeat(101), "]".repeat(101));
        let too_deep = raw_post(port, &call(&format!(r#"{{"x":{deep}}}"#), ""), session, &[]);
        let text = call_text(&too_deep);
        assert!(text.contains("[type=json_invalid]"), "{text}");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "no rejected call ran");

        // A spoofed marker on a valid call must not reject it.
        let spoofed = raw_post(
            port,
            &call(
                "{}",
                r#","_meta":{"io.jobniac/rejected-input":"too_deep"}"#,
            ),
            session,
            &[],
        );
        let answer = json_message(&spoofed);
        assert_eq!(answer["result"]["isError"], json!(false), "{answer}");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the valid call ran once");
    })
    .await
    .unwrap();
    harness.stop().await.unwrap();
}

/// A session-less request is served only as an `initialize`: one that carries its own supported
/// protocol version in `_meta` is refused, not run statelessly by the SDK.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_less_call_with_inline_meta_does_not_run_the_tool() {
    let calls = Arc::new(AtomicUsize::new(0));
    let harness = Harness::start_with(counting_services(Arc::clone(&calls))).await;
    let port = harness.port;
    let reply = tokio::task::spawn_blocking(move || {
        let body = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"health","arguments":{},"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-11-25","io.modelcontextprotocol/clientCapabilities":{}}}}"#;
        raw_post(port, body, None, &[("MCP-Protocol-Version", "2025-11-25")])
    })
    .await
    .unwrap();
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(
        reply.text().contains("Missing session ID"),
        "{}",
        reply.text()
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the tool did not run without a session"
    );
    harness.stop().await.unwrap();
}

/// A repeated `_meta` or version key (the SDK keeps the last one) does not slip a session-less
/// call past the guard.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_less_call_with_duplicate_keys_does_not_run_the_tool() {
    let calls = Arc::new(AtomicUsize::new(0));
    let harness = Harness::start_with(counting_services(Arc::clone(&calls))).await;
    let port = harness.port;
    let replies = tokio::task::spawn_blocking(move || {
        let meta = r#""io.modelcontextprotocol/protocolVersion":"2025-11-25","io.modelcontextprotocol/clientCapabilities":{}"#;
        let bodies = [
            format!(
                r#"{{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{{"name":"health","arguments":{{}},"_meta":{{}},"_meta":{{{meta}}}}}}}"#
            ),
            format!(
                r#"{{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{{"name":"health","arguments":{{}},"_meta":{{"io.modelcontextprotocol/protocolVersion":"x",{meta}}}}}}}"#
            ),
        ];
        bodies.map(|body| {
            raw_post(port, &body, None, &[("MCP-Protocol-Version", "2025-11-25")])
        })
    })
    .await
    .unwrap();
    for reply in replies {
        assert_eq!(reply.status, 400, "{}", reply.text());
        assert!(
            reply.text().contains("Missing session ID"),
            "{}",
            reply.text()
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no tool ran");
    harness.stop().await.unwrap();
}

/// The SDK's own error texts never echo request content: a malformed body is a fixed JSON-RPC
/// parse error with status 400, and an unsupported protocol version is named without its value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sdk_error_answers_never_echo_the_request() {
    let harness = Harness::start().await;
    let port = harness.port;
    tokio::task::spawn_blocking(move || {
        let malformed = raw_post(port, r#"{"canary-body-5d1e": "#, None, &[]);
        assert_eq!(malformed.status, 400, "{}", malformed.text());
        assert!(!malformed.text().contains("canary-body-5d1e"));
        assert_eq!(malformed.header("content-type"), Some("application/json"));
        assert_eq!(json_error(&malformed)["code"], json!(-32700));
        assert_eq!(malformed.header("cache-control"), Some("no-store"));

        let version = raw_post(
            port,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
            None,
            &[("MCP-Protocol-Version", "canary-version-8c3f")],
        );
        assert_eq!(version.status, 400, "{}", version.text());
        assert!(version.text().contains("Unsupported MCP-Protocol-Version"));
        assert!(!version.text().contains("canary-version-8c3f"));
    })
    .await
    .unwrap();
    harness.stop().await.unwrap();
}

/// No `Date` and no `Server` header on an answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answers_carry_no_date_or_server_header() {
    let harness = Harness::start().await;
    let port = harness.port;
    let reply = tokio::task::spawn_blocking(move || post(port, &initialize(1), None))
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    assert!(reply.header("date").is_none(), "{:?}", reply.headers);
    assert!(reply.header("server").is_none(), "{:?}", reply.headers);
    harness.stop().await.unwrap();
}

/// A GET stream does not hold an admission slot: sixteen sessions with an open GET stream each
/// still leave every POST admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_get_streams_do_not_use_up_request_admission() {
    use std::io::{Read, Write};

    let harness = Harness::start().await;
    let port = harness.port;
    tokio::task::spawn_blocking(move || {
        let mut sessions = Vec::new();
        for id in 0..16_u64 {
            let first = post(port, &initialize(id), None);
            assert_eq!(first.status, 200, "session {id}");
            let session = session_of(&first);
            let initialized = post(
                port,
                &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                Some(&session),
            );
            assert_eq!(initialized.status, 202);
            sessions.push(session);
        }
        // Open the stand-alone stream of each session and keep it open.
        let mut streams = Vec::new();
        for session in &sessions {
            let mut stream = support::connect(port);
            let head = format!(
                "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: text/event-stream\r\nAuthorization: Bearer {TOKEN}\r\nMcp-Session-Id: {session}\r\nMCP-Protocol-Version: 2025-11-25\r\n\r\n"
            );
            stream.write_all(head.as_bytes()).unwrap();
            let mut buffer = [0_u8; 512];
            let read = stream.read(&mut buffer).unwrap();
            assert!(
                buffer[..read].starts_with(b"HTTP/1.1 200"),
                "{}",
                String::from_utf8_lossy(&buffer[..read])
            );
            streams.push(stream);
        }
        // Sixteen streams are open; a POST on an existing session is still admitted.
        let list = json!({"jsonrpc":"2.0","id":9,"method":"tools/list","params":{}});
        let reply = post(port, &list, Some(&sessions[0]));
        assert_eq!(reply.status, 200, "{}", reply.text());
        drop(streams);
    })
    .await
    .unwrap();
    harness.stop().await.unwrap();
}
