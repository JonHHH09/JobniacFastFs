//! The fetch worker children run as real processes of the built CLI binary. No test
//! needs a server: each case is refused before a connection is made (by the URL check in the parent
//! or in the child, or by the marker check), cancelled before a connection starts, or fails name
//! resolution of a reserved `.invalid` name. Every confined child must still return its structured
//! frame, because the child checks its confinement before it answers.

use std::error::Error;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use jobniac_core::job_links::JobLinkContentFetcher;
use jobniac_core::job_links::JobLinkFetchFailure;
use jobniac_infra::workers::env::child_environment;
use jobniac_infra::workers::fetch::board::BoardCategory;
use jobniac_infra::workers::fetch::wire;
use jobniac_infra::workers::protocol::MARKER_VARIABLE;
use jobniac_infra::workers::{WorkerKind, WorkerSupervisor};
use jobniac_runtime::worker_fetch::{WorkerBoardTransport, WorkerLinkFetcher};

type TestResult = Result<(), Box<dyn Error>>;

fn supervisor() -> Arc<WorkerSupervisor> {
    let environment: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    Arc::new(WorkerSupervisor::new(
        PathBuf::from(env!("CARGO_BIN_EXE_jobniac-cli")),
        environment,
    ))
}

/// Runs one fetch-link child with a raw request and returns the decoded link outcome.
fn child_link(body: &[u8]) -> Result<Result<(), JobLinkFetchFailure>, Box<dyn Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let response = runtime.block_on(supervisor().run(
        WorkerKind::FetchLink,
        body,
        std::future::pending::<()>(),
    ))?;
    let decoded = wire::decode_link(&response).map_err(|_| "malformed link frame")?;
    Ok(decoded.map(|_| ()))
}

#[test]
fn child_refuses_to_run_without_the_marker() -> TestResult {
    let mut command = Command::new(env!("CARGO_BIN_EXE_jobniac-cli"));
    command
        .arg("__worker")
        .arg("fetch-link")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let output = command.output()?;
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "nothing is written without the marker"
    );
    Ok(())
}

#[test]
fn child_refuses_the_marker_of_another_kind() -> TestResult {
    let mut command = Command::new(env!("CARGO_BIN_EXE_jobniac-cli"));
    command
        .arg("__worker")
        .arg("fetch-arbeitnow")
        .env_clear()
        .env(MARKER_VARIABLE, WorkerKind::FetchLink.marker_value())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    assert!(!command.output()?.status.success());
    Ok(())
}

#[test]
fn child_applies_the_url_policy_before_any_socket() -> TestResult {
    // The child refuses a literal non-public address itself, without any DNS or connection.
    assert_eq!(
        child_link(b"http://127.0.0.1/private")?,
        Err(JobLinkFetchFailure::UnsafeAddress)
    );
    // A host name over plain http is refused as an IP-literal requirement.
    assert_eq!(
        child_link(b"http://example.com/")?,
        Err(JobLinkFetchFailure::IpLiteralRequired)
    );
    assert_eq!(
        child_link(b"https://[::ffff:127.0.0.1]/")?,
        Err(JobLinkFetchFailure::UnsafeAddress)
    );
    assert_eq!(
        child_link(b"https://localhost/")?,
        Err(JobLinkFetchFailure::IpLiteralRequired)
    );
    assert_eq!(
        child_link(b"ftp://example.com/")?,
        Err(JobLinkFetchFailure::InvalidUrl)
    );
    Ok(())
}

#[test]
fn parent_refuses_bad_urls_before_starting_a_child() {
    let fetcher = WorkerLinkFetcher::new(supervisor(), None);
    assert_eq!(
        fetcher.fetch("http://example.com/").err(),
        Some(JobLinkFetchFailure::IpLiteralRequired)
    );
    assert_eq!(
        fetcher.fetch("https://127.0.0.1/").err(),
        Some(JobLinkFetchFailure::UnsafeAddress)
    );
}

#[test]
fn cancellation_is_reported_as_interrupted_before_any_connection() {
    // The check is true from the start, so the child is never started and nothing connects.
    let started = Instant::now();
    let fetcher = WorkerLinkFetcher::new(supervisor(), Some(Arc::new(|| true)));
    assert_eq!(
        fetcher.fetch("https://8.8.8.8/").err(),
        Some(JobLinkFetchFailure::Interrupted)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn board_child_refuses_a_page_outside_the_range_without_network() -> TestResult {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let request = wire::encode_board_request(0, None);
    let response = runtime.block_on(supervisor().run(
        WorkerKind::FetchArbeitnow,
        &request,
        std::future::pending::<()>(),
    ))?;
    let decoded = wire::decode_board(&response).map_err(|_| "malformed board frame")?;
    let failure = decoded.err().ok_or("expected a failure")?;
    assert_eq!(failure.category, BoardCategory::HttpStatus);
    assert!(!failure.retryable);
    Ok(())
}

#[test]
fn board_transport_refuses_an_out_of_range_page_in_the_parent() {
    let transport = WorkerBoardTransport::new(supervisor());
    let failure = transport.fetch(0, None).expect_err("refused");
    assert_eq!(failure.category, BoardCategory::HttpStatus);
}

#[test]
fn child_environment_of_fetch_kinds_is_the_allowlist_only() {
    let source: Vec<(OsString, OsString)> = [
        ("PATH", "/bin"),
        ("SYSTEMROOT", "C:\\Windows"),
        ("HTTPS_PROXY", "http://proxy.example.invalid:3128"),
        ("SSL_CERT_FILE", "/etc/ssl/custom.pem"),
        ("JOBNIAC_DB_PATH", "/srv/jobniac-synthetic/jobniac.sqlite3"),
    ]
    .into_iter()
    .map(|(name, value)| (OsString::from(name), OsString::from(value)))
    .collect();
    for kind in [WorkerKind::FetchLink, WorkerKind::FetchArbeitnow] {
        let environment = child_environment(&source, kind);
        for (name, _) in &environment {
            let name = name.to_string_lossy().to_uppercase();
            let allowed = jobniac_infra::workers::env::ALLOWED_KEYS.contains(&name.as_str())
                || name == MARKER_VARIABLE
                || name == "LANG"
                || name == "LC_ALL";
            assert!(allowed, "{name} must not cross into a fetch child");
        }
        assert!(!environment.iter().any(|(name, _)| name == "HTTPS_PROXY"));
        assert!(!environment.iter().any(|(name, _)| name == "SSL_CERT_FILE"));
    }
}

/// The real child resolves a name that cannot exist and answers with a structured failure frame,
/// not a failed child: the confinement is in force and DNS and sockets still work through it.
#[test]
fn a_confined_child_answers_an_unresolvable_host_with_a_failure_frame() -> TestResult {
    let outcome = child_link(b"https://nonexistent.invalid/")?;
    assert!(outcome.is_err(), "no page can come from a .invalid name");
    Ok(())
}

/// A refused URL is answered through the confined path too.
#[test]
fn a_confined_child_answers_a_refused_url_with_its_frame() -> TestResult {
    assert_eq!(
        child_link(b"not a url")?.unwrap_err(),
        JobLinkFetchFailure::InvalidUrl
    );
    Ok(())
}
