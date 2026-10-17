//! The loopback listener: admission, the body read, the MCP service and shutdown.
//!
//! Built over the rmcp Streamable HTTP service. Connections are served by hyper's HTTP/1 server; every request
//! passes the [`Guard`] before the service sees it.
//!
//! Replies: the service answers every POST as SSE; a POST whose SSE stream holds exactly one
//! JSON-RPC response is sent as `application/json` (`json_response`).

use std::borrow::Cow;
use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use http::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, WWW_AUTHENTICATE};
use http::request::Parts;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode};
use http_body::{Body, Frame, SizeHint};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::{AbortHandle, JoinSet};
use tower_service::Service;

use super::config::LoopbackHttpConfig;
use super::guard::{Guard, MAX_BODY_BYTES};
use crate::server::JobniacMcpServer;
use crate::stdin_admission::admit_changed;

/// Requests admitted at once; more get 503 immediately, never a queue.
pub const MAX_CONCURRENT_REQUESTS: usize = 16;
/// Stateful sessions open at once (`max_sessions`); an opening POST beyond it gets 503.
pub const MAX_SESSIONS: usize = 16;
/// Connections open at once. There is no protocol limit; this documented cap bounds the
/// descriptors and tasks a local process can hold. A connection beyond it takes the place of the
/// oldest connection that has no request in progress (a silent one, or an idle keep-alive one);
/// when every connection is busy it is closed at once.
pub const MAX_CONNECTIONS: usize = 64;
/// Time a connection gets to send a request head, and the idle time of a keep-alive connection
/// before it is closed. This is uvicorn's default keep-alive timeout (5 seconds).
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(5);
/// Total time allowed for a POST body to arrive.
pub const BODY_DEADLINE: Duration = Duration::from_secs(20);
/// Time open connections get to finish after shutdown starts, before they are aborted.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// Body of every refusal. It never echoes request content.
const REFUSAL_BODY: &[u8] = b"MCP HTTP request refused";
/// Body of the session-capacity refusal: the JSON-RPC error the SDK sends.
const SESSION_CAPACITY_BODY: &[u8] =
    br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"Too many open sessions"}}"#;
/// Body of the refusal of a session-less request that is not an `initialize`: the JSON-RPC error
/// the SDK sends.
const MISSING_SESSION_BODY: &[u8] = br#"{"jsonrpc":"2.0","id":"server-error","error":{"code":-32600,"message":"Bad Request: Missing session ID"}}"#;
/// Body of the answer to a POST body that is not valid JSON: the JSON-RPC parse error.
const PARSE_ERROR_BODY: &[u8] =
    br#"{"jsonrpc":"2.0","id":"server-error","error":{"code":-32700,"message":"Parse error"}}"#;
/// The SDK's text for an unsupported `MCP-Protocol-Version`, without the header value it echoes.
const UNSUPPORTED_VERSION_TEXT: &[u8] = b"Bad Request: Unsupported MCP-Protocol-Version";
/// The `x-content-type-options` header of every response.
const X_CONTENT_TYPE_OPTIONS: HeaderName = HeaderName::from_static("x-content-type-options");
/// The MCP session header.
const MCP_SESSION_ID: HeaderName = HeaderName::from_static("mcp-session-id");

/// Failure of the loopback listener. The text never names an address, a setting or a token.
#[derive(Debug, thiserror::Error)]
pub enum HttpServeError {
    /// The listener could not be bound or accepted on.
    #[error("the loopback HTTP listener could not run")]
    Listener(#[from] io::Error),
    /// The listener is not bound to 127.0.0.1 on the configured port.
    #[error("the loopback HTTP listener is not on 127.0.0.1")]
    NotLoopback,
}

/// Bind `127.0.0.1:<port>` and serve `server` on `/mcp` until `shutdown` resolves.
///
/// This is the only listener of the product (`jobniac-cli http`). It binds loopback only.
pub async fn serve_loopback<F>(
    server: JobniacMcpServer,
    config: LoopbackHttpConfig,
    shutdown: F,
) -> Result<(), HttpServeError>
where
    F: Future<Output = ()>,
{
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.port())).await?;
    serve_loopback_on(server, config, listener, shutdown).await
}

