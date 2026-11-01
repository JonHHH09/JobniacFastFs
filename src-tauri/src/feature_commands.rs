//! The optional feature commands: read the switches and change one.
//!
//! The desktop app turns the in-app optional features on by default and the three MCP capabilities
//! (Arbeitnow scan, link capture and deletion for MCP clients) off (see
//! [`jobniac_runtime::desktop_features`]). These commands persist the Settings switch, which is the
//! user's opt-in. The in-app features are composed once at startup, so a change applies after a
//! restart, and a feature whose variable the process environment refuses ignores the switch.
//!
//! **Revoking an MCP capability applies at once.** After the switch is saved off, the supervisor
//! drops the grant and kills a running server that was started with it, in one critical section;
//! when the server is set to run, it is started again without the capability (new token, so MCP
//! clients reconnect). Every later start, port change or restart uses the narrowed grants, and a
//! grant never comes back in the same run: turning the switch on again waits for the next start of
//! the app. The command does not answer until the old server is confirmed gone, so it never reports
//! success while a server that may keep the revoked capability is running. When the old server
//! cannot be confirmed gone (the kill failed, or it did not exit in time) the command fails with a
//! fixed message, the status of the server is `failed` with `stopIncomplete`, no new server starts
//! until the old one is gone, and the switch stays off. A failed save changes nothing: the switch
//! stays on, the capability stays granted and the running server is untouched.
//!
//! A change takes the settings slot, so it cannot interleave with a server toggle or a port change.

use jobniac_runtime::app_settings::AppSettings;
use jobniac_runtime::desktop_features::Feature;
use jobniac_runtime::mcp_server::{McpFailure, McpServerStatus, Revocation};
use tauri::State;

use crate::error::CommandError;
use crate::feature_dto::{FeatureIdDto, FeaturesDto};
use crate::settings_commands::{
    SETTINGS_MCP_FAILED, SETTINGS_MCP_STOP_INCOMPLETE, SETTINGS_SAVE_FAILED,
};
use crate::state::AppState;

/// Reads the switches, what this run made of each feature, and whether the environment forces it.
#[tauri::command]
#[specta::specta]
pub async fn features_get(state: State<'_, AppState>) -> Result<FeaturesDto, CommandError> {
    Ok(snapshot(&state))
}

/// Saves the switch of one feature. An in-app feature and the turning on of an MCP capability apply
/// at the next start of the app. Turning an MCP capability off applies at once: the MCP server
/// stops exposing it (it restarts without it if it was running). The answer is the full list, like
/// the other settings commands.
#[tauri::command]
#[specta::specta]
pub async fn feature_set_enabled(
    state: State<'_, AppState>,
    id: FeatureIdDto,
    enabled: bool,
) -> Result<FeaturesDto, CommandError> {
    run_set_enabled(&state, id, enabled).await
}

/// The persisted switches with what this run made of each feature. The MCP capabilities are read
/// live: one is usable only while the supervisor still grants it, and one that was granted at
/// startup and revoked since is marked so that turning it on again reads as waiting for a restart.
fn snapshot(state: &AppState) -> FeaturesDto {
    let mcp = state.mcp_supervisor();
    let grants = state.mcp_grants();
    let mut states = state.feature_states();
    for feature in Feature::ALL {
        if !feature.is_mcp_capability() {
            continue;
        }
        let granted = mcp.has_grant(feature.variable(), "1");
        states = states
            .with_effective(feature, states.effective(feature) && granted)
            .with_revoked(feature, grants.granted_at_startup(feature) && !granted);
    }
    // A server that could not be confirmed stopped may still serve a capability that is off. The
    // refresh also notices a survivor that has exited since.
    let status = mcp.refresh();
    let stop_incomplete =
        status == McpServerStatus::Failed && mcp.failure() == Some(McpFailure::StopFailed);
    FeaturesDto::new(&state.app_settings(), &states, stop_incomplete)
}

async fn run_set_enabled(
    state: &AppState,
    id: FeatureIdDto,
    enabled: bool,
) -> Result<FeaturesDto, CommandError> {
    let _slot = state.begin_settings()?;
    let feature: Feature = id.into();
    let mut settings = state.app_settings();
    settings.features.set(feature, enabled);
    // Persist first: a failed save leaves the switch, the grants and the server as they were.
    state
        .save_settings(settings)
        .map_err(|_| CommandError::new("internal_error", SETTINGS_SAVE_FAILED))?;
    if feature.is_mcp_capability() && !enabled {
        revoke(state, &settings).await?;
    }
    Ok(snapshot(state))
}

