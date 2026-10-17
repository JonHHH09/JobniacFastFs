//! Preparation budgets of the resume bodies.

use jobniac_text::java::{BudgetField, PreparationBudget, PreparationLimitExceeded};
use time::Date;

use super::ResumeVariant;
use crate::errors::{ApplicationError, validation_error};
use crate::profiles::ProfileGraph;

/// The validation error of an exceeded preparation budget: the message, field and reason of the
/// refusal.
#[must_use]
pub fn budget_error(error: PreparationLimitExceeded) -> ApplicationError {
    validation_error(
        error.message(),
        Some(error.field().as_str()),
        Some(error.reason()),
    )
}

impl From<PreparationLimitExceeded> for ApplicationError {
    fn from(error: PreparationLimitExceeded) -> Self {
        budget_error(error)
    }
}

/// The ISO `YYYY-MM-DD` text of an optional date.
fn iso_text(date: Option<Date>) -> Option<String> {
    date.map(|value| {
        format!(
            "{:04}-{:02}-{:02}",
            value.year(),
            u8::from(value.month()),
            value.day()
        )
    })
}

/// The protected-term candidates in order: the full name, the experience companies, the
/// education institutions, the project names, the skills, the project technologies, then the
/// technologies of each project. `None` entries are kept.
pub(super) fn raw_protected_terms(graph: &ProfileGraph) -> Vec<Option<&str>> {
    let mut terms = vec![Some(graph.profile.full_name.as_str())];
    terms.extend(graph.experiences.iter().map(|row| row.company.as_deref()));
    terms.extend(graph.education.iter().map(|row| row.institution.as_deref()));
    terms.extend(graph.projects.iter().map(|row| row.name.as_deref()));
    terms.extend(graph.skills.iter().map(|row| Some(row.skill.as_str())));
    terms.extend(
        graph
            .project_technologies
            .iter()
            .map(|row| Some(row.technology.as_str())),
    );
    for project in &graph.projects {
        terms.extend(
            project
                .technologies
                .iter()
                .map(|row| Some(row.technology.as_str())),
        );
    }
    terms
}

/// Counts the selected profile values of one variant against the profile budget.
/// Only the total counts matter: a refusal is the same whichever value crosses
/// the limit.
pub(super) fn budget_inputs(
    graph: &ProfileGraph,
    variant: ResumeVariant,
) -> Result<(), ApplicationError> {
    let mut budget = PreparationBudget::new(BudgetField::Profile);
    let profile = &graph.profile;
    budget.add_text(Some(profile.full_name.as_str()))?;
    budget.add_text(Some(profile.email.as_str()))?;
    budget.add_text(profile.summary.as_deref())?;
    for row in &graph.contacts {
        budget.add_entries(1)?;
        budget.add_text(row.label.as_deref())?;
        budget.add_text(Some(row.contact_type.as_str()))?;
        budget.add_text(Some(row.contact_value.as_str()))?;
    }
    for row in &graph.links {
        budget.add_entries(1)?;
        budget.add_text(row.label.as_deref())?;
        budget.add_text(Some(row.link_type.as_str()))?;
        budget.add_text(Some(row.url.as_str()))?;
    }
    for row in &graph.skills {
        budget.add_entries(1)?;
        budget.add_text(Some(row.skill.as_str()))?;
        budget.add_text(row.category.as_deref())?;
    }
    for row in &graph.languages {
        budget.add_entries(1)?;
        budget.add_text(Some(row.language.as_str()))?;
        budget.add_text(row.proficiency.as_deref())?;
    }
    for row in &graph.experiences {
        budget.add_entries(1)?;
        budget.add_text(row.company.as_deref())?;
        budget.add_text(row.title.as_deref())?;
        let start = iso_text(row.start_date);
        let end = iso_text(row.end_date);
        budget.add_text(start.as_deref())?;
        budget.add_text(end.as_deref())?;
        budget.add_text(row.location.as_deref())?;
        budget.add_text(row.description.as_deref())?;
    }
    for row in &graph.education {
        budget.add_entries(1)?;
        budget.add_text(row.degree.as_deref())?;
        budget.add_text(row.field.as_deref())?;
        budget.add_text(row.institution.as_deref())?;
        budget.add_text(row.location.as_deref())?;
        let start = iso_text(row.start_date);
        let end = iso_text(row.end_date);
        budget.add_text(start.as_deref())?;
        budget.add_text(end.as_deref())?;
        budget.add_text(row.relevant_focus.as_deref())?;
    }
    match variant {
        ResumeVariant::MasterResume => {
            for project in &graph.projects {
                budget.add_entries(1)?;
                budget.add_text(project.name.as_deref())?;
                budget.add_text(project.url.as_deref())?;
                budget.add_text(project.description.as_deref())?;
                for technology in &project.technologies {
                    budget.add_entries(1)?;
                    budget.add_text(Some(technology.technology.as_str()))?;
                }
            }
        }
        ResumeVariant::CanadianResumeFr => {
            for project in &graph.projects {
                budget.add_entries(1)?;
                budget.add_text(project.name.as_deref())?;
                for technology in &project.technologies {
                    budget.add_entries(1)?;
                    budget.add_text(Some(technology.technology.as_str()))?;
                }
            }
            for row in &graph.project_technologies {
                budget.add_entries(1)?;
                budget.add_text(Some(row.technology.as_str()))?;
            }
            // The protected-term list is an additional selected collection: its raw entries are
            // counted before the source filtering and de-duplication.
            for _ in raw_protected_terms(graph) {
                budget.add_entries(1)?;
            }
        }
        ResumeVariant::CanadianResume => {}
    }
    Ok(())
}