/// Serve `server` on a listener the caller already bound (the test seam: tests bind port 0 and
/// pass the resulting port in the configuration).
///
/// The listener must be bound to `127.0.0.1:<config port>`; any other address is refused.
pub async fn serve_loopback_on<F>(
    server: JobniacMcpServer,
    config: LoopbackHttpConfig,
    listener: TcpListener,
    shutdown: F,
) -> Result<(), HttpServeError>
where
    F: Future<Output = ()>,
{
    let local = listener.local_addr()?;
    if local != SocketAddr::from((Ipv4Addr::LOCALHOST, config.port())) {
        return Err(HttpServeError::NotLoopback);
    }
    let shared = Arc::new(Shared::new(server, &config));
    let connection_slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let (stop_tx, stop_rx) = watch::channel(false);
    let mut connections = JoinSet::new();
    // Open connections, oldest first, with the count of requests each has in progress.
    let mut tracked: Vec<Tracked> = Vec::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    tracked.retain(|connection| !connection.abort.is_finished());
                    let slot = match Arc::clone(&connection_slots).try_acquire_owned() {
                        Ok(slot) => Some(slot),
                        Err(_) => evict_idle(&mut tracked, &connection_slots).await,
                    };
                    match slot {
                        Some(slot) => {
                            let activity = Arc::new(AtomicUsize::new(0));
                            let abort = connections.spawn(serve_connection(
                                stream,
                                peer,
                                local,
                                Arc::clone(&shared),
                                stop_rx.clone(),
                                slot,
                                Arc::clone(&activity),
                            ));
                            tracked.push(Tracked { activity, abort });
                        }
                        // Every connection is busy: the socket is closed before any byte is read.
                        None => drop(stream),
                    }
                }
                // Accept failures (for example descriptor exhaustion) are retried after a pause.
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    // Lifecycle: stop admitting, cancel the SDK sessions, then drain.
    shared.running.store(false, Ordering::SeqCst);
    shared.cancel_sessions();
    drop(listener);
    // Let the idle and in-flight connections finish their current exchange.
    let _ = stop_tx.send(true);
    let drain = async { while connections.join_next().await.is_some() {} };
    if tokio::time::timeout(SHUTDOWN_GRACE, drain).await.is_err() {
        connections.abort_all();
    }
    Ok(())
}

/// One open connection: its in-progress request count and the handle that ends it.
struct Tracked {
    activity: Arc<AtomicUsize>,
    abort: AbortHandle,
}

/// Free a connection slot at the cap by ending the oldest connection with no request in
/// progress. A silent connection holding a slot must not lock real clients out, and a client
/// that is idle between requests is cheap to reconnect. Returns `None` when every connection is
/// busy.
async fn evict_idle(
    tracked: &mut Vec<Tracked>,
    slots: &Arc<Semaphore>,
) -> Option<OwnedSemaphorePermit> {
    let position = tracked
        .iter()
        .position(|connection| connection.activity.load(Ordering::SeqCst) == 0)?;
    tracked.remove(position).abort.abort();
    // The slot returns when the aborted task is dropped, which is immediate in practice.
    tokio::time::timeout(Duration::from_secs(1), Arc::clone(slots).acquire_owned())
        .await
        .ok()?
        .ok()
}

/// Counts one request in progress on a connection while it lives.
struct Activity(Arc<AtomicUsize>);