/// Applies the saved switches to the supervisor: the grants of the switches that are off are
/// dropped, a server that held one is killed in the same step, and a server that is set to run
/// starts again without it. No value is copied to decide which grants stay.
///
/// A server that cannot be confirmed gone is an error, not a success: the grants are already
/// narrowed (no later start has the capability) and the status says `failed`, but a process may
/// still hold it.
async fn revoke(state: &AppState, settings: &AppSettings) -> Result<(), CommandError> {
    let mcp = state.mcp_supervisor();
    let grants = state.mcp_grants();
    let features = settings.features;
    let restart_on = settings.mcp.enabled.then_some(settings.mcp.port);
    // Stopping a server waits for it (bounded), and so does the restart: both block, so both run on
    // a blocking thread, in one step. The old server is gone before the restart can start, so a
    // failure of the restart leaves a stopped or failed server, never one with the tool.
    let revocation = tokio::task::spawn_blocking(move || {
        // From here no later start gets the revoked capability. A survivor of an earlier stop is
        // checked on every call, so a revocation that narrows nothing new still reports it.
        let revocation = mcp.retain_grants(|pair| grants.allows(&features, pair));
        if revocation == Revocation::Stopped
            && let Some(port) = restart_on
        {
            mcp.start(port);
        }
        revocation
    })
    .await
    .map_err(|_| CommandError::new("internal_error", SETTINGS_MCP_FAILED))?;
    match revocation {
        Revocation::StopFailed => Err(CommandError::new(
            "internal_error",
            SETTINGS_MCP_STOP_INCOMPLETE,
        )),
        Revocation::Unaffected | Revocation::Stopped => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use jobniac_runtime::app_settings::{AppSettings, FeatureSettings, McpSettings};
    use jobniac_runtime::arbeitnow::{CANDIDATE_KEY, CURSOR_SECRET_KEY};
    use jobniac_runtime::desktop_features::{DesktopEnvironment, FeatureStates, McpGrants};
    use jobniac_runtime::mcp_server::{
        McpChild, McpFailure, McpServerStatus, McpSupervisor, SpawnError, SpawnRequest, Spawner,
        Termination,
    };
    use jobniac_runtime::services::StartupError;

    use super::*;

    const DELETE: &str = "JOBNIAC_DELETION_ENABLED";
    const LINK: &str = "JOBNIAC_LINK_FETCH_ENABLED";
    const ARBEITNOW: &str = "JOBNIAC_ARBEITNOW_ENABLED";

    fn state(directory: Option<&std::path::Path>) -> AppState {
        AppState::new(Err(StartupError))
            .with_settings(
                directory.map(std::path::Path::to_path_buf),
                AppSettings::default(),
            )
            .with_feature_states(
                FeatureStates::new()
                    .with_effective(Feature::Deletion, true)
                    .with_forced(Feature::Generation, true),
            )
    }

    fn entry(dto: &FeaturesDto, id: FeatureIdDto) -> Option<crate::feature_dto::FeatureDto> {
        dto.features
            .iter()
            .copied()
            .find(|feature| feature.id == id)
    }

    #[test]
    fn a_first_run_reports_every_in_app_switch_on_and_the_mcp_capabilities_off() {
        let dto = snapshot(&state(None));
        assert_eq!(dto.features.len(), 8);
        for feature in &dto.features {
            let mcp = matches!(
                feature.id,
                FeatureIdDto::McpArbeitnow | FeatureIdDto::McpLinkFetch | FeatureIdDto::McpDeletion
            );
            assert_eq!(feature.enabled, !mcp, "{:?}", feature.id);
            assert!(!feature.pending_restart, "{:?}", feature.id);
        }
        assert_eq!(
            entry(&dto, FeatureIdDto::Deletion).map(|feature| feature.effective),
            Some(true)
        );
        assert_eq!(
            entry(&dto, FeatureIdDto::Generation).map(|feature| feature.forced_by_environment),
            Some(true)
        );
    }

    #[tokio::test]
    async fn a_switch_is_saved_and_answered_without_changing_the_effective_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let state = state(Some(dir.path()));
        let dto = run_set_enabled(&state, FeatureIdDto::Deletion, false).await?;
        let deletion = entry(&dto, FeatureIdDto::Deletion).ok_or("missing entry")?;
        assert!(!deletion.enabled);
        // The running app keeps what it composed at startup.
        assert!(deletion.effective);
        assert!(deletion.pending_restart);
        let stored = AppSettings::load(dir.path());
        assert!(!stored.features.deletion);
        assert!(stored.features.arbeitnow && stored.features.generation);
        // A switch can come back on.
        let dto = run_set_enabled(&state, FeatureIdDto::Deletion, true).await?;
        assert!(entry(&dto, FeatureIdDto::Deletion).is_some_and(|feature| feature.enabled));
        assert!(
            entry(&dto, FeatureIdDto::Deletion).is_some_and(|feature| !feature.pending_restart)
        );
        assert!(AppSettings::load(dir.path()).features.deletion);
        Ok(())
    }

    #[tokio::test]
    async fn an_mcp_capability_is_opted_in_and_out_by_its_own_switch()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let state = state(Some(dir.path()));
        let dto = run_set_enabled(&state, FeatureIdDto::McpDeletion, true).await?;
        let mcp = entry(&dto, FeatureIdDto::McpDeletion).ok_or("missing entry")?;
        assert!(mcp.enabled);
        // The running app was not granted it at startup: the opt-in waits for a restart.
        assert!(!mcp.effective && mcp.pending_restart);
        let stored = AppSettings::load(dir.path()).features;
        assert!(stored.mcp_deletion);
        // Nothing else moved: not the other MCP capabilities, not the in-app switches.
        assert!(!stored.mcp_link_fetch && !stored.mcp_arbeitnow);
        assert!(stored.deletion && stored.link_fetch && stored.arbeitnow);
        assert!(
            entry(&dto, FeatureIdDto::LinkFetch).is_some_and(|feature| feature.enabled),
            "the in-app switch stays as it was"
        );
        // Back off: nothing is pending, and the file says off.
        let dto = run_set_enabled(&state, FeatureIdDto::McpDeletion, false).await?;
        assert!(
            entry(&dto, FeatureIdDto::McpDeletion)
                .is_some_and(|feature| !feature.enabled && !feature.pending_restart)
        );
        assert!(!AppSettings::load(dir.path()).features.mcp_deletion);
        Ok(())
    }

    #[tokio::test]
    async fn switching_an_in_app_feature_never_opts_an_mcp_capability_in()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let state = state(Some(dir.path()));
        for id in [
            FeatureIdDto::Deletion,
            FeatureIdDto::LinkFetch,
            FeatureIdDto::Arbeitnow,
        ] {
            for enabled in [false, true] {
                run_set_enabled(&state, id, enabled).await?;
                let stored = AppSettings::load(dir.path()).features;
                assert!(
                    !stored.mcp_deletion && !stored.mcp_link_fetch && !stored.mcp_arbeitnow,
                    "{id:?}"
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_save_changes_nothing() {
        let state = state(None);
        assert_eq!(
            run_set_enabled(&state, FeatureIdDto::Arbeitnow, false)
                .await
                .err(),
            Some(CommandError::new("internal_error", SETTINGS_SAVE_FAILED))
        );
        assert!(state.app_settings().features.arbeitnow);
    }

    #[tokio::test]
    async fn a_second_change_while_one_runs_is_refused_as_busy()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let state = state(Some(dir.path()));
        let _held = state.begin_settings()?;
        assert_eq!(
            run_set_enabled(&state, FeatureIdDto::Arbeitnow, false)
                .await
                .err(),
            Some(CommandError::busy(crate::settings_commands::SETTINGS_BUSY))
        );
        Ok(())
    }

    // ---- Revocation against a real supervisor with a recording spawner ----------------------

    /// One child the spawner made: whether it is alive and whether it was given each capability.
    struct Spawned {
        alive: Arc<AtomicBool>,
        delete: bool,
        link: bool,
        arbeitnow: bool,
        candidate_key: bool,
    }

    #[derive(Default)]
    struct Recorder {
        children: Mutex<Vec<Spawned>>,
        fail: AtomicBool,
        /// Makes the spawn panic, as a lost blocking thread would.
        panic_spawn: AtomicBool,
        /// While set, a child cannot be confirmed gone when it is terminated.
        stubborn: Arc<AtomicBool>,
    }

    struct Child {
        alive: Arc<AtomicBool>,
        stubborn: Arc<AtomicBool>,
    }

    impl McpChild for Child {
        fn is_running(&mut self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }

        fn terminate(&mut self) -> Termination {
            if self.stubborn.load(Ordering::SeqCst) {
                return Termination::MayBeAlive;
            }
            self.alive.store(false, Ordering::SeqCst);
            Termination::Gone
        }
    }

    /// The supervisor owns its spawner, so the test keeps a second handle to read the records.
    struct SharedRecorder(Arc<Recorder>);

    impl Spawner for SharedRecorder {
        fn available(&self) -> bool {
            true
        }

        fn port_free(&self, _port: u16) -> bool {
            true
        }

        fn spawn(&self, request: SpawnRequest) -> Result<Box<dyn McpChild>, SpawnError> {
            let this = &self.0;
            assert!(
                !this.panic_spawn.load(Ordering::SeqCst),
                "synthetic panic in the spawner"
            );
            if this.fail.load(Ordering::SeqCst) {
                return Err(SpawnError::Failed);
            }
            let has = |name: &str| {
                request
                    .environment
                    .iter()
                    .any(|(key, value)| key == name && value == "1")
            };
            let alive = Arc::new(AtomicBool::new(true));
            if let Ok(mut children) = this.children.lock() {
                children.push(Spawned {
                    alive: Arc::clone(&alive),
                    delete: has(DELETE),
                    link: has(LINK),
                    arbeitnow: has(ARBEITNOW),
                    candidate_key: request
                        .environment
                        .iter()
                        .any(|(key, _)| key == CANDIDATE_KEY),
                });
            }
            Ok(Box::new(Child {
                alive,
                stubborn: Arc::clone(&this.stubborn),
            }))
        }
    }

    impl Recorder {
        fn count(&self) -> usize {
            self.children.lock().map_or(0, |children| children.len())
        }

        /// `(delete, link, arbeitnow, candidate key)` of the child number `index`.
        fn grants_of(&self, index: usize) -> Option<(bool, bool, bool, bool)> {
            let children = self.children.lock().ok()?;
            let child = children.get(index)?;
            Some((
                child.delete,
                child.link,
                child.arbeitnow,
                child.candidate_key,
            ))
        }

        fn alive(&self, index: usize) -> bool {
            self.children
                .lock()
                .ok()
                .and_then(|children| {
                    children
                        .get(index)
                        .map(|child| child.alive.load(Ordering::SeqCst))
                })
                .unwrap_or(false)
        }

        /// How many live children were given deletion.
        fn live_with_delete(&self) -> usize {
            self.children.lock().map_or(0, |children| {
                children
                    .iter()
                    .filter(|child| child.alive.load(Ordering::SeqCst) && child.delete)
                    .count()
            })
        }
    }

    fn every_mcp_switch_on() -> FeatureSettings {
        let mut features = FeatureSettings::default();
        features.set(Feature::McpArbeitnow, true);
        features.set(Feature::McpLinkFetch, true);
        features.set(Feature::McpDeletion, true);
        features
    }

    /// The grants of a first start with every capability on, from a synthetic environment with a
    /// valid Arbeitnow key pair.
    fn startup_grants(features: &FeatureSettings) -> McpGrants {
        DesktopEnvironment {
            pairs: vec![
                (
                    OsString::from(CANDIDATE_KEY),
                    OsString::from("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8"),
                ),
                (
                    OsString::from(CURSOR_SECRET_KEY),
                    OsString::from("cursor-secret-0123456789abcdefghijklmnop"),
                ),
            ],
            process_count: 0,
            features: *features,
            warnings: Vec::new(),
        }
        .mcp_grants()
    }

    struct Fixture {
        state: Arc<AppState>,
        recorder: Arc<Recorder>,
        listener: TcpListener,
        port: u16,
        dir: tempfile::TempDir,
    }

    /// A state whose supervisor was started with every capability granted. The settings file lives
    /// in a temporary directory unless `persist` is false.
    fn fixture(persist: bool, mcp_enabled: bool) -> Result<Fixture, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let features = every_mcp_switch_on();
        let grants = startup_grants(&features);
        let recorder = Arc::new(Recorder::default());
        let supervisor = McpSupervisor::new(
            Arc::new(SharedRecorder(Arc::clone(&recorder))),
            vec![(
                OsString::from("JOBNIAC_DB_PATH"),
                OsString::from("/srv/jobniac-synthetic/jobniac.sqlite3"),
            )],
        )
        .with_ready_deadline(Duration::from_millis(400))
        .with_grants(grants.pairs());
        let settings = AppSettings {
            mcp: McpSettings {
                enabled: mcp_enabled,
                port,
            },
            features,
            ..AppSettings::default()
        };
        let mut started = FeatureStates::new();
        for feature in [
            Feature::McpArbeitnow,
            Feature::McpLinkFetch,
            Feature::McpDeletion,
        ] {
            started = started
                .with_started_on(feature, true)
                .with_effective(feature, true);
        }
        let state = AppState::new(Err(StartupError))
            .with_settings(persist.then(|| dir.path().to_path_buf()), settings)
            .with_feature_states(started)
            .with_mcp_grants(grants)
            .with_mcp_supervisor(Arc::new(supervisor));
        Ok(Fixture {
            state: Arc::new(state),
            recorder,
            listener,
            port,
            dir,
        })
    }

    fn start_server(fixture: &Fixture) -> McpServerStatus {
        fixture.state.mcp_supervisor().start(fixture.port)
    }

    #[tokio::test]
    async fn a_first_start_has_every_granted_capability_and_the_screen_says_so()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        assert_eq!(
            fixture.recorder.grants_of(0),
            Some((true, true, true, true))
        );
        let dto = snapshot(&fixture.state);
        for id in [
            FeatureIdDto::McpArbeitnow,
            FeatureIdDto::McpLinkFetch,
            FeatureIdDto::McpDeletion,
        ] {
            let row = entry(&dto, id).ok_or("missing entry")?;
            assert!(
                row.enabled && row.effective && !row.pending_restart,
                "{id:?}"
            );
        }
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn revoking_a_capability_restarts_the_running_server_without_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        let old_token = fixture
            .state
            .mcp_supervisor()
            .connection()
            .map(|connection| secrecy::ExposeSecret::expose_secret(&connection.token).to_owned());

        let dto = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await?;

        // The old server is gone, and the new one has everything but deletion.
        assert!(!fixture.recorder.alive(0));
        assert_eq!(fixture.recorder.count(), 2);
        assert_eq!(
            fixture.recorder.grants_of(1),
            Some((false, true, true, true))
        );
        assert!(fixture.recorder.alive(1));
        assert_eq!(fixture.recorder.live_with_delete(), 0);
        assert_eq!(
            fixture.state.mcp_supervisor().status(),
            McpServerStatus::Running
        );
        let new_token = fixture
            .state
            .mcp_supervisor()
            .connection()
            .map(|connection| secrecy::ExposeSecret::expose_secret(&connection.token).to_owned());
        assert_ne!(old_token, new_token, "clients must reconnect");
        // The screen: off, not usable, nothing waits for a restart. The others are untouched.
        let deletion = entry(&dto, FeatureIdDto::McpDeletion).ok_or("missing entry")?;
        assert!(!deletion.enabled && !deletion.effective && !deletion.pending_restart);
        for id in [FeatureIdDto::McpLinkFetch, FeatureIdDto::McpArbeitnow] {
            let row = entry(&dto, id).ok_or("missing entry")?;
            assert!(
                row.enabled && row.effective && !row.pending_restart,
                "{id:?}"
            );
        }
        // The file says off.
        assert!(!AppSettings::load(fixture.dir.path()).features.mcp_deletion);
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn no_later_start_stop_or_port_change_brings_a_revoked_capability_back()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        run_set_enabled(&fixture.state, FeatureIdDto::McpLinkFetch, false).await?;
        let supervisor = fixture.state.mcp_supervisor();
        // Stop and start, and a start on another port (a port change restarts the same way).
        supervisor.stop();
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        let other = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let other_port = other.local_addr()?.port();
        assert_eq!(supervisor.start(other_port), McpServerStatus::Running);
        for index in 1..fixture.recorder.count() {
            let (delete, link, arbeitnow, _) =
                fixture.recorder.grants_of(index).ok_or("missing child")?;
            assert!(delete && arbeitnow && !link, "child {index}");
        }
        drop(other);
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn turning_a_revoked_capability_back_on_waits_for_a_restart_of_the_app()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await?;
        let children = fixture.recorder.count();

        let dto = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, true).await?;

        // Saved on, but not granted again and the running server is not touched.
        assert_eq!(fixture.recorder.count(), children);
        assert!(!fixture.state.mcp_supervisor().has_grant(DELETE, "1"));
        let deletion = entry(&dto, FeatureIdDto::McpDeletion).ok_or("missing entry")?;
        assert!(deletion.enabled && !deletion.effective && deletion.pending_restart);
        assert!(AppSettings::load(fixture.dir.path()).features.mcp_deletion);
        // Revoking another capability afterwards does not bring it back either.
        run_set_enabled(&fixture.state, FeatureIdDto::McpLinkFetch, false).await?;
        let last = fixture.recorder.count() - 1;
        assert_eq!(
            fixture.recorder.grants_of(last),
            Some((false, false, true, true))
        );
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn revoking_every_capability_leaves_the_child_without_flags_and_keys()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        for id in [
            FeatureIdDto::McpArbeitnow,
            FeatureIdDto::McpLinkFetch,
            FeatureIdDto::McpDeletion,
        ] {
            run_set_enabled(&fixture.state, id, false).await?;
        }
        let last = fixture.recorder.count() - 1;
        // No capability flag, and the Arbeitnow candidate key is gone with its grant.
        assert_eq!(
            fixture.recorder.grants_of(last),
            Some((false, false, false, false))
        );
        assert!(fixture.recorder.alive(last));
        assert_eq!(fixture.recorder.live_with_delete(), 0);
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn a_revocation_while_the_server_is_not_running_applies_to_the_next_start()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, false)?;
        let dto = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await?;
        // Nothing was started by the revocation.
        assert_eq!(fixture.recorder.count(), 0);
        assert!(entry(&dto, FeatureIdDto::McpDeletion).is_some_and(|row| !row.effective));
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        assert_eq!(
            fixture.recorder.grants_of(0),
            Some((false, true, true, true))
        );
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn a_server_that_is_set_to_stay_off_is_not_started_by_a_revocation()
    -> Result<(), Box<dyn std::error::Error>> {
        // The supervisor holds a live child although the setting says off (a stale child): the
        // revocation kills it, and, as the server is not set to run, starts nothing.
        let fixture = fixture(true, false)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await?;
        assert!(!fixture.recorder.alive(0));
        assert_eq!(fixture.recorder.count(), 1);
        assert_eq!(
            fixture.state.mcp_supervisor().status(),
            McpServerStatus::Stopped
        );
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_save_leaves_the_grants_and_the_running_server_alone()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(false, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        let result = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await;
        assert_eq!(
            result.err(),
            Some(CommandError::new("internal_error", SETTINGS_SAVE_FAILED))
        );
        // The user's consent stands: the switch is on, the grant is held, the same server runs.
        assert!(fixture.state.app_settings().features.mcp_deletion);
        assert!(fixture.state.mcp_supervisor().has_grant(DELETE, "1"));
        assert!(fixture.recorder.alive(0));
        assert_eq!(fixture.recorder.count(), 1);
        let dto = snapshot(&fixture.state);
        assert!(
            entry(&dto, FeatureIdDto::McpDeletion)
                .is_some_and(|row| row.enabled && row.effective && !row.pending_restart)
        );
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn a_refused_revocation_while_another_change_runs_changes_nothing()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        let held = fixture.state.begin_settings()?;
        let result = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await;
        assert_eq!(
            result.err(),
            Some(CommandError::busy(crate::settings_commands::SETTINGS_BUSY))
        );
        assert!(fixture.state.mcp_supervisor().has_grant(DELETE, "1"));
        assert!(fixture.recorder.alive(0));
        drop(held);
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn a_restart_that_fails_leaves_no_server_holding_the_revoked_capability()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        // The new server cannot be spawned: the old one is already gone, so success is truthful.
        fixture.recorder.fail.store(true, Ordering::SeqCst);
        let dto = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await?;
        assert!(!fixture.recorder.alive(0));
        assert_eq!(fixture.recorder.live_with_delete(), 0);
        assert_eq!(
            fixture.state.mcp_supervisor().status(),
            McpServerStatus::Failed
        );
        assert!(
            entry(&dto, FeatureIdDto::McpDeletion)
                .is_some_and(|row| !row.enabled && !row.effective)
        );
        // Once the spawner works again the server starts without the revoked capability.
        fixture.recorder.fail.store(false, Ordering::SeqCst);
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        let last = fixture.recorder.count() - 1;
        assert_eq!(
            fixture.recorder.grants_of(last),
            Some((false, true, true, true))
        );
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn a_revocation_whose_server_cannot_be_stopped_is_an_error_and_the_status_says_so()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        // The child cannot be confirmed gone, as if the kill failed.
        fixture.recorder.stubborn.store(true, Ordering::SeqCst);

        let result = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await;

        // Not a success: the command says the server could not be stopped, with a fixed message.
        assert_eq!(
            result.err(),
            Some(CommandError::new(
                "internal_error",
                SETTINGS_MCP_STOP_INCOMPLETE
            ))
        );
        // The switch is saved off and the grant is gone for every later start.
        assert!(!AppSettings::load(fixture.dir.path()).features.mcp_deletion);
        let mcp = fixture.state.mcp_supervisor();
        assert!(!mcp.has_grant(DELETE, "1"));
        // The server is neither running nor reported stopped, and nothing was started beside it.
        assert_eq!(mcp.status(), McpServerStatus::Failed);
        assert_eq!(mcp.failure(), Some(McpFailure::StopFailed));
        assert_eq!(fixture.recorder.count(), 1);
        let dto = crate::settings_dto::SettingsDto::new(
            &fixture.state.app_settings(),
            mcp.status(),
            mcp.failure(),
        );
        assert!(dto.mcp.stop_incomplete);
        // The features screen reads: off, not usable, nothing waiting for a restart.
        let features = snapshot(&fixture.state);
        let row = entry(&features, FeatureIdDto::McpDeletion).ok_or("missing entry")?;
        assert!(!row.enabled && !row.effective && !row.pending_restart);

        // A start is refused while the survivor cannot be confirmed gone, and a second revocation
        // is honest too.
        assert_eq!(start_server(&fixture), McpServerStatus::Failed);
        assert_eq!(fixture.recorder.count(), 1);
        let again = run_set_enabled(&fixture.state, FeatureIdDto::McpLinkFetch, false).await;
        assert_eq!(
            again.err(),
            Some(CommandError::new(
                "internal_error",
                SETTINGS_MCP_STOP_INCOMPLETE
            ))
        );
        assert_eq!(fixture.recorder.count(), 1);

        // Once the survivor can be confirmed gone, the next start has neither revoked capability.
        fixture.recorder.stubborn.store(false, Ordering::SeqCst);
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        assert_eq!(
            fixture.recorder.grants_of(1),
            Some((false, false, true, true))
        );
        assert!(mcp.failure().is_none());
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn a_restart_that_panics_after_the_revocation_leaves_no_server_with_the_capability()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        // The blocking restart is lost (its thread panics), so the command reports a failed server
        // command; the revocation itself was already complete.
        fixture.recorder.panic_spawn.store(true, Ordering::SeqCst);
        let result = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await;
        assert_eq!(
            result.err(),
            Some(CommandError::new("internal_error", SETTINGS_MCP_FAILED))
        );
        assert!(!fixture.recorder.alive(0));
        assert_eq!(fixture.recorder.live_with_delete(), 0);
        assert!(!AppSettings::load(fixture.dir.path()).features.mcp_deletion);
        let mcp = fixture.state.mcp_supervisor();
        assert!(!mcp.has_grant(DELETE, "1"));
        // The screen, re-read after the error, shows the truth: off, nothing waiting.
        let features = snapshot(&fixture.state);
        let row = entry(&features, FeatureIdDto::McpDeletion).ok_or("missing entry")?;
        assert!(!row.enabled && !row.effective && !row.pending_restart);
        // The supervisor still works, and the next start has no deletion.
        fixture.recorder.panic_spawn.store(false, Ordering::SeqCst);
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        let last = fixture.recorder.count() - 1;
        assert_eq!(
            fixture.recorder.grants_of(last),
            Some((false, true, true, true))
        );
        drop(fixture.listener);
        Ok(())
    }

    fn stop_incomplete_error() -> CommandError {
        CommandError::new("internal_error", SETTINGS_MCP_STOP_INCOMPLETE)
    }

    #[tokio::test]
    async fn a_second_revocation_that_narrows_nothing_still_reports_the_survivor()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        fixture.recorder.stubborn.store(true, Ordering::SeqCst);
        let first = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await;
        assert_eq!(first.err(), Some(stop_incomplete_error()));

        // The same switch turned off again narrows nothing: not a success while the first server
        // may still be serving the capability.
        let again = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await;
        assert_eq!(again.err(), Some(stop_incomplete_error()));
        let third = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await;
        assert_eq!(third.err(), Some(stop_incomplete_error()));
        // And nothing was started beside the survivor.
        assert_eq!(fixture.recorder.count(), 1);
        let mcp = fixture.state.mcp_supervisor();
        assert_eq!(mcp.status(), McpServerStatus::Failed);
        assert_eq!(mcp.failure(), Some(McpFailure::StopFailed));
        // The screen shows the warning flag, and the revoked row is not marked as safely stopped
        // by anything but the flag.
        let features = snapshot(&fixture.state);
        assert!(features.stop_incomplete);
        let row = entry(&features, FeatureIdDto::McpDeletion).ok_or("missing entry")?;
        assert!(!row.enabled && !row.effective && !row.pending_restart);
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn turning_a_capability_on_while_a_survivor_may_run_still_shows_the_warning()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        fixture.recorder.stubborn.store(true, Ordering::SeqCst);
        let failed = run_set_enabled(&fixture.state, FeatureIdDto::McpLinkFetch, false).await;
        assert_eq!(failed.err(), Some(stop_incomplete_error()));

        // Turning a capability on is not a revocation and succeeds, but the answer carries the
        // warning: a server that was told to stop may still be running.
        let dto = run_set_enabled(&fixture.state, FeatureIdDto::McpLinkFetch, true).await?;
        assert!(dto.stop_incomplete);
        let dto = run_set_enabled(&fixture.state, FeatureIdDto::McpArbeitnow, true).await?;
        assert!(dto.stop_incomplete);
        // Reading the features again says the same.
        assert!(snapshot(&fixture.state).stop_incomplete);
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn once_the_survivor_is_gone_a_revocation_heals_and_restarts_the_server()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        fixture.recorder.stubborn.store(true, Ordering::SeqCst);
        let failed = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await;
        assert_eq!(failed.err(), Some(stop_incomplete_error()));
        assert_eq!(fixture.recorder.count(), 1);

        // The survivor can now be confirmed gone: the next call finds it, says nothing is wrong,
        // and starts the server again (it is set to run), without the revoked capability.
        fixture.recorder.stubborn.store(false, Ordering::SeqCst);
        let dto = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await?;
        assert!(!dto.stop_incomplete);
        assert_eq!(fixture.recorder.count(), 2);
        assert_eq!(
            fixture.recorder.grants_of(1),
            Some((false, true, true, true))
        );
        assert_eq!(
            fixture.state.mcp_supervisor().status(),
            McpServerStatus::Running
        );
        assert!(!fixture.recorder.alive(0));
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn a_heal_does_not_start_a_server_that_is_set_to_stay_off()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, false)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        fixture.recorder.stubborn.store(true, Ordering::SeqCst);
        let failed = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await;
        assert_eq!(failed.err(), Some(stop_incomplete_error()));
        fixture.recorder.stubborn.store(false, Ordering::SeqCst);
        let dto = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await?;
        assert!(!dto.stop_incomplete);
        assert_eq!(fixture.recorder.count(), 1, "nothing was started");
        assert_eq!(
            fixture.state.mcp_supervisor().status(),
            McpServerStatus::Stopped
        );
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test]
    async fn reading_the_features_notices_a_survivor_that_exited_by_itself()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(true, true)?;
        assert_eq!(start_server(&fixture), McpServerStatus::Running);
        fixture.recorder.stubborn.store(true, Ordering::SeqCst);
        let failed = run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await;
        assert_eq!(failed.err(), Some(stop_incomplete_error()));
        assert!(snapshot(&fixture.state).stop_incomplete);
        // The survivor exits on its own (its pipe closed): the next read heals the warning.
        fixture.recorder.stubborn.store(false, Ordering::SeqCst);
        assert!(!snapshot(&fixture.state).stop_incomplete);
        assert_eq!(
            fixture.state.mcp_supervisor().status(),
            McpServerStatus::Stopped
        );
        drop(fixture.listener);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_revocation_racing_server_starts_never_leaves_a_live_server_with_the_capability()
    -> Result<(), Box<dyn std::error::Error>> {
        for round in 0..12 {
            let fixture = fixture(true, true)?;
            let supervisor = fixture.state.mcp_supervisor();
            let port = fixture.port;
            // The startup start and a toggle (stop, start) run while the revocation does.
            let starters: Vec<_> = (0..2)
                .map(|_| {
                    let supervisor = Arc::clone(&supervisor);
                    std::thread::spawn(move || {
                        supervisor.start(port);
                        supervisor.stop();
                        supervisor.start(port);
                    })
                })
                .collect();
            if round % 2 == 0 {
                tokio::task::yield_now().await;
            }
            run_set_enabled(&fixture.state, FeatureIdDto::McpDeletion, false).await?;
            for starter in starters {
                starter.join().map_err(|_| "a starter panicked")?;
            }
            // The revocation is over and the starters are done: whichever child lives, it was not
            // given deletion after the revocation, and none that has deletion is alive.
            assert_eq!(fixture.recorder.live_with_delete(), 0, "round {round}");
            assert!(!supervisor.has_grant(DELETE, "1"));
            // And whatever starts next does not have it.
            supervisor.start(port);
            let last = fixture.recorder.count() - 1;
            assert_eq!(
                fixture.recorder.grants_of(last).map(|grants| grants.0),
                Some(false),
                "round {round}"
            );
            drop(fixture.listener);
        }
        Ok(())
    }
}
