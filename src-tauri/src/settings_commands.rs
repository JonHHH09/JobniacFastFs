//! The Settings commands: the persisted MCP server switch and port, and the clipboard hand-off of the
//! connection details.
//!
//! The desktop app opens no listener. The switch makes the app supervise the `jobniac-cli http`
//! child through [`jobniac_runtime::mcp_server::McpSupervisor`]; these commands only persist the
//! setting and ask the supervisor to start or stop. A toggle and a port change take the settings
//! slot (`begin_settings`), so rapid toggling cannot race; the slot is not a window close refusal.
//!
//! Every setter persists first and then acts, and returns the full settings with the resulting
//! status. A start that fails or finds no sidecar is a status (`failed`, `unavailable`), not an
//! error: an error means the command itself could not run. The bearer token never crosses IPC: it
//! goes from the supervisor to the clipboard inside Rust.

use jobniac_runtime::app_settings::is_valid_mcp_port;
use jobniac_runtime::mcp_server::{McpConnection, McpServerStatus};
use secrecy::ExposeSecret;
use tauri::{AppHandle, State};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::error::CommandError;
use crate::settings_dto::SettingsDto;
use crate::state::AppState;

/// Message id of a settings change refused because another one is running. The switch is disabled
/// while the change runs, so the refusal rarely shows.
pub const SETTINGS_BUSY: &str = "settings.busy";

/// Message id of a port outside `1024..=65535`.
pub const SETTINGS_INVALID_PORT: &str = "settings.mcp.invalidPort";

/// Message id of a settings file that could not be written; nothing was started or stopped.
pub const SETTINGS_SAVE_FAILED: &str = "settings.saveFailed";

/// Message id of a server command that could not run (the blocking start or stop was lost). A start
/// that ran and failed is the `failed` status instead.
pub const SETTINGS_MCP_FAILED: &str = "settings.mcp.failed";

/// Message id of a copy request while the server is not running.
pub const SETTINGS_MCP_NOT_RUNNING: &str = "settings.mcp.notRunning";

/// Message id of a clipboard that refused the connection details.
pub const SETTINGS_CLIPBOARD_FAILED: &str = "settings.mcp.clipboardFailed";

fn save_failed() -> CommandError {
    CommandError::new("internal_error", SETTINGS_SAVE_FAILED)
}

fn mcp_failed() -> CommandError {
    CommandError::new("internal_error", SETTINGS_MCP_FAILED)
}

/// The persisted settings with the live server status. A child that died is noticed here.
fn snapshot(state: &AppState) -> SettingsDto {
    let status = state.mcp_supervisor().refresh();
    SettingsDto::new(&state.app_settings(), status)
}

/// Reads the settings and the server status. It takes no slot, so the screen can poll the status
/// while a toggle runs.
#[tauri::command]
#[specta::specta]
pub async fn settings_get(state: State<'_, AppState>) -> Result<SettingsDto, CommandError> {
    Ok(snapshot(&state))
}

/// Turns the MCP server on or off: saves the switch, then starts or stops the child. The returned
/// status is the outcome of the start (`running`, `failed` or `unavailable`) or `stopped`.
#[tauri::command]
#[specta::specta]
pub async fn mcp_server_set_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<SettingsDto, CommandError> {
    run_set_enabled(&state, enabled).await
}

async fn run_set_enabled(state: &AppState, enabled: bool) -> Result<SettingsDto, CommandError> {
    let _slot = state.begin_settings()?;
    let mut settings = state.app_settings();
    settings.mcp.enabled = enabled;
    // Persist first: a failed save leaves the server as it was.
    state.save_settings(settings).map_err(|_| save_failed())?;
    let mcp = state.mcp_supervisor();
    let port = settings.mcp.port;
    // The start waits for the listener and the stop reaps the child: both block.
    tokio::task::spawn_blocking(move || {
        if enabled {
            mcp.start(port);
        } else {
            mcp.stop();
        }
    })
    .await
    .map_err(|_| mcp_failed())?;
    Ok(snapshot(state))
}

/// Changes the loopback port: validates it, saves it, and restarts the server when it is running,
/// starting or failed (a failed start is retried on the new port).
#[tauri::command]
#[specta::specta]
pub async fn mcp_server_set_port(
    state: State<'_, AppState>,
    port: u16,
) -> Result<SettingsDto, CommandError> {
    run_set_port(&state, port).await
}

