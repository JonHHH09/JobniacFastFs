//! Socket-level helpers for the loopback HTTP tests: a real listener on an ephemeral loopback
//! port and a raw HTTP/1.1 client, so tests control every header byte. All data is synthetic.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use jobniac_core::clock::FixedClock;
use jobniac_core::health::{HealthProbe, HealthProbeResult, HealthService};
use jobniac_mcp::http::{LoopbackHttpConfig, serve_loopback_on};
use jobniac_mcp::{JobniacMcpServer, McpServices};
use time::macros::datetime;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::common;

/// The synthetic bearer token of every loopback test (50 characters of the allowed alphabet).
pub const TOKEN: &str = "synthetic_loopback_token_0123456789_abcdefghijklmn";
/// The `Accept` value the streamable HTTP transport requires.
pub const ACCEPT: &str = "application/json, text/event-stream";

/// A running listener. Dropping the harness does not stop it: call [`Harness::stop`].
pub struct Harness {
    pub port: u16,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), jobniac_mcp::http::HttpServeError>>,
}

/// A health probe that counts its calls: the observable effect of a `health` tool call.
#[derive(Debug)]
pub struct CountingProbe(pub Arc<AtomicUsize>);

#[async_trait]
impl HealthProbe for CountingProbe {
    async fn probe(&self) -> HealthProbeResult {
        self.0.fetch_add(1, Ordering::SeqCst);
        HealthProbeResult::ready()
    }
}

/// The shared synthetic catalog, with a `health` probe that counts its calls in `calls`.
pub fn counting_services(calls: Arc<AtomicUsize>) -> McpServices {
    let mut services = common::services();
    let clock = FixedClock::new(datetime!(2026-01-02 03:04:05.000006 UTC));
    services.health = Arc::new(HealthService::new(
        Arc::new(CountingProbe(calls)),
        None,
        Arc::new(clock),
    ));
    services
}

impl Harness {
    /// Bind an ephemeral loopback port and serve the shared synthetic catalog on it.
    pub async fn start() -> Self {
        Self::start_with(common::services()).await
    }

    /// Bind an ephemeral loopback port and serve `services` on it.
    pub async fn start_with(services: McpServices) -> Self {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap_or_else(|error| panic!("loopback bind failed: {error}"));
        let port = listener
            .local_addr()
            .map(|address| address.port())
            .unwrap_or_else(|error| panic!("loopback address unavailable: {error}"));
        let config = LoopbackHttpConfig::new(port, TOKEN)
            .unwrap_or_else(|_| panic!("ephemeral port {port} is below the configured minimum"));
        let server = JobniacMcpServer::new(services)
            .unwrap_or_else(|error| panic!("catalog failed: {error}"));
        let (shutdown, stop) = oneshot::channel::<()>();
        let task = tokio::spawn(serve_loopback_on(server, config, listener, async move {
            let _ = stop.await;
        }));
        Self {
            port,
            shutdown: Some(shutdown),
            task,
        }
    }

    /// Start shutdown and wait for the listener to finish.
    pub async fn stop(mut self) -> Result<(), jobniac_mcp::http::HttpServeError> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        match self.task.await {
            Ok(result) => result,
            Err(error) => panic!("listener task failed: {error}"),
        }
    }
}

/// A parsed HTTP/1.1 response.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    /// The first value of a header, compared case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The body as text, lossily.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Address of the listener on the loopback interface.
pub fn address(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

/// Open a raw connection to the listener with a read timeout.
pub fn connect(port: u16) -> TcpStream {
    let stream = TcpStream::connect(address(port))
        .unwrap_or_else(|error| panic!("connect to the loopback listener failed: {error}"));
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    stream
}

/// The header block of a valid streamable HTTP initialize-style POST, without `Host`, which
/// each test adds (or omits) itself.
pub fn post_headers(body_len: usize) -> Vec<(String, String)> {
    vec![
        ("Accept".into(), ACCEPT.into()),
        ("Authorization".into(), format!("Bearer {TOKEN}")),
        ("Content-Type".into(), "application/json".into()),
        ("Content-Length".into(), body_len.to_string()),
    ]
}

/// Serialize one request: the request line, `Host` (unless `host` is `None`), the given header
/// fields in order, and `Connection: close`.
pub fn request_bytes(
    method: &str,
    path: &str,
    host: Option<&str>,
    headers: &[(String, String)],
    body: &[u8],
) -> Vec<u8> {
    let mut text = format!("{method} {path} HTTP/1.1\r\n");
    if let Some(host) = host {
        text.push_str(&format!("Host: {host}\r\n"));
    }
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str("Connection: close\r\n\r\n");
    let mut bytes = text.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// A valid JSON-RPC `tools/list` body with the given id.
pub fn tools_list_body(id: u32) -> Vec<u8> {
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"tools/list\",\"params\":{{}}}}")
        .into_bytes()
}

/// A valid JSON-RPC `initialize` body (protocol 2025-11-25) with the given id.
pub fn initialize_body(id: u32) -> Vec<u8> {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"initialize\",\"params\":{{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{{}},\"clientInfo\":{{\"name\":\"test\",\"version\":\"0\"}}}}}}"
    )
    .into_bytes()
}

/// Send raw request bytes from a blocking thread, so the listener's runtime keeps running.
pub async fn exchange(port: u16, bytes: Vec<u8>) -> Reply {
    tokio::task::spawn_blocking(move || send(port, &bytes))
        .await
        .unwrap_or_else(|error| panic!("client task failed: {error}"))
}

/// The fixed body of every guard refusal.
pub const REFUSAL: &str = "MCP HTTP request refused";

/// Whether a reply is a guard refusal rather than an answer from the MCP service.
pub fn is_refusal(reply: &Reply) -> bool {
    reply.text() == REFUSAL
}

/// Send one request on a fresh connection and read the reply until the server closes it.
pub fn send(port: u16, bytes: &[u8]) -> Reply {
    let mut stream = connect(port);
    stream
        .write_all(bytes)
        .unwrap_or_else(|error| panic!("write failed: {error}"));
    read_reply(&mut stream)
}

/// Read and parse one reply, to end of stream.
pub fn read_reply(stream: &mut TcpStream) -> Reply {
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    parse_reply(&raw)
}

/// Parse a raw HTTP/1.1 response; a chunked body is decoded.
pub fn parse_reply(raw: &[u8]) -> Reply {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or(raw.len());
    let head = String::from_utf8_lossy(raw.get(..split).unwrap_or_default()).into_owned();
    let rest = raw.get(split + 4..).unwrap_or_default();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    let chunked = headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("transfer-encoding") && value.eq_ignore_ascii_case("chunked")
    });
    let body = if chunked {
        decode_chunked(rest)
    } else {
        rest.to_vec()
    };
    Reply {
        status,
        headers,
        body,
    }
}

fn decode_chunked(mut rest: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(end) = rest.windows(2).position(|window| window == b"\r\n") else {
            return out;
        };
        let size_text = String::from_utf8_lossy(rest.get(..end).unwrap_or_default()).into_owned();
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("0").trim(), 16)
            .unwrap_or(0);
        if size == 0 {
            return out;
        }
        let start = end + 2;
        let Some(chunk) = rest.get(start..start + size) else {
            return out;
        };
        out.extend_from_slice(chunk);
        rest = rest.get(start + size + 2..).unwrap_or_default();
    }
}

/// Shut both halves of a raw connection down (used to simulate a client that leaves).
pub fn hang_up(stream: TcpStream) {
    let _ = stream.shutdown(Shutdown::Both);
}
