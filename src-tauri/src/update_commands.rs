//! The updater commands: check for a new version, install it, and the opt-in auto-check switch.
//!
//! The updater plugin is called from Rust only; the webview is granted no updater or process
//! permission, so it can neither pick an endpoint nor feed a download. The endpoint and the public
//! key come from `plugins.updater` in `tauri.release.conf.json`. A build without that section (a
//! development build) does not register the plugin: the commands then answer `update.unavailable`
//! and the settings report `supported: false`. The update is verified against the public key
//! before it is installed.
//!
//! A copy installed from a `.deb` package (Linux without `APPIMAGE`) never replaces itself: the
//! package manager owns its files, so the screen links to the releases page instead.
//!
//! The auto-check is off unless the user turns it on. When on, the app checks once after startup
//! and keeps the result in [`UpdateNotice`] (and emits [`UPDATE_AVAILABLE_EVENT`]) for the screen.

use std::ffi::OsStr;
use std::sync::{Mutex, PoisonError};

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::UpdaterExt;

use crate::error::CommandError;
use crate::settings_commands::SETTINGS_SAVE_FAILED;
use crate::state::AppState;
use crate::update_dto::{AnnouncedUpdate, InstallKindDto, UpdateCheckDto, UpdateSettingsDto};

/// The event the startup check emits when it found a newer version. The payload is the
/// [`UpdateCheckDto`].
pub const UPDATE_AVAILABLE_EVENT: &str = "update:available";

/// Message id of a build that carries no updater configuration (a development build).
pub const UPDATE_UNAVAILABLE: &str = "update.unavailable";

/// Message id of a check that could not reach or read the update endpoint.
pub const UPDATE_CHECK_FAILED: &str = "update.checkFailed";

/// Message id of a download, verification or installation that failed. The app stays as it was.
pub const UPDATE_INSTALL_FAILED: &str = "update.installFailed";

/// Message id of an install request on a copy that a package manager owns (`.deb`).
pub const UPDATE_MANAGED_BY_PACKAGE: &str = "update.managedByPackage";

/// Message id of an install whose confirmed version is no longer the one announced.
pub const UPDATE_VERSION_CHANGED: &str = "update.versionChanged";

/// Whether the version the user confirmed is the one the endpoint announces now.
fn is_confirmed_version(confirmed: &str, announced: &str) -> bool {
    !confirmed.is_empty() && confirmed == announced
}

/// The key of the updater section in `plugins` of the Tauri configuration.
const UPDATER_PLUGIN_KEY: &str = "updater";

/// The latest result of the startup check, for a screen that opened after the event.
#[derive(Debug, Default)]
pub struct UpdateNotice {
    pending: Mutex<Option<UpdateCheckDto>>,
}

impl UpdateNotice {
    fn set(&self, check: Option<UpdateCheckDto>) {
        *self.pending.lock().unwrap_or_else(PoisonError::into_inner) = check;
    }

