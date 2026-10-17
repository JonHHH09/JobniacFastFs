//! Wire types of "Add from link".
//!
//! The capture result is the job capture of the paste screen ([`crate::job_dto::JobCaptureDto`]).
//! The link text itself never comes back: it crosses IPC as input only.

use serde::Serialize;
use specta::Type;

/// Whether the link capture is composed. The row is hidden when this
/// is `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobLinkCapabilitiesDto {
    /// `true` only when the exact `JOBNIAC_LINK_FETCH_ENABLED=1` flag and a database are present.
    pub job_link: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_capability_is_serialized_as_a_camel_case_flag() -> Result<(), serde_json::Error> {
        assert_eq!(
            serde_json::to_value(JobLinkCapabilitiesDto { job_link: true })?,
            json!({"jobLink": true})
        );
        assert_eq!(
            serde_json::to_value(JobLinkCapabilitiesDto { job_link: false })?,
            json!({"jobLink": false})
        );
        Ok(())
    }
}