async fn run_set_port(state: &AppState, port: u16) -> Result<SettingsDto, CommandError> {
    if !is_valid_mcp_port(port) {
        return Err(CommandError::new("validation_error", SETTINGS_INVALID_PORT));
    }
    let _slot = state.begin_settings()?;
    let mut settings = state.app_settings();
    settings.mcp.port = port;
    state.save_settings(settings).map_err(|_| save_failed())?;
    let mcp = state.mcp_supervisor();
    let restart = settings.mcp.enabled
        && matches!(
            mcp.status(),
            McpServerStatus::Running | McpServerStatus::Starting | McpServerStatus::Failed
        );
    if restart {
        tokio::task::spawn_blocking(move || mcp.start(port))
            .await
            .map_err(|_| mcp_failed())?;
    }
    Ok(snapshot(state))
}

/// Copies the MCP endpoint and the bearer token to the system clipboard, from Rust. The token is
/// never returned to the webview. Refused while the server is not running.
#[tauri::command]
#[specta::specta]
pub async fn mcp_server_copy_connection(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    run_copy(&state, &AppClipboard(app))
}

/// Where the connection details are written. The real sink is the system clipboard; tests fake it.
pub(crate) trait ClipboardSink {
    /// Writes `text`, marked as a secret where the platform supports it. `Err` when refused.
    fn write_secret(&self, text: &str) -> Result<(), ()>;
}

/// The system clipboard through the Tauri plugin, called from Rust only (no webview permission).
struct AppClipboard(AppHandle);

impl ClipboardSink for AppClipboard {
    fn write_secret(&self, text: &str) -> Result<(), ()> {
        self.0.clipboard().write_secret(text).map_err(|_| ())
    }
}

/// The two-line text that lands on the clipboard.
fn connection_text(connection: &McpConnection) -> String {
    format!(
        "URL: {}\nToken: {}",
        connection.endpoint,
        connection.token.expose_secret()
    )
}