    fn get(&self) -> Option<UpdateCheckDto> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Whether the configuration carries the updater section. The plugin is registered only then.
#[must_use]
pub fn updater_configured(plugins: &tauri::utils::config::PluginConfig) -> bool {
    plugins.0.contains_key(UPDATER_PLUGIN_KEY)
}

/// How this copy was installed. `linux` is whether the target OS is Linux and `appimage` is the
/// value of the `APPIMAGE` environment variable (set by the AppImage runtime). A Linux copy
/// without it came from a package.
#[must_use]
pub fn install_kind(linux: bool, appimage: Option<&OsStr>) -> InstallKindDto {
    let in_appimage = appimage.is_some_and(|value| !value.is_empty());
    if linux && !in_appimage {
        InstallKindDto::Deb
    } else {
        InstallKindDto::Installer
    }
}

fn current_install_kind() -> InstallKindDto {
    install_kind(
        cfg!(target_os = "linux"),
        std::env::var_os("APPIMAGE").as_deref(),
    )
}

fn unavailable() -> CommandError {
    CommandError::new("not_configured", UPDATE_UNAVAILABLE)
}

fn check_failed() -> CommandError {
    CommandError::new("upstream_unavailable", UPDATE_CHECK_FAILED)
}

fn install_failed() -> CommandError {
    CommandError::new("internal_error", UPDATE_INSTALL_FAILED)
}

/// Asks the update endpoint whether a newer version exists.
async fn check(app: &AppHandle) -> Result<UpdateCheckDto, CommandError> {
    if !updater_configured(&app.config().plugins) {
        return Err(unavailable());
    }
    let updater = app.updater().map_err(|_| check_failed())?;
    let found = updater.check().await.map_err(|_| check_failed())?;
    Ok(UpdateCheckDto::from_announced(
        found.map(|update| AnnouncedUpdate {
            version: update.version,
            notes: update.body,
        }),
        current_install_kind(),
    ))
}

/// Checks for a new version. A failed check is an error, not "no update".
#[tauri::command]
#[specta::specta]
pub async fn check_for_update(app: AppHandle) -> Result<UpdateCheckDto, CommandError> {
    check(&app).await
}

/// Downloads the newer version, verifies its signature against the public key of the build,
/// installs it and restarts the app. `version` is the version the user confirmed: when the endpoint
/// now announces another one, nothing is installed and the answer is `update.versionChanged` (the
/// screen checks again). A copy installed from a `.deb` is refused. On success the process restarts,
/// so the call does not return; any failure leaves the installed version as it was.
#[tauri::command]
#[specta::specta]
pub async fn install_update(
    app: AppHandle,
    state: State<'_, AppState>,
    version: String,
) -> Result<(), CommandError> {
    if !updater_configured(&app.config().plugins) {
        return Err(unavailable());
    }
    if current_install_kind() == InstallKindDto::Deb {
        return Err(CommandError::new("conflict", UPDATE_MANAGED_BY_PACKAGE));
    }
    // Shares the settings slot, so a server toggle and an install never overlap.
    let _slot = state.begin_settings()?;
    // The supervised MCP child does not outlive the process, including the one the installer ends.
    let mcp = state.mcp_supervisor();
    let updater = app
        .updater_builder()
        .on_before_exit(move || mcp.close())
        .build()
        .map_err(|_| install_failed())?;
    let update = updater
        .check()
        .await
        .map_err(|_| check_failed())?
        .ok_or_else(install_failed)?;
    if !is_confirmed_version(&version, &update.version) {
        return Err(CommandError::new("conflict", UPDATE_VERSION_CHANGED));
    }
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|_| install_failed())?;
    state.mcp_supervisor().close();
    app.restart()
}

fn settings_dto(
    state: &AppState,
    notice: &UpdateNotice,
    supported: bool,
    kind: InstallKindDto,
) -> UpdateSettingsDto {
    UpdateSettingsDto {
        auto_check: state.app_settings().updates.auto_check,
        supported,
        install_kind: kind,
        pending: notice.get(),
    }
}

/// Reads the auto-check switch, whether this build can update, and the startup check result.
#[tauri::command]
#[specta::specta]
pub async fn update_settings_get(
    app: AppHandle,
    state: State<'_, AppState>,
    notice: State<'_, UpdateNotice>,
) -> Result<UpdateSettingsDto, CommandError> {
    Ok(settings_dto(
        &state,
        &notice,
        updater_configured(&app.config().plugins),
        current_install_kind(),
    ))
}

/// Turns the startup check on or off and saves the switch. It makes no request itself.
#[tauri::command]
#[specta::specta]
pub async fn update_set_auto_check(
    app: AppHandle,
    state: State<'_, AppState>,
    notice: State<'_, UpdateNotice>,
    enabled: bool,
) -> Result<UpdateSettingsDto, CommandError> {
    run_set_auto_check(&state, enabled)?;
    Ok(settings_dto(
        &state,
        &notice,
        updater_configured(&app.config().plugins),
        current_install_kind(),
    ))
}

