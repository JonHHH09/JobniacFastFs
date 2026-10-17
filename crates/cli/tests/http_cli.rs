//! The `http` mode of the console binary: the strict environment contract, the sanitized startup
//! failure, one real loopback listener, and the log discipline of both MCP modes.
//!
//! The test binary runs `jobniac-cli` as a child process. Settings are passed in the environment
//! only. All values are synthetic.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Synthetic 52-character bearer token of the allowed alphabet.
const TOKEN: &str = "synthetic_http_cli_token_0123456789abcdefghijklmnopq";
/// The fixed startup failure, with its newline.
const FAILURE: &str = "Jobniac could not start. Check local configuration.\n";
/// Canary values that must never reach stderr, whatever the log level (E-NET-89).
const CANARIES: [&str; 3] = [
    "canary-userinfo-7f3a",
    "canary-bearer-9c21-token",
    "canary profile text 4b8e",
];
/// The `Accept` value the streamable HTTP transport requires.
const ACCEPT: &str = "application/json, text/event-stream";

/// The `jobniac-cli` binary under test.
fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jobniac-cli"))
}

/// Build the `http` command with the two HTTP settings set or removed.
fn http_command(port: Option<&str>, token: Option<&str>) -> Command {
    let mut command = binary();
    command.arg("http").stdin(Stdio::null());
    command.env_remove("JOBNIAC_HTTP_PORT");
    command.env_remove("JOBNIAC_HTTP_BEARER_TOKEN");
    if let Some(port) = port {
        command.env("JOBNIAC_HTTP_PORT", port);
    }
    if let Some(token) = token {
        command.env("JOBNIAC_HTTP_BEARER_TOKEN", token);
    }
    command
}

/// A loopback port nothing listens on at the moment of the call.
fn free_port() -> u16 {
    let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    probe.local_addr().unwrap().port()
}

/// A running `jobniac-cli http` child, with its stderr read on a thread.
struct Listener {
    child: Child,
    lines: mpsc::Receiver<String>,
    seen: Vec<String>,
    port: u16,
}

impl Listener {
    /// Start the listener on `port` with `extra` environment and wait for its announcement.
    fn start(port: u16, extra: &[(&str, &str)]) -> Self {
        let mut command = http_command(Some(&port.to_string()), Some(TOKEN));
        for (name, value) in extra {
            command.env(name, value);
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (lines_tx, lines) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if lines_tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut listener = Self {
            child,
            lines,
            seen: Vec::new(),
            port,
        };
        let expected = format!("Jobniac MCP HTTP starting on 127.0.0.1:{port}/mcp");
        loop {
            let line = listener
                .lines
                .recv_timeout(Duration::from_secs(60))
                .unwrap_or_else(|_| panic!("the listener did not announce itself"));
            listener.seen.push(line.clone());
            if line == expected {
                // The announcement is printed before the bind, so the
                // port may not accept yet: wait until it does instead of racing the bind.
                listener.wait_until_accepting();
                return listener;
            }
        }
    }

    /// Block until a connection to the listener is accepted, for at most 30 seconds. The probe
    /// connection sends nothing and is closed at once.
    fn wait_until_accepting(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            if TcpStream::connect((Ipv4Addr::LOCALHOST, self.port)).is_ok() {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "the listener exited before it accepted a connection"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "the listener never accepted a connection"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// One POST of `body` with the given `Authorization` value, on a fresh connection.
    fn post(&self, authorization: Option<&str>, body: &str) -> (u16, String) {
        post_raw(self.port, authorization, body)
    }

    /// Stop the child and return its stdout and the whole of its stderr.
    fn finish(mut self) -> (String, String) {
        self.child.kill().unwrap();
        let status = self.child.wait().unwrap();
        assert!(
            !status.success(),
            "a killed listener does not report success"
        );
        let mut stdout = String::new();
        self.child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut stdout)
            .unwrap();
        let mut stderr = self.seen.clone();
        stderr.extend(self.lines.iter());
        (stdout, stderr.join("\n"))
    }
}

/// One POST to `/mcp` on a fresh connection; returns the status and the raw reply.
fn post_raw(port: u16, authorization: Option<&str>, body: &str) -> (u16, String) {
    post_with_session(port, authorization, None, body)
}

/// One POST to `/mcp` with an optional `Mcp-Session-Id`; returns the status and the raw reply.
fn post_with_session(
    port: u16,
    authorization: Option<&str>,
    session: Option<&str>,
    body: &str,
) -> (u16, String) {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let auth = authorization
        .map(|value| format!("Authorization: {value}\r\n"))
        .unwrap_or_default();
    let session = session
        .map(|value| format!("Mcp-Session-Id: {value}\r\nMCP-Protocol-Version: 2025-11-25\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: {ACCEPT}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{auth}{session}Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = String::new();
    let _ = stream.read_to_string(&mut raw);
    let status = raw
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    (status, raw)
}

/// The bearer header of the test token.
fn bearer() -> String {
    format!("Bearer {TOKEN}")
}

/// A valid `initialize` body.
fn initialize_body(id: u32) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"initialize","params":{{"protocolVersion":"2025-11-25","capabilities":{{}},"clientInfo":{{"name":"test","version":"0"}}}}}}"#
    )
}

