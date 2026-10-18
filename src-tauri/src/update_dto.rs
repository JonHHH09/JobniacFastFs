//! Wire types of the in-app updater (`check_for_update`, `update_settings_get`,
//! `update_set_auto_check`).
//!
//! The DTOs carry the announced version, the release notes, how this copy was installed and the
//! auto-check switch. No URL, signature, file path or key crosses IPC.

use serde::Serialize;
use specta::Type;

/// The longest release note text that crosses IPC, in characters. The notes are a CHANGELOG
/// section; the banner shows a short excerpt.
pub const MAX_NOTES_CHARS: usize = 2000;

/// How this copy of the app was installed, which decides whether it can replace itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum InstallKindDto {
    /// A Windows installer or a Linux AppImage: the app downloads, verifies and installs updates.
    Installer,
    /// A Linux `.deb` package: the package manager owns the files, so the app only points to the
    /// releases page.
    Deb,
}

/// The answer of one update check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckDto {
    /// Whether a newer version is published.
    pub available: bool,
    /// The newer version, set only when `available`.
    pub version: Option<String>,
    /// The release notes of the newer version, shortened to [`MAX_NOTES_CHARS`].
    pub notes: Option<String>,
    /// How this copy was installed.
    pub install_kind: InstallKindDto,
}

/// A version and notes announced by the update endpoint, before shaping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncedUpdate {
    /// The announced version.
    pub version: String,
    /// The announced release notes, when the endpoint sent any.
    pub notes: Option<String>,
}

impl UpdateCheckDto {
    /// Shapes the endpoint answer: `None` means this copy is current.
    #[must_use]
    pub fn from_announced(
        announced: Option<AnnouncedUpdate>,
        install_kind: InstallKindDto,
    ) -> Self {
        match announced {
            Some(update) => Self {
                available: true,
                version: Some(update.version),
                notes: update
                    .notes
                    .map(|text| shorten(text.trim(), MAX_NOTES_CHARS))
                    .filter(|text| !text.is_empty()),
                install_kind,
            },
            None => Self {
                available: false,
                version: None,
                notes: None,
                install_kind,
            },
        }
    }
}

/// `text` cut to at most `limit` characters, with an ellipsis when it was cut.
fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(limit).collect();
    cut.push('\u{2026}');
    cut
}

/// The updater section of the Settings screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettingsDto {
    /// Whether the app checks once after startup (the persisted switch, off by default).
    pub auto_check: bool,
    /// False in a build without updater configuration (a development build).
    pub supported: bool,
    /// How this copy was installed.
    pub install_kind: InstallKindDto,
    /// The update the startup check found, if it ran and found one.
    pub pending: Option<UpdateCheckDto>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn announced(version: &str, notes: Option<&str>) -> Option<AnnouncedUpdate> {
        Some(AnnouncedUpdate {
            version: version.to_owned(),
            notes: notes.map(str::to_owned),
        })
    }

    #[test]
    fn a_current_copy_has_no_version_and_no_notes() -> Result<(), serde_json::Error> {
        let dto = UpdateCheckDto::from_announced(None, InstallKindDto::Installer);
        assert!(!dto.available);
        assert_eq!(
            serde_json::to_string(&dto)?,
            r#"{"available":false,"version":null,"notes":null,"installKind":"installer"}"#
        );
        Ok(())
    }

    #[test]
    fn a_newer_version_carries_its_version_and_notes() -> Result<(), serde_json::Error> {
        let dto = UpdateCheckDto::from_announced(
            announced("1.2.3", Some("  Added\n- Updater\n")),
            InstallKindDto::Deb,
        );
        assert_eq!(
            serde_json::to_string(&dto)?,
            r#"{"available":true,"version":"1.2.3","notes":"Added\n- Updater","installKind":"deb"}"#
        );
        Ok(())
    }

    #[test]
    fn blank_notes_become_none_and_long_notes_are_cut() {
        let blank = UpdateCheckDto::from_announced(
            announced("1.2.3", Some("  \n ")),
            InstallKindDto::Installer,
        );
        assert_eq!(blank.notes, None);
        let long = "x".repeat(MAX_NOTES_CHARS + 50);
        let cut = UpdateCheckDto::from_announced(
            announced("1.2.3", Some(&long)),
            InstallKindDto::Installer,
        );
        let notes = cut.notes.unwrap_or_default();
        assert_eq!(notes.chars().count(), MAX_NOTES_CHARS + 1);
        assert!(notes.ends_with('\u{2026}'));
    }
}
