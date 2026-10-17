//! The fixed-host Arbeitnow transport, run in the `fetch-arbeitnow` child.
//!
//! The host is fixed (`www.arbeitnow.com`, port 443) and resolved once through the core pinned
//! resolver. The child returns the raw response: status, header pairs and body. The header
//! framing and the declared length are admitted here by the core rules with the board body bound
//! ([`MAX_BODY_BYTES`], also the bound of the raw header guard), and the body length is checked.
//! The board mapping (`admit_response`, `parse_page`) and the rate-limit metadata belong to the
//! Arbeitnow board port and are applied by its caller, not here.
//!
//! The request is the page number and the visa filter only; the host, path and headers are fixed.
//! Redirects are never followed, and a non-200 status is returned with an empty body.

use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request, header};
use http_body_util::Empty;
use jobniac_core::job_links::html::admit_response_headers_with_limits;
use jobniac_core::outbound::USER_AGENT;
use tokio::time::Instant;

use super::FetchEnvironment;
use super::connect::{self, ConnectError};
use super::io::{self, BodyError};
use super::target::Target;
use super::wire;
use crate::workers::child::{WorkerFault, write_response};
use crate::workers::kinds::FETCH_ARBEITNOW_IPC_BYTES;
use crate::workers::sandbox::{self, SandboxProfile};

/// The fixed provider host.
pub const HOST: &str = "www.arbeitnow.com";

/// The total deadline of one board request.
pub const OPERATION: Duration = Duration::from_secs(10);

/// The largest decoded body (four mebibytes).
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// The largest raw header block.
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// The largest page number.
const MAX_PAGE: i64 = 2_147_483_647;

/// The category of a transport failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardCategory {
    /// A transport or framing failure.
    Transport,
    /// The body is over the bound.
    BodySize,
    /// The request was not accepted before any network use.
    HttpStatus,
}

/// A failed board request. It never carries a header, body or address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardFailure {
    /// The category.
    pub category: BoardCategory,
    /// Whether a later attempt may succeed.
    pub retryable: bool,
}

/// The raw answer of the provider: status, header pairs in wire order and the body. `Debug` prints
/// only the status and the sizes: the headers and the body are provider text and stay out of logs.
#[derive(Clone, PartialEq, Eq)]
pub struct BoardResponse {
    /// The HTTP status code.
    pub status: u16,
    /// The header name and value pairs, names as the parser lowercased them.
    pub headers: Vec<(Vec<u8>, Vec<u8>)>,
    /// The body; empty unless the status is 200.
    pub body: Vec<u8>,
}

impl std::fmt::Debug for BoardResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoardResponse")
            .field("status", &self.status)
            .field("headers", &self.headers.len())
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// Checks the page and visa filter and returns the page number.
///
/// # Errors
///
/// Returns an `HttpStatus` failure, not retryable, for a page outside `1..=2147483647`.
pub fn validate_request(page: i64) -> Result<u32, BoardFailure> {
    if (1..=MAX_PAGE).contains(&page) {
        u32::try_from(page).map_err(|_| invalid_request())
    } else {
        Err(invalid_request())
    }
}

const fn invalid_request() -> BoardFailure {
    BoardFailure {
        category: BoardCategory::HttpStatus,
        retryable: false,
    }
}

const fn transport(retryable: bool) -> BoardFailure {
    BoardFailure {
        category: BoardCategory::Transport,
        retryable,
    }
}

/// The request target of the board API for a validated page.
#[must_use]
pub fn request_target(page: u32, visa: Option<bool>) -> String {
    let mut target = format!("/api/job-board-api?page={page}");
    match visa {
        None => {}
        Some(true) => target.push_str("&visa_sponsorship=true"),
        Some(false) => target.push_str("&visa_sponsorship=false"),
    }
    target
}

/// The child operation of `fetch-arbeitnow`: the request is the page and the visa byte; the
/// response is the [`wire`] board frame.
///
/// # Errors
///
/// Returns [`WorkerFault`] when the response cannot be written, or when the child cannot confine
/// itself (it then refuses to fetch).
pub fn handle(request: &[u8], output: &mut dyn std::io::Write) -> Result<(), WorkerFault> {
    let outcome = run_in_child(wire::decode_board_request(request))?;
    write_response(
        output,
        &wire::encode_board(&outcome, FETCH_ARBEITNOW_IPC_BYTES as usize),
    )
}

