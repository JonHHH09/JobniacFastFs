//! Isolation of worker children, proven with real processes of the built CLI binary.
//!
//! Each probe operation exceeds one limit of the probe profile. The tests check that the child is
//! stopped by that limit, that the parent gets a sanitized category and no output, and that the
//! parent keeps running and releases its permit. No test touches a database, a file outside the
//! temporary worker process, or the network.

use std::error::Error;
use std::ffi::OsString;
use std::future::pending;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use jobniac_infra::workers::env::child_environment;
#[cfg(not(target_os = "macos"))]
use jobniac_infra::workers::probe::ALLOCATION_REFUSED_EXIT;
use jobniac_infra::workers::probe::SECRET_VARIABLE;
use jobniac_infra::workers::protocol::{GO_BYTE, MARKER_VARIABLE, write_frame};
use jobniac_infra::workers::{
    ExitClass, WorkerError, WorkerErrorKind, WorkerKind, WorkerSupervisor,
};
use serde_json::Value;

type TestResult = Result<(), Box<dyn Error>>;

/// The value of the probe secret in the parent's environment. It must never reach a child.
const SECRET_VALUE: &str = "probe-secret-value-that-must-not-cross";

/// The wall-clock limit of the probe profile, which the tests compare against.
const PROBE_WALL: Duration = Duration::from_secs(10);

/// Serializes the tests that burn CPU or memory, so that the CPU limit is measured on an otherwise
/// idle machine and cannot be starved into the wall-clock limit by the other tests.
static CPU_BOUND: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A supervisor for the built CLI binary, with a secret in the parent's environment.
fn supervisor() -> WorkerSupervisor {
    let mut environment: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    environment.push((
        OsString::from(SECRET_VARIABLE),
        OsString::from(SECRET_VALUE),
    ));
    WorkerSupervisor::new(
        PathBuf::from(env!("CARGO_BIN_EXE_jobniac-cli")),
        environment,
    )
}

/// The request body of one probe operation.
fn probe(operation: &str) -> Vec<u8> {
    format!(r#"{{"op":"{operation}"}}"#).into_bytes()
}

/// The response body of a successful probe operation, as JSON.
fn json(body: &[u8]) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_slice(body)?)
}

/// The category of a failed run.
fn kind_of(result: &Result<Vec<u8>, WorkerError>) -> Option<WorkerErrorKind> {
    result.as_ref().err().map(WorkerError::kind)
}

/// Runs a probe operation and returns its result with the time it took.
async fn run_probe(operation: &str) -> (Result<Vec<u8>, WorkerError>, Duration) {
    let started = Instant::now();
    let result = supervisor()
        .run(WorkerKind::Probe, &probe(operation), pending())
        .await;
    (result, started.elapsed())
}

#[tokio::test]
async fn echo_round_trips_through_a_real_child() -> TestResult {
    let (result, _) = run_probe("echo").await;
    assert_eq!(result?, probe("echo"));
    Ok(())
}

#[test]
fn child_refuses_to_run_without_the_marker() -> TestResult {
    let (status, stdout) = run_worker("probe", &[])?;
    assert!(!status.success());
    assert!(stdout.is_empty());
    Ok(())
}

#[test]
fn child_refuses_the_marker_of_another_kind() -> TestResult {
    let (status, stdout) = run_worker("probe", &[(MARKER_VARIABLE, "pdf-extract-v1")])?;
    assert!(!status.success());
    assert!(stdout.is_empty());
    Ok(())
}

#[test]
fn unknown_kind_is_refused() -> TestResult {
    let (status, stdout) = run_worker("no-such-kind", &[(MARKER_VARIABLE, "probe-v1")])?;
    assert!(!status.success());
    assert!(stdout.is_empty());
    Ok(())
}

#[tokio::test]
async fn pdf_kinds_have_no_operation_in_this_build() -> TestResult {
    let result = supervisor()
        .run(WorkerKind::PdfExtract, b"{}", pending())
        .await;
    assert!(matches!(
        kind_of(&result),
        Some(WorkerErrorKind::ChildFailed(_))
    ));
    Ok(())
}

