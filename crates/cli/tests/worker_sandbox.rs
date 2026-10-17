//! Confinement of worker children proven with
//! real processes of the built CLI binary.
//!
//! The probe kind applies a sandbox profile to itself and then tries one forbidden or allowed
//! action. Landlock needs kernel 5.13 or later; on a host without it the file system assertions
//! are skipped with a note, and the seccomp assertions still run. The probe is test-only, so no
//! production path reaches it.

#![cfg(target_os = "linux")]

use std::error::Error;
use std::ffi::OsString;
use std::future::pending;
use std::path::PathBuf;

use jobniac_infra::workers::sandbox::{SandboxLevel, host_level};
use jobniac_infra::workers::{WorkerKind, WorkerSupervisor};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

const EPERM: i64 = 1;
const EACCES: i64 = 13;
const ECONNREFUSED: i64 = 111;

/// Every profile of the probe.
const PROFILES: [&str; 3] = ["pdf-extract", "pdf-render", "fetch"];

fn supervisor() -> WorkerSupervisor {
    let environment: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    WorkerSupervisor::new(
        PathBuf::from(env!("CARGO_BIN_EXE_jobniac-cli")),
        environment,
    )
}

/// Runs one sandboxed probe action and returns its `{level, ok, errno}` report.
async fn attempt(
    profile: &str,
    action: &str,
    argument: Option<&str>,
) -> Result<Value, Box<dyn Error>> {
    let mut request = json!({ "op": "sandbox", "profile": profile, "action": action });
    if let Some(argument) = argument {
        request["path"] = json!(argument);
    }
    let body = supervisor()
        .run(WorkerKind::Probe, request.to_string().as_bytes(), pending())
        .await?;
    Ok(serde_json::from_slice(&body)?)
}

/// Whether the host has any Landlock; without it only the seccomp layer is asserted.
fn has_landlock() -> bool {
    let landlock = host_level() != SandboxLevel::SeccompOnly;
    if !landlock {
        eprintln!("skipped: this kernel has no usable Landlock; asserting seccomp only");
    }
    landlock
}

#[tokio::test]
async fn the_child_reports_the_level_the_host_allows() -> TestResult {
    for profile in PROFILES {
        let report = attempt(profile, "level", None).await?;
        eprintln!("{profile}: sandbox level {}", report["level"]);
        assert_eq!(report["ok"], json!(true), "{profile}");
        assert_eq!(report["level"], json!(host_level().name()), "{profile}");
    }
    Ok(())
}

#[tokio::test]
async fn files_outside_the_allowed_set_are_denied() -> TestResult {
    if !has_landlock() {
        return Ok(());
    }
    for profile in PROFILES {
        let report = attempt(profile, "open", Some("/etc/passwd")).await?;
        assert_eq!(report["ok"], json!(false), "{profile}");
        assert_eq!(report["errno"], json!(EACCES), "{profile}");
        let report = attempt(profile, "create", Some("/tmp/jobniac-sandbox-probe")).await?;
        assert_eq!(report["errno"], json!(EACCES), "{profile}");
    }
    assert!(!std::path::Path::new("/tmp/jobniac-sandbox-probe").exists());
    Ok(())
}

#[tokio::test]
async fn a_file_held_before_the_sandbox_stays_readable() -> TestResult {
    // The same file that `open` is refused after the sandbox: the confinement removes paths, not
    // the descriptors that the child already holds.
    for profile in PROFILES {
        let report = attempt(profile, "held_file", Some("/etc/passwd")).await?;
        assert_eq!(report["ok"], json!(true), "{profile}");
    }
    Ok(())
}

