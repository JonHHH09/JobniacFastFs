//! The search postings of a job against the golden vectors in `golden/job-search-terms.json`,
//! recorded on synthetic jobs. No database is involved.

use std::error::Error;

use jobniac_core::jobs::{JobRecord, JobSkillRecord};
use jobniac_infra::sqlite::jobs::terms::job_search_terms;
use serde_json::Value;
use time::macros::datetime;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const VECTORS: &str = include_str!("golden/job-search-terms.json");

/// A required text value of a vector object.
fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str, Box<dyn Error>> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("vector field {key} must be a string").into())
}

/// An optional text value of a vector object (`null` is absent).
fn optional(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::to_owned)
}

/// The job row of a vector, with the fields that the postings read.
fn job_of(job: &Value, id: Uuid) -> Result<JobRecord, Box<dyn Error>> {
    let created_at = datetime!(2026-09-29 10:00 UTC);
    Ok(JobRecord {
        id,
        source_method: "text".to_owned(),
        source_label: None,
        title: required(job, "title")?.to_owned(),
        company: optional(job, "company"),
        location: optional(job, "location"),
        description: required(job, "description")?.to_owned(),
        experience_requirement: optional(job, "experienceRequirement"),
        employment_type: optional(job, "employmentType"),
        seniority: optional(job, "seniority"),
        posted_at: None,
        canonical_fingerprint: "0".repeat(64),
        created_at,
        updated_at: created_at,
        revision: 0,
    })
}

#[test]
fn the_postings_of_every_scenario_match_the_golden_vectors() -> TestResult {
    let document: Value = serde_json::from_str(VECTORS)?;
    let job_id = Uuid::parse_str(required(&document, "jobId")?)?;
    let scenarios = document["scenarios"]
        .as_array()
        .ok_or("the vectors must hold a scenario list")?;
    assert!(!scenarios.is_empty());
    let mut compared = 0_usize;
    for scenario in scenarios {
        let name = required(scenario, "name")?;
        let job = job_of(&scenario["job"], job_id)?;
        let mut skills = Vec::new();
        for (index, skill) in scenario["skills"]
            .as_array()
            .ok_or("the skills must be a list")?
            .iter()
            .enumerate()
        {
            skills.push(JobSkillRecord {
                id: Uuid::parse_str(required(skill, "id")?)?,
                job_id,
                skill: required(skill, "skill")?.to_owned(),
                normalized_skill: String::new(),
                required: true,
                display_order: i64::try_from(index)?,
                created_at: job.created_at,
            });
        }
        let actual: Vec<(String, String, i64)> = job_search_terms(&job, &skills)
            .into_iter()
            .map(|term| (term.field_key, term.term, term.weight))
            .collect();
        let expected: Vec<(String, String, i64)> = scenario["terms"]
            .as_array()
            .ok_or("the postings must be a list")?
            .iter()
            .map(|term| -> Result<(String, String, i64), Box<dyn Error>> {
                let field = term[0].as_str().ok_or("the field must be a string")?;
                let text = term[1].as_str().ok_or("the term must be a string")?;
                let weight = term[2].as_i64().ok_or("the weight must be an integer")?;
                Ok((field.to_owned(), text.to_owned(), weight))
            })
            .collect::<Result<_, _>>()?;
        compared += expected.len();
        assert_eq!(actual, expected, "scenario {name}");
    }
    assert!(
        compared > 40,
        "the vectors must hold a meaningful number of postings"
    );
    Ok(())
}