#[tokio::test]
async fn child_environment_is_exactly_the_allowlist() -> TestResult {
    let mut source: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    source.push((
        OsString::from(SECRET_VARIABLE),
        OsString::from(SECRET_VALUE),
    ));
    let mut expected: Vec<String> = child_environment(&source, WorkerKind::Probe)
        .into_iter()
        .filter_map(|(name, _)| name.into_string().ok())
        .collect();
    expected.sort();

    let body = supervisor()
        .run(WorkerKind::Probe, &probe("env_keys"), pending())
        .await?;
    #[allow(unused_mut)]
    let mut keys: Vec<String> = serde_json::from_value(json(&body)?["keys"].clone())?;
    // CoreFoundation, which the binary links for the system trust store, adds this variable to the
    // environment of every process that loads it. It carries a text-encoding default, not a parent
    // value.
    #[cfg(target_os = "macos")]
    keys.retain(|key| key != "__CF_USER_TEXT_ENCODING");
    assert_eq!(keys, expected);
    assert!(!keys.iter().any(|key| key == SECRET_VARIABLE));
    Ok(())
}

#[tokio::test]
async fn secret_variable_of_the_parent_is_not_visible() -> TestResult {
    let body = supervisor()
        .run(WorkerKind::Probe, &probe("env_secret"), pending())
        .await?;
    assert_eq!(json(&body)?["present"], Value::Bool(false));
    Ok(())
}

#[tokio::test]
async fn memory_limit_stops_an_allocation_past_the_cap() -> TestResult {
    let _serial = CPU_BOUND.lock().await;
    let (result, elapsed) = run_probe("alloc").await;
    // Linux: the address-space limit refuses the allocation and the probe says so. macOS enforces
    // no such limit, so the parent's footprint watch kills the child tree (SIGKILL).
    #[cfg(not(target_os = "macos"))]
    assert_eq!(
        kind_of(&result),
        Some(WorkerErrorKind::ChildFailed(ExitClass::Code(
            ALLOCATION_REFUSED_EXIT
        ))),
        "the probe must report that its allocation was refused, not fail for another reason"
    );
    #[cfg(target_os = "macos")]
    assert_eq!(
        kind_of(&result),
        Some(WorkerErrorKind::ChildFailed(ExitClass::Signal(9))),
        "the memory watch must kill the probe before it holds its whole target"
    );
    assert!(elapsed < PROBE_WALL * 2, "took {elapsed:?}");
    Ok(())
}

#[tokio::test]
async fn cpu_limit_stops_a_spin_before_the_wall_clock() -> TestResult {
    let _serial = CPU_BOUND.lock().await;
    let (result, elapsed) = run_probe("spin").await;
    // Linux: the soft and hard CPU limits are equal, so the kernel's SIGXCPU or SIGKILL ends the
    // child. Windows: the job's per-process user-time limit terminates the process, and the exit
    // status is whatever the job reports; it must be a child failure, never a success or a timeout.
    #[cfg(unix)]
    assert!(
        matches!(
            kind_of(&result),
            Some(WorkerErrorKind::ChildFailed(ExitClass::Signal(9 | 24)))
        ),
        "a spin must end by the CPU limit's signal, got {:?}",
        kind_of(&result)
    );
    #[cfg(windows)]
    assert!(
        matches!(kind_of(&result), Some(WorkerErrorKind::ChildFailed(_))),
        "a spin must end by the CPU limit as a child failure, got {:?}",
        kind_of(&result)
    );
    assert!(elapsed < PROBE_WALL, "took {elapsed:?}");
    Ok(())
}

#[tokio::test]
async fn wall_clock_limit_kills_a_sleeping_child() -> TestResult {
    let (result, elapsed) = run_probe("sleep").await;
    assert_eq!(kind_of(&result), Some(WorkerErrorKind::TimedOut));
    assert!(elapsed >= PROBE_WALL, "returned early after {elapsed:?}");
    assert!(elapsed < PROBE_WALL * 3, "took {elapsed:?}");
    Ok(())
}

#[tokio::test]
async fn output_limit_kills_a_flooding_child() -> TestResult {
    let (result, elapsed) = run_probe("flood").await;
    assert_eq!(kind_of(&result), Some(WorkerErrorKind::OutputLimitExceeded));
    assert!(elapsed < PROBE_WALL, "took {elapsed:?}");
    Ok(())
}

/// On Windows the Job Object's active-process limit refuses the descendant, with the Job's
/// ERROR_NOT_ENOUGH_QUOTA (1816), not some other spawn failure.
#[cfg(windows)]
#[tokio::test]
async fn a_child_cannot_start_a_process_that_outlives_the_operation() -> TestResult {
    let body = supervisor()
        .run(WorkerKind::Probe, &probe("spawn"), pending())
        .await?;
    let response = json(&body)?;
    assert_eq!(response["spawned"], Value::Bool(false));
    assert_eq!(response["osError"], serde_json::json!(1816));
    Ok(())
}

