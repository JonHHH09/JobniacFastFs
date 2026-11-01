//! The persisted application settings: `settings.json` in the app config directory.
//!
//! The file holds `{ "mcp": { "enabled": false, "port": 8765 }, "updates": { "autoCheck": false },
//! "features": { "arbeitnow": true, "linkFetch": true, "deletion": true, "documentArtifacts": true,
//! "generation": true, "mcpArbeitnow": false, "mcpLinkFetch": false, "mcpDeletion": false } }` and
//! nothing else. The five in-app `features` switches default to on (see
//! [`crate::desktop_features`]); an older file without the section, or without a key, gives on. The
//! three MCP capability switches (`mcpArbeitnow`, `mcpLinkFetch`, `mcpDeletion`) default to off and
//! are only ever on after the user turned them on: an older file, a file without the keys, and the
//! in-app `arbeitnow`, `linkFetch` and `deletion` values all give off. It never
//! holds the bearer token: the token is generated per start and kept in memory only (see
//! [`crate::mcp_server`]). The caller supplies the directory, so tests use a temporary one and the
//! file path never crosses IPC.
//!
//! Loading never fails: a missing, unreadable, oversized or malformed file gives the defaults (the
//! MCP server off, port 8765, the in-app features on, the MCP capabilities off), because a broken
//! settings file must not block startup. Saving is atomic: the new text goes to a temporary file in
//! the same directory, which then replaces the settings file.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The settings file name inside the config directory.
pub const SETTINGS_FILE: &str = "settings.json";

/// The default loopback port of the MCP server.
pub const DEFAULT_MCP_PORT: u16 = 8765;

/// The lowest port the loopback listener accepts (`MIN_PORT` in `jobniac_mcp::http`).
pub const MIN_MCP_PORT: u16 = 1024;

/// The largest settings file that is read. The real file is under 100 bytes.
const MAX_SETTINGS_BYTES: u64 = 16 * 1024;

/// The temporary file the atomic save writes before the rename.
const TEMP_FILE: &str = "settings.json.tmp";

/// Whether `port` may be used by the MCP listener (`1024..=65535`).
#[must_use]
pub const fn is_valid_mcp_port(port: u16) -> bool {
    port >= MIN_MCP_PORT
}

/// The settings of the supervised MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSettings {
    /// Whether the desktop app starts the `jobniac-cli http` child. Off on a first run.
    pub enabled: bool,
    /// The loopback port the child listens on.
    pub port: u16,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_MCP_PORT,
        }
    }
}

/// The settings of the in-app updater.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateSettings {
    /// Whether the app checks for a new version once after startup. Off on a first run: no network
    /// request is made until the user turns it on or presses the check button.
    pub auto_check: bool,
}

/// The switches of the optional features of the desktop app. The five in-app switches are on by
/// default; the three MCP capability switches are off until the user turns them on. A process
/// environment variable set to something other than `1` turns a feature off over its switch; a
/// variable of `1` enables the in-app feature but never an MCP capability (see
/// [`crate::desktop_features`]). A change applies at the next start.
///
/// The MCP switches have their own keys (`mcpArbeitnow`, `mcpLinkFetch`, `mcpDeletion`), separate
/// from the in-app `arbeitnow`, `linkFetch` and `deletion`. A file written by a build that turned
/// the in-app features on by default holds those values as `true` without any consent to expose
/// them to MCP clients, so they must never switch the MCP capabilities on: only the MCP keys do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FeatureSettings {
    /// Arbeitnow board scan and import.
    pub arbeitnow: bool,
    /// Job posting capture from a link, in the app.
    pub link_fetch: bool,
    /// Hard deletion of profiles and jobs, in the app.
    pub deletion: bool,
    /// Document artifacts and their export.
    pub document_artifacts: bool,
    /// Resume and cover-letter PDF generation.
    pub generation: bool,
    /// Whether MCP clients may scan the Arbeitnow board and import a candidate
    /// (`scan_arbeitnow_jobs`, which reaches the network, and `import_arbeitnow_job`). Off until
    /// the user opts in.
    pub mcp_arbeitnow: bool,
    /// Whether MCP clients may add a job from a link (`add_job_from_link`, which fetches the page).
    /// Off until the user opts in.
    pub mcp_link_fetch: bool,
    /// Whether MCP clients may delete profiles and jobs (`delete_profile`, `delete_job`). Off until
    /// the user opts in.
    pub mcp_deletion: bool,
}

impl Default for FeatureSettings {
    fn default() -> Self {
        Self {
            arbeitnow: true,
            link_fetch: true,
            deletion: true,
            document_artifacts: true,
            generation: true,
            mcp_arbeitnow: false,
            mcp_link_fetch: false,
            mcp_deletion: false,
        }
    }
}

