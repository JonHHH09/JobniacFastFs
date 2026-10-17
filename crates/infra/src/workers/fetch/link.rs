//! The job-page fetch, run in the `fetch-link` child.
//!
//! The URL must pass the core [`wire_url`] check before any name or socket is touched. The GET is
//! single-shot: a 3xx answer is never followed. Its Location is only checked (a bad target is
//! classified as the target's own failure), and the answer is then `REDIRECT_NOT_FOLLOWED`. Other statuses keep their body, bounded to 2 MiB and checked against
//! the declared length.

use std::io::Write;
use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request, header};
use http_body_util::Empty;
use jobniac_core::job_links::html::{MAX_BODY_BYTES, admit_response_headers, parse_job_html};
use jobniac_core::job_links::urls::wire_url;
use jobniac_core::job_links::{JobLinkFetchFailure, JobLinkFetchResult};
use jobniac_core::outbound::USER_AGENT;
use tokio::time::Instant;

use super::FetchEnvironment;
use super::connect::{self, ConnectError};
use super::io;
use super::target;
use super::wire;
use crate::workers::child::{WorkerFault, write_response};
use crate::workers::kinds::FETCH_LINK_IPC_BYTES;
use crate::workers::sandbox::{self, SandboxProfile};

/// The total deadline of one link fetch.
pub const OPERATION: Duration = Duration::from_secs(20);

/// The child operation of `fetch-link`: the URL is the request body, and the response is the
/// [`wire`] link frame. Every failure is still a response frame.
///
/// # Errors
///
/// Returns [`WorkerFault`] when the response cannot be written, or when the child cannot confine
/// itself (it then refuses to fetch).
pub fn handle(request: &[u8], output: &mut dyn Write) -> Result<(), WorkerFault> {
    let outcome = run_in_child(std::str::from_utf8(request).ok())?;
    write_response(
        output,
        &wire::encode_link(&outcome, FETCH_LINK_IPC_BYTES as usize),
    )
}

/// Runs one fetch on a fresh single-threaded runtime with the production environment.
///
/// The system resolver configuration and the trust store are read first. The child then confines
/// itself (no file system access, IPv4 and IPv6 sockets only) on every path, a refused URL
/// included, and before it opens any socket, so no byte from the network is handled unconfined.
/// `None` is a body that is not UTF-8. The outer error is a failed confinement.
fn run_in_child(
    url: Option<&str>,
) -> Result<Result<JobLinkFetchResult, JobLinkFetchFailure>, WorkerFault> {
    let env = FetchEnvironment::production().ok();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok();
    sandbox::apply(SandboxProfile::Fetch)?;
    let Some(url) = url else {
        return Ok(Err(JobLinkFetchFailure::InvalidUrl));
    };
    let wire = match wire_url(url) {
        Ok(wire) => wire,
        Err(failure) => return Ok(Err(failure)),
    };
    let (Some(env), Some(runtime)) = (env, runtime) else {
        return Ok(Err(JobLinkFetchFailure::Unavailable));
    };
    Ok(runtime.block_on(fetch_wire(&env, &wire, OPERATION)))
}

/// Fetches one retrieval URL with the given environment. Test seam only: the child always passes
/// the production environment, and this function is not compiled into production builds.
///
/// # Errors
///
/// Returns the classified failure of the first check or hop that refused.
#[cfg(any(test, feature = "test-hooks"))]
pub async fn fetch(
    env: &FetchEnvironment,
    retrieval_url: &str,
) -> Result<JobLinkFetchResult, JobLinkFetchFailure> {
    fetch_within(env, retrieval_url, OPERATION).await
}

/// [`fetch`] with an explicit total deadline. Test seam only (see [`fetch`]); the tests use a
/// shorter deadline to exercise the total timeout.
///
/// # Errors
///
/// Returns the classified failure of the first check or hop that refused.
#[cfg(any(test, feature = "test-hooks"))]
pub async fn fetch_within(
    env: &FetchEnvironment,
    retrieval_url: &str,
    operation: Duration,
) -> Result<JobLinkFetchResult, JobLinkFetchFailure> {
    let wire = wire_url(retrieval_url)?;
    fetch_wire(env, &wire, operation).await
}

async fn fetch_wire(
    env: &FetchEnvironment,
    wire: &str,
    operation: Duration,
) -> Result<JobLinkFetchResult, JobLinkFetchFailure> {
    let deadline = Instant::now() + operation;
    match tokio::time::timeout_at(deadline, fetch_once(env, wire, deadline)).await {
        Ok(result) => result,
        Err(_) => Err(JobLinkFetchFailure::Unavailable),
    }
}

async fn fetch_once(
    env: &FetchEnvironment,
    wire: &str,
    deadline: Instant,
) -> Result<JobLinkFetchResult, JobLinkFetchFailure> {
    let target = target::split(wire).ok_or(JobLinkFetchFailure::Unavailable)?;
    let stream =
        connect::open(env, &target, MAX_BODY_BYTES)
            .await
            .map_err(|error| match error {
                ConnectError::Unsafe => JobLinkFetchFailure::UnsafeAddress,
                ConnectError::Unavailable => JobLinkFetchFailure::Unavailable,
            })?;
    let request = Request::builder()
        .method(Method::GET)
        .uri(target.request_target.as_str())
        .header(header::HOST, target.host_header.as_str())
        .header(header::USER_AGENT, USER_AGENT)
        .header(header::ACCEPT, "*/*")
        .header(header::ACCEPT_ENCODING, "identity")
        .header(header::CONNECTION, "close")
        .body(Empty::<Bytes>::new())
        .map_err(|_| JobLinkFetchFailure::Unavailable)?;
    let response = connect::send(stream, request)
        .await
        .map_err(|_| JobLinkFetchFailure::Unavailable)?;

    let status = response.status().as_u16();
    if (300..=399).contains(&status) {
        if let Some(location) = response.headers().get(header::LOCATION) {
            let text = location
                .to_str()
                .map_err(|_| JobLinkFetchFailure::InvalidUrl)?;
            let base = url::Url::parse(wire).map_err(|_| JobLinkFetchFailure::InvalidUrl)?;
            let joined = base
                .join(text)
                .map_err(|_| JobLinkFetchFailure::InvalidUrl)?;
            wire_url(joined.as_str())?;
        }
        return Err(JobLinkFetchFailure::RedirectNotFollowed);
    }

    let pairs: Vec<(&[u8], &[u8])> = response
        .headers()
        .iter()
        .map(|(name, value)| (name.as_str().as_bytes(), value.as_bytes()))
        .collect();
    let declared = admit_response_headers(&pairs)?;

    let body_limit = usize::try_from(MAX_BODY_BYTES).unwrap_or(usize::MAX);
    // Data and trailers share the 2 MiB budget; any refusal is the generic unavailable failure.
    let bytes = io::read_body(response.into_body(), body_limit, deadline)
        .await
        .map_err(|_| JobLinkFetchFailure::Unavailable)?;
    if declared.is_some_and(|length| length != bytes.len() as u64) {
        return Err(JobLinkFetchFailure::Unavailable);
    }
    let result = parse_job_html(&bytes, i64::from(status))?;
    if Instant::now() >= deadline {
        return Err(JobLinkFetchFailure::Unavailable);
    }
    Ok(result)
}