/// On Unix there is no process-count limit, so the descendant starts, and the supervisor must
/// kill it with its worker.
#[cfg(unix)]
#[tokio::test]
async fn a_child_cannot_start_a_process_that_outlives_the_operation() -> TestResult {
    let body = supervisor()
        .run(WorkerKind::Probe, &probe("spawn"), pending())
        .await?;
    let response = json(&body)?;
    assert_eq!(response["spawned"], Value::Bool(true));
    let pid = response["pid"]
        .as_u64()
        .ok_or("grandchild pid is missing")?;
    assert!(
        wait_until_gone(pid, Duration::from_secs(5)),
        "descendant {pid} survived its worker"
    );
    Ok(())
}

/// A descendant that the child started is killed when the call is dropped on timeout, not only when
/// the child exits on its own (E-NET-33).
#[cfg(unix)]
#[tokio::test]
async fn a_descendant_is_killed_when_the_call_is_dropped() -> TestResult {
    let supervisor = supervisor();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let token = format!("drop-{}-{nanos}", std::process::id());
    let body = serde_json::json!({ "op": "spawn-sleep", "token": token }).to_string();
    let mut call = Box::pin(supervisor.run(WorkerKind::Probe, body.as_bytes(), pending()));
    // Find the grandchild while the call is still running: dropping the call kills it at once.
    let mut found = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while found.is_none() && Instant::now() < deadline {
        tokio::select! {
            _ = &mut call => return Err("the call must still be running".into()),
            () = tokio::time::sleep(Duration::from_millis(50)) => {
                found = scan_proc_for_token(&token);
            }
        }
    }
    let pid = found.ok_or("the grandchild was never started")?;
    drop(call);
    assert!(
        wait_until_gone(pid, Duration::from_secs(5)),
        "descendant {pid} survived the dropped call"
    );
    Ok(())
}