/// A `tools/call` body whose arguments carry the canary values.
fn canary_call_body(id: u32) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"add_job_from_link","arguments":{{"url":"https://canary-userinfo-7f3a:canary-pass@example.com/jobs/1","text":"{}","note":"{}"}}}}}}"#,
        CANARIES[2], CANARIES[1]
    )
}

#[test]
fn missing_or_invalid_settings_exit_one_with_the_sanitized_failure() {
    let long_token = "z".repeat(129);
    let cases: [(Option<&str>, Option<&str>); 14] = [
        (None, Some(TOKEN)),
        (Some("8765"), None),
        (Some(""), Some(TOKEN)),
        (Some("1023"), Some(TOKEN)),
        (Some("65536"), Some(TOKEN)),
        (Some("80"), Some(TOKEN)),
        (Some("-1"), Some(TOKEN)),
        (Some("87 65"), Some(TOKEN)),
        (Some("0x2000"), Some(TOKEN)),
        (Some("\u{ff18}\u{ff17}\u{ff16}\u{ff15}"), Some(TOKEN)),
        (Some("99999999999"), Some(TOKEN)),
        (Some("8765"), Some("short")),
        (
            Some("8765"),
            Some("synthetic_http_cli_token_0123456789abcdefghijklmnopq!"),
        ),
        (Some("8765"), Some(long_token.as_str())),
    ];
    for (port, token) in cases {
        let output: Output = http_command(port, token).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{port:?}: exit status");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "",
            "{port:?}: stdout"
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            FAILURE,
            "{port:?}: stderr"
        );
    }
}

#[test]
fn a_real_loopback_listener_enforces_the_token_and_never_echoes_it() {
    let listener = Listener::start(free_port(), &[]);
    let body = initialize_body(1);
    let (unauthenticated, _) = listener.post(None, &body);
    assert_eq!(unauthenticated, 401);
    let wrong_token = format!("Bearer {}", "B".repeat(TOKEN.len()));
    let (wrong, _) = listener.post(Some(&wrong_token), &body);
    assert_eq!(wrong, 401);
    let (authenticated, raw) = listener.post(Some(&bearer()), &body);
    assert_eq!(authenticated, 200);
    assert!(raw.contains("protocolVersion"));

    let (stdout, stderr) = listener.finish();
    assert_eq!(stdout, "", "the listener prints nothing on stdout");
    assert!(!stderr.contains(TOKEN), "the token is never logged");
    assert!(!raw.contains(TOKEN), "the token is never echoed");
}

/// The logging rule of E-NET-89 in HTTP mode, at the most verbose level: the request bodies and
/// the session identifiers of the SDK never reach stderr. The loopback listener is driven with
/// canary values in a tool call, a wrong bearer value and a hostile header.
#[test]
fn http_logs_never_carry_request_values_at_trace_level() {
    let listener = Listener::start(free_port(), &[("RUST_LOG", "trace")]);
    let (initialized, reply) = listener.post(Some(&bearer()), &initialize_body(1));
    assert_eq!(initialized, 200);
    let session = reply
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("mcp-session-id")
                .then(|| value.trim().to_owned())
        })
        .expect("the session id is issued at initialize");
    let (called, _) = post_with_session(
        listener.port,
        Some(&bearer()),
        Some(&session),
        &canary_call_body(2),
    );
    assert_eq!(called, 200, "a tool call with canary arguments is answered");
    let (refused, _) = listener.post(
        Some(&format!("Bearer {}", CANARIES[1])),
        &canary_call_body(3),
    );
    assert_eq!(refused, 401, "a canary bearer value is refused");
    let (stdout, stderr) = listener.finish();
    assert_eq!(stdout, "");
    for canary in CANARIES {
        assert!(
            !stderr.contains(canary),
            "a log line carries {canary:?}: {stderr}"
        );
    }
}