impl Activity {
    fn begin(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(counter))
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// State shared by every connection of one listener.
struct Shared {
    running: AtomicBool,
    admission: Arc<Semaphore>,
    /// Serialises session-less POSTs, so that the capacity check and the session they open are
    /// one step: two openings cannot both see a free slot.
    session_gate: Mutex<()>,
    sessions: Arc<LocalSessionManager>,
    guard: Guard,
    service: StreamableHttpService<JobniacMcpServer, LocalSessionManager>,
}

impl Shared {
    fn new(server: JobniacMcpServer, config: &LoopbackHttpConfig) -> Self {
        let host = format!("127.0.0.1:{}", config.port());
        let http_config = StreamableHttpServerConfig::default()
            .with_json_response(true)
            .with_allowed_hosts([host.clone()])
            .with_allowed_origins([format!("http://{host}")])
            .with_max_request_body_bytes(MAX_BODY_BYTES);
        let sessions = Arc::new(LocalSessionManager::default());
        let service = StreamableHttpService::new(
            move || Ok(server.clone()),
            Arc::clone(&sessions),
            http_config,
        );
        Self {
            running: AtomicBool::new(true),
            admission: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
            session_gate: Mutex::new(()),
            sessions,
            guard: Guard::new(config.port(), config.token_digest()),
            service,
        }
    }

    fn cancel_sessions(&self) {
        self.service.config.cancellation_token.cancel();
    }

    /// One request, answered with a body that keeps the request counted as in progress and holds
    /// its admission permit (if it took one) until the response has been sent.
    async fn handle(
        &self,
        request: Request<Incoming>,
        peer: SocketAddr,
        local: SocketAddr,
        activity: &Arc<AtomicUsize>,
    ) -> Response<BoxBody<Bytes, Infallible>> {
        let active = Activity::begin(activity);
        let mut permit = None;
        let response = self.respond(request, peer, local, &mut permit).await;
        let (parts, body) = response.into_parts();
        Response::from_parts(
            parts,
            BoxBody::new(HeldBody {
                inner: body,
                _permit: permit,
                _active: active,
            }),
        )
    }

