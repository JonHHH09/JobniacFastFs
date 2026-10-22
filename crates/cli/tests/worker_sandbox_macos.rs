//! Seatbelt confinement of worker children on macOS, proven with real processes of the built CLI
//! binary.
//!
//! The probe kind applies a sandbox profile to itself and then tries one forbidden or allowed
//! action, as in `worker_sandbox.rs` for Linux. The probe is test-only, so no production path
//! reaches it. Seatbelt refuses with `EPERM` where Landlock answers `EACCES`.

#![cfg(target_os = "macos")]

use std::error::Error;
use std::ffi::OsString;
use std::future::pending;
use std::path::PathBuf;

use jobniac_infra::workers::fetch::wire;
use jobniac_infra::workers::sandbox::{SandboxLevel, host_level};
use jobniac_infra::workers::{WorkerKind, WorkerSupervisor};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

const EPERM: i64 = 1;
const ECONNREFUSED: i64 = 61;

/// Every profile of the probe.
const PROFILES: [&str; 3] = ["pdf-extract", "pdf-render", "fetch"];

fn supervisor() -> WorkerSupervisor {
    let environment: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    WorkerSupervisor::new(
        PathBuf::from(env!("CARGO_BIN_EXE_jobniac-cli")),
        environment,
    )
}

/// Runs one sandboxed probe action and returns its `{level, confined, ok, errno}` report.
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

#[tokio::test]
async fn the_child_reports_the_seatbelt_level_and_passes_the_tripwire() -> TestResult {
    assert_eq!(host_level(), SandboxLevel::Seatbelt);
    for profile in PROFILES {
        let report = attempt(profile, "level", None).await?;
        assert_eq!(report["ok"], json!(true), "{profile}");
        assert_eq!(report["level"], json!("seatbelt"), "{profile}");
        assert_eq!(report["confined"], json!(true), "{profile}");
    }
    Ok(())
}

#[tokio::test]
async fn files_outside_the_allowed_set_are_denied() -> TestResult {
    let directory = tempfile::tempdir()?;
    let created = directory.path().join("created");
    for profile in PROFILES {
        for path in ["/etc/hosts", "/usr/bin/true"] {
            let report = attempt(profile, "open", Some(path)).await?;
            assert_eq!(report["ok"], json!(false), "{profile} {path}");
            assert_eq!(report["errno"], json!(EPERM), "{profile} {path}");
        }
        let report = attempt(profile, "create", created.to_str()).await?;
        assert_eq!(report["ok"], json!(false), "{profile}");
        assert_eq!(report["errno"], json!(EPERM), "{profile}");
        let report = attempt(profile, "list_dir", directory.path().to_str()).await?;
        assert_eq!(report["errno"], json!(EPERM), "{profile}");
    }
    assert!(!created.exists());
    Ok(())
}

#[tokio::test]
async fn only_the_render_profile_reads_the_font_directories() -> TestResult {
    let font_directory = "/System/Library/Fonts";
    for directory in [font_directory, "/Library/Fonts"] {
        let report = attempt("pdf-render", "list_dir", Some(directory)).await?;
        assert_eq!(report["ok"], json!(true), "{directory}");
    }
    for profile in ["pdf-extract", "fetch"] {
        let report = attempt(profile, "list_dir", Some(font_directory)).await?;
        assert_eq!(report["errno"], json!(EPERM), "{profile}");
    }
    // The home directory is never readable, not even by the render profile.
    let home = std::env::var("HOME")?;
    let report = attempt("pdf-render", "list_dir", Some(&home)).await?;
    assert_eq!(report["errno"], json!(EPERM));
    Ok(())
}

#[tokio::test]
async fn a_file_held_before_the_sandbox_stays_readable() -> TestResult {
    for profile in PROFILES {
        let report = attempt(profile, "held_file", Some("/etc/hosts")).await?;
        assert_eq!(report["ok"], json!(true), "{profile}");
    }
    Ok(())
}

#[tokio::test]
async fn pdf_children_cannot_connect_bind_or_send() -> TestResult {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?.to_string();
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("probe.sock");
    for profile in ["pdf-extract", "pdf-render"] {
        for (action, argument) in [
            ("tcp_connect", address.as_str()),
            ("tcp_bind", "127.0.0.1:0"),
            ("udp_bind", "127.0.0.1:0"),
            // Binds first, so this proves that the bind is refused; outbound traffic is proven
            // refused by `tcp_connect`.
            ("udp_send", "127.0.0.1:53"),
            ("unix_socket", socket.to_str().ok_or("path")?),
        ] {
            let report = attempt(profile, action, Some(argument)).await?;
            assert_eq!(report["ok"], json!(false), "{profile} {action}");
            assert_eq!(report["errno"], json!(EPERM), "{profile} {action}");
        }
    }
    assert!(!socket.exists());
    Ok(())
}

