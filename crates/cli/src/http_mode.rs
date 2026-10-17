//! The `http` mode: the loopback Streamable HTTP MCP listener, opt-in by invocation.
//!
//! The listener has no
//! enable flag: it exists only when this mode runs with a valid port and bearer token, both read
//! from the environment (`JOBNIAC_HTTP_PORT`, `JOBNIAC_HTTP_BEARER_TOKEN`). Every problem with
//! that configuration gives the same sanitized startup failure; no setting or token is printed.

use std::io::Read;
use std::process::ExitCode;

use jobniac_core::env::Environment;
use jobniac_mcp::JobniacMcpServer;
use jobniac_mcp::http::{LoopbackHttpConfig, serve_loopback};
use tokio::sync::oneshot;

use crate::console::stderr_line;
use crate::modes::{
    build_runtime, compose_mcp_services, init_tracing, interrupt, unexpected_failure,
};

/// The sanitized failure of every startup problem.
const STARTUP_FAILURE: &str = "Jobniac could not start. Check local configuration.";
/// The only address the listener binds.
const HTTP_HOST: &str = "127.0.0.1";
/// Exit status of an interrupted run (128 + SIGINT); uvicorn re-raises the interrupt.
const INTERRUPTED: u8 = 130;

/// The internal marker set by the desktop app: exit when stdin closes. Active only for exactly `1`.
const PARENT_PIPE_VARIABLE: &str = "JOBNIAC_INTERNAL_PARENT_PIPE";

/// Whether the parent-pipe marker is exactly `1`. Not a CLI argument: the grammar is a fixed
/// surface, and an operator's console or service must not stop on a closed stdin.
fn parent_pipe_requested(env: &Environment) -> bool {
    env.get(PARENT_PIPE_VARIABLE) == Some("1")
}

/// Watch stdin on a plain thread; EOF or an error means the parent died, so ask for the stop.
fn watch_parent_pipe(stop: oneshot::Sender<()>) {
    // A failed spawn only means no watch; the listener then behaves as without the marker.
    let _ = std::thread::Builder::new()
        .name("parent-pipe".to_owned())
        .spawn(move || {
            let mut buffer = [0_u8; 256];
            let mut stdin = std::io::stdin().lock();
            while matches!(stdin.read(&mut buffer), Ok(read) if read > 0) {}
            let _ = stop.send(());
        });
}

/// Resolves when the parent-pipe watch saw stdin close; pends forever without the watch.
async fn parent_pipe_closed(watch: Option<oneshot::Receiver<()>>) {
    if let Some(receiver) = watch
        && receiver.await.is_ok()
    {
        return;
    }
    std::future::pending::<()>().await;
}

/// Read the strict loopback settings from the environment.
///
/// The port is ASCII digits, at most five characters, in `1024..=65535`. The token is 43 to 128
/// characters of `[A-Za-z0-9_-]`. A missing or non-Unicode value is invalid as well.
pub fn load_config(env: &Environment) -> Option<LoopbackHttpConfig> {
    let raw_port = env.get("JOBNIAC_HTTP_PORT").unwrap_or("");
    let token = env.get("JOBNIAC_HTTP_BEARER_TOKEN").unwrap_or("");
    let digits_only = !raw_port.is_empty()
        && raw_port.len() <= 5
        && raw_port.bytes().all(|byte| byte.is_ascii_digit());
    if !digits_only {
        return None;
    }
    let port = raw_port.parse::<u16>().ok()?;
    LoopbackHttpConfig::new(port, token).ok()
}

/// Serve the guarded MCP app on 127.0.0.1 until interrupted (Ctrl+C exits 130).
pub fn run_http() -> ExitCode {
    let env = jobniac_runtime::process_environment();
    let Some(config) = load_config(&env) else {
        stderr_line(STARTUP_FAILURE);
        return ExitCode::FAILURE;
    };
    let mcp = match compose_mcp_services(&env) {
        Ok(mcp) => mcp,
        Err(_) => {
            stderr_line(STARTUP_FAILURE);
            return ExitCode::FAILURE;
        }
    };
    init_tracing();
    let port = config.port();
    let server = match JobniacMcpServer::new(mcp) {
        Ok(server) => server,
        Err(_) => return unexpected_failure(),
    };
    let Some(runtime) = build_runtime() else {
        return unexpected_failure();
    };
    stderr_line(&format!(
        "Jobniac MCP HTTP starting on {HTTP_HOST}:{port}/mcp"
    ));
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let parent_closed = parent_pipe_requested(&env).then(|| {
        let (parent_tx, parent_rx) = oneshot::channel::<()>();
        watch_parent_pipe(parent_tx);
        parent_rx
    });
    let outcome = runtime.block_on(async {
        let serving = serve_loopback(server, config, async {
            let _ = stop_rx.await;
        });
        tokio::pin!(serving);
        tokio::select! {
            result = &mut serving => Some(result),
            () = parent_pipe_closed(parent_closed) => {
                // The app is gone: the same graceful drain, but a normal exit.
                let _ = stop_tx.send(());
                Some(serving.await)
            }
            () = interrupt() => {
                // Ctrl+C: stop admitting, drain within the grace period, then report the signal.
                let _ = stop_tx.send(());
                let _ = serving.await;
                None
            }
        }
    });
    runtime.shutdown_background();
    match outcome {
        None => ExitCode::from(INTERRUPTED),
        Some(Ok(())) => ExitCode::SUCCESS,
        Some(Err(_)) => unexpected_failure(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "synthetic_http_cli_token_0123456789abcdefghijklmnopq";

    fn env(port: Option<&str>, token: Option<&str>) -> Environment {
        let mut pairs = Vec::new();
        if let Some(port) = port {
            pairs.push(("JOBNIAC_HTTP_PORT".to_owned(), port.to_owned()));
        }
        if let Some(token) = token {
            pairs.push(("JOBNIAC_HTTP_BEARER_TOKEN".to_owned(), token.to_owned()));
        }
        Environment::from_pairs(pairs)
    }

    #[test]
    fn the_parent_pipe_marker_is_exactly_one() {
        let with = |value: Option<&str>| {
            Environment::from_pairs(
                value
                    .map(|v| vec![(PARENT_PIPE_VARIABLE.to_owned(), v.to_owned())])
                    .unwrap_or_default(),
            )
        };
        assert!(parent_pipe_requested(&with(Some("1"))));
        for other in [
            None,
            Some(""),
            Some("0"),
            Some("true"),
            Some("01"),
            Some(" 1"),
            Some("1 "),
        ] {
            assert!(!parent_pipe_requested(&with(other)), "{other:?}");
        }
    }

    #[test]
    fn accepts_a_valid_port_and_token() {
        let config = load_config(&env(Some("8765"), Some(TOKEN)));
        assert_eq!(config.map(|c| c.port()), Some(8765));
        assert_eq!(
            load_config(&env(Some("08765"), Some(TOKEN))).map(|c| c.port()),
            Some(8765)
        );
    }

    #[test]
    fn refuses_every_invalid_setting() {
        let long_token = "z".repeat(129);
        let cases = [
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
            assert!(
                load_config(&env(port, token)).is_none(),
                "{port:?} / token refused"
            );
        }
    }
}