    /// One request: guard, admission, body, session capacity, service, then the private headers.
    async fn respond(
        &self,
        request: Request<Incoming>,
        peer: SocketAddr,
        local: SocketAddr,
        held: &mut Option<OwnedSemaphorePermit>,
    ) -> Response<BoxBody<Bytes, Infallible>> {
        let (mut parts, body) = request.into_parts();
        if let Err(status) = self.guard.check(&parts, peer, local) {
            return refusal(status);
        }
        if !self.running.load(Ordering::SeqCst) {
            return refusal(StatusCode::SERVICE_UNAVAILABLE);
        }
        let is_post = parts.method == Method::POST;
        let is_delete = parts.method == Method::DELETE;
        // The admission gate bounds the work of POST and DELETE. A GET opens the server-to-client
        // stream, which lasts as long as its session (at most `MAX_SESSIONS` of them), so it never
        // holds a permit.
        if parts.method != Method::GET {
            let Ok(permit) = Arc::clone(&self.admission).try_acquire_owned() else {
                return refusal(StatusCode::SERVICE_UNAVAILABLE);
            };
            *held = Some(permit);
        }
        let bytes = if is_post {
            match read_body(body, &parts).await {
                Ok(bytes) => admit_body(&mut parts, bytes),
                Err(status) => return refusal(status),
            }
        } else {
            Bytes::new()
        };
        let has_session = parts.headers.contains_key(&MCP_SESSION_ID);
        // Only a session opens with an `initialize`. A session-less request that carries its own
        // supported protocol version would otherwise be served statelessly by the SDK: no
        // session, no capacity check, and the gate below held for the whole tool run.
        if is_post && !has_session && is_stateless_call(&bytes) {
            return missing_session_refusal();
        }
        // A POST without a session id opens a session: it is refused while the sessions are full.
        let opening = is_post && !has_session;
        let _gate = if opening {
            let gate = self.session_gate.lock().await;
            if self.sessions.sessions.read().await.len() >= MAX_SESSIONS {
                return session_capacity_refusal();
            }
            Some(gate)
        } else {
            None
        };
        let mut service = self.service.clone();
        let response = match service
            .call(Request::from_parts(parts, Full::new(bytes)))
            .await
        {
            Ok(response) => response,
            Err(never) => match never {},
        };
        let response = scrub_sdk_error(response).await;
        let response = if is_post {
            single_json_reply(response).await
        } else {
            response
        };
        let (mut parts, body) = response.into_parts();
        // The service answers a session DELETE with 202, not the SDK's 200.
        if is_delete && parts.status == StatusCode::ACCEPTED {
            parts.status = StatusCode::OK;
        }
        set_private_headers(&mut parts.headers);
        Response::from_parts(parts, body)
    }
}

/// Serve one accepted connection until it ends or the listener shuts down.
///
/// The head timeout is the connection's whole patience: it bounds a silent or trickling client
/// and an idle keep-alive connection alike (hyper restarts it while it waits for each head).
async fn serve_connection(
    stream: TcpStream,
    peer: SocketAddr,
    local: SocketAddr,
    shared: Arc<Shared>,
    mut stop: watch::Receiver<bool>,
    slot: OwnedSemaphorePermit,
    activity: Arc<AtomicUsize>,
) {
    // The connection slot is held for the whole connection.
    let _slot = slot;
    let service = service_fn(move |request: Request<Incoming>| {
        let shared = Arc::clone(&shared);
        let activity = Arc::clone(&activity);
        async move { Ok::<_, Infallible>(shared.handle(request, peer, local, &activity).await) }
    });
    let mut builder = http1::Builder::new();
    // No `Date` header is sent.
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(HEAD_TIMEOUT)
        .auto_date_header(false);
    let connection = builder.serve_connection(TokioIo::new(stream), service);
    tokio::pin!(connection);
    tokio::select! {
        // Connection errors (a client that hangs up, malformed framing, a timed-out head) end the
        // connection and are not logged: their text could carry request content.
        _ = connection.as_mut() => {}
        _ = stop.changed() => {
            connection.as_mut().graceful_shutdown();
            let _ = connection.as_mut().await;
        }
    }
}

/// Read a POST body under the total deadline, within the size limit, and check its length
/// against the declared Content-Length. A client that goes away mid-body gets no answer.
async fn read_body(body: Incoming, parts: &Parts) -> Result<Bytes, StatusCode> {
    let collect = async {
        let mut body = body;
        let mut buffer = BytesMut::new();
        while let Some(frame) = body.frame().await {
            // A read error is a disconnect: the peer is gone, so the answer is never seen.
            let frame = frame.map_err(|_| StatusCode::BAD_REQUEST)?;
            if let Ok(data) = frame.into_data() {
                if buffer.len() + data.len() > MAX_BODY_BYTES {
                    return Err(StatusCode::PAYLOAD_TOO_LARGE);
                }
                buffer.extend_from_slice(&data);
            }
        }
        Ok(buffer.freeze())
    };
    let bytes = match tokio::time::timeout(BODY_DEADLINE, collect).await {
        Ok(result) => result?,
        Err(_) => return Err(StatusCode::REQUEST_TIMEOUT),
    };
    let declared = parts
        .headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.parse::<usize>().ok());
    if declared.is_some_and(|length| length != bytes.len()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(bytes)
}

/// Turn a single-event SSE answer to a POST into a plain `application/json` answer.
///
/// The service frames every POST answer as SSE. When the stream holds exactly one complete
/// JSON-RPC response (a `result` or an `error`, with no `method`) the answer is that JSON. Any
/// other stream (a notification before the response, several events, a malformed event) is sent
/// unchanged, as the bytes the service wrote.
async fn single_json_reply(
    response: Response<BoxBody<Bytes, Infallible>>,
) -> Response<BoxBody<Bytes, Infallible>> {
    let is_sse = response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"text/event-stream"));
    if !is_sse {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(never) => match never {},
    };
    match single_json_message(&bytes) {
        Some(json) => {
            parts
                .headers
                .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            parts.headers.remove(CONTENT_LENGTH);
            Response::from_parts(parts, Full::new(json).boxed())
        }
        None => Response::from_parts(parts, Full::new(bytes).boxed()),
    }
}

