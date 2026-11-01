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
}

impl From<FeatureIdDto> for Feature {
    fn from(id: FeatureIdDto) -> Self {
        match id {
            FeatureIdDto::Arbeitnow => Self::Arbeitnow,
            FeatureIdDto::LinkFetch => Self::LinkFetch,
            FeatureIdDto::Deletion => Self::Deletion,
            FeatureIdDto::DocumentArtifacts => Self::DocumentArtifacts,
            FeatureIdDto::Generation => Self::Generation,
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
}

impl FeaturesDto {
    /// The persisted `settings` together with what this run made of each feature.
    #[must_use]
    pub fn new(settings: &AppSettings, states: &FeatureStates) -> Self {
        Self {
            features: Feature::ALL
                .into_iter()
                .map(|feature| FeatureDto {
                    id: feature.into(),
                    enabled: settings.features.is_on(feature),
                    effective: states.effective(feature),
                    forced_by_environment: states.forced(feature),
                    pending_restart: settings.features.is_on(feature) != states.started_on(feature),
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
        let dto = FeaturesDto::new(&AppSettings::default(), &states);
        let value = serde_json::to_value(&dto)?;
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
                "generation"
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
        let dto = FeaturesDto::new(&settings, &states);
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
            ]
        );
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
