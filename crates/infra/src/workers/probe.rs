//! The test-only probe worker kind.
//!
//! Each operation deliberately exceeds one limit of the probe profile, so tests can prove that the
//! limit kills the child without harming the parent. The kind is compiled into every build but
//! refused unless the internal marker is set (see [`crate::workers::child`]), so no production
//! path reaches it. The probe reads only its own request and never a database, file or network.

use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};

use crate::workers::child::{WorkerFault, write_response};
use crate::workers::kinds::WorkerKind;
use crate::workers::platform;
use crate::workers::protocol::{GO_BYTE, MARKER_VARIABLE, write_frame};
#[cfg(any(test, feature = "worker-probe"))]
use crate::workers::sandbox::{self, SandboxProfile};

/// The variable that tests put in the parent's environment to prove that it never crosses.
pub const SECRET_VARIABLE: &str = "JOBNIAC_PROBE_SECRET";

/// The variable that carries a test token into a grandchild, so a test can find its pid.
pub const TOKEN_VARIABLE: &str = "JOBNIAC_PROBE_TOKEN";

/// The bytes that the `alloc` operation tries to hold: more than the 256 MiB probe limit.
const ALLOCATION_TARGET_BYTES: usize = 1024 * 1024 * 1024;

/// The size of each block that the `alloc` operation touches.
const ALLOCATION_BLOCK_BYTES: usize = 16 * 1024 * 1024;

/// The bytes that the `flood` operation writes to stdout: more than the 64 KiB probe output limit.
const FLOOD_BYTES: usize = 4 * 1024 * 1024;

/// The bytes written in each `flood` step.
const FLOOD_STEP_BYTES: usize = 64 * 1024;

/// How long the `sleep` and `hang` operations wait. It is longer than any probe wall limit.
const SLEEP_SECONDS: u64 = 60;

/// Runs one probe operation, selected by the `op` field of the request.
///
/// # Errors
///
/// Returns [`WorkerFault`] for an unknown operation, a malformed request, or a failed write. The
/// limit-exceeding operations fail or never return, which is the behaviour under test.
pub fn handle(request: &[u8], output: &mut dyn Write) -> Result<(), WorkerFault> {
    let value: Value = serde_json::from_slice(request).map_err(|_| WorkerFault)?;
    match value.get("op").and_then(Value::as_str) {
        Some("echo") => write_response(output, request),
        Some("alloc") => allocate(output),
        Some("spin") => spin(),
        Some("sleep") => {
            std::thread::sleep(Duration::from_secs(SLEEP_SECONDS));
            write_response(output, b"{}")
        }
        Some("hang") => loop {
            std::thread::sleep(Duration::from_secs(SLEEP_SECONDS));
        },
        Some("flood") => flood(output),
        Some("spawn") => spawn_hanging_child(output),
        Some("spawn-sleep") => spawn_and_sleep(request),
        Some("env_keys") => environment_keys(output),
        Some("env_secret") => secret_visibility(output),
        Some("rlimits") => report_rlimits(output),
        Some("contained") => report_containment(output),
        #[cfg(target_os = "macos")]
        Some("close_fds") => close_descriptors(output),
        Some("partial") => partial_then_fail(output),
        Some("garbage") => garbage(output),
        Some("stderr") => flood_stderr(request, output),
        #[cfg(any(test, feature = "worker-probe"))]
        Some("sandbox") => sandboxed_action(request, output),
        _ => Err(WorkerFault),
    }
}

/// Writes a complete, valid response frame and then fails. The supervisor must discard it.
fn partial_then_fail(output: &mut dyn Write) -> Result<(), WorkerFault> {
    write_response(output, b"{\"partial\":true}")?;
    Err(WorkerFault)
}

/// Writes bytes that are not one response frame, then succeeds. The supervisor must reject them.
fn garbage(output: &mut dyn Write) -> Result<(), WorkerFault> {
    output.write_all(b"not a frame")?;
    Ok(())
}

/// Writes a megabyte to stderr, which the supervisor must drain, and then echoes the request.
fn flood_stderr(request: &[u8], output: &mut dyn Write) -> Result<(), WorkerFault> {
    let block = [b'e'; 4096];
    let mut stderr = std::io::stderr().lock();
    for _ in 0..256 {
        stderr.write_all(&block)?;
    }
    stderr.flush()?;
    drop(stderr);
    write_response(output, request)
}

