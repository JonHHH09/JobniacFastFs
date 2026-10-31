//! Wire types of the optional feature switches (`features_get`, `feature_set_enabled`).
//!
//! The DTOs carry the switch, the effective state and whether the process environment forces the
//! feature. They never carry an environment variable name or value, a key or a path.

use jobniac_runtime::app_settings::AppSettings;
use jobniac_runtime::desktop_features::{Feature, FeatureStates};
use serde::{Deserialize, Serialize};
use specta::Type;

/// The identifier of an optional feature, as in the settings file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FeatureIdDto {
    /// Arbeitnow board scan and import.
    Arbeitnow,
    /// Job posting capture from a link.
    LinkFetch,
    /// Hard deletion of profiles and jobs.
    Deletion,
    /// Document artifacts and their export.
    DocumentArtifacts,
    /// Resume and cover-letter PDF generation.
    Generation,
    /// MCP clients may scan the Arbeitnow board and import a candidate. Off until the user opts in.
    McpArbeitnow,
    /// MCP clients may add a job from a link. Off until the user opts in.
    McpLinkFetch,
    /// MCP clients may delete profiles and jobs. Off until the user opts in.
    McpDeletion,
}

impl From<FeatureIdDto> for Feature {
    fn from(id: FeatureIdDto) -> Self {
        match id {
            FeatureIdDto::Arbeitnow => Self::Arbeitnow,
            FeatureIdDto::LinkFetch => Self::LinkFetch,
            FeatureIdDto::Deletion => Self::Deletion,
            FeatureIdDto::DocumentArtifacts => Self::DocumentArtifacts,
            FeatureIdDto::Generation => Self::Generation,
            FeatureIdDto::McpArbeitnow => Self::McpArbeitnow,
            FeatureIdDto::McpLinkFetch => Self::McpLinkFetch,
            FeatureIdDto::McpDeletion => Self::McpDeletion,
        }
    }
}

impl From<Feature> for FeatureIdDto {
    fn from(feature: Feature) -> Self {
        match feature {
            Feature::Arbeitnow => Self::Arbeitnow,
            Feature::LinkFetch => Self::LinkFetch,
            Feature::Deletion => Self::Deletion,
            Feature::DocumentArtifacts => Self::DocumentArtifacts,
            Feature::Generation => Self::Generation,
            Feature::McpArbeitnow => Self::McpArbeitnow,
            Feature::McpLinkFetch => Self::McpLinkFetch,
            Feature::McpDeletion => Self::McpDeletion,
        }
    }
}

/// One optional feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FeatureDto {
    /// Which feature.
    pub id: FeatureIdDto,
    /// The persisted switch. A change applies after a restart.
    pub enabled: bool,
    /// Whether the feature is composed and usable in this run.
    pub effective: bool,
    /// Whether the process environment sets the feature's variable, so the switch has no effect.
    pub forced_by_environment: bool,
    /// Whether the saved switch differs from the switch at startup, so the change is waiting for a
    /// restart. A feature that is on but did not start has no restart pending.
    pub pending_restart: bool,
}

/// The optional features, in display order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FeaturesDto {
    /// One entry per feature.
    pub features: Vec<FeatureDto>,
    /// Whether an MCP server that was told to stop could not be confirmed stopped. A capability
    /// that is off may then still be served by that server until it is gone, so the screen must not
    /// show the capability as safely off.
    pub stop_incomplete: bool,
}

/// Whether the saved switch of `feature` waits for a restart of the app.
///
/// An in-app feature waits when its switch differs from the one at startup. An MCP capability is
/// revoked at once, so turning it off never waits; turning it on waits unless it is in force:
/// it was off at startup, or it was granted at startup and has been revoked since (a grant never
/// comes back in the same run).
fn pending_restart(settings: &AppSettings, states: &FeatureStates, feature: Feature) -> bool {
    let on = settings.features.is_on(feature);
    if feature.is_mcp_capability() {
        on && (!states.started_on(feature) || states.revoked(feature))
    } else {
        on != states.started_on(feature)
    }
}

