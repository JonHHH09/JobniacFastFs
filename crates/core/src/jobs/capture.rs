//! Paste-first capture (`add_job_from_text`), split at the duplicate lookup.
//!
//! The order is: validate the pasted text, hash it, look the hash up (a duplicate returns
//! before any optional field is looked at), then validate the optional fields, derive the title,
//! experience and skills, and build the aggregate. The service runs the lookup between the two
//! halves, so the order of validation and of repository calls is fixed.

use jobniac_text::java::java_strip;
use serde_json::Value;
use time::OffsetDateTime;

use crate::errors::AppError;

use super::model::{IdSource, JobAggregate, JobRecord, JobSkillRecord, TextIngestionRecord};
use super::request::{explicit_skills, instant_field, job_invalid, mapping, optional_text};
use super::text::{
    derive_title, extract_experience, extract_skills, fingerprint, input_text_hash, merge_skills,
    normalized_key,
};

/// The validated pasted text and its hash (the first half of the capture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureHash {
    /// The pasted text after the Java strip.
    pub input_text: String,
    /// SHA-256 hex of the canonical text; the duplicate key of the capture.
    pub input_text_hash: String,
}

/// The validated optional fields of a new text job (the second half of the capture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextJobFields {
    input_text_hash: String,
    source_label: Option<String>,
    title: String,
    company: Option<String>,
    location: Option<String>,
    description: String,
    experience_requirement: Option<String>,
    employment_type: Option<String>,
    seniority: Option<String>,
    posted_at: Option<OffsetDateTime>,
    skills: Vec<String>,
    canonical_fingerprint: String,
}

/// Validates the pasted text and computes its hash. Nothing else is validated here.
///
/// # Errors
///
/// `Invalid job request` with field `request` for a non-object request, or field `text` (reason
/// `must not be blank`) when the text is missing, not a string, or blank after the Java strip.
pub fn capture_hash(request: &Value) -> Result<CaptureHash, AppError> {
    let request = mapping(request)?;
    let text = match request.get("text") {
        Some(Value::String(text)) if !java_strip(text).is_empty() => text,
        _ => return Err(job_invalid("text", "must not be blank")),
    };
    let input_text = java_strip(text).to_owned();
    let hash = input_text_hash(&input_text);
    Ok(CaptureHash {
        input_text,
        input_text_hash: hash,
    })
}

/// Validates the optional fields of a new text job and derives the title, the experience
/// requirement, the skills and the canonical fingerprint. Runs after the duplicate lookup.
///
/// # Errors
///
/// `Invalid job request` for the first invalid optional field, in this order: `sourceLabel`,
/// `title`, `company`, `location`, `description`, `experienceRequirement`, `employmentType`,
/// `seniority`, `skills`, `postedAt`.
pub fn prepare_text_job(request: &Value, capture: &CaptureHash) -> Result<TextJobFields, AppError> {
    let request = mapping(request)?;
    let source_label = optional_text(request, "sourceLabel")?;
    let title_override = optional_text(request, "title")?;
    let company = optional_text(request, "company")?;
    let location = optional_text(request, "location")?;
    let description =
        optional_text(request, "description")?.unwrap_or_else(|| capture.input_text.clone());
    let experience_override = optional_text(request, "experienceRequirement")?;
    let employment_type = optional_text(request, "employmentType")?;
    let seniority = optional_text(request, "seniority")?;
    let explicit = explicit_skills(request)?;
    let posted_at = instant_field(request.get("postedAt"), "postedAt")?;

    let title = title_override.unwrap_or_else(|| derive_title(&capture.input_text));
    let experience_requirement = experience_override.or_else(|| extract_experience(&description));
    let skills = merge_skills(&explicit, &extract_skills(&description));
    let canonical_fingerprint = fingerprint(
        &title,
        company.as_deref(),
        location.as_deref(),
        &description,
        None,
    );
    Ok(TextJobFields {
        input_text_hash: capture.input_text_hash.clone(),
        source_label,
        title,
        company,
        location,
        description,
        experience_requirement,
        employment_type,
        seniority,
        posted_at,
        skills,
        canonical_fingerprint,
    })
}

/// Builds the aggregate of a new text job with `created_at` and `updated_at` set to `now`.
///
/// Identifiers are drawn in this order: the job, then each skill in display order, then the
/// text provenance.
pub fn assemble_text_job(
    fields: TextJobFields,
    now: OffsetDateTime,
    ids: &dyn IdSource,
) -> JobAggregate {
    let job_id = ids.next_id();
    let skills = fields
        .skills
        .iter()
        .enumerate()
        .map(|(index, skill)| JobSkillRecord {
            id: ids.next_id(),
            job_id,
            skill: skill.clone(),
            normalized_skill: normalized_key(Some(skill)),
            required: true,
            display_order: i64::try_from(index).unwrap_or(i64::MAX),
            created_at: now,
        })
        .collect();
    let text_ingestion = TextIngestionRecord {
        id: ids.next_id(),
        job_id,
        source_label: fields.source_label.clone(),
        input_text_hash: fields.input_text_hash,
        created_at: now,
    };
    let job = JobRecord {
        id: job_id,
        source_method: "text".to_owned(),
        source_label: fields.source_label,
        title: fields.title,
        company: fields.company,
        location: fields.location,
        description: fields.description,
        experience_requirement: fields.experience_requirement,
        employment_type: fields.employment_type,
        seniority: fields.seniority,
        posted_at: fields.posted_at,
        canonical_fingerprint: fields.canonical_fingerprint,
        created_at: now,
        updated_at: now,
        revision: 0,
    };
    JobAggregate {
        job,
        skills,
        link_ingestion: None,
        text_ingestion: Some(text_ingestion),
    }
}