/// All application settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// The MCP server section.
    pub mcp: McpSettings,
    /// The updater section.
    pub updates: UpdateSettings,
    /// The optional feature switches.
    pub features: FeatureSettings,
}

/// The settings could not be written. The text names no path and no cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the settings could not be saved")]
pub struct SettingsSaveError;

impl AppSettings {
    /// Reads the settings from `directory`. Any problem with the file gives the defaults; a port
    /// outside `1024..=65535` falls back to the default port and keeps the other values.
    #[must_use]
    pub fn load(directory: &Path) -> Self {
        let Some(text) = read_limited(&directory.join(SETTINGS_FILE)) else {
            return Self::default();
        };
        let mut settings: Self = serde_json::from_str(&text).unwrap_or_default();
        if !is_valid_mcp_port(settings.mcp.port) {
            settings.mcp.port = DEFAULT_MCP_PORT;
        }
        settings
    }

    /// Writes the settings to `directory` atomically (temporary file, then rename), creating the
    /// directory when it is missing.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsSaveError`] when the port is invalid or the file cannot be written.
    pub fn save(&self, directory: &Path) -> Result<(), SettingsSaveError> {
        if !is_valid_mcp_port(self.mcp.port) {
            return Err(SettingsSaveError);
        }
        let text = serde_json::to_string_pretty(self).map_err(|_| SettingsSaveError)?;
        fs::create_dir_all(directory).map_err(|_| SettingsSaveError)?;
        let temporary = directory.join(TEMP_FILE);
        let written = write_synced(&temporary, text.as_bytes())
            .and_then(|()| fs::rename(&temporary, directory.join(SETTINGS_FILE)));
        if written.is_err() {
            // Best effort: a leftover temporary file is overwritten by the next save.
            drop(fs::remove_file(&temporary));
            return Err(SettingsSaveError);
        }
        Ok(())
    }
}

/// Reads a UTF-8 file of at most [`MAX_SETTINGS_BYTES`]; `None` when it is missing, unreadable,
/// larger than the limit or not UTF-8.
fn read_limited(path: &Path) -> Option<String> {
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_SETTINGS_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= MAX_SETTINGS_BYTES).then_some(text)
}

