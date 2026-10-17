//! Wire types of the Settings screen (`settings_get`, `mcp_server_set_enabled`,
//! `mcp_server_set_port`).
//!
//! The DTOs carry the switch, the port and the server status only. The bearer token and the settings
//! file path never cross IPC; the token reaches the clipboard from Rust (`mcp_server_copy_connection`).

use jobniac_runtime::app_settings::AppSettings;
use jobniac_runtime::mcp_server::McpServerStatus;
use serde::Serialize;
use specta::Type;

/// The state of the supervised MCP server, as lower-case words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum McpServerStatusDto {
    /// No child runs and none was requested.
    Stopped,
    /// A child was spawned and is not accepting connections yet.
    Starting,
    /// The child is alive and accepts connections on the port.
    Running,
    /// The last start failed, or the child died.
    Failed,
    /// The sidecar executable is not next to the app.
    Unavailable,
}

impl From<McpServerStatus> for McpServerStatusDto {
    fn from(status: McpServerStatus) -> Self {
        match status {
            McpServerStatus::Stopped => Self::Stopped,
            McpServerStatus::Starting => Self::Starting,
            McpServerStatus::Running => Self::Running,
            McpServerStatus::Failed => Self::Failed,
            McpServerStatus::Unavailable => Self::Unavailable,
        }
    }
}

/// The MCP server section of the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpSettingsDto {
    /// Whether the app starts the server (the persisted switch).
    pub enabled: bool,
    /// The loopback port, `1024..=65535`.
    pub port: u16,
    /// What the server is doing now.
    pub status: McpServerStatusDto,
}

/// The application settings with the live status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    /// The MCP server section.
    pub mcp: McpSettingsDto,
}

impl SettingsDto {
    /// The persisted `settings` together with the current server `status`.
    #[must_use]
    pub fn new(settings: &AppSettings, status: McpServerStatus) -> Self {
        Self {
            mcp: McpSettingsDto {
                enabled: settings.mcp.enabled,
                port: settings.mcp.port,
                status: status.into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dto_has_the_wire_shape_the_screen_reads() -> Result<(), serde_json::Error> {
        let dto = SettingsDto::new(&AppSettings::default(), McpServerStatus::Stopped);
        assert_eq!(
            serde_json::to_string(&dto)?,
            r#"{"mcp":{"enabled":false,"port":8765,"status":"stopped"}}"#
        );
        Ok(())
    }

    #[test]
    fn every_status_is_a_lower_case_word() -> Result<(), serde_json::Error> {
        for (status, word) in [
            (McpServerStatus::Stopped, "stopped"),
            (McpServerStatus::Starting, "starting"),
            (McpServerStatus::Running, "running"),
            (McpServerStatus::Failed, "failed"),
            (McpServerStatus::Unavailable, "unavailable"),
        ] {
            assert_eq!(
                serde_json::to_string(&McpServerStatusDto::from(status))?,
                format!("\"{word}\"")
            );
        }
        Ok(())
    }
}
