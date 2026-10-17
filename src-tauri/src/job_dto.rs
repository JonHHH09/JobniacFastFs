//! IPC mirror of the job use-case results.
//!
//! Field names are camelCase. Revisions and every other `i64` cross as decimal strings.
//! Nullable text fields are sent as empty strings, because the screens show them as text and
//! missing values are coerced to empty strings. Instants are the `format_instant`
//! text. Embeddings, raw text inputs and link or text provenance never cross IPC.

use jobniac_core::clock::format_instant;
use jobniac_core::jobs::{
    CaptureStatus, JobAggregate, JobCaptureResult, JobListPage, JobRecord, JobSearchResponse,
};
use serde::Serialize;
use specta::Type;

/// A job row of the saved-jobs list and of the search results (the UI reads these four fields).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobSummaryDto {
    pub id: String,
    pub title: String,
    pub company: String,
    pub location: String,
}

impl From<&JobRecord> for JobSummaryDto {
    fn from(job: &JobRecord) -> Self {
        Self {
            id: job.id.to_string(),
            title: job.title.clone(),
            company: job.company.clone().unwrap_or_default(),
            location: job.location.clone().unwrap_or_default(),
        }
    }
}

/// One page of the saved-jobs list. An empty `nextCursor` (null) means there is no next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobPageDto {
    pub jobs: Vec<JobSummaryDto>,
    pub next_cursor: Option<String>,
}

impl From<&JobListPage> for JobPageDto {
    fn from(page: &JobListPage) -> Self {
        Self {
            jobs: page.jobs.iter().map(JobSummaryDto::from).collect(),
            next_cursor: page.next_cursor.clone(),
        }
    }
}

/// The job row of a detail. `revision` is the compare-and-set value as a decimal string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobFieldsDto {
    pub id: String,
    pub revision: String,
    pub title: String,
    pub company: String,
    pub location: String,
    pub description: String,
    pub source_label: String,
    pub employment_type: String,
    pub seniority: String,
    pub experience_requirement: String,
    pub posted_at: String,
    pub source_method: String,
}

impl From<&JobRecord> for JobFieldsDto {
    fn from(job: &JobRecord) -> Self {
        Self {
            id: job.id.to_string(),
            revision: job.revision.to_string(),
            title: job.title.clone(),
            company: job.company.clone().unwrap_or_default(),
            location: job.location.clone().unwrap_or_default(),
            description: job.description.clone(),
            source_label: job.source_label.clone().unwrap_or_default(),
            employment_type: job.employment_type.clone().unwrap_or_default(),
            seniority: job.seniority.clone().unwrap_or_default(),
            experience_requirement: job.experience_requirement.clone().unwrap_or_default(),
            posted_at: job.posted_at.map(format_instant).unwrap_or_default(),
            source_method: job.source_method.clone(),
        }
    }
}

/// A complete job: the job row and its skills in display order (the skill text only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobDetailDto {
    pub job: JobFieldsDto,
    pub skills: Vec<String>,
}

impl From<&JobAggregate> for JobDetailDto {
    fn from(aggregate: &JobAggregate) -> Self {
        Self {
            job: JobFieldsDto::from(&aggregate.job),
            skills: aggregate
                .skills
                .iter()
                .map(|skill| skill.skill.clone())
                .collect(),
        }
    }
}

/// Outcome of a paste-first capture. The wire values are `created_job` and
/// `reused_existing_job`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStatusDto {
    CreatedJob,
    ReusedExistingJob,
}

impl From<CaptureStatus> for CaptureStatusDto {
    fn from(status: CaptureStatus) -> Self {
        match status {
            CaptureStatus::CreatedJob => Self::CreatedJob,
            CaptureStatus::ReusedExistingJob => Self::ReusedExistingJob,
        }
    }
}

/// The result of a capture: the status and the saved job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobCaptureDto {
    pub status: CaptureStatusDto,
    pub job: JobDetailDto,
}

impl From<&JobCaptureResult> for JobCaptureDto {
    fn from(result: &JobCaptureResult) -> Self {
        Self {
            status: CaptureStatusDto::from(result.status),
            job: JobDetailDto::from(&result.job),
        }
    }
}

/// One search result. The screens show only the job row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobSearchMatchDto {
    pub job: JobSummaryDto,
}

/// The bounded search response. `totalMatches` is null only when the total is unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobSearchDto {
    pub query: String,
    pub total_matches: Option<String>,
    pub matched_count: String,
    pub has_more: bool,
    pub returned_count: u32,
    pub jobs: Vec<JobSearchMatchDto>,
}

