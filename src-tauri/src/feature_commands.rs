//! The optional feature commands: read the switches and change one.
//!
//! The desktop app turns every optional feature on by default (see
//! [`jobniac_runtime::desktop_features`]); these commands only persist the Settings switch. The
//! features are composed once at startup, so a change applies after a restart, and a feature whose
//! variable the process environment sets ignores the switch. A change takes the settings slot.

use tauri::State;

use crate::error::CommandError;
use crate::feature_dto::{FeatureIdDto, FeaturesDto};
use crate::settings_commands::SETTINGS_SAVE_FAILED;
use crate::state::AppState;

/// Reads the switches, what this run made of each feature, and whether the environment forces it.
#[tauri::command]
#[specta::specta]
pub async fn features_get(state: State<'_, AppState>) -> Result<FeaturesDto, CommandError> {
    Ok(snapshot(&state))
}

/// Saves the switch of one feature. It starts or stops nothing: the change applies at the next
/// start. The answer is the full list, like the other settings commands.
#[tauri::command]
#[specta::specta]
pub async fn feature_set_enabled(
    state: State<'_, AppState>,
    id: FeatureIdDto,
    enabled: bool,
) -> Result<FeaturesDto, CommandError> {
    run_set_enabled(&state, id, enabled)
}

fn snapshot(state: &AppState) -> FeaturesDto {
    FeaturesDto::new(&state.app_settings(), &state.feature_states())
}

fn run_set_enabled(
    state: &AppState,
    id: FeatureIdDto,
    enabled: bool,
) -> Result<FeaturesDto, CommandError> {
    let _slot = state.begin_settings()?;
    let mut settings = state.app_settings();
    settings.features.set(id.into(), enabled);
    state
        .save_settings(settings)
        .map_err(|_| CommandError::new("internal_error", SETTINGS_SAVE_FAILED))?;
    Ok(snapshot(state))
}

#[cfg(test)]
mod tests {
    use jobniac_runtime::app_settings::AppSettings;
    use jobniac_runtime::desktop_features::{Feature, FeatureStates};
    use jobniac_runtime::services::StartupError;

    use super::*;

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
    fn a_first_run_reports_every_switch_on() {
        let dto = snapshot(&state(None));
        assert_eq!(dto.features.len(), 5);
        assert!(dto.features.iter().all(|feature| feature.enabled));
        assert_eq!(
            entry(&dto, FeatureIdDto::Deletion).map(|feature| feature.effective),
            Some(true)
        );
        assert_eq!(
            entry(&dto, FeatureIdDto::Generation).map(|feature| feature.forced_by_environment),
            Some(true)
        );
    }

    #[test]
    fn a_switch_is_saved_and_answered_without_changing_the_effective_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let state = state(Some(dir.path()));
        let dto = run_set_enabled(&state, FeatureIdDto::Deletion, false)?;
        let deletion = entry(&dto, FeatureIdDto::Deletion).ok_or("missing entry")?;
        assert!(!deletion.enabled);
        // The running app keeps what it composed at startup.
        assert!(deletion.effective);
        assert!(deletion.pending_restart);
        let stored = AppSettings::load(dir.path());
        assert!(!stored.features.deletion);
        assert!(stored.features.arbeitnow && stored.features.generation);
        // A switch can come back on.
        let dto = run_set_enabled(&state, FeatureIdDto::Deletion, true)?;
        assert!(entry(&dto, FeatureIdDto::Deletion).is_some_and(|feature| feature.enabled));
        assert!(
            entry(&dto, FeatureIdDto::Deletion).is_some_and(|feature| !feature.pending_restart)
        );
        assert!(AppSettings::load(dir.path()).features.deletion);
        Ok(())
    }

    #[test]
    fn a_failed_save_changes_nothing() {
        let state = state(None);
        assert_eq!(
            run_set_enabled(&state, FeatureIdDto::Arbeitnow, false).err(),
            Some(CommandError::new("internal_error", SETTINGS_SAVE_FAILED))
        );
        assert!(state.app_settings().features.arbeitnow);
    }

    #[test]
    fn a_second_change_while_one_runs_is_refused_as_busy() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let state = state(Some(dir.path()));
        let _held = state.begin_settings()?;
        assert_eq!(
            run_set_enabled(&state, FeatureIdDto::Arbeitnow, false).err(),
            Some(CommandError::busy(crate::settings_commands::SETTINGS_BUSY))
        );
        Ok(())
    }
}
