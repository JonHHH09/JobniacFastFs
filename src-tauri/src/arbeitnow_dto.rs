//! Wire types of the Arbeitnow screen.
//!
//! Candidate tokens and the next-page cursor never appear here: a row is addressed by its opaque
//! `rowId`, which Rust issues per scan and holds in the session table.

use serde::{Deserialize, Serialize};
use specta::Type;

/// Whether the Arbeitnow services are composed. The page is hidden
/// when this is `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArbeitnowCapabilitiesDto {
    /// `true` only when the exact `JOBNIAC_ARBEITNOW_ENABLED=1` flag, the keys and a database are present.
    pub arbeitnow: bool,
}

/// The scan form. Query and location are trimmed by Rust (blank is absent); tags and job types stay
/// raw text and Rust splits them on commas.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArbeitnowScanRequestDto {
    /// Search terms, or `null` when blank.
    pub query: Option<String>,
    /// Location filter, or `null` when blank.
    pub location: Option<String>,
    /// Remote-only flag; `false` is never sent to the service.
    pub remote_only: bool,
    /// Visa sponsorship flag; `false` is never sent to the service.
    pub visa_sponsorship: bool,
    /// Comma-separated tags, raw text.
    pub tags: String,
    /// Comma-separated job types, raw text.
    pub job_types: String,
}

/// The import status of a result row as returned by a scan or a next page. Always `none`: a returned
/// row is never imported yet. The screen records the status of a row from the import result, which
/// carries `createdJob` or `reusedExistingJob` ([`ArbeitnowImportStatusDto`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ArbeitnowRowStatusDto {
    /// Not imported in this session.
    None,
}

/// One result row. The posting `url` is not carried: the screen never shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArbeitnowRowDto {
    /// The opaque row id of this scan; the only handle for an import.
    pub row_id: String,
    /// Job title, plain text.
    pub title: String,
    /// Company name, plain text.
    pub company: String,
    /// Location, plain text.
    pub location: String,
    /// Remote flag.
    pub remote: bool,
    /// UTC posting date `YYYY-MM-DD`, or `null`.
    pub posted_at: Option<String>,
    /// First 280 code points of the description.
    pub excerpt: String,
    /// Tags.
    pub tags: Vec<String>,
    /// Job types.
    pub job_types: Vec<String>,
    /// Import status in this session.
    pub import_status: ArbeitnowRowStatusDto,
}

/// A scan or next-page result: the new rows, whether a next page exists and the upstream notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArbeitnowPageDto {
    /// The rows of this response: all rows of a scan, the appended rows of a next page.
    pub rows: Vec<ArbeitnowRowDto>,
    /// Whether a next page can be loaded.
    pub has_next_page: bool,
    /// The consistency notice of the service, plain text.
    pub consistency_notice: String,
}

/// The outcome of one import: created or reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ArbeitnowImportStatusDto {
    /// A new job was written.
    CreatedJob,
    /// The posting was saved before; nothing was written.
    ReusedExistingJob,
}

/// The result of one import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArbeitnowImportResultDto {
    /// The row that was imported.
    pub row_id: String,
    /// Created or reused.
    pub status: ArbeitnowImportStatusDto,
    /// The stored job, as a UUID string.
    pub job_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_capability_is_a_camel_case_flag() -> Result<(), serde_json::Error> {
        assert_eq!(
            serde_json::to_value(ArbeitnowCapabilitiesDto { arbeitnow: true })?,
            json!({"arbeitnow": true})
        );
        Ok(())
    }

    #[test]
    fn the_row_status_and_import_status_use_the_wire_words() -> Result<(), serde_json::Error> {
        assert_eq!(
            serde_json::to_value(ArbeitnowRowStatusDto::None)?,
            json!("none")
        );
        assert_eq!(
            serde_json::to_value(ArbeitnowImportStatusDto::CreatedJob)?,
            json!("createdJob")
        );
        Ok(())
    }

    #[test]
    fn the_scan_request_reads_camel_case_keys() -> Result<(), serde_json::Error> {
        let request: ArbeitnowScanRequestDto = serde_json::from_value(json!({
            "query": null,
            "location": "Berlin",
            "remoteOnly": true,
            "visaSponsorship": false,
            "tags": "rust, go",
            "jobTypes": ""
        }))?;
        assert_eq!(request.query, None);
        assert_eq!(request.location.as_deref(), Some("Berlin"));
        assert!(request.remote_only);
        assert_eq!(request.job_types, "");
        Ok(())
    }
}