impl From<&JobSearchResponse> for JobSearchDto {
    fn from(response: &JobSearchResponse) -> Self {
        Self {
            query: response.query.clone(),
            total_matches: response.total_matches.map(|total| total.to_string()),
            matched_count: response.matched_count.to_string(),
            has_more: response.has_more,
            returned_count: u32::try_from(response.returned_count).unwrap_or(u32::MAX),
            jobs: response
                .jobs
                .iter()
                .map(|found| JobSearchMatchDto {
                    job: JobSummaryDto::from(&found.job),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobniac_core::jobs::{JobSearchMatch, JobSkillRecord};
    use serde_json::json;
    use time::OffsetDateTime;
    use uuid::Uuid;

    const JOB_ID: &str = "00000000-0000-4000-8000-0000000000c1";

    fn record(company: Option<&str>) -> JobRecord {
        let when = OffsetDateTime::UNIX_EPOCH;
        JobRecord {
            id: Uuid::parse_str(JOB_ID).unwrap_or(Uuid::nil()),
            source_method: "text".to_owned(),
            source_label: None,
            title: "Synthetic Engineer".to_owned(),
            company: company.map(str::to_owned),
            location: None,
            description: "Synthetic description".to_owned(),
            experience_requirement: None,
            employment_type: None,
            seniority: None,
            posted_at: Some(when),
            canonical_fingerprint: "fingerprint".to_owned(),
            created_at: when,
            updated_at: when,
            revision: 12,
        }
    }

    fn aggregate() -> JobAggregate {
        let job = record(Some("Example Labs"));
        JobAggregate {
            skills: vec![JobSkillRecord {
                id: Uuid::nil(),
                job_id: job.id,
                skill: "Rust".to_owned(),
                normalized_skill: "rust".to_owned(),
                required: false,
                display_order: 0,
                created_at: job.created_at,
            }],
            job,
            link_ingestion: None,
            text_ingestion: None,
        }
    }

    #[test]
    fn summaries_show_missing_text_as_empty_strings() {
        let summary = JobSummaryDto::from(&record(None));
        assert_eq!(summary.company, "");
        assert_eq!(summary.location, "");
        assert_eq!(summary.id, JOB_ID);
    }

    #[test]
    fn a_detail_keeps_the_revision_as_text_and_the_skills_as_names() -> Result<(), serde_json::Error>
    {
        let json = serde_json::to_value(JobDetailDto::from(&aggregate()))?;
        assert_eq!(json["job"]["revision"], "12");
        assert_eq!(json["job"]["sourceMethod"], "text");
        assert_eq!(json["job"]["postedAt"], "1970-01-01T00:00:00Z");
        assert_eq!(json["job"]["sourceLabel"], "");
        assert_eq!(json["skills"], json!(["Rust"]));
        Ok(())
    }

    #[test]
    fn a_capture_status_uses_the_wire_values() -> Result<(), serde_json::Error> {
        let result = JobCaptureResult {
            status: CaptureStatus::ReusedExistingJob,
            job: aggregate(),
        };
        let json = serde_json::to_value(JobCaptureDto::from(&result))?;
        assert_eq!(json["status"], "reused_existing_job");
        assert_eq!(json["job"]["job"]["revision"], "12");
        Ok(())
    }

    #[test]
    fn a_search_keeps_counts_as_text_and_null_when_the_total_is_unknown()
    -> Result<(), serde_json::Error> {
        let response = JobSearchResponse {
            query: "rust".to_owned(),
            query_tokens: vec!["rust".to_owned()],
            total_matches: None,
            matched_count: 3,
            has_more: true,
            returned_count: 1,
            jobs: vec![JobSearchMatch {
                job: record(Some("Example Labs")),
                skills: Vec::new(),
                score: 10,
                matched_fields: vec!["job.title".to_owned()],
            }],
        };
        let json = serde_json::to_value(JobSearchDto::from(&response))?;
        assert_eq!(json["totalMatches"], serde_json::Value::Null);
        assert_eq!(json["matchedCount"], "3");
        assert_eq!(json["hasMore"], true);
        assert_eq!(json["returnedCount"], 1);
        assert_eq!(json["jobs"][0]["job"]["company"], "Example Labs");
        assert!(json["jobs"][0].get("skills").is_none());
        Ok(())
    }

    #[test]
    fn a_page_keeps_the_cursor_and_the_rows() -> Result<(), serde_json::Error> {
        let page = JobListPage {
            jobs: vec![record(None)],
            next_cursor: Some("cursor-2".to_owned()),
        };
        let json = serde_json::to_value(JobPageDto::from(&page))?;
        assert_eq!(json["nextCursor"], "cursor-2");
        assert_eq!(json["jobs"][0]["title"], "Synthetic Engineer");
        Ok(())
    }
}
