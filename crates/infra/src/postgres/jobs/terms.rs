//! Search postings of a job.
//!
//! The field keys and weights are:  `job.title` 8, `job.company` 5, `job.location` 3,
//! `job.description` 3, `job.experienceRequirement` 4, `job.employmentType` 2, `job.seniority` 2, and
//! one `skills:<skill id>` key of weight 7 per skill. The tokens come from the shared tokenizer, so a
//! field contributes each distinct token once.

use jobniac_core::jobs::{JobRecord, JobSkillRecord};
use jobniac_text::search::SearchTerm;

/// Weight of each job text field, in the order of the postings.
const JOB_FIELDS: [(&str, u8); 7] = [
    ("job.title", 8),
    ("job.company", 5),
    ("job.location", 3),
    ("job.description", 3),
    ("job.experienceRequirement", 4),
    ("job.employmentType", 2),
    ("job.seniority", 2),
];

/// Weight of each skill name.
const SKILL_WEIGHT: i64 = 7;

/// Every posting of a job: its text fields in that order, then each skill.
pub fn job_search_terms(job: &JobRecord, skills: &[JobSkillRecord]) -> Vec<SearchTerm> {
    let values: [Option<&str>; 7] = [
        Some(job.title.as_str()),
        job.company.as_deref(),
        job.location.as_deref(),
        Some(job.description.as_str()),
        job.experience_requirement.as_deref(),
        job.employment_type.as_deref(),
        job.seniority.as_deref(),
    ];
    let mut terms = Vec::new();
    for ((field_key, weight), value) in JOB_FIELDS.iter().zip(values) {
        terms.extend(SearchTerm::from_text(
            job.id,
            field_key,
            value,
            i64::from(*weight),
        ));
    }
    for skill in skills {
        terms.extend(SearchTerm::from_text(
            job.id,
            &format!("skills:{}", skill.id),
            Some(skill.skill.as_str()),
            SKILL_WEIGHT,
        ));
    }
    terms
}
