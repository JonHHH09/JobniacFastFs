//! The persisted application settings: `settings.json` in the app config directory.
//!
//! The file holds `{ "mcp": { "enabled": false, "port": 8765 } }` and nothing else. It never holds
//! the bearer token: the token is generated per start and kept in memory only (see
//! [`crate::mcp_server`]). The caller supplies the directory, so tests use a temporary one and the
//! file path never crosses IPC.
//!
//! Loading never fails: a missing, unreadable, oversized or malformed file gives the defaults (the
//! MCP server off, port 8765), because a broken settings file must not block startup. Saving is
//! atomic: the new text goes to a temporary file in the same directory, which then replaces the
//! settings file.

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

/// All application settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// The MCP server section.
    pub mcp: McpSettings,
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
            serde_json::json!({ "mcp": { "enabled": false, "port": 8765 } })
        );
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