/// Reads the system configuration, confines the child (no file system access, IPv4 and IPv6 sockets
/// only) on every path, a refused request included, then fetches. `None` is an undecodable
/// request. The outer error is a failed confinement.
fn run_in_child(
    request: Option<(u32, Option<bool>)>,
) -> Result<Result<BoardResponse, BoardFailure>, WorkerFault> {
    let env = FetchEnvironment::production().ok();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok();
    sandbox::apply(SandboxProfile::Fetch)?;
    let Some((page, visa)) = request else {
        return Ok(Err(transport(true)));
    };
    let page = match validate_request(i64::from(page)) {
        Ok(page) => page,
        Err(failure) => return Ok(Err(failure)),
    };
    let (Some(env), Some(runtime)) = (env, runtime) else {
        return Ok(Err(transport(true)));
    };
    Ok(runtime.block_on(fetch_page(&env, page, visa)))
}

/// Fetches one page with the given environment. Test seam only: the child builds the production
/// environment itself, and this function is not compiled into production builds.
///
/// # Errors
///
/// Returns the transport failure of the first refused or failed step.
#[cfg(any(test, feature = "test-hooks"))]
pub async fn fetch(
    env: &FetchEnvironment,
    page: i64,
    visa: Option<bool>,
) -> Result<BoardResponse, BoardFailure> {
    let page = validate_request(page)?;
    fetch_page(env, page, visa).await
}

async fn fetch_page(
    env: &FetchEnvironment,
    page: u32,
    visa: Option<bool>,
) -> Result<BoardResponse, BoardFailure> {
    let deadline = Instant::now() + OPERATION;
    match tokio::time::timeout_at(deadline, fetch_once(env, page, visa, deadline)).await {
        Ok(result) => result,
        Err(_) => Err(transport(true)),
    }
}

async fn fetch_once(
    env: &FetchEnvironment,
    page: u32,
    visa: Option<bool>,
    deadline: Instant,
) -> Result<BoardResponse, BoardFailure> {
    let target = Target {
        https: true,
        host: HOST.to_owned(),
        literal: None,
        port: 443,
        request_target: request_target(page, visa),
        host_header: HOST.to_owned(),
    };
    let body_limit = u64::try_from(MAX_BODY_BYTES).unwrap_or(u64::MAX);
    let stream = connect::open(env, &target, body_limit)
        .await
        .map_err(|error| match error {
            ConnectError::Unsafe | ConnectError::Unavailable => transport(true),
        })?;
    let request = Request::builder()
        .method(Method::GET)
        .uri(target.request_target.as_str())
        .header(header::HOST, HOST)
        .header(header::ACCEPT, "application/json")
        .header(header::USER_AGENT, USER_AGENT)
        .header(header::ACCEPT_ENCODING, "identity")
        .header(header::CONNECTION, "close")
        .body(Empty::<Bytes>::new())
        .map_err(|_| transport(true))?;
    let response = connect::send(stream, request)
        .await
        .map_err(|_| transport(true))?;

    let status = response.status().as_u16();
    let headers: Vec<(Vec<u8>, Vec<u8>)> = response
        .headers()
        .iter()
        .map(|(name, value)| (name.as_str().as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect();
    let header_bytes: usize = headers.iter().map(|(n, v)| n.len() + v.len() + 4).sum();
    if header_bytes > MAX_HEADER_BYTES {
        return Err(transport(true));
    }
    if status != 200 {
        return Ok(BoardResponse {
            status,
            headers,
            body: Vec::new(),
        });
    }

    let pairs: Vec<(&[u8], &[u8])> = headers
        .iter()
        .map(|(name, value)| (name.as_slice(), value.as_slice()))
        .collect();
    // The Arbeitnow transport compares the header size with the limit as it is (the job page
    // fetcher adds 16 bytes).
    let declared =
        admit_response_headers_with_limits(&pairs, body_limit, 0).map_err(|_| transport(true))?;
    // Data and trailers share the 4 MiB budget. An oversize body is a body-size failure; a trailer
    // section that does not fit, or any framing failure, is a retryable transport failure.
    let bytes = io::read_body(response.into_body(), MAX_BODY_BYTES, deadline)
        .await
        .map_err(|error| match error {
            BodyError::Oversize => BoardFailure {
                category: BoardCategory::BodySize,
                retryable: false,
            },
            BodyError::Framing => transport(true),
        })?;
    if declared.is_some_and(|length| length != bytes.len() as u64) {
        return Err(transport(true));
    }
    Ok(BoardResponse {
        status,
        headers,
        body: bytes,
    })
}