#[tokio::test]
async fn the_fetch_child_connects_out_and_nothing_else() -> TestResult {
    // A listener of this test: the child may connect to it, because production admission of
    // addresses is the job of the fetch guard, not of the profile.
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?.to_string();
    let report = attempt("fetch", "tcp_connect", Some(&address)).await?;
    assert_eq!(report["ok"], json!(true));
    drop(listener);
    // Nothing listens on the discard port: the connect reached the network stack and was refused.
    let report = attempt("fetch", "tcp_connect", Some("127.0.0.1:9")).await?;
    assert_eq!(report["errno"], json!(ECONNREFUSED));
    // The resolver may send a datagram to port 53, and no other port.
    let report = attempt("fetch", "udp_send", Some("127.0.0.1:53")).await?;
    assert_eq!(report["ok"], json!(true));
    let report = attempt("fetch", "udp_send", Some("127.0.0.1:9999")).await?;
    assert_eq!(report["errno"], json!(EPERM));
    // No listening sockets.
    let report = attempt("fetch", "tcp_bind", Some("127.0.0.1:0")).await?;
    assert_eq!(report["errno"], json!(EPERM));
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("probe.sock");
    let report = attempt("fetch", "unix_socket", socket.to_str()).await?;
    assert_eq!(report["errno"], json!(EPERM));
    assert!(!socket.exists());
    Ok(())
}

#[tokio::test]
async fn no_child_can_start_a_program() -> TestResult {
    for profile in PROFILES {
        let report = attempt(profile, "exec", Some("/usr/bin/true")).await?;
        assert_eq!(report["ok"], json!(false), "{profile}");
        assert_eq!(report["errno"], json!(EPERM), "{profile}");
    }
    Ok(())
}

#[tokio::test]
async fn threads_still_start_after_the_sandbox() -> TestResult {
    for profile in PROFILES {
        let report = attempt(profile, "thread", None).await?;
        assert_eq!(report["ok"], json!(true), "{profile}");
    }
    Ok(())
}

/// The probe opens a file (a descriptor above stderr), then closes everything above stderr as a
/// child does at its start, and reports whether the file's descriptor was open before and after.
#[tokio::test]
async fn a_leaked_descriptor_is_closed_at_the_start_of_a_child() -> TestResult {
    let body = supervisor()
        .run(WorkerKind::Probe, br#"{"op":"close_fds"}"#, pending())
        .await?;
    let report: Value = serde_json::from_slice(&body)?;
    assert_eq!(report["before"], json!(true));
    assert_eq!(report["after"], json!(false));
    Ok(())
}

#[tokio::test]
async fn no_descriptor_above_stderr_crosses_into_the_child() -> TestResult {
    for profile in PROFILES {
        let report = attempt(profile, "level", None).await?;
        assert_eq!(report["descriptors"], json!([]), "{profile}");
    }
    Ok(())
}

#[tokio::test]
async fn an_unknown_profile_is_a_failed_child() -> TestResult {
    let request = json!({ "op": "sandbox", "profile": "none", "action": "level" }).to_string();
    let result = supervisor()
        .run(WorkerKind::Probe, request.as_bytes(), pending())
        .await;
    assert!(result.is_err());
    Ok(())
}

/// A real HTTPS fetch from inside the Fetch profile: DNS over UDP (bind, send and receive), TCP and
/// TLS against the system trust store. It needs the network, so it only runs when asked for:
/// `cargo test -p jobniac-cli --test worker_sandbox_macos -- --ignored`.
#[tokio::test]
#[ignore = "needs network access"]
async fn a_real_https_fetch_works_under_the_fetch_profile() -> TestResult {
    let body = supervisor()
        .run(WorkerKind::FetchLink, b"https://example.com/", pending())
        .await?;
    let result = wire::decode_link(&body).map_err(|_| "malformed link frame")?;
    let page = result.map_err(|failure| format!("fetch refused: {failure:?}"))?;
    assert_eq!(page.http_status, 200);
    Ok(())
}