fn run_set_auto_check(state: &AppState, enabled: bool) -> Result<(), CommandError> {
    let _slot = state.begin_settings()?;
    let mut settings = state.app_settings();
    settings.updates.auto_check = enabled;
    state
        .save_settings(settings)
        .map_err(|_| CommandError::new("internal_error", SETTINGS_SAVE_FAILED))
}

/// The startup check: runs once, off the main thread, only when the user turned it on and the
/// build can update. A failure is silent (the screen's own button reports failures); a found
/// update is kept in [`UpdateNotice`] and emitted.
pub fn spawn_startup_check(app: &AppHandle) {
    let enabled = app
        .try_state::<AppState>()
        .is_some_and(|state| state.app_settings().updates.auto_check);
    if !enabled || !updater_configured(&app.config().plugins) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Ok(found) = check(&app).await else {
            return;
        };
        if !found.available {
            return;
        }
        if let Some(notice) = app.try_state::<UpdateNotice>() {
            notice.set(Some(found.clone()));
        }
        // The event only informs the screen; a failed emit leaves the stored result for it to read.
        let _ = app.emit(UPDATE_AVAILABLE_EVENT, found);
    });
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use jobniac_runtime::app_settings::AppSettings;

    use super::*;

    fn state() -> AppState {
        AppState::new(Err(jobniac_runtime::services::StartupError))
    }

    #[test]
    fn a_linux_copy_without_the_appimage_variable_is_a_deb() {
        assert_eq!(install_kind(true, None), InstallKindDto::Deb);
        assert_eq!(
            install_kind(true, Some(OsString::new().as_os_str())),
            InstallKindDto::Deb
        );
    }

    #[test]
    fn an_appimage_or_a_non_linux_copy_updates_itself() {
        assert_eq!(
            install_kind(true, Some(OsStr::new("/tmp/JobniacFast.AppImage"))),
            InstallKindDto::Installer
        );
        assert_eq!(install_kind(false, None), InstallKindDto::Installer);
    }

    #[test]
    fn only_the_confirmed_version_may_be_installed() {
        assert!(is_confirmed_version("1.2.3", "1.2.3"));
        assert!(!is_confirmed_version("1.2.3", "1.2.4"));
        assert!(!is_confirmed_version("", ""));
    }

    #[test]
    fn the_updater_is_configured_only_with_its_section() {
        let mut plugins = tauri::utils::config::PluginConfig::default();
        assert!(!updater_configured(&plugins));
        plugins
            .0
            .insert("updater".to_owned(), serde_json::json!({ "endpoints": [] }));
        assert!(updater_configured(&plugins));
    }

    #[test]
    fn the_auto_check_is_off_by_default() {
        assert!(!AppSettings::default().updates.auto_check);
        let dto = settings_dto(
            &state(),
            &UpdateNotice::default(),
            true,
            InstallKindDto::Installer,
        );
        assert!(!dto.auto_check);
        assert_eq!(dto.pending, None);
    }

    #[test]
    fn the_switch_is_saved_and_read_back() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let state = state().with_settings(Some(dir.path().to_path_buf()), AppSettings::default());
        run_set_auto_check(&state, true)?;
        assert!(state.app_settings().updates.auto_check);
        assert!(AppSettings::load(dir.path()).updates.auto_check);
        run_set_auto_check(&state, false)?;
        assert!(!AppSettings::load(dir.path()).updates.auto_check);
        Ok(())
    }

    #[test]
    fn a_failed_save_keeps_the_switch_off() {
        let state = state();
        assert!(run_set_auto_check(&state, true).is_err());
        assert!(!state.app_settings().updates.auto_check);
    }

    #[test]
    fn the_notice_keeps_the_startup_result_for_the_screen() {
        let notice = UpdateNotice::default();
        assert_eq!(notice.get(), None);
        let found = UpdateCheckDto::from_announced(
            Some(AnnouncedUpdate {
                version: "9.9.9".to_owned(),
                notes: None,
            }),
            InstallKindDto::Installer,
        );
        notice.set(Some(found.clone()));
        assert_eq!(notice.get(), Some(found));
    }
}
