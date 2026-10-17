//! The `deterministic-v1` scorer.
//!
//! The score uses only canonical skill names and fixed fact strings; no prose and no inferred
//! evidence enter a report. Weights are 40, 25, 15, 10 and 10 points. All arithmetic is integer, with
//! fixed rounding: the technical floor is `matches * 40 // skills`, and the overall score is
//! round-half-up of `100 * earned / known`.

use std::collections::HashSet;

use jobniac_text::java::java_is_blank;
use jobniac_text::search::lower_text;
use time::OffsetDateTime;

use crate::clock::{parse_instant, truncate_to_micros};
use crate::errors::AppError;

use super::graph::{JobGraph, ProfileGraph, uuid_from_text};
use super::privacy::{require_safe_skill, trim_control_and_space};
use super::report::{
    ALGORITHM_VERSION, Component, ComponentStatus, MatchComponent, MatchEvidence, MatchReport,
    Outcome, report_id, report_identity,
};

/// Skill aliases, applied after lower-casing and only on an exact key match.
const ALIASES: [(&str, &str); 3] = [
    ("postgres", "postgresql"),
    ("k8s", "kubernetes"),
    ("spring boot", "spring"),
];

/// Experience points earned when the requirement and a dated experience row are both present.
const EXPERIENCE_POINTS: i64 = 13;
const EXPERIENCE_FACT: &str = "profile contains dated experience";
const EXPERIENCE_SOURCE: &str = "experience_dates";
const SKILL_SOURCE: &str = "normalized_skill";

/// Scores one profile against one job at instant `now`.
///
/// Validation runs in this order: each job skill is checked as it is scored, then the
/// identifiers and then the revisions are read. The identifier and revision of the report come from
/// the graph text, and `now` only sets `created_at`, which is not part of the identity.
///
/// # Errors
///
/// `DomainValue` with `fact must be a normalized privacy-safe skill` for an unsafe job skill, or the
/// instant message for a revision that is not an ISO-8601 UTC timestamp. `Internal` when an
/// identifier cannot be read as a UUID, which is reported as an unexpected error.
pub fn score_match(
    profile: &ProfileGraph,
    job: &JobGraph,
    now: OffsetDateTime,
) -> Result<MatchReport, AppError> {
    score_match_with_index(profile, &ProfileSkillIndex::new(profile), job, now)
}

/// The canonical skill set of one profile, built once so a batch of jobs does not rebuild it.
#[derive(Debug)]
pub struct ProfileSkillIndex(HashSet<String>);

impl ProfileSkillIndex {
    /// Canonicalizes the skills of `profile`.
    #[must_use]
    pub fn new(profile: &ProfileGraph) -> Self {
        Self(
            profile
                .skills
                .iter()
                .map(|skill| canonical_skill(skill))
                .collect(),
        )
    }
}

/// [`score_match`] with a precomputed [`ProfileSkillIndex`] of the same `profile`.
///
/// # Errors
///
/// The same errors as [`score_match`].
pub fn score_match_with_index(
    profile: &ProfileGraph,
    index: &ProfileSkillIndex,
    job: &JobGraph,
    now: OffsetDateTime,
) -> Result<MatchReport, AppError> {
    let mut evidence = Vec::new();
    let technical = technical_component(&index.0, job, &mut evidence)?;
    let experience = experience_component(profile, job, &mut evidence);
    let components = vec![
        technical,
        experience,
        fixed_component(Component::Domain),
        fixed_component(Component::Delivery),
        fixed_component(Component::HardRequirements),
    ];
    let earned: i64 = components
        .iter()
        .map(|component| component.earned_points)
        .sum();
    let known: i64 = components
        .iter()
        .filter(|component| component.status != ComponentStatus::Unknown)
        .map(|component| component.available_points)
        .sum();
    let overall_score = if known == 0 {
        0
    } else {
        (2 * earned * 100 + known) / (2 * known)
    };
    let outcome = match overall_score {
        _ if known == 0 => Outcome::InsufficientEvidence,
        75.. => Outcome::StrongMatch,
        45..=74 => Outcome::PartialMatch,
        _ => Outcome::WeakMatch,
    };
    let profile_id = uuid_from_text(&profile.id).ok_or(AppError::Internal)?;
    let job_id = uuid_from_text(&job.id).ok_or(AppError::Internal)?;
    let profile_revision = parse_instant(&profile.updated_at)?;
    let job_revision = parse_instant(&job.updated_at)?;
    let identity = report_identity(profile_id, profile_revision, job_id, job_revision);
    Ok(MatchReport {
        id: report_id(&identity),
        profile_id,
        job_id,
        profile_revision,
        job_revision,
        algorithm_version: ALGORITHM_VERSION.to_owned(),
        overall_score,
        confidence: known,
        outcome,
        blocker_mismatch: false,
        components,
        evidence,
        created_at: truncate_to_micros(now),
    })
}