/// The payload of an SSE stream that holds exactly one complete event carrying one JSON-RPC
/// response. Comments, events without data and `event`/`id`/`retry` fields are ignored.
fn single_json_message(stream: &Bytes) -> Option<Bytes> {
    let text = std::str::from_utf8(stream).ok()?;
    let mut messages: Vec<Cow<'_, str>> = Vec::new();
    let mut data: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            // An event whose data is empty (the SDK's priming event) carries no message.
            let payload = match data.as_slice() {
                [only] => Cow::Borrowed(*only),
                _ => Cow::Owned(data.join("\n")),
            };
            if !payload.is_empty() {
                messages.push(payload);
            }
            data.clear();
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    // An event that never ends is not a complete answer.
    if !data.is_empty() || messages.len() != 1 {
        return None;
    }
    let payload = messages.pop()?;
    let shape: Shape = serde_json::from_str(&payload).ok()?;
    if !shape.is_response() {
        return None;
    }
    Some(match payload {
        // The common single-line answer is a view into the collected body, not a copy.
        Cow::Borrowed(text) => stream.slice_ref(text.as_bytes()),
        Cow::Owned(text) => Bytes::from(text),
    })
}

/// The top-level shape of a JSON-RPC message, read without building a value tree. Keys count by
/// presence (a `null` value is present) and a repeated key keeps its last value, as a parsed
/// JSON object does.
#[derive(Default)]
struct Shape {
    jsonrpc_is_2_0: bool,
    has_method: bool,
    has_result: bool,
    has_error: bool,
}

impl Shape {
    /// A response: `jsonrpc` is `"2.0"`, no `method`, and a `result` or an `error`.
    fn is_response(&self) -> bool {
        self.jsonrpc_is_2_0 && !self.has_method && (self.has_result || self.has_error)
    }
}

impl<'de> serde::Deserialize<'de> for Shape {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ShapeVisitor;

        impl<'de> serde::de::Visitor<'de> for ShapeVisitor {
            type Value = Shape;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Shape, A::Error> {
                let mut shape = Shape::default();
                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    match key.as_ref() {
                        "jsonrpc" => shape.jsonrpc_is_2_0 = map.next_value::<IsVersion>()?.0,
                        "method" => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                            shape.has_method = true;
                        }
                        "result" => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                            shape.has_result = true;
                        }
                        "error" => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                            shape.has_error = true;
                        }
                        _ => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(shape)
            }
        }

        deserializer.deserialize_map(ShapeVisitor)
    }
}

/// A value that is the string `"2.0"`; any other value, of any type, reads as `false`.
struct IsVersion(bool);

