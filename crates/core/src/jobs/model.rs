//! Job aggregate records and their JSON shape.
//!
//! Field names serialize in camelCase, in a fixed key order. Instants are
//! written with [`format_instant`] (`Instant.toString()` form) and read with [`parse_instant`].
//! `link_ingestion` is part of the aggregate shape but its link-create path lives in the link use case: the
//! record carries the identity needed by the fingerprint rule and keeps every other column of the
//! row in `fields`, so an update writes it back unchanged.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use time::OffsetDateTime;
use uuid::Uuid;

/// The job row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    /// Identifier of the job.
    pub id: Uuid,
    /// Ingestion method: `text` for paste-first capture, `link` for link ingestion.
    pub source_method: String,
    /// Optional label of the source, trimmed.
    pub source_label: Option<String>,
    /// Job title.
    pub title: String,
    /// Optional company, trimmed.
    pub company: Option<String>,
    /// Optional location, trimmed.
    pub location: Option<String>,
    /// Description of the job.
    pub description: String,
    /// Optional experience requirement, trimmed.
    pub experience_requirement: Option<String>,
    /// Optional employment type, trimmed.
    pub employment_type: Option<String>,
    /// Optional seniority, trimmed.
    pub seniority: Option<String>,
    /// Optional posting instant (UTC).
    #[serde(with = "optional_instant")]
    pub posted_at: Option<OffsetDateTime>,
    /// SHA-256 hex of the normalized title, company, location and description.
    pub canonical_fingerprint: String,
    /// Creation instant.
    #[serde(with = "instant")]
    pub created_at: OffsetDateTime,
    /// Last update instant.
    #[serde(with = "instant")]
    pub updated_at: OffsetDateTime,
    /// Revision: starts at 0 and increases by one per successful write.
    pub revision: i64,
}

/// One skill row of a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSkillRecord {
    /// Identifier of the skill row.
    pub id: Uuid,
    /// The owning job.
    pub job_id: Uuid,
    /// The skill as written (trimmed).
    pub skill: String,
    /// The normalized key used for de-duplication and search.
    pub normalized_skill: String,
    /// Always true for captured and updated skills.
    pub required: bool,
    /// Position of the skill in the job's list, from 0.
    pub display_order: i64,
    /// Creation instant of the row.
    #[serde(with = "instant")]
    pub created_at: OffsetDateTime,
}

/// The provenance of pasted text: a hash of the canonical text, never the text itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextIngestionRecord {
    /// Identifier of the provenance row.
    pub id: Uuid,
    /// The owning job.
    pub job_id: Uuid,
    /// The source label of the capture, trimmed.
    pub source_label: Option<String>,
    /// SHA-256 hex of the canonical input text.
    pub input_text_hash: String,
    /// Creation instant of the row.
    #[serde(with = "instant")]
    pub created_at: OffsetDateTime,
}

/// The provenance of a link-sourced job. Only the identity and the normalized URL are typed here
/// (the fingerprint rule needs them); the other columns of the row travel in `fields` until the
/// link-create path types them.
///
/// Serialization writes the keys in a fixed order: `id`, `jobId`, then the other
/// columns in stored order with `normalizedUrl` right after `url`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkIngestionRecord {
    /// Identifier of the provenance row.
    pub id: Uuid,
    /// The owning job.
    pub job_id: Uuid,
    /// The canonical source URL that the fingerprint includes.
    pub normalized_url: String,
    /// Every other column of the row, as stored.
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

impl Serialize for LinkIngestionRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("jobId", &self.job_id)?;
        let mut normalized_written = false;
        for (key, value) in &self.fields {
            map.serialize_entry(key, value)?;
            if key == "url" {
                map.serialize_entry("normalizedUrl", &self.normalized_url)?;
                normalized_written = true;
            }
        }
        if !normalized_written {
            map.serialize_entry("normalizedUrl", &self.normalized_url)?;
        }
        map.end()
    }
}

/// A complete job aggregate: `{job, skills, linkIngestion, textIngestion}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobAggregate {
    /// The job row.
    pub job: JobRecord,
    /// The skill rows, in display order.
    pub skills: Vec<JobSkillRecord>,
    /// The link provenance, or `None` for a text-sourced job.
    #[serde(default)]
    pub link_ingestion: Option<LinkIngestionRecord>,
    /// The text provenance, or `None` for a link-sourced job.
    #[serde(default)]
    pub text_ingestion: Option<TextIngestionRecord>,
}

/// Outcome of a paste-first capture: whether a new job was created or an existing one reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStatus {
    /// A new job was written.
    CreatedJob,
    /// The text hash or the canonical fingerprint matched an existing job; nothing was written.
    ReusedExistingJob,
}

/// The `{status, job}` envelope of a capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobCaptureResult {
    /// The outcome.
    pub status: CaptureStatus,
    /// The job aggregate, as persisted.
    pub job: JobAggregate,
}

/// One bounded page of job rows: `{jobs, nextCursor}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobListPage {
    /// The job rows of the page, without ingestion text.
    pub jobs: Vec<JobRecord>,
    /// The cursor of the next page, or null.
    pub next_cursor: Option<String>,
}

/// Source of new identifiers. Production uses random version-4 identifiers; tests substitute a
/// counter so that the order of generation is observable.
pub trait IdSource: Send + Sync + std::fmt::Debug {
    /// The next identifier.
    fn next_id(&self) -> Uuid;
}

/// Random version-4 identifiers.
#[derive(Debug, Clone, Copy, Default)]
pub struct RandomIds;

impl IdSource for RandomIds {
    fn next_id(&self) -> Uuid {
        Uuid::new_v4()
    }
}

/// `#[serde(with)]` adapter for an instant in the `Instant.toString()` form.
mod instant {
    use serde::{Deserialize, Deserializer, Serializer};
    use time::OffsetDateTime;

    use crate::clock::{format_instant, parse_instant};

    pub(super) fn serialize<S: Serializer>(
        value: &OffsetDateTime,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format_instant(*value))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<OffsetDateTime, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse_instant(&text).map_err(|error| serde::de::Error::custom(error.to_string()))
    }
}

/// `#[serde(with)]` adapter for an optional instant; null stays null.
mod optional_instant {
    use serde::{Deserialize, Deserializer, Serializer};
    use time::OffsetDateTime;

    use crate::clock::{format_instant, parse_instant};

    pub(super) fn serialize<S: Serializer>(
        value: &Option<OffsetDateTime>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(instant) => serializer.serialize_str(&format_instant(*instant)),
            None => serializer.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<OffsetDateTime>, D::Error> {
        match Option::<String>::deserialize(deserializer)? {
            Some(text) => parse_instant(&text)
                .map(Some)
                .map_err(|error| serde::de::Error::custom(error.to_string())),
            None => Ok(None),
        }
    }
}
