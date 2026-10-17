//! The error shape that crosses IPC.

use serde::Serialize;
use specta::Type;

/// A failed command: a stable code and a message id. The UI renders the copy for the id from its
/// catalogue; no source text, path or secret is ever sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    /// Wire code, such as `internal_error` or `conflict`.
    pub code: String,
    /// Identifier of the copy in the UI catalogue, such as `startup.failed`.
    pub message_id: String,
}

impl CommandError {
    /// Build an error from a code and a message id.
    pub fn new(code: &str, message_id: &str) -> Self {
        Self {
            code: code.to_owned(),
            message_id: message_id.to_owned(),
        }
    }

    /// The composition root failed at startup; the services do not exist.
    pub fn startup_failed() -> Self {
        Self::new("internal_error", "startup.failed")
    }

    /// The same operation is already running; the request is refused, not queued. The wire code is
    /// `busy` (a guard refusal), kept apart from a real `conflict` (a stale revision).
    pub fn busy(message_id: &str) -> Self {
        Self::new("busy", message_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_failure_wire_shape() -> Result<(), serde_json::Error> {
        let json = serde_json::to_string(&CommandError::startup_failed())?;
        assert_eq!(
            json,
            r#"{"code":"internal_error","messageId":"startup.failed"}"#
        );
        Ok(())
    }

    #[test]
    fn a_guard_refusal_is_busy_not_conflict() {
        assert_eq!(CommandError::busy("job.busy").code, "busy");
    }
}