impl<'de> serde::Deserialize<'de> for IsVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct VersionVisitor;

        impl<'de> serde::de::Visitor<'de> for VersionVisitor {
            type Value = IsVersion;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("any JSON value")
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<IsVersion, E> {
                Ok(IsVersion(text == "2.0"))
            }

            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<IsVersion, E> {
                Ok(IsVersion(false))
            }

            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<IsVersion, E> {
                Ok(IsVersion(false))
            }

            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<IsVersion, E> {
                Ok(IsVersion(false))
            }

            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<IsVersion, E> {
                Ok(IsVersion(false))
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<IsVersion, E> {
                Ok(IsVersion(false))
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<IsVersion, E> {
                Ok(IsVersion(false))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<IsVersion, A::Error> {
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(IsVersion(false))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<IsVersion, A::Error> {
                while map
                    .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                    .is_some()
                {}
                Ok(IsVersion(false))
            }
        }

        deserializer.deserialize_any(VersionVisitor)
    }
}

/// Run a POST body through the admission of the stdin transport, so that both transports
/// treat a lone surrogate escape, nesting past the limit and a client-supplied rejection marker
/// alike. A body that is not UTF-8 or needs no repair is passed on as it is.
fn admit_body(parts: &mut Parts, bytes: Bytes) -> Bytes {
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return bytes;
    };
    match admit_changed(text) {
        Some(admitted) => {
            let repaired = Bytes::from(admitted.text);
            if parts.headers.contains_key(CONTENT_LENGTH) {
                parts
                    .headers
                    .insert(CONTENT_LENGTH, HeaderValue::from(repaired.len()));
            }
            repaired
        }
        None => bytes,
    }
}

/// Whether a session-less POST body is a request that carries its own supported protocol
/// version in `params._meta` (a stateless call), whatever its method but `initialize`. A body
/// that does not parse is left to the SDK, which answers it.
fn is_stateless_call(body: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct Probe {
        method: Option<String>,
        params: Option<Params>,
    }
    #[derive(serde::Deserialize)]
    struct Params {
        #[serde(rename = "_meta")]
        meta: Option<Meta>,
    }
    #[derive(serde::Deserialize)]
    struct Meta {
        #[serde(rename = "io.modelcontextprotocol/protocolVersion")]
        version: Option<String>,
    }
    // The 2026-07-28 stateless protocol is answered by the SDK itself as unsupported.
    const UNSUPPORTED_MODERN: &str = "2026-07-28";
    let Some(first) = body.iter().find(|byte| !byte.is_ascii_whitespace()) else {
        return false;
    };
    if *first != b'{' {
        return false;
    }
    // The SDK reads a body with last-wins semantics for a repeated key. The strict structs below
    // reject a repeated key, so a JSON object they cannot read is refused (fail closed) rather
    // than left to the SDK, which could see a different `method` or `_meta` than the probe.
    if serde_json::from_slice::<serde_json::Value>(body).is_err() {
        return false;
    }
    match serde_json::from_slice::<Probe>(body) {
        Ok(probe) => {
            probe.method.as_deref() != Some("initialize")
                && probe
                    .params
                    .and_then(|params| params.meta)
                    .and_then(|meta| meta.version)
                    .is_some_and(|version| version != UNSUPPORTED_MODERN)
        }
        Err(_) => true,
    }
}

/// Replace the SDK's own text errors that echo the request or its internals.
///
/// A malformed body is answered 400 with the JSON-RPC parse error (the SDK says 415 with the
/// parser's message, which quotes the body), an internal error is the fixed refusal, and the
/// unsupported-version text loses the header value it echoes. Every other answer, JSON-RPC
/// errors included, passes unchanged.
async fn scrub_sdk_error(
    response: Response<BoxBody<Bytes, Infallible>>,
) -> Response<BoxBody<Bytes, Infallible>> {
    let status = response.status();
    if response.headers().contains_key(CONTENT_TYPE) {
        return response;
    }
    match status {
        StatusCode::INTERNAL_SERVER_ERROR => refusal(status),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => {
            let mut reply = Response::new(Full::new(Bytes::from_static(PARSE_ERROR_BODY)).boxed());
            *reply.status_mut() = StatusCode::BAD_REQUEST;
            reply
                .headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            reply
        }
        StatusCode::BAD_REQUEST => {
            let (mut parts, body) = response.into_parts();
            let bytes = match body.collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(never) => match never {},
            };
            let bytes = if bytes.starts_with(UNSUPPORTED_VERSION_TEXT) {
                Bytes::from_static(UNSUPPORTED_VERSION_TEXT)
            } else {
                bytes
            };
            parts.headers.remove(CONTENT_LENGTH);
            Response::from_parts(parts, Full::new(bytes).boxed())
        }
        _ => response,
    }
}

/// The private headers of every response: no caching, no content sniffing.
fn set_private_headers(headers: &mut HeaderMap) {
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
}