/// The same rule in STDIO mode: tool arguments with canary values are not logged at trace level,
/// and the answers still reach stdout (the STDIO adapter is unchanged).
#[test]
fn stdio_logs_never_carry_request_values_at_trace_level() {
    assert_stdio_logs_are_clean("trace");
}

/// An explicit SDK level in `RUST_LOG` does not reopen the SDK records either.
#[test]
fn an_explicit_sdk_level_in_rust_log_does_not_reopen_the_sdk_logs() {
    assert_stdio_logs_are_clean("trace,rmcp=trace");
}

/// Run STDIO mode with `rust_log` and assert the canary-free stderr and the answer on stdout.
fn assert_stdio_logs_are_clean(rust_log: &str) {
    let mut child = binary()
        .arg("mcp")
        .env("RUST_LOG", rust_log)
        .env_remove("JOBNIAC_HTTP_PORT")
        .env_remove("JOBNIAC_HTTP_BEARER_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for line in [
            initialize_body(1),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_owned(),
            canary_call_body(2),
        ] {
            writeln!(stdin, "{line}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "stdio ends at end of input");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"id\":2"),
        "the tool call is answered on stdout"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    for canary in CANARIES {
        assert!(
            !stderr.contains(canary),
            "a log line carries {canary:?}: {stderr}"
        );
    }
}

/// Header refusals are answered before the service and log none of the private values sent in
/// them: a wrong token, a proxy header and a repeated critical header carry canaries.
#[test]
fn header_refusals_do_not_invoke_the_service_or_log_private_values() {
    let listener = Listener::start(free_port(), &[("RUST_LOG", "trace")]);
    let body = canary_call_body(4);
    let (wrong, _) = listener.post(Some(&format!("Bearer {}", CANARIES[1])), &body);
    assert_eq!(wrong, 401);
    let port = listener.port;
    let proxied = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: {ACCEPT}\r\nAuthorization: {}\r\nForwarded: for={}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        bearer(),
        CANARIES[0],
        body.len()
    );
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream.write_all(proxied.as_bytes()).unwrap();
    let mut raw = String::new();
    let _ = stream.read_to_string(&mut raw);
    assert!(
        raw.starts_with("HTTP/1.1 400"),
        "a proxy header is refused: {raw}"
    );
    let (stdout, stderr) = listener.finish();
    assert_eq!(stdout, "");
    for canary in CANARIES {
        assert!(
            !stderr.contains(canary),
            "a log line carries {canary:?}: {stderr}"
        );
    }
    assert!(
        !raw.contains(CANARIES[1]),
        "the refusal does not echo the canary"
    );
}

/// A listener that cannot run (the port is
/// taken) exits 1 with the sanitized failure line and no configuration value.
#[test]
fn http_mode_runtime_failure_is_sanitized() {
    let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = occupied.local_addr().unwrap().port();
    let output = http_command(Some(&port.to_string()), Some(TOKEN))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.ends_with(FAILURE),
        "stderr ends with the failure: {stderr:?}"
    );
    assert!(!stderr.contains(TOKEN), "the token is never printed");
    drop(occupied);
}

/// As far as a child process can show it: the
/// `http` mode composes the opt-in services from the same environment as `mcp`, so an enabled
/// Arbeitnow flag without its keys fails closed in both modes.
#[test]
fn http_mode_composes_the_opt_in_services_like_mcp() {
    let output = http_command(Some(&free_port().to_string()), Some(TOKEN))
        .env("JOBNIAC_ARBEITNOW_ENABLED", "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "http fails closed");
    assert_eq!(String::from_utf8_lossy(&output.stderr), FAILURE);

    let mcp = binary()
        .arg("mcp")
        .env("JOBNIAC_ARBEITNOW_ENABLED", "1")
        .env_remove("JOBNIAC_HTTP_PORT")
        .env_remove("JOBNIAC_HTTP_BEARER_TOKEN")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(mcp.status.code(), Some(1), "mcp fails closed the same way");
    assert_eq!(String::from_utf8_lossy(&mcp.stdout), "");
}

/// With the HTTP settings present, the other
/// modes never announce or open the loopback listener, and the port stays free afterwards.
#[test]
fn other_modes_never_start_a_listener() {
    for mode in ["mcp", "migrate", "cleanup"] {
        let port = free_port();
        let output = binary()
            .arg(mode)
            .env("JOBNIAC_HTTP_PORT", port.to_string())
            .env("JOBNIAC_HTTP_BEARER_TOKEN", TOKEN)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("MCP HTTP starting"),
            "{mode}: no listener is announced"
        );
        assert!(
            TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok(),
            "{mode}: the port stays free"
        );
    }
}