#[tokio::test]
async fn pdf_children_cannot_create_a_socket_or_connect() -> TestResult {
    for profile in ["pdf-extract", "pdf-render"] {
        for (action, argument) in [
            ("tcp_connect", "127.0.0.1:9"),
            ("tcp_bind", "127.0.0.1:0"),
            ("udp_bind", "127.0.0.1:0"),
            ("unix_socket", "/tmp/jobniac-sandbox-probe.sock"),
        ] {
            let report = attempt(profile, action, Some(argument)).await?;
            assert_eq!(report["ok"], json!(false), "{profile} {action}");
            assert_eq!(report["errno"], json!(EPERM), "{profile} {action}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn fetch_children_keep_ip_sockets_and_lose_the_rest() -> TestResult {
    // Port 9 on the loopback refuses: the socket call and the connect were allowed and reached the
    // network stack.
    let report = attempt("fetch", "tcp_connect", Some("127.0.0.1:9")).await?;
    assert_eq!(report["errno"], json!(ECONNREFUSED));
    let report = attempt("fetch", "udp_bind", Some("127.0.0.1:0")).await?;
    assert_eq!(report["ok"], json!(true));
    let report = attempt(
        "fetch",
        "unix_socket",
        Some("/tmp/jobniac-sandbox-probe.sock"),
    )
    .await?;
    assert_eq!(report["errno"], json!(EPERM));
    assert!(!std::path::Path::new("/tmp/jobniac-sandbox-probe.sock").exists());
    Ok(())
}

#[tokio::test]
async fn no_child_can_start_a_program() -> TestResult {
    for profile in PROFILES {
        let report = attempt(profile, "exec", Some("/bin/true")).await?;
        assert_eq!(report["ok"], json!(false), "{profile}");
        assert_eq!(report["errno"], json!(EPERM), "{profile}");
    }
    Ok(())
}

#[tokio::test]
async fn threads_still_start_after_the_sandbox() -> TestResult {
    // `clone3` answers ENOSYS, so the C library falls back to `clone` for `pthread_create`.
    for profile in PROFILES {
        let report = attempt(profile, "thread", None).await?;
        assert_eq!(report["ok"], json!(true), "{profile}");
    }
    Ok(())
}

#[tokio::test]
async fn an_unknown_profile_or_action_is_a_failed_child() -> TestResult {
    let request = json!({ "op": "sandbox", "profile": "none", "action": "level" }).to_string();
    let result = supervisor()
        .run(WorkerKind::Probe, request.as_bytes(), pending())
        .await;
    assert!(result.is_err());
    Ok(())
}

/// The result of a raw system call by name, through the probe.
async fn raw(profile: &str, name: &str) -> Result<Value, Box<dyn Error>> {
    attempt(profile, "raw", Some(name)).await
}

#[tokio::test]
async fn signals_reach_only_the_child_itself() -> TestResult {
    // Refused on every Landlock ABI: seccomp matches the target pid, so this does not depend on
    // the ABI v6 signal scope, which cannot stop `kill(-1, ..)` or `kill(0, ..)` anyway.
    for profile in PROFILES {
        for name in [
            "kill_parent",
            "kill_all",
            "kill_group",
            "tgkill_parent",
            "pidfd_send_signal",
        ] {
            let report = raw(profile, name).await?;
            assert_eq!(report["ok"], json!(false), "{profile} {name}");
            assert_eq!(report["errno"], json!(EPERM), "{profile} {name}");
        }
        for name in ["kill_self", "tgkill_self"] {
            let report = raw(profile, name).await?;
            assert_eq!(report["ok"], json!(true), "{profile} {name}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn the_x32_system_call_table_is_refused() -> TestResult {
    for profile in PROFILES {
        let report = raw(profile, "x32_execve").await?;
        assert_eq!(report["errno"], json!(EPERM), "{profile}");
    }
    Ok(())
}

#[tokio::test]
async fn namespaces_ipc_and_personality_changes_are_refused() -> TestResult {
    for profile in PROFILES {
        for name in ["clone_newns", "clone_newuser", "shmget", "personality_set"] {
            let report = raw(profile, name).await?;
            assert_eq!(report["errno"], json!(EPERM), "{profile} {name}");
        }
        // A personality query stays possible.
        let report = raw(profile, "personality_query").await?;
        assert_eq!(report["ok"], json!(true), "{profile}");
    }
    Ok(())
}

#[tokio::test]
async fn system_fonts_are_readable_only_by_the_render_profile() -> TestResult {
    if !has_landlock() {
        return Ok(());
    }
    let Some(directory) = ["/usr/share/fonts", "/usr/local/share/fonts"]
        .into_iter()
        .find(|directory| std::path::Path::new(directory).is_dir())
    else {
        eprintln!("skipped: no system font directory on this host");
        return Ok(());
    };
    let report = attempt("pdf-render", "list_dir", Some(directory)).await?;
    assert_eq!(report["ok"], json!(true), "pdf-render reads {directory}");
    for profile in ["pdf-extract", "fetch"] {
        let report = attempt(profile, "list_dir", Some(directory)).await?;
        assert_eq!(report["errno"], json!(EACCES), "{profile}");
    }
    // The grant is read-only and does not reach other paths.
    let report = attempt("pdf-render", "open", Some("/etc/passwd")).await?;
    assert_eq!(report["errno"], json!(EACCES));
    Ok(())
}

#[tokio::test]
async fn an_unprivileged_child_has_no_capabilities() -> TestResult {
    let uid_line = std::fs::read_to_string("/proc/self/status")?;
    let root = uid_line
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|rest| rest.split_whitespace().nth(1))
        == Some("0");
    for profile in PROFILES {
        let report = attempt(profile, "level", None).await?;
        let effective = report["capEff"].as_str().ok_or("no CapEff")?;
        // Root drops everything; an unprivileged process never had anything.
        assert!(effective.chars().all(|digit| digit == '0'), "{profile}");
        let _ = root;
    }
    Ok(())
}

/// Runs the built binary as a probe child that inherited descriptor 7, which the shell opens on
/// `/etc/hostname`, and returns the report of a `level` action.
fn run_with_leaked_descriptor() -> Result<Value, Box<dyn Error>> {
    use jobniac_infra::workers::protocol::{GO_BYTE, MARKER_VARIABLE, write_frame};
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"exec 7</etc/hostname; exec "$0" __worker probe"#)
        .arg(env!("CARGO_BIN_EXE_jobniac-cli"))
        .env_clear()
        .env(MARKER_VARIABLE, WorkerKind::Probe.marker_value())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let request = json!({ "op": "sandbox", "profile": "fetch", "action": "level" }).to_string();
    let mut frame = vec![GO_BYTE];
    write_frame(&mut frame, request.as_bytes())?;
    child.stdin.take().ok_or("stdin")?.write_all(&frame)?;
    let output = child.wait_with_output()?;
    assert!(output.status.success());
    Ok(serde_json::from_slice(
        output.stdout.get(4..).ok_or("short")?,
    )?)
}

#[test]
fn descriptors_above_stderr_do_not_survive_into_the_child() -> TestResult {
    let report = run_with_leaked_descriptor()?;
    assert_eq!(report["descriptors"], json!([]));
    Ok(())
}
