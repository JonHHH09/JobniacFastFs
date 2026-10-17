//! Wire types of the profile and job delete.
//!
//! The confirmation token is the only value the screen echoes back. It is an opaque, single-use,
//! short-lived identifier; it names no path and no secret. Identifiers cross as canonical UUID text.

use std::fmt;

use serde::Serialize;
use specta::Type;

/// Whether the deletion use cases are composed. Both the profile
/// and the job use case are composed together, or neither is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DeletionCapabilitiesDto {
    /// `true` only when the exact `JOBNIAC_DELETION_ENABLED=1` flag and a database are present.
    pub deletion: bool,
}

/// The confirmation token issued by a delete request. It must be echoed to the matching confirm
/// command, once, before it expires.
#[derive(Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConfirmationDto {
    /// The opaque, single-use token.
    pub confirmation_token: String,
}

impl fmt::Debug for DeleteConfirmationDto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The token is the confirmation secret: it never appears in debug output.
        f.debug_struct("DeleteConfirmationDto")
            .field("confirmation_token", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::DeleteConfirmationDto;

    #[test]
    fn the_confirmation_debug_text_never_shows_the_token() {
        let dto = DeleteConfirmationDto {
            confirmation_token: "synthetic-token-value".to_owned(),
        };
        let text = format!("{dto:?}");
        assert!(text.contains("<redacted>"));
        assert!(!text.contains("synthetic-token-value"));
    }
}

/// The result of a profile delete (`{ profileId, deleted }`). `deleted: false` means the profile no
/// longer existed, which is not an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDeleteResultDto {
    /// The canonical profile identifier.
    pub profile_id: String,
    /// Whether this call removed the profile.
    pub deleted: bool,
}

/// The result of a job delete (`{ jobId, deleted: true }`). A missing job is an error (`not_found`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobDeleteResultDto {
    /// The canonical job identifier.
    pub job_id: String,
    /// Always `true` on success.
    pub deleted: bool,
}