/// A refusal: status, `text/plain` and the fixed body, with the private headers.
fn refusal(status: StatusCode) -> Response<BoxBody<Bytes, Infallible>> {
    let mut response = Response::new(Full::new(Bytes::from_static(REFUSAL_BODY)).boxed());
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    set_private_headers(headers);
    if status == StatusCode::UNAUTHORIZED {
        headers.insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    }
    response
}

/// The 503 of an opening POST while the sessions are full: the SDK's JSON-RPC error.
fn session_capacity_refusal() -> Response<BoxBody<Bytes, Infallible>> {
    let mut response = Response::new(Full::new(Bytes::from_static(SESSION_CAPACITY_BODY)).boxed());
    *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    set_private_headers(headers);
    response
}

/// The 400 of a session-less request that is not an `initialize`: the SDK's JSON-RPC error.
fn missing_session_refusal() -> Response<BoxBody<Bytes, Infallible>> {
    let mut response = Response::new(Full::new(Bytes::from_static(MISSING_SESSION_BODY)).boxed());
    *response.status_mut() = StatusCode::BAD_REQUEST;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    set_private_headers(headers);
    response
}

/// A response body that keeps its request counted as in progress, and holds its admission
/// permit if it has one, until it is dropped.
struct HeldBody {
    inner: BoxBody<Bytes, Infallible>,
    _permit: Option<OwnedSemaphorePermit>,
    _active: Activity,
}

impl Body for HeldBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        Pin::new(&mut this.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(stream: &str) -> Option<Bytes> {
        single_json_message(&Bytes::from(stream.to_owned()))
    }

    #[test]
    fn a_single_complete_response_event_is_unwrapped() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        assert_eq!(
            one(&format!("data: {body}\n\n")),
            Some(Bytes::from(body.to_owned()))
        );
        // The priming event without data does not count as a message.
        assert_eq!(
            one(&format!("id: 0\ndata:\n\nid: 1\ndata: {body}\n\n")),
            Some(Bytes::from(body.to_owned()))
        );
    }

    #[test]
    fn anything_but_one_complete_response_is_left_as_a_stream() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        let request = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        assert_eq!(one(&format!("data: {response}\n")), None, "unfinished");
        assert_eq!(
            one(&format!("data: {response}\n\ndata: {response}\n\n")),
            None
        );
        assert_eq!(one(&format!("data: {request}\n\n")), None);
        assert_eq!(one(": comment\n\n"), None);
    }

    #[test]
    fn a_multi_line_event_is_joined_before_the_check() {
        let stream = Bytes::from("data: {\"jsonrpc\":\"2.0\",\ndata: \"result\":{}}\n\n");
        assert_eq!(
            single_json_message(&stream),
            Some(Bytes::from_static(b"{\"jsonrpc\":\"2.0\",\n\"result\":{}}"))
        );
    }

    #[test]
    fn a_stateless_call_needs_a_supported_inline_version() {
        let call = |method: &str, version: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{{"_meta":{{"io.modelcontextprotocol/protocolVersion":"{version}"}}}}}}"#
            )
        };
        assert!(is_stateless_call(
            call("tools/call", "2025-11-25").as_bytes()
        ));
        assert!(!is_stateless_call(
            call("initialize", "2025-11-25").as_bytes()
        ));
        assert!(!is_stateless_call(
            call("tools/call", "2026-07-28").as_bytes()
        ));
        assert!(!is_stateless_call(
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#
        ));
        // A repeated key is read last-wins by the SDK, so the probe refuses what it cannot read.
        assert!(is_stateless_call(
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"_meta":{},"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-11-25"}}}"#
        ));
        assert!(is_stateless_call(
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"x","io.modelcontextprotocol/protocolVersion":"2025-11-25"}}}"#
        ));
        assert!(is_stateless_call(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","method":"tools/call"}"#
        ));
        assert!(!is_stateless_call(b"not json"));
        assert!(!is_stateless_call(b"[1]"));
        assert!(!is_stateless_call(b"  "));
    }
}