/// Writes `bytes` to a new file and flushes it to disk.
fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> Result<tempfile::TempDir, std::io::Error> {
        tempfile::tempdir()
    }

    #[test]
    fn a_missing_file_gives_the_defaults() -> Result<(), std::io::Error> {
        let dir = directory()?;
        let settings = AppSettings::load(dir.path());
        assert!(!settings.mcp.enabled);
        assert_eq!(settings.mcp.port, 8765);
        Ok(())
    }

    #[test]
    fn a_saved_value_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let dir = directory()?;
        let settings = AppSettings {
            mcp: McpSettings {
                enabled: true,
                port: 4321,
            },
            updates: UpdateSettings { auto_check: true },
            features: FeatureSettings {
                arbeitnow: false,
                link_fetch: true,
                deletion: false,
                document_artifacts: true,
                generation: false,
                mcp_arbeitnow: false,
                mcp_link_fetch: true,
                mcp_deletion: false,
            },
        };
        settings.save(dir.path())?;
        assert_eq!(AppSettings::load(dir.path()), settings);
        // The temporary file does not outlive the save.
        assert!(!dir.path().join(TEMP_FILE).exists());
        Ok(())
    }

    #[test]
    fn the_saved_file_has_the_documented_shape_and_no_token()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = directory()?;
        AppSettings::default().save(dir.path())?;
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.path().join(SETTINGS_FILE))?)?;
        assert_eq!(
            value,
            serde_json::json!({
                "mcp": { "enabled": false, "port": 8765 },
                "updates": { "autoCheck": false },
                "features": {
                    "arbeitnow": true,
                    "linkFetch": true,
                    "deletion": true,
                    "documentArtifacts": true,
                    "generation": true,
                    "mcpArbeitnow": false,
                    "mcpLinkFetch": false,
                    "mcpDeletion": false
                }
            })
        );
        Ok(())
    }

    #[test]
    fn the_update_auto_check_is_off_by_default_and_for_an_older_file()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(!AppSettings::default().updates.auto_check);
        let dir = directory()?;
        fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"mcp":{"enabled":true,"port":4000}}"#,
        )?;
        let settings = AppSettings::load(dir.path());
        assert!(settings.mcp.enabled);
        assert!(!settings.updates.auto_check);
        Ok(())
    }

    #[test]
    fn every_in_app_feature_is_on_by_default_and_for_an_older_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let all_on = FeatureSettings::default();
        assert!(
            all_on.arbeitnow
                && all_on.link_fetch
                && all_on.deletion
                && all_on.document_artifacts
                && all_on.generation
        );
        let dir = directory()?;
        // A file from before the section existed, and one that names a single key.
        fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"mcp":{"enabled":true,"port":4000},"updates":{"autoCheck":true}}"#,
        )?;
        assert_eq!(AppSettings::load(dir.path()).features, all_on);
        fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"features":{"deletion":false}}"#,
        )?;
        let settings = AppSettings::load(dir.path());
        assert!(!settings.features.deletion);
        assert!(settings.features.arbeitnow && settings.features.generation);
        Ok(())
    }

    #[test]
    fn the_mcp_capabilities_are_off_by_default() {
        let defaults = AppSettings::default();
        assert!(!defaults.features.mcp_arbeitnow);
        assert!(!defaults.features.mcp_link_fetch);
        assert!(!defaults.features.mcp_deletion);
        // The in-app features keep their defaults: the opt-in concerns the MCP exposure only.
        assert!(
            defaults.features.arbeitnow
                && defaults.features.link_fetch
                && defaults.features.deletion
        );
    }

    #[test]
    fn no_older_file_switches_an_mcp_capability_on() -> Result<(), Box<dyn std::error::Error>> {
        let dir = directory()?;
        let files = [
            // Before the features section existed.
            r#"{"mcp":{"enabled":true,"port":4000},"updates":{"autoCheck":true}}"#,
            // The section of a build without the MCP keys, with every in-app feature on.
            r#"{"features":{"arbeitnow":true,"linkFetch":true,"deletion":true,"documentArtifacts":true,"generation":true}}"#,
            // The MCP server on, which is no consent to expose deletion, link capture or Arbeitnow.
            r#"{"mcp":{"enabled":true,"port":4000},"features":{"arbeitnow":true,"linkFetch":true,"deletion":true}}"#,
            // Spellings of the MCP keys that are not the keys.
            r#"{"features":{"mcp_deletion":true,"mcp_link_fetch":true,"mcp_arbeitnow":true}}"#,
            r#"{"features":{"mcp":true,"MCPDELETION":true,"McpLinkFetch":true}}"#,
            // An empty section and an empty file.
            r#"{"features":{}}"#,
            "{}",
        ];
        for text in files {
            fs::write(dir.path().join(SETTINGS_FILE), text)?;
            let features = AppSettings::load(dir.path()).features;
            assert!(
                !features.mcp_arbeitnow && !features.mcp_link_fetch && !features.mcp_deletion,
                "{text}"
            );
        }
        Ok(())
    }

    #[test]
    fn an_mcp_capability_is_on_only_for_its_own_explicit_key()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = directory()?;
        fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"features":{"mcpDeletion":true}}"#,
        )?;
        let features = AppSettings::load(dir.path()).features;
        assert!(features.mcp_deletion && !features.mcp_link_fetch && !features.mcp_arbeitnow);
        fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"features":{"mcpLinkFetch":true}}"#,
        )?;
        let features = AppSettings::load(dir.path()).features;
        assert!(features.mcp_link_fetch && !features.mcp_deletion && !features.mcp_arbeitnow);
        fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"features":{"mcpArbeitnow":true}}"#,
        )?;
        let features = AppSettings::load(dir.path()).features;
        assert!(features.mcp_arbeitnow && !features.mcp_deletion && !features.mcp_link_fetch);
        // Another type than a boolean is not an opt-in: the file is broken and gives the defaults.
        for broken in [
            r#"{"features":{"mcpDeletion":"yes"}}"#,
            r#"{"features":{"mcpLinkFetch":1}}"#,
            r#"{"features":{"mcpDeletion":null}}"#,
            r#"{"features":{"mcpArbeitnow":"true"}}"#,
        ] {
            fs::write(dir.path().join(SETTINGS_FILE), broken)?;
            assert_eq!(AppSettings::load(dir.path()), AppSettings::default());
        }
        Ok(())
    }

    #[test]
    fn saving_after_an_older_file_does_not_turn_an_mcp_capability_on()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = directory()?;
        fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"mcp":{"enabled":true,"port":4000},"features":{"arbeitnow":true,"linkFetch":true,"deletion":true}}"#,
        )?;
        // Any save (a port change, another switch) rewrites the whole file.
        let mut settings = AppSettings::load(dir.path());
        settings.mcp.port = 4001;
        settings.features.generation = false;
        settings.save(dir.path())?;
        let reloaded = AppSettings::load(dir.path());
        assert!(!reloaded.features.mcp_arbeitnow);
        assert!(!reloaded.features.mcp_link_fetch && !reloaded.features.mcp_deletion);
        assert!(
            reloaded.features.arbeitnow
                && reloaded.features.link_fetch
                && reloaded.features.deletion
        );
        assert_eq!(reloaded, settings);
        Ok(())
    }

    #[test]
    fn an_mcp_opt_in_survives_a_save_and_a_reload() -> Result<(), Box<dyn std::error::Error>> {
        let dir = directory()?;
        let mut settings = AppSettings::default();
        settings.features.mcp_deletion = true;
        settings.features.mcp_arbeitnow = true;
        settings.save(dir.path())?;
        assert_eq!(AppSettings::load(dir.path()), settings);
        settings.features.mcp_deletion = false;
        settings.features.mcp_arbeitnow = false;
        settings.save(dir.path())?;
        let reloaded = AppSettings::load(dir.path()).features;
        assert!(!reloaded.mcp_deletion && !reloaded.mcp_arbeitnow);
        Ok(())
    }

    #[test]
    fn save_creates_a_missing_directory() -> Result<(), Box<dyn std::error::Error>> {
        let dir = directory()?;
        let nested = dir.path().join("config").join("app");
        AppSettings::default().save(&nested)?;
        assert!(nested.join(SETTINGS_FILE).is_file());
        Ok(())
    }

    #[test]
    fn a_malformed_file_gives_the_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let dir = directory()?;
        for text in ["", "not json", "[1,2]", r#"{"mcp":{"port":"x"}}"#, "\u{0}"] {
            fs::write(dir.path().join(SETTINGS_FILE), text)?;
            assert_eq!(AppSettings::load(dir.path()), AppSettings::default());
        }
        fs::write(dir.path().join(SETTINGS_FILE), [0xff_u8, 0xfe, 0xfd])?;
        assert_eq!(AppSettings::load(dir.path()), AppSettings::default());
        Ok(())
    }

    #[test]
    fn an_oversized_file_gives_the_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let dir = directory()?;
        let padding = " ".repeat(usize::try_from(MAX_SETTINGS_BYTES)?);
        let text = format!(r#"{{"mcp":{{"enabled":true,"port":4000}}}}{padding}"#);
        fs::write(dir.path().join(SETTINGS_FILE), text)?;
        assert_eq!(AppSettings::load(dir.path()), AppSettings::default());
        Ok(())
    }

    #[test]
    fn a_directory_in_place_of_the_file_gives_the_defaults()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = directory()?;
        fs::create_dir(dir.path().join(SETTINGS_FILE))?;
        assert_eq!(AppSettings::load(dir.path()), AppSettings::default());
        Ok(())
    }

    #[test]
    fn partial_files_fill_the_missing_fields_with_defaults()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = directory()?;
        fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"mcp":{"enabled":true}}"#,
        )?;
        let settings = AppSettings::load(dir.path());
        assert!(settings.mcp.enabled);
        assert_eq!(settings.mcp.port, DEFAULT_MCP_PORT);
        Ok(())
    }

    #[test]
    fn port_bounds_follow_the_loopback_listener_rule() {
        assert!(!is_valid_mcp_port(0));
        assert!(!is_valid_mcp_port(80));
        assert!(!is_valid_mcp_port(1023));
        assert!(is_valid_mcp_port(1024));
        assert!(is_valid_mcp_port(65535));
    }

    #[test]
    fn a_stored_port_below_the_minimum_falls_back_to_the_default_port()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = directory()?;
        fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"mcp":{"enabled":true,"port":80}}"#,
        )?;
        let settings = AppSettings::load(dir.path());
        assert!(settings.mcp.enabled);
        assert_eq!(settings.mcp.port, DEFAULT_MCP_PORT);
        Ok(())
    }

    #[test]
    fn saving_an_invalid_port_is_refused_and_writes_nothing() -> Result<(), std::io::Error> {
        let dir = directory()?;
        let settings = AppSettings {
            mcp: McpSettings {
                enabled: true,
                port: 80,
            },
            updates: UpdateSettings::default(),
            features: FeatureSettings::default(),
        };
        assert_eq!(settings.save(dir.path()), Err(SettingsSaveError));
        assert!(!dir.path().join(SETTINGS_FILE).exists());
        Ok(())
    }

    #[test]
    fn a_save_that_cannot_write_is_an_error() -> Result<(), Box<dyn std::error::Error>> {
        let dir = directory()?;
        // A file where the directory should be: the directory cannot be created.
        let blocker = dir.path().join("blocked");
        fs::write(&blocker, "x")?;
        assert_eq!(
            AppSettings::default().save(&blocker.join("inner")),
            Err(SettingsSaveError)
        );
        Ok(())
    }
}