impl FeaturesDto {
    /// The persisted `settings` together with what this run made of each feature, and whether the
    /// MCP server could not be confirmed stopped.
    #[must_use]
    pub fn new(settings: &AppSettings, states: &FeatureStates, stop_incomplete: bool) -> Self {
        Self {
            stop_incomplete,
            features: Feature::ALL
                .into_iter()
                .map(|feature| FeatureDto {
                    id: feature.into(),
                    enabled: settings.features.is_on(feature),
                    effective: states.effective(feature),
                    forced_by_environment: states.forced(feature),
                    pending_restart: pending_restart(settings, states, feature),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dto_has_the_wire_shape_the_screen_reads() -> Result<(), serde_json::Error> {
        let states = FeatureStates::new()
            .with_effective(Feature::Arbeitnow, true)
            .with_forced(Feature::Deletion, true);
        let dto = FeaturesDto::new(&AppSettings::default(), &states, false);
        let value = serde_json::to_value(&dto)?;
        assert_eq!(value["stopIncomplete"], false);
        let ids: Vec<_> = value["features"]
            .as_array()
            .map(|features| features.iter().map(|f| f["id"].clone()).collect())
            .unwrap_or_default();
        assert_eq!(
            serde_json::Value::Array(ids),
            serde_json::json!([
                "arbeitnow",
                "linkFetch",
                "deletion",
                "documentArtifacts",
                "generation",
                "mcpArbeitnow",
                "mcpLinkFetch",
                "mcpDeletion"
            ])
        );
        assert_eq!(
            value["features"][0],
            serde_json::json!({
                "id": "arbeitnow",
                "enabled": true,
                "effective": true,
                "forcedByEnvironment": false,
                "pendingRestart": false
            })
        );
        assert_eq!(value["features"][2]["forcedByEnvironment"], true);
        Ok(())
    }

    #[test]
    fn a_restart_is_pending_only_when_the_switch_differs_from_the_startup_switch() {
        let mut settings = AppSettings::default();
        settings.features.set(Feature::Deletion, false);
        // Started on, now saved off: pending. Started off, saved off: not pending, whatever the
        // effective state. Started on and saved on but not effective (failed to start): not pending.
        let states = FeatureStates::new().with_started_on(Feature::LinkFetch, false);
        settings.features.set(Feature::LinkFetch, false);
        let dto = FeaturesDto::new(&settings, &states, false);
        let pending: Vec<_> = dto
            .features
            .iter()
            .map(|feature| (feature.id, feature.pending_restart))
            .collect();
        assert_eq!(
            pending,
            vec![
                (FeatureIdDto::Arbeitnow, false),
                (FeatureIdDto::LinkFetch, false),
                (FeatureIdDto::Deletion, true),
                (FeatureIdDto::DocumentArtifacts, false),
                (FeatureIdDto::Generation, false),
                (FeatureIdDto::McpArbeitnow, false),
                (FeatureIdDto::McpLinkFetch, false),
                (FeatureIdDto::McpDeletion, false),
            ]
        );
    }

    #[test]
    fn an_mcp_capability_waits_for_a_restart_only_when_turned_on_and_not_in_force() {
        let pending = |on: bool, started_on: bool, revoked: bool| {
            let mut settings = AppSettings::default();
            settings.features.set(Feature::McpDeletion, on);
            let states = FeatureStates::new()
                .with_started_on(Feature::McpDeletion, started_on)
                .with_revoked(Feature::McpDeletion, revoked);
            FeaturesDto::new(&settings, &states, false)
                .features
                .iter()
                .find(|feature| feature.id == FeatureIdDto::McpDeletion)
                .map(|feature| feature.pending_restart)
        };
        // Turned on and not granted at startup: waits for the next start of the app.
        assert_eq!(pending(true, false, false), Some(true));
        // Granted at startup and still on: in force, nothing waits.
        assert_eq!(pending(true, true, false), Some(false));
        // Granted at startup, revoked, turned on again: a grant never comes back in the same run.
        assert_eq!(pending(true, true, true), Some(true));
        // Turning it off is applied at once, so it never waits, whatever the startup said.
        assert_eq!(pending(false, true, false), Some(false));
        assert_eq!(pending(false, true, true), Some(false));
        assert_eq!(pending(false, false, false), Some(false));
    }

    #[test]
    fn a_first_run_reports_the_mcp_capabilities_off_with_nothing_to_restart() {
        let dto = FeaturesDto::new(&AppSettings::default(), &FeatureStates::new(), false);
        for feature in &dto.features {
            let mcp = matches!(
                feature.id,
                FeatureIdDto::McpArbeitnow | FeatureIdDto::McpLinkFetch | FeatureIdDto::McpDeletion
            );
            assert_eq!(feature.enabled, !mcp, "{:?}", feature.id);
            assert!(!feature.pending_restart, "{:?}", feature.id);
        }
    }

    #[test]
    fn an_identifier_parses_from_its_camel_case_word() -> Result<(), serde_json::Error> {
        let id: FeatureIdDto = serde_json::from_str("\"documentArtifacts\"")?;
        assert_eq!(Feature::from(id), Feature::DocumentArtifacts);
        for feature in Feature::ALL {
            assert_eq!(Feature::from(FeatureIdDto::from(feature)), feature);
        }
        Ok(())
    }
}
