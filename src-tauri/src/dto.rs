//! IPC mirror of the core health envelope (`jobniac_core::health::HealthReport`).
//!
//! The field names and enum spellings match the core wire contract (camelCase, SCREAMING_SNAKE_CASE
//! values). Counts that come from `u64` cross as decimal strings: they can exceed
//! the JavaScript safe-integer range and would otherwise need `bigint`.

use jobniac_core::health::{
    CleanupReport, CleanupStatus, ErrorCategory, HealthReport, HealthStatus,
};
use serde::Serialize;
use specta::Type;

/// Overall readiness status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HealthStatusDto {
    Up,
    Down,
}

/// Sanitized error category of the readiness probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCategoryDto {
    None,
    ConnectionUnavailable,
    AuthenticationFailed,
    QueryFailed,
    Timeout,
    Unknown,
}

/// Status of the generated-resume cleanup backlog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CleanupStatusDto {
    Unknown,
    Degraded,
    Healthy,
}

/// Check counters of the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HealthMetadataDto {
    /// ISO 8601 text with a `Z` suffix, kept as produced.
    pub checked_at: String,
    pub total_checks: u32,
    pub successful_checks: u32,
    pub failed_checks: u32,
}

/// Observed generated-output cleanup health. Counts are decimal strings (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReportDto {
    pub status: CleanupStatusDto,
    pub pending_count: String,
    pub processing_count: String,
    pub expired_completed_count: String,
    pub oldest_due_age_seconds: String,
    pub repeated_failure: bool,
}

/// Health result returned by the `health_check` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HealthReportDto {
    pub status: HealthStatusDto,
    pub error_category: ErrorCategoryDto,
    pub metadata: HealthMetadataDto,
    pub generated_resume_cleanup: CleanupReportDto,
}

impl From<&HealthReport> for HealthReportDto {
    fn from(report: &HealthReport) -> Self {
        Self {
            status: match report.status {
                HealthStatus::Up => HealthStatusDto::Up,
                HealthStatus::Down => HealthStatusDto::Down,
            },
            error_category: error_category(report.error_category),
            metadata: HealthMetadataDto {
                checked_at: report.metadata.checked_at.clone(),
                total_checks: report.metadata.total_checks,
                successful_checks: report.metadata.successful_checks,
                failed_checks: report.metadata.failed_checks,
            },
            generated_resume_cleanup: cleanup(&report.generated_resume_cleanup),
        }
    }
}

fn error_category(category: ErrorCategory) -> ErrorCategoryDto {
    match category {
        ErrorCategory::None => ErrorCategoryDto::None,
        ErrorCategory::ConnectionUnavailable => ErrorCategoryDto::ConnectionUnavailable,
        ErrorCategory::AuthenticationFailed => ErrorCategoryDto::AuthenticationFailed,
        ErrorCategory::QueryFailed => ErrorCategoryDto::QueryFailed,
        ErrorCategory::Timeout => ErrorCategoryDto::Timeout,
        ErrorCategory::Unknown => ErrorCategoryDto::Unknown,
    }
}

fn cleanup(report: &CleanupReport) -> CleanupReportDto {
    CleanupReportDto {
        status: match report.status {
            CleanupStatus::Unknown => CleanupStatusDto::Unknown,
            CleanupStatus::Degraded => CleanupStatusDto::Degraded,
            CleanupStatus::Healthy => CleanupStatusDto::Healthy,
        },
        pending_count: report.pending_count.to_string(),
        processing_count: report.processing_count.to_string(),
        expired_completed_count: report.expired_completed_count.to_string(),
        oldest_due_age_seconds: report.oldest_due_age_seconds.to_string(),
        repeated_failure: report.repeated_failure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobniac_core::health::HealthMetadata;

    fn down_report() -> HealthReport {
        HealthReport {
            status: HealthStatus::Down,
            error_category: ErrorCategory::ConnectionUnavailable,
            metadata: HealthMetadata {
                checked_at: "2026-10-13T12:00:00.250000Z".to_owned(),
                total_checks: 1,
                successful_checks: 0,
                failed_checks: 1,
            },
            generated_resume_cleanup: CleanupReport {
                status: CleanupStatus::Degraded,
                pending_count: 3,
                processing_count: 1,
                expired_completed_count: 2,
                oldest_due_age_seconds: 9_007_199_254_740_993,
                repeated_failure: true,
            },
        }
    }

    #[test]
    fn report_keeps_the_core_wire_names_and_stringifies_u64_counts() -> Result<(), serde_json::Error>
    {
        let dto = HealthReportDto::from(&down_report());
        let json = serde_json::to_string(&dto)?;
        assert_eq!(
            json,
            concat!(
                r#"{"status":"DOWN","errorCategory":"CONNECTION_UNAVAILABLE","metadata":"#,
                r#"{"checkedAt":"2026-10-13T12:00:00.250000Z","totalChecks":1,"successfulChecks":0,"failedChecks":1},"#,
                r#""generatedResumeCleanup":{"status":"DEGRADED","pendingCount":"3","processingCount":"1","#,
                r#""expiredCompletedCount":"2","oldestDueAgeSeconds":"9007199254740993","repeatedFailure":true}}"#
            )
        );
        Ok(())
    }
}