/// Reports the resource limits that this process reads back, as `[soft, hard]` pairs (Linux and
/// macOS; macOS has no address-space limit, so its report has no `as`).
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn report_rlimits(output: &mut dyn Write) -> Result<(), WorkerFault> {
    use rlimit::Resource;
    let read = |resource: Resource| -> Result<[u64; 2], WorkerFault> {
        let (soft, hard) = rlimit::getrlimit(resource).map_err(|_| WorkerFault)?;
        Ok([soft, hard])
    };
    // Each value is read back from the kernel, so a limit that was never applied shows up here.
    #[cfg(target_os = "linux")]
    let address_space = json!(read(Resource::AS)?);
    #[cfg(not(target_os = "linux"))]
    let address_space = Value::Null;
    let report = json!({
        "as": address_space,
        "cpu": read(Resource::CPU)?,
        "fsize": read(Resource::FSIZE)?,
        "core": read(Resource::CORE)?,
        "nofile": read(Resource::NOFILE)?,
    });
    write_response(output, report.to_string().as_bytes())
}

/// Reports that this platform applies no resource limits to the child itself (Windows).
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn report_rlimits(output: &mut dyn Write) -> Result<(), WorkerFault> {
    write_response(output, json!({ "supported": false }).to_string().as_bytes())
}

/// Opens a file, closes every descriptor above stderr as a child does at its start, and reports
/// whether the file's descriptor was open before and is open after (macOS).
#[cfg(target_os = "macos")]
fn close_descriptors(output: &mut dyn Write) -> Result<(), WorkerFault> {
    use std::os::fd::IntoRawFd;
    // The descriptor number is kept without an owner, so closing it behind the file's back cannot
    // trip the standard library's ownership check.
    let descriptor = std::fs::File::open("/dev/null")?.into_raw_fd();
    let is_open = || std::fs::metadata(format!("/dev/fd/{descriptor}")).is_ok();
    let before = is_open();
    platform::close_inherited_descriptors()?;
    let after = is_open();
    write_response(
        output,
        json!({ "before": before, "after": after })
            .to_string()
            .as_bytes(),
    )
}

/// Reports whether this process is inside its containment, as `contained` (Windows Job Object)
/// or `null` when the platform has no such query.
fn report_containment(output: &mut dyn Write) -> Result<(), WorkerFault> {
    let contained = platform::self_in_job();
    write_response(
        output,
        json!({ "contained": contained }).to_string().as_bytes(),
    )
}

/// The exit status of a probe whose allocation the memory limit refused. Distinct from the status 1
/// of every other fault, so a test can prove that the memory limit is what stopped the child.
pub const ALLOCATION_REFUSED_EXIT: i32 = 42;

/// Holds more memory than the limit allows. Every block is touched, so it is committed.
///
/// Linux: the address-space limit refuses an allocation and the probe exits with
/// [`ALLOCATION_REFUSED_EXIT`]. macOS: the probe holds the memory until the parent kills it.
#[cfg_attr(target_os = "macos", allow(unused_variables))]
fn allocate(output: &mut dyn Write) -> Result<(), WorkerFault> {
    let mut held: Vec<Vec<u8>> = Vec::new();
    let mut total = 0_usize;
    while total < ALLOCATION_TARGET_BYTES {
        let mut block: Vec<u8> = Vec::new();
        if block.try_reserve_exact(ALLOCATION_BLOCK_BYTES).is_err() {
            std::process::exit(ALLOCATION_REFUSED_EXIT);
        }
        block.resize(ALLOCATION_BLOCK_BYTES, 1);
        total += ALLOCATION_BLOCK_BYTES;
        held.push(block);
    }
    // macOS refuses no allocation: the parent polls the footprint and kills the child. The probe
    // therefore holds what it has and waits to be killed, and never reports success, so how fast
    // the host runs cannot decide the outcome.
    #[cfg(target_os = "macos")]
    loop {
        std::thread::sleep(Duration::from_secs(SLEEP_SECONDS));
    }
    #[cfg(not(target_os = "macos"))]
    write_response(
        output,
        json!({ "allocatedBytes": total }).to_string().as_bytes(),
    )
}

/// Spins on the CPU until the CPU limit ends the process.
fn spin() -> Result<(), WorkerFault> {
    let mut counter: u64 = 0;
    loop {
        counter = std::hint::black_box(counter.wrapping_add(1));
    }
}