fn run_copy(state: &AppState, sink: &dyn ClipboardSink) -> Result<(), CommandError> {
    let mcp = state.mcp_supervisor();
    mcp.refresh();
    let connection = mcp
        .connection()
        .ok_or_else(|| CommandError::new("not_configured", SETTINGS_MCP_NOT_RUNNING))?;
    sink.write_secret(&connection_text(&connection))
        .map_err(|()| CommandError::new("internal_error", SETTINGS_CLIPBOARD_FAILED))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::net::{Ipv4Addr, TcpListener};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use jobniac_runtime::app_settings::{AppSettings, SETTINGS_FILE};
    use jobniac_runtime::mcp_server::{
        HTTP_TOKEN_VARIABLE, McpChild, McpSupervisor, SpawnError, SpawnRequest, Spawner,
    };

    use super::*;

    struct FakeChild(Arc<AtomicBool>);

    impl McpChild for FakeChild {
        fn is_running(&mut self) -> bool {
            self.0.load(Ordering::SeqCst)
        }

        fn terminate(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    /// Records, for every spawn, what `settings.json` held at that moment and the child's token.
    struct FakeSpawner {
        directory: PathBuf,
        fail: bool,
        spawns: AtomicUsize,
        file_at_spawn: Mutex<Vec<Option<AppSettings>>>,
        tokens: Mutex<Vec<String>>,
    }

    /// The supervisor owns its spawner, so the test keeps a second handle to read the records.
    struct SharedSpawner(Arc<FakeSpawner>);

    impl Spawner for SharedSpawner {
        fn available(&self) -> bool {
            true
        }

        /// The tests hold a real listener on the port to stand in for the child's.
        fn port_free(&self, _port: u16) -> bool {
            true
        }

        fn spawn(&self, request: SpawnRequest) -> Result<Box<dyn McpChild>, SpawnError> {
            let this = &self.0;
            this.spawns.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut seen) = this.file_at_spawn.lock() {
                seen.push(stored(&this.directory));
            }
            if let Ok(mut tokens) = this.tokens.lock() {
                for (name, value) in &request.environment {
                    if name == HTTP_TOKEN_VARIABLE {
                        tokens.push(value.to_string_lossy().into_owned());
                    }
                }
            }
            if this.fail {
                return Err(SpawnError::Failed);
            }
            Ok(Box::new(FakeChild(Arc::new(AtomicBool::new(true)))))
        }
    }

    /// The settings file as stored, or `None` when it does not exist.
    fn stored(directory: &Path) -> Option<AppSettings> {
        let text = std::fs::read_to_string(directory.join(SETTINGS_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    struct Fixture {
        state: AppState,
        spawner: Arc<FakeSpawner>,
        // Keeps the directory alive for the test.
        _dir: tempfile::TempDir,
    }

    fn fixture(port: u16, fail: bool, with_directory: bool) -> Result<Fixture, std::io::Error> {
        let dir = tempfile::tempdir()?;
        let spawner = Arc::new(FakeSpawner {
            directory: dir.path().to_path_buf(),
            fail,
            spawns: AtomicUsize::new(0),
            file_at_spawn: Mutex::new(Vec::new()),
            tokens: Mutex::new(Vec::new()),
        });
        let supervisor = McpSupervisor::new(
            Arc::new(SharedSpawner(Arc::clone(&spawner))),
            vec![(
                OsString::from("JOBNIAC_DB_PATH"),
                OsString::from("/srv/jobniac-synthetic/jobniac.sqlite3"),
            )],
        )
        .with_ready_deadline(Duration::from_millis(400));
        let mut settings = AppSettings::default();
        settings.mcp.port = port;
        let state = AppState::new(Err(jobniac_runtime::services::StartupError))
            .with_settings(with_directory.then(|| dir.path().to_path_buf()), settings)
            .with_mcp_supervisor(Arc::new(supervisor));
        Ok(Fixture {
            state,
            spawner,
            _dir: dir,
        })
    }

    fn open_port() -> Result<(TcpListener, u16), std::io::Error> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        Ok((listener, port))
    }

    struct RecordingClipboard {
        written: Mutex<Vec<String>>,
        refuse: bool,
    }

    impl ClipboardSink for RecordingClipboard {
        fn write_secret(&self, text: &str) -> Result<(), ()> {
            if self.refuse {
                return Err(());
            }
            if let Ok(mut written) = self.written.lock() {
                written.push(text.to_owned());
            }
            Ok(())
        }
    }

    fn clipboard(refuse: bool) -> RecordingClipboard {
        RecordingClipboard {
            written: Mutex::new(Vec::new()),
            refuse,
        }
    }

    #[tokio::test]
    async fn the_switch_is_persisted_before_the_child_is_started()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_listener, port) = open_port()?;
        let fixture = fixture(port, false, true)?;
        let dto = run_set_enabled(&fixture.state, true).await?;
        assert_eq!(
            dto.mcp.status,
            crate::settings_dto::McpServerStatusDto::Running
        );
        assert!(dto.mcp.enabled);
        // At the moment of the spawn the file already said enabled on the chosen port.
        let seen = fixture
            .spawner
            .file_at_spawn
            .lock()
            .map_err(|_| "poisoned")?;
        let [Some(at_spawn)] = seen.as_slice() else {
            return Err("expected one spawn with a stored file".into());
        };
        assert!(at_spawn.mcp.enabled);
        assert_eq!(at_spawn.mcp.port, port);
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_save_neither_starts_nor_changes_the_switch()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(8765, false, false)?;
        let error = run_set_enabled(&fixture.state, true).await.err();
        assert_eq!(
            error,
            Some(CommandError::new("internal_error", SETTINGS_SAVE_FAILED))
        );
        assert_eq!(fixture.spawner.spawns.load(Ordering::SeqCst), 0);
        assert!(!fixture.state.app_settings().mcp.enabled);
        Ok(())
    }

    #[tokio::test]
    async fn turning_the_switch_off_stops_the_child_and_persists()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_listener, port) = open_port()?;
        let fixture = fixture(port, false, true)?;
        run_set_enabled(&fixture.state, true).await?;
        let dto = run_set_enabled(&fixture.state, false).await?;
        assert_eq!(
            dto.mcp.status,
            crate::settings_dto::McpServerStatusDto::Stopped
        );
        assert!(!dto.mcp.enabled);
        assert!(
            stored(fixture._dir.path()).is_some_and(|settings| !settings.mcp.enabled),
            "the off switch was not persisted"
        );
        // Off again is a no-op that still answers.
        let again = run_set_enabled(&fixture.state, false).await?;
        assert_eq!(
            again.mcp.status,
            crate::settings_dto::McpServerStatusDto::Stopped
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_start_that_fails_is_a_status_not_an_error() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = fixture(8765, true, true)?;
        let dto = run_set_enabled(&fixture.state, true).await?;
        assert_eq!(
            dto.mcp.status,
            crate::settings_dto::McpServerStatusDto::Failed
        );
        // The switch stays on, so the next launch tries again.
        assert!(dto.mcp.enabled);
        Ok(())
    }

    #[tokio::test]
    async fn an_invalid_port_is_refused_before_anything_is_saved()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(8765, false, true)?;
        for port in [0_u16, 80, 1023] {
            assert_eq!(
                run_set_port(&fixture.state, port).await.err(),
                Some(CommandError::new("validation_error", SETTINGS_INVALID_PORT))
            );
        }
        assert_eq!(fixture.state.app_settings().mcp.port, 8765);
        assert!(stored(fixture._dir.path()).is_none());
        Ok(())
    }

    #[tokio::test]
    async fn a_port_change_while_stopped_is_saved_without_a_start()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(8765, false, true)?;
        let dto = run_set_port(&fixture.state, 4321).await?;
        assert_eq!(dto.mcp.port, 4321);
        assert_eq!(
            dto.mcp.status,
            crate::settings_dto::McpServerStatusDto::Stopped
        );
        assert_eq!(fixture.spawner.spawns.load(Ordering::SeqCst), 0);
        assert_eq!(
            stored(fixture._dir.path()).map(|settings| settings.mcp.port),
            Some(4321)
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_port_change_while_running_restarts_on_the_new_port_with_a_new_token()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_first, first_port) = open_port()?;
        let (_second, second_port) = open_port()?;
        let fixture = fixture(first_port, false, true)?;
        run_set_enabled(&fixture.state, true).await?;
        let dto = run_set_port(&fixture.state, second_port).await?;
        assert_eq!(
            dto.mcp.status,
            crate::settings_dto::McpServerStatusDto::Running
        );
        assert_eq!(dto.mcp.port, second_port);
        assert_eq!(fixture.spawner.spawns.load(Ordering::SeqCst), 2);
        let tokens = fixture.spawner.tokens.lock().map_err(|_| "poisoned")?;
        assert_eq!(tokens.len(), 2);
        assert_ne!(tokens[0], tokens[1]);
        Ok(())
    }

    #[tokio::test]
    async fn a_second_change_while_one_runs_is_refused_as_busy()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(8765, false, true)?;
        let _held = fixture.state.begin_settings()?;
        assert_eq!(
            run_set_enabled(&fixture.state, true).await.err(),
            Some(CommandError::busy(SETTINGS_BUSY))
        );
        assert_eq!(
            run_set_port(&fixture.state, 4321).await.err(),
            Some(CommandError::busy(SETTINGS_BUSY))
        );
        assert_eq!(fixture.spawner.spawns.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn copy_is_refused_while_the_server_is_not_running() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(8765, false, true)?;
        let sink = clipboard(false);
        assert_eq!(
            run_copy(&fixture.state, &sink).err(),
            Some(CommandError::new(
                "not_configured",
                SETTINGS_MCP_NOT_RUNNING
            ))
        );
        assert!(sink.written.lock().map_err(|_| "poisoned")?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn copy_writes_the_endpoint_and_the_token_to_the_clipboard_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_listener, port) = open_port()?;
        let fixture = fixture(port, false, true)?;
        let dto = run_set_enabled(&fixture.state, true).await?;
        let sink = clipboard(false);
        run_copy(&fixture.state, &sink)?;
        let tokens = fixture.spawner.tokens.lock().map_err(|_| "poisoned")?;
        let token = tokens.last().ok_or("no token")?;
        let written = sink.written.lock().map_err(|_| "poisoned")?;
        assert_eq!(
            written.as_slice(),
            [format!("URL: http://127.0.0.1:{port}/mcp\nToken: {token}")]
        );
        // The settings the webview receives carry no token.
        assert!(!serde_json::to_string(&dto)?.contains(token.as_str()));
        Ok(())
    }

    #[tokio::test]
    async fn a_refusing_clipboard_is_a_command_error() -> Result<(), Box<dyn std::error::Error>> {
        let (_listener, port) = open_port()?;
        let fixture = fixture(port, false, true)?;
        run_set_enabled(&fixture.state, true).await?;
        assert_eq!(
            run_copy(&fixture.state, &clipboard(true)).err(),
            Some(CommandError::new(
                "internal_error",
                SETTINGS_CLIPBOARD_FAILED
            ))
        );
        Ok(())
    }

    #[tokio::test]
    async fn settings_get_reports_the_defaults_off_on_a_first_run()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(8765, false, true)?;
        let dto = snapshot(&fixture.state);
        assert!(!dto.mcp.enabled);
        assert_eq!(dto.mcp.port, 8765);
        assert_eq!(
            dto.mcp.status,
            crate::settings_dto::McpServerStatusDto::Stopped
        );
        Ok(())
    }
}