/// The canonical form of a skill: Java-trimmed, lower-cased, then aliased.
pub fn canonical_skill(value: &str) -> String {
    let normalized = lower_text(trim_control_and_space(value));
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == normalized)
        .map_or(normalized, |(_, canonical)| (*canonical).to_owned())
}

fn technical_component(
    profile_skills: &HashSet<String>,
    job: &JobGraph,
    evidence: &mut Vec<MatchEvidence>,
) -> Result<MatchComponent, AppError> {
    if job.skills.is_empty() {
        return Ok(component(Component::Technical, 0, ComponentStatus::Unknown));
    }
    let mut matches: i64 = 0;
    for skill in &job.skills {
        let fact = canonical_skill(skill);
        require_safe_skill(&fact)?;
        let matched = profile_skills.contains(&fact);
        matches += i64::from(matched);
        evidence.push(MatchEvidence {
            component: Component::Technical,
            status: if matched {
                ComponentStatus::Match
            } else {
                ComponentStatus::Mismatch
            },
            source_type: SKILL_SOURCE.to_owned(),
            fact,
            outcome_changing_defect: false,
        });
    }
    let total = i64::try_from(job.skills.len()).map_err(|_| AppError::Internal)?;
    let status = if matches == total {
        ComponentStatus::Match
    } else if matches == 0 {
        ComponentStatus::Mismatch
    } else {
        ComponentStatus::Partial
    };
    Ok(component(
        Component::Technical,
        matches * 40 / total,
        status,
    ))
}

fn experience_component(
    profile: &ProfileGraph,
    job: &JobGraph,
    evidence: &mut Vec<MatchEvidence>,
) -> MatchComponent {
    let requires_experience = [
        job.experience_requirement.as_deref(),
        job.seniority.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| !java_is_blank(value));
    if requires_experience && profile.has_experience {
        evidence.push(MatchEvidence {
            component: Component::ExperienceSeniority,
            status: ComponentStatus::Partial,
            source_type: EXPERIENCE_SOURCE.to_owned(),
            fact: EXPERIENCE_FACT.to_owned(),
            outcome_changing_defect: false,
        });
        component(
            Component::ExperienceSeniority,
            EXPERIENCE_POINTS,
            ComponentStatus::Partial,
        )
    } else {
        component(Component::ExperienceSeniority, 0, ComponentStatus::Unknown)
    }
}

fn fixed_component(name: Component) -> MatchComponent {
    component(name, 0, ComponentStatus::Unknown)
}

fn component(name: Component, earned: i64, status: ComponentStatus) -> MatchComponent {
    MatchComponent {
        component: name,
        earned_points: earned,
        available_points: name.available_points(),
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn canonical_skill_trims_lowers_and_aliases() {
        assert_eq!(canonical_skill("\u{0}\u{20}Postgres\u{1f}"), "postgresql");
        assert_eq!(canonical_skill("K8S"), "kubernetes");
        assert_eq!(canonical_skill("Spring  Boot"), "spring  boot");
        assert_eq!(canonical_skill("Spring Boot"), "spring");
        assert_eq!(canonical_skill("\u{a0}Java"), "\u{a0}java");
    }

    #[test]
    fn indexed_scoring_equals_plain_scoring() {
        let profile = ProfileGraph {
            id: "11111111-1111-4111-8111-111111111111".into(),
            updated_at: "2025-01-02T03:04:05Z".into(),
            skills: vec!["Postgres".into(), "java".into()],
            has_experience: true,
        };
        let job = JobGraph {
            id: "22222222-2222-4222-8222-222222222222".into(),
            updated_at: "2025-02-03T04:05:06Z".into(),
            experience_requirement: Some("3 years".into()),
            seniority: None,
            skills: vec!["postgresql".into(), "rust".into()],
        };
        let now = datetime!(2026-10-02 12:00 UTC);
        let index = ProfileSkillIndex::new(&profile);
        assert_eq!(
            score_match_with_index(&profile, &index, &job, now),
            score_match(&profile, &job, now)
        );
    }

    #[test]
    fn rounding_is_half_up_on_the_known_points() {
        let profile = ProfileGraph {
            id: "11111111-1111-4111-8111-111111111111".into(),
            updated_at: "2025-01-02T03:04:05Z".into(),
            skills: vec!["java".into()],
            has_experience: false,
        };
        let job = JobGraph {
            id: "22222222-2222-4222-8222-222222222222".into(),
            updated_at: "2025-02-03T04:05:06Z".into(),
            experience_requirement: None,
            seniority: None,
            skills: vec!["java".into(), "rust".into(), "go".into()],
        };
        let report = score_match(&profile, &job, datetime!(2026-10-02 12:00 UTC));
        // One of three skills gives 13 earned of 40 known: 100 * 13 / 40 = 32.5, rounded to 33.
        assert_eq!(report.map(|value| value.overall_score), Ok(33));
    }
}