/// Writes more stdout bytes than the output limit, with no frame at all.
fn flood(output: &mut dyn Write) -> Result<(), WorkerFault> {
    let block = vec![b'x'; FLOOD_STEP_BYTES];
    for _ in 0..(FLOOD_BYTES / FLOOD_STEP_BYTES) {
        output.write_all(&block)?;
    }
    Ok(())
}

/// Tries to start another worker child, which then waits for a request that never comes.
///
/// On Windows the job's active-process limit refuses the start, and the response says so. On
/// Linux the start succeeds, and the response carries the pid, so the caller can prove that the
/// supervisor killed the descendant with the child.
fn spawn_hanging_child(output: &mut dyn Write) -> Result<(), WorkerFault> {
    let response = match spawn_grandchild(None) {
        Ok(pid) => json!({ "spawned": true, "pid": pid }),
        // The raw OS error code tells the Job Object refusal (ERROR_NOT_ENOUGH_QUOTA, 1816 on
        // Windows) apart from any other spawn failure.
        Err(error) => json!({ "spawned": false, "osError": error.raw_os_error() }),
    };
    write_response(output, response.to_string().as_bytes())
}

/// Starts a grandchild worker that waits for a request and then hangs. A `token` is put in its
/// environment, so a test can find the process through `/proc` even after the response is lost.
/// Returns the pid of the grandchild.
fn spawn_grandchild(token: Option<&str>) -> Result<u32, std::io::Error> {
    let executable = std::env::current_exe()?;
    let mut command = std::process::Command::new(executable);
    command
        .arg("__worker")
        .arg(WorkerKind::Probe.name())
        .env_clear()
        .env(MARKER_VARIABLE, WorkerKind::Probe.marker_value())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(token) = token {
        command.env(TOKEN_VARIABLE, token);
    }
    let mut child = command.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        // The grandchild reads this request, then runs its `hang` operation.
        let mut request_bytes = vec![GO_BYTE];
        write_frame(&mut request_bytes, br#"{"op":"hang"}"#)
            .map_err(|_| std::io::Error::other("frame"))?;
        std::io::Write::write_all(&mut stdin, &request_bytes)?;
    }
    Ok(child.id())
}

/// Starts a grandchild tagged with the request's `token`, then sleeps past the wall-clock limit
/// without answering. The parent must kill the descendant when it kills the child.
fn spawn_and_sleep(request: &[u8]) -> Result<(), WorkerFault> {
    let value: Value = serde_json::from_slice(request).map_err(|_| WorkerFault)?;
    let token = value
        .get("token")
        .and_then(Value::as_str)
        .ok_or(WorkerFault)?;
    spawn_grandchild(Some(token))?;
    std::thread::sleep(Duration::from_secs(SLEEP_SECONDS));
    Ok(())
}

/// Reports the names of the variables that this process can see, sorted.
fn environment_keys(output: &mut dyn Write) -> Result<(), WorkerFault> {
    let mut keys: Vec<String> = std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .collect();
    keys.sort();
    write_response(output, json!({ "keys": keys }).to_string().as_bytes())
}

/// Reports whether the probe secret variable is visible to this process.
fn secret_visibility(output: &mut dyn Write) -> Result<(), WorkerFault> {
    let present = std::env::var_os(SECRET_VARIABLE).is_some();
    write_response(output, json!({ "present": present }).to_string().as_bytes())
}

