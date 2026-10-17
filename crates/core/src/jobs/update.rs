//! Revision-checked job update, split into its pure steps.
//!
//! The service runs, in order: [`parse_update`] (every request check, before any repository call),
//! the read of the stored aggregate, the revision check, [`planned_fingerprint`] and the duplicate
//! check, the revision-maximum check, and [`build_update`] followed by the compare-and-set write.
//! Omitted and null text fields keep the stored value; a blank value of an optional field clears it;
//! a blank `title` or `description` is refused. The fingerprint is recomputed on every update and
//! includes the canonical source URL of a link-sourced job.

use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::errors::AppError;

use super::model::{IdSource, JobAggregate, JobRecord, JobSkillRecord};
use super::request::{
    expected_revision, instant_field, job_id_field, job_invalid, mapping, optional_text,
    update_skill_names,
};
use super::text::{fingerprint, normalized_key};

/// The editable text fields, in the order they are validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextField {
    /// `sourceLabel`.
    SourceLabel,
    /// `title` (required: a blank value is refused).
    Title,
    /// `company`.
    Company,
    /// `location`.
    Location,
    /// `description` (required: a blank value is refused).
    Description,
    /// `experienceRequirement`.
    ExperienceRequirement,
    /// `employmentType`.
    EmploymentType,
    /// `seniority`.
    Seniority,
}

impl TextField {
    /// Every editable text field, in validation order.
    pub const ORDER: [TextField; 8] = [
        TextField::SourceLabel,
        TextField::Title,
        TextField::Company,
        TextField::Location,
        TextField::Description,
        TextField::ExperienceRequirement,
        TextField::EmploymentType,
        TextField::Seniority,
    ];

    /// The wire key of the field.
    pub const fn name(self) -> &'static str {
        match self {
            Self::SourceLabel => "sourceLabel",
            Self::Title => "title",
            Self::Company => "company",
            Self::Location => "location",
            Self::Description => "description",
            Self::ExperienceRequirement => "experienceRequirement",
            Self::EmploymentType => "employmentType",
            Self::Seniority => "seniority",
        }
    }

    /// Whether a blank value is refused rather than cleared.
    pub const fn is_required(self) -> bool {
        matches!(self, Self::Title | Self::Description)
    }

    fn assign(self, job: &mut JobRecord, value: Option<String>) {
        match self {
            Self::SourceLabel => job.source_label = value,
            Self::Title => job.title = value.unwrap_or_default(),
            Self::Company => job.company = value,
            Self::Location => job.location = value,
            Self::Description => job.description = value.unwrap_or_default(),
            Self::ExperienceRequirement => job.experience_requirement = value,
            Self::EmploymentType => job.employment_type = value,
            Self::Seniority => job.seniority = value,
        }
    }
}

/// A validated update request: every check has passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRequest {
    /// The job to update.
    pub job_id: Uuid,
    /// The revision the caller read.
    pub expected_revision: i64,
    /// The supplied text fields. A `None` value clears the field; `title` and `description` never
    /// carry `None`.
    pub text: Vec<(TextField, Option<String>)>,
    /// The new posting instant, when supplied. An omitted or null value keeps the stored one.
    pub posted_at: Option<OffsetDateTime>,
    /// The replacement skills, when supplied (an empty list clears them). `None` keeps the stored
    /// skills.
    pub skills: Option<Vec<String>>,
}

/// Validates an update request, in this order: the request object, `jobId`, the supplied text
/// fields (each type check, then the blank check of a required field), `expectedRevision`,
/// `postedAt`, `skills`.
///
/// # Errors
///
/// `Invalid job request` for the first invalid field. No repository call precedes this check.
pub fn parse_update(request: &Value) -> Result<UpdateRequest, AppError> {
    let request = mapping(request)?;
    let job_id = job_id_field(request.get("jobId"))?;
    let mut text = Vec::new();
    for field in TextField::ORDER {
        if matches!(request.get(field.name()), None | Some(Value::Null)) {
            continue;
        }
        let value = optional_text(request, field.name())?;
        if field.is_required() && value.is_none() {
            return Err(job_invalid(field.name(), "must not be blank"));
        }
        text.push((field, value));
    }
    let expected = expected_revision(request.get("expectedRevision"))?;
    let posted_at = instant_field(request.get("postedAt"), "postedAt")?;
    let skills = update_skill_names(request)?;
    Ok(UpdateRequest {
        job_id,
        expected_revision: expected,
        text,
        posted_at,
        skills,
    })
}

/// The job row after the supplied text fields are applied to the stored one.
fn patched_job(existing: &JobAggregate, update: &UpdateRequest) -> JobRecord {
    let mut job = existing.job.clone();
    for (field, value) in &update.text {
        field.assign(&mut job, value.clone());
    }
    job
}

/// The fingerprint the updated job will carry (computed before the duplicate check). It
/// includes the normalized URL of the stored link provenance, when there is one.
pub fn planned_fingerprint(existing: &JobAggregate, update: &UpdateRequest) -> String {
    let job = patched_job(existing, update);
    let link_url = existing
        .link_ingestion
        .as_ref()
        .map(|link| link.normalized_url.as_str());
    fingerprint(
        &job.title,
        job.company.as_deref(),
        job.location.as_deref(),
        &job.description,
        link_url,
    )
}

/// Builds the aggregate that the compare-and-set write stores: the patched job with the given
/// fingerprint, `updated_at` = `now` and `revision` = the expected revision plus one; the posting
/// instant when supplied; the replacement skills (fresh identifiers, in display order) or the stored
/// ones; and the stored provenance rows, unchanged.
///
/// # Errors
///
/// [`AppError::Internal`] when the expected revision is the signed 64-bit maximum (the service
/// refuses that revision before calling this function).
pub fn build_update(
    existing: &JobAggregate,
    update: &UpdateRequest,
    fingerprint: &str,
    now: OffsetDateTime,
    ids: &dyn IdSource,
) -> Result<JobAggregate, AppError> {
    let mut job = patched_job(existing, update);
    job.canonical_fingerprint = fingerprint.to_owned();
    job.updated_at = now;
    job.revision = update
        .expected_revision
        .checked_add(1)
        .ok_or(AppError::Internal)?;
    if let Some(posted_at) = update.posted_at {
        job.posted_at = Some(posted_at);
    }
    let skills = match &update.skills {
        None => existing.skills.clone(),
        Some(names) => names
            .iter()
            .enumerate()
            .map(|(index, name)| JobSkillRecord {
                id: ids.next_id(),
                job_id: update.job_id,
                skill: name.clone(),
                normalized_skill: normalized_key(Some(name)),
                required: true,
                display_order: i64::try_from(index).unwrap_or(i64::MAX),
                created_at: now,
            })
            .collect(),
    };
    Ok(JobAggregate {
        job,
        skills,
        link_ingestion: existing.link_ingestion.clone(),
        text_ingestion: existing.text_ingestion.clone(),
    })
}