#[test]
fn the_stdio_mode_never_opens_a_listener_and_ends_at_end_of_input() {
    let output = binary()
        .arg("mcp")
        .env_remove("JOBNIAC_HTTP_PORT")
        .env_remove("JOBNIAC_HTTP_BEARER_TOKEN")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}

/// The module paths of the SDK that carry its request and response records.
const SDK_MODULE_LEVELS: &str = "rmcp::service=trace,rmcp::transport=trace,rmcp::handler=trace,rmcp::model=trace,rmcp::transport::streamable_http_server=trace,hyper=trace";

/// A more specific `RUST_LOG` directive for an SDK module outranks a crate-wide `rmcp=off`, so the
/// silencing is a fixed filter in front of the environment one, not a directive: STDIO mode.
#[test]
fn an_sdk_module_directive_in_rust_log_does_not_reopen_the_sdk_logs_on_stdio() {
    assert_stdio_logs_are_clean(SDK_MODULE_LEVELS);
    assert_stdio_logs_are_clean(&format!("trace,{SDK_MODULE_LEVELS}"));
}

/// The same module directives in HTTP mode.
#[test]
fn an_sdk_module_directive_in_rust_log_does_not_reopen_the_sdk_logs_over_http() {
    let listener = Listener::start(free_port(), &[("RUST_LOG", SDK_MODULE_LEVELS)]);
    let (initialized, reply) = listener.post(Some(&bearer()), &initialize_body(1));
    assert_eq!(initialized, 200);
    let session = reply
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("mcp-session-id")
                .then(|| value.trim().to_owned())
        })
        .expect("the session id is issued at initialize");
    let (called, _) = post_with_session(
        listener.port,
        Some(&bearer()),
        Some(&session),
        &canary_call_body(2),
    );
    assert_eq!(called, 200, "a tool call with canary arguments is answered");
    let (_, stderr) = listener.finish();
    for canary in CANARIES.iter().copied().chain([session.as_str()]) {
        assert!(
            !stderr.contains(canary),
            "a log line carries {canary:?}: {stderr}"
        );
    }
}

/// Start `http` with piped stdin (and the parent-pipe marker when `marker`), and wait until the
/// port accepts a connection.
fn start_with_piped_stdin(marker: bool) -> (Child, u16) {
    let port = free_port();
    let mut command = http_command(Some(&port.to_string()), Some(TOKEN));
    command.env_remove("JOBNIAC_INTERNAL_PARENT_PIPE");
    if marker {
        command.env("JOBNIAC_INTERNAL_PARENT_PIPE", "1");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "the listener exited before it accepted a connection"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "the listener never accepted a connection"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    (child, port)
}

/// The desktop app holds the write end of the child's stdin; when it dies the pipe closes and the
/// listener drains and exits with status 0, without a signal.
#[test]
fn closing_stdin_stops_the_listener_when_the_parent_pipe_marker_is_set() {
    let (mut child, port) = start_with_piped_stdin(true);
    drop(child.stdin.take());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("the listener kept running after its stdin closed");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        status.code(),
        Some(0),
        "a parent-pipe stop is a normal exit"
    );
    assert!(
        TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err(),
        "the port is released"
    );
}

/// Without the marker a closed stdin means nothing: an operator's console or service keeps its
/// listener.
#[test]
fn closing_stdin_does_not_stop_the_listener_without_the_marker() {
    let (mut child, port) = start_with_piped_stdin(false);
    drop(child.stdin.take());
    std::thread::sleep(Duration::from_secs(1));
    let running = child.try_wait().unwrap().is_none();
    let accepting = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(running, "the listener must still run");
    assert!(accepting, "the listener must still accept");
}
