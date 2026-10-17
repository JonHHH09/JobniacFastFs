//! Wire type of the shell dirty report.
//!
//! Only the job and profile editors report their dirty flag in this milestone; the other workspaces
//! join the gate with their own milestones.

use serde::{Deserialize, Serialize};
use specta::Type;

/// The workspace whose draft reports its dirty flag to the window close gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum ShellDirtyWorkspace {
    /// The job editor.
    Job,
    /// The profile editor, including its personal details.
    Profile,
}

#[cfg(test)]
mod tests {
    use super::ShellDirtyWorkspace;

    #[test]
    fn the_workspace_names_cross_ipc_in_lowercase() -> Result<(), serde_json::Error> {
        assert_eq!(
            serde_json::to_string(&ShellDirtyWorkspace::Job)?,
            r#""job""#
        );
        let profile: ShellDirtyWorkspace = serde_json::from_str(r#""profile""#)?;
        assert_eq!(profile, ShellDirtyWorkspace::Profile);
        assert!(serde_json::from_str::<ShellDirtyWorkspace>(r#""match""#).is_err());
        Ok(())
    }
}
