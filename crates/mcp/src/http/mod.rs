//! Loopback Streamable HTTP MCP: the guard and listener in front of the rmcp service for `/mcp`.
//!
//! The listener exists only when the console binary runs
//! `jobniac-cli http` with a valid port and bearer token (there is no enable flag). It binds
//! `127.0.0.1` only and serves the same tool catalog as STDIO.
//!
//! Every request passes the guard in a fixed order (peer, path, method, headers, Host,
//! Origin, Bearer token, framing, media type, admission, body, service); the first failing check
//! decides the status code. Refusals are `text/plain` with a fixed body and never echo the
//! request.

mod config;
mod guard;
mod serve;

pub use config::{HttpConfigError, LoopbackHttpConfig, MIN_PORT};
pub use guard::{MAX_BODY_BYTES, MAX_HEADER_BYTES, check_request};
pub use serve::{
    BODY_DEADLINE, HEAD_TIMEOUT, HttpServeError, MAX_CONCURRENT_REQUESTS, MAX_CONNECTIONS,
    MAX_SESSIONS, SHUTDOWN_GRACE, serve_loopback, serve_loopback_on,
};