#[tokio::test]
async fn cancellation_kills_the_child_and_releases_its_permit() -> TestResult {
    let supervisor = supervisor();
    let started = Instant::now();
    let result = supervisor
        .run(
            WorkerKind::Probe,
            &probe("sleep"),
            tokio::time::sleep(Duration::from_millis(300)),
        )
        .await;
    assert_eq!(kind_of(&result), Some(WorkerErrorKind::Cancelled));
    assert!(
        started.elapsed() < PROBE_WALL,
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(
        supervisor.available_permits(WorkerKind::Probe),
        WorkerKind::Probe.max_concurrent()
    );
    Ok(())
}

#[tokio::test]
async fn dropping_the_call_releases_its_permit() -> TestResult {
    let supervisor = supervisor();
    let attempt = tokio::time::timeout(
        Duration::from_millis(400),
        supervisor.run(WorkerKind::Probe, &probe("sleep"), pending()),
    )
    .await;
    assert!(attempt.is_err(), "the call must still be running");
    assert_eq!(
        supervisor.available_permits(WorkerKind::Probe),
        WorkerKind::Probe.max_concurrent()
    );
    Ok(())
}

#[tokio::test]
async fn the_semaphore_bounds_concurrent_children_of_a_kind() -> TestResult {
    let supervisor = supervisor();
    let sleep_request = probe("sleep");
    let echo_request = probe("echo");
    let (first, second, third, held) = tokio::join!(
        supervisor.run(
            WorkerKind::Probe,
            &sleep_request,
            tokio::time::sleep(Duration::from_millis(1500)),
        ),
        supervisor.run(
            WorkerKind::Probe,
            &sleep_request,
            tokio::time::sleep(Duration::from_millis(1500)),
        ),
        // A third child waits for a permit and runs once the first two are cancelled.
        async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            supervisor
                .run(WorkerKind::Probe, &echo_request, pending())
                .await
        },
        // Polls until both permits are held, so a slow start of the first children cannot race
        // the observation.
        async {
            let started = Instant::now();
            while supervisor.available_permits(WorkerKind::Probe) != 0 {
                if started.elapsed() > Duration::from_secs(5) {
                    return false;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            true
        },
    );
    assert_eq!(kind_of(&first), Some(WorkerErrorKind::Cancelled));
    assert_eq!(kind_of(&second), Some(WorkerErrorKind::Cancelled));
    assert!(held, "both permits must be held while two children run");
    assert_eq!(third?, probe("echo"));
    Ok(())
}

#[tokio::test]
async fn failed_child_output_is_discarded() -> TestResult {
    // The child writes one complete frame, then exits with status 1. No partial output may escape.
    let (result, _) = run_probe("partial").await;
    assert_eq!(
        kind_of(&result),
        Some(WorkerErrorKind::ChildFailed(ExitClass::Code(1)))
    );
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn unknown_operation_fails_with_exit_status_one() -> TestResult {
    let (result, _) = run_probe("no-such-operation").await;
    assert_eq!(
        kind_of(&result),
        Some(WorkerErrorKind::ChildFailed(ExitClass::Code(1)))
    );
    Ok(())
}

#[tokio::test]
async fn malformed_stdout_is_a_protocol_violation() -> TestResult {
    let (result, _) = run_probe("garbage").await;
    assert_eq!(kind_of(&result), Some(WorkerErrorKind::ProtocolViolation));
    Ok(())
}

#[tokio::test]
async fn stderr_is_drained_and_never_blocks_the_child() -> TestResult {
    // A megabyte of stderr would fill an undrained pipe and stall the child until the deadline.
    let (result, elapsed) = run_probe("stderr").await;
    assert_eq!(result?, probe("stderr"));
    assert!(elapsed < PROBE_WALL, "took {elapsed:?}");
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn unix_child_has_its_resource_limits_read_back() -> TestResult {
    let body = supervisor()
        .run(WorkerKind::Probe, &probe("rlimits"), pending())
        .await?;
    let report = json(&body)?;
    let limits = WorkerKind::Probe.limits();
    // macOS has no address-space limit; its memory is watched by the parent.
    #[cfg(target_os = "linux")]
    assert_eq!(
        report["as"],
        serde_json::json!([limits.memory_bytes, limits.memory_bytes])
    );
    #[cfg(target_os = "macos")]
    assert_eq!(report["as"], Value::Null);
    assert_eq!(
        report["cpu"],
        serde_json::json!([limits.cpu_seconds, limits.cpu_seconds])
    );
    assert_eq!(report["fsize"], serde_json::json!([0, 0]));
    assert_eq!(report["core"], serde_json::json!([0, 0]));
    assert_eq!(
        report["nofile"],
        serde_json::json!([limits.open_files, limits.open_files])
    );
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn windows_child_is_inside_its_job_when_it_runs() -> TestResult {
    // The parent attaches the child before it writes the go byte, so the child sees its job.
    let body = supervisor()
        .run(WorkerKind::Probe, &probe("contained"), pending())
        .await?;
    assert_eq!(json(&body)?["contained"], Value::Bool(true));
    Ok(())
}

/// Starts the built binary as `__worker <kind>` with a cleared environment plus `environment`,
/// sends it a complete echo request, and returns its status and stdout.
fn run_worker(
    kind: &str,
    environment: &[(&str, &str)],
) -> Result<(std::process::ExitStatus, Vec<u8>), Box<dyn Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_jobniac-cli"));
    command
        .args(["__worker", kind])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (name, value) in environment {
        command.env(name, value);
    }
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().ok_or("child stdin is missing")?;
    let mut frame = vec![GO_BYTE];
    write_frame(&mut frame, &probe("echo"))?;
    // A child that refuses may already have exited, so a failed write is expected here.
    let _ = stdin.write_all(&frame);
    drop(stdin);
    let output = child.wait_with_output()?;
    Ok((output.status, output.stdout))
}

/// Waits until the process is gone or a zombie, for up to `limit`.
#[cfg(unix)]
fn wait_until_gone(pid: u64, limit: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < limit {
        if process_is_gone(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    process_is_gone(pid)
}

/// One pass over `/proc`: the pid of the process whose environment carries `token`, if any.
#[cfg(target_os = "linux")]
fn scan_proc_for_token(token: &str) -> Option<u64> {
    let needle = format!("{}={token}", jobniac_infra::workers::probe::TOKEN_VARIABLE);
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u64>().ok())
        else {
            continue;
        };
        let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else {
            continue;
        };
        if environ
            .split(|byte| *byte == 0)
            .any(|variable| variable == needle.as_bytes())
        {
            return Some(pid);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn scan_proc_for_token(token: &str) -> Option<u64> {
    let needle = format!("{}={token}", jobniac_infra::workers::probe::TOKEN_VARIABLE);
    // `ps -E` appends the environment of each process to its command line.
    let output = Command::new("/bin/ps")
        .args(["-axwwE", "-o", "pid=,command="])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains(&needle))
        .find_map(|line| line.split_whitespace().next()?.parse().ok())
}

#[cfg(target_os = "linux")]
fn process_is_gone(pid: u64) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => {
            stat.rsplit(')')
                .next()
                .and_then(|rest| rest.trim_start().chars().next())
                == Some('Z')
        }
    }
}

#[cfg(target_os = "macos")]
fn process_is_gone(pid: u64) -> bool {
    let output = Command::new("/bin/ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    let state = String::from_utf8_lossy(&output.stdout);
    let state = state.trim();
    state.is_empty() || state.starts_with('Z')
}