/// Applies a sandbox profile, then tries one action and reports what the confinement answered.
///
/// The request carries `profile` (`pdf-extract`, `pdf-render` or `fetch`), `action` (`level`,
/// `open`, `create`, `tcp_connect`, `udp_send`, `tcp_bind`, `udp_bind`, `unix_socket`, `exec`,
/// `thread`, `held_file`) and, for the actions that need one, `path` or `address`. `held_file`
/// opens `path` before the sandbox and reads it after, which must keep working. The response is
/// `{level, confined, ok, errno}`: the level that the sandbox reached, whether the tripwire of
/// [`sandbox::verify_confined`] passes, whether the action succeeded, and the raw OS error code
/// when it failed.
#[cfg(any(test, feature = "worker-probe"))]
fn sandboxed_action(request: &[u8], output: &mut dyn Write) -> Result<(), WorkerFault> {
    let value: Value = serde_json::from_slice(request).map_err(|_| WorkerFault)?;
    let profile = match value.get("profile").and_then(Value::as_str) {
        Some("pdf-extract") => SandboxProfile::PdfExtract,
        Some("pdf-render") => SandboxProfile::PdfRender,
        Some("fetch") => SandboxProfile::Fetch,
        _ => return Err(WorkerFault),
    };
    let action = value
        .get("action")
        .and_then(Value::as_str)
        .ok_or(WorkerFault)?;
    let argument = value
        .get("path")
        .or_else(|| value.get("address"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let held = if action == "held_file" {
        Some(std::fs::File::open(&argument)?)
    } else {
        None
    };
    // Read before the sandbox, which removes `/proc`: the descriptors that crossed into the child,
    // and a handle on the status file that is read again after the sandbox (a held descriptor keeps
    // working).
    let descriptors = inherited_descriptors();
    let status = std::fs::File::open("/proc/self/status").ok();
    let level = sandbox::apply(profile)?;
    let outcome: Result<(), std::io::Error> = match action {
        "level" => Ok(()),
        "open" => std::fs::File::open(&argument).map(drop),
        "create" => std::fs::File::create(&argument).map(drop),
        "tcp_connect" => std::net::TcpStream::connect(&argument).map(drop),
        "udp_send" => std::net::UdpSocket::bind("0.0.0.0:0")
            .and_then(|socket| socket.send_to(b"x", &argument))
            .map(drop),
        "tcp_bind" => std::net::TcpListener::bind(&argument).map(drop),
        "udp_bind" => std::net::UdpSocket::bind(&argument).map(drop),
        "unix_socket" => unix_socket(&argument),
        "exec" => std::process::Command::new(&argument).status().map(drop),
        "thread" => std::thread::Builder::new()
            .spawn(|| ())
            .and_then(|handle| handle.join().map_err(|_| std::io::Error::other("join"))),
        "list_dir" => std::fs::read_dir(&argument).map(drop),
        "raw" => raw_syscall(&argument),
        "held_file" => held
            .ok_or_else(|| std::io::Error::other("no file"))
            .and_then(|mut file| std::io::Read::read_to_end(&mut file, &mut Vec::new()))
            .map(drop),
        _ => return Err(WorkerFault),
    };
    let response = json!({
        "level": level.name(),
        "confined": sandbox::verify_confined().is_ok(),
        "ok": outcome.is_ok(),
        "errno": outcome.err().and_then(|error| error.raw_os_error()),
        "descriptors": descriptors,
        "capEff": status.and_then(effective_capabilities),
    });
    write_response(output, response.to_string().as_bytes())
}

/// Binds a UNIX socket at `path`.
#[cfg(all(unix, any(test, feature = "worker-probe")))]
fn unix_socket(path: &str) -> Result<(), std::io::Error> {
    std::os::unix::net::UnixListener::bind(path).map(drop)
}

/// This platform has no UNIX sockets to bind.
#[cfg(all(not(unix), any(test, feature = "worker-probe")))]
fn unix_socket(_path: &str) -> Result<(), std::io::Error> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

/// The targets of the descriptors above stderr, other than the `/proc` directory handle that the
/// listing itself uses.
#[cfg(all(not(target_os = "macos"), any(test, feature = "worker-probe")))]
fn inherited_descriptors() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
                .is_some_and(|number| number > 2)
        })
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .map(|target| target.display().to_string())
        .filter(|target| !target.starts_with("/proc/"))
        .collect()
}

/// The descriptors above stderr that are open, by number. `/dev/fd/N` exists exactly for the open
/// ones, and the target of a descriptor is not read on this platform.
#[cfg(all(target_os = "macos", any(test, feature = "worker-probe")))]
fn inherited_descriptors() -> Vec<String> {
    (3..256)
        .filter(|number| std::fs::metadata(format!("/dev/fd/{number}")).is_ok())
        .map(|number| format!("fd {number}"))
        .collect()
}

/// The `CapEff` line of an open `/proc/self/status`.
#[cfg(any(test, feature = "worker-probe"))]
fn effective_capabilities(mut file: std::fs::File) -> Option<String> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut file, &mut text).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("CapEff:"))
        .map(|value| value.trim().to_owned())
}

/// A raw system call that the filters refuse, by name (Linux only).
#[cfg(all(target_os = "linux", any(test, feature = "worker-probe")))]
fn raw_syscall(name: &str) -> Result<(), std::io::Error> {
    platform::probe_syscall(name)
}

/// Other platforms have no raw calls to try.
#[cfg(all(not(target_os = "linux"), any(test, feature = "worker-probe")))]
fn raw_syscall(_name: &str) -> Result<(), std::io::Error> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}
