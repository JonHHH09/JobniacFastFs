//! Section writers of the resume bodies.
//!
//! Each writer appends lines to a body. When a [`French`] context is given, the writer produces the
//! French body; without it, the English body. The same writers serve the master, Canadian English
//! and Canadian French variants.

use indexmap::IndexMap;
use jobniac_text::java::java_strip;

use super::bullets::experience_bullets;
use super::text::{default_text, has_text, optional_text, ordered_nullable, ordered_text, period};
use crate::errors::ApplicationError;
use crate::generation::translation::{FrenchTranslator, PreparedProtectedTerms};
use crate::profiles::model::{ContactRecord, EducationRecord, ExperienceRecord, ProfileGraph};

/// The French translation context of one resume: the translator and the prepared protected terms.
#[derive(Debug, Clone, Copy)]
pub(super) struct French<'a> {
    pub(super) translator: &'a FrenchTranslator,
    pub(super) protected: &'a PreparedProtectedTerms,
}

impl French<'_> {
    /// Translates narrative text with the protected terms.
    pub(super) fn translate_text(&self, text: &str) -> Result<String, ApplicationError> {
        Ok(self.translator.translate_narrative(text, self.protected)?)
    }
}

/// True for a contact whose type, label or value names an e-mail address.
pub(super) fn is_email_contact(row: &ContactRecord) -> bool {
    [
        Some(row.contact_type.as_str()),
        row.label.as_deref(),
        Some(row.contact_value.as_str()),
    ]
    .into_iter()
    .flatten()
    .any(|value| {
        has_text(Some(value)) && {
            let stripped = java_strip(value);
            stripped.to_lowercase().contains("email") || stripped.contains('@')
        }
    })
}

/// The skills block, grouped by category in first-seen order.
pub(super) fn append_skills(
    lines: &mut Vec<String>,
    graph: &ProfileGraph,
    heading: &str,
    french: Option<&French<'_>>,
) -> Result<(), ApplicationError> {
    if graph.skills.is_empty() {
        return Ok(());
    }
    lines.push(heading.to_owned());
    let mut groups: IndexMap<String, Vec<&str>> = IndexMap::new();
    for skill in ordered_text(
        &graph.skills,
        |row| row.display_order,
        |row| row.skill.as_str(),
    ) {
        let category = default_text(skill.category.as_deref(), "General");
        groups
            .entry(category)
            .or_default()
            .push(skill.skill.as_str());
    }
    for (category, names) in groups {
        let mut values = names.join(", ");
        let mut label = category;
        if let Some(french) = french {
            label = french.translator.translate_label(&label)?;
            values = french.translator.translate_technology_list(&values)?;
        }
        lines.push(format!("{label}: {values}"));
    }
    lines.push(String::new());
    Ok(())
}

/// The languages line.
pub(super) fn append_languages(
    lines: &mut Vec<String>,
    graph: &ProfileGraph,
    french: Option<&French<'_>>,
) -> Result<(), ApplicationError> {
    if graph.languages.is_empty() {
        return Ok(());
    }
    lines.push(
        if french.is_some() {
            "LANGUES"
        } else {
            "LANGUAGES"
        }
        .to_owned(),
    );
    let mut values = Vec::new();
    for language in ordered_text(
        &graph.languages,
        |row| row.display_order,
        |row| row.language.as_str(),
    ) {
        let proficiency = language.proficiency.as_deref();
        let (name, suffix) = match french {
            Some(french) => {
                let name = french
                    .translator
                    .translate_narrative_unprotected(&language.language)?;
                let suffix = if has_text(proficiency) {
                    let label = french
                        .translator
                        .translate_label(proficiency.unwrap_or_default())?;
                    format!(" ({label})")
                } else {
                    String::new()
                };
                (name, suffix)
            }
            None => (
                language.language.clone(),
                optional_text(proficiency, " (", ")"),
            ),
        };
        values.push(format!("{name}{suffix}"));
    }
    lines.push(values.join(", "));
    lines.push(String::new());
    Ok(())
}

/// The education block, latest end date first.
pub(super) fn append_education(
    lines: &mut Vec<String>,
    graph: &ProfileGraph,
    french: Option<&French<'_>>,
) -> Result<(), ApplicationError> {
    if graph.education.is_empty() {
        return Ok(());
    }
    lines.push(
        if french.is_some() {
            "FORMATION"
        } else {
            "EDUCATION"
        }
        .to_owned(),
    );
    let mut rows: Vec<&EducationRecord> = graph.education.iter().collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.end_date));
    for row in rows {
        let is_french = french.is_some();
        let mut degree = default_text(
            row.degree.as_deref(),
            if is_french { "Diplôme" } else { "Education" },
        );
        let mut field = row.field.clone();
        let institution = default_text(
            row.institution.as_deref(),
            if is_french {
                "Établissement"
            } else {
                "Institution"
            },
        );
        let mut location = row.location.clone();
        let mut focus = row.relevant_focus.clone();
        if let Some(french) = french {
            degree = french.translate_text(&degree)?;
            field = translate_present(french, field.as_deref())?;
            location = translate_present(french, location.as_deref())?;
            focus = translate_present(french, focus.as_deref())?;
        }
        lines.push(format!(
            "{degree}{}",
            optional_text(field.as_deref(), ", ", "")
        ));
        lines.push(format!(
            "{institution}{}",
            optional_text(location.as_deref(), " | ", "")
        ));
        lines.push(period(row.start_date, row.end_date, is_french));
        if has_text(focus.as_deref()) {
            lines.push(format!(
                "- {}",
                java_strip(focus.as_deref().unwrap_or_default())
            ));
        }
        lines.push(String::new());
    }
    Ok(())
}

/// Translates an optional value when it has text, else gives `None`.
fn translate_present(
    french: &French<'_>,
    value: Option<&str>,
) -> Result<Option<String>, ApplicationError> {
    if has_text(value) {
        Ok(Some(french.translate_text(value.unwrap_or_default())?))
    } else {
        Ok(None)
    }
}

/// The experience order: by display order and company, then for the Canadian bodies by the latest
/// end (or start) date first.
fn experience_order(graph: &ProfileGraph, canadian: bool) -> Vec<&ExperienceRecord> {
    let mut rows = ordered_nullable(
        &graph.experiences,
        |row| row.display_order,
        |row| row.company.as_deref(),
    );
    if canadian {
        rows.sort_by(|left, right| {
            let key = |row: &ExperienceRecord| row.end_date.or(row.start_date);
            key(right).cmp(&key(left))
        });
    }
    rows
}

/// The experiences block.
///
/// The Canadian bodies split each description into bullets, with `sentences` as the finder. The
/// master body keeps the whole stripped description as one bullet.
pub(super) fn append_experiences(
    lines: &mut Vec<String>,
    graph: &ProfileGraph,
    canadian: bool,
    sentences: &dyn Fn(&str) -> Option<Vec<usize>>,
    french: Option<&French<'_>>,
) -> Result<(), ApplicationError> {
    if graph.experiences.is_empty() {
        return Ok(());
    }
    let is_french = french.is_some();
    lines.push(
        if is_french {
            "EXPÉRIENCE PROFESSIONNELLE"
        } else if canadian {
            "PROFESSIONAL EXPERIENCE"
        } else {
            "EXPERIENCE"
        }
        .to_owned(),
    );
    for row in experience_order(graph, canadian) {
        let mut title = default_text(row.title.as_deref(), "Role");
        let company = default_text(
            row.company.as_deref(),
            if is_french { "Entreprise" } else { "Company" },
        );
        let mut location = row.location.clone();
        if let Some(french) = french {
            title = french.translate_text(&title)?;
            location = translate_present(french, location.as_deref())?;
        }
        lines.push(format!("{title} | {company}"));
        lines.push(format!(
            "{}{}",
            period(row.start_date, row.end_date, is_french),
            optional_text(location.as_deref(), " | ", "")
        ));
        if has_text(row.description.as_deref()) {
            let description = row.description.as_deref().unwrap_or_default();
            let bullets = if canadian {
                experience_bullets(Some(description), sentences)?
            } else {
                vec![java_strip(description).to_owned()]
            };
            for bullet in bullets {
                let text = match french {
                    Some(french) => french.translate_text(&bullet)?,
                    None => bullet,
                };
                lines.push(format!("- {text}"));
            }
        }
        lines.push(String::new());
    }
    Ok(())
}

/// The projects block of the master body.
pub(super) fn append_projects(lines: &mut Vec<String>, graph: &ProfileGraph) {
    if graph.projects.is_empty() {
        return;
    }
    lines.push("PROJECTS".to_owned());
    for project in ordered_nullable(
        &graph.projects,
        |row| row.display_order,
        |row| row.name.as_deref(),
    ) {
        lines.push(format!(
            "{}{}",
            default_text(project.name.as_deref(), "Project"),
            optional_text(project.url.as_deref(), " - ", "")
        ));
        let technologies = ordered_text(
            &project.technologies,
            |row| row.display_order,
            |row| row.technology.as_str(),
        )
        .into_iter()
        .map(|row| row.technology.as_str())
        .collect::<Vec<_>>()
        .join(", ");
        if has_text(Some(technologies.as_str())) {
            lines.push(format!("Technologies: {technologies}"));
        }
        if has_text(project.description.as_deref()) {
            lines.push(format!(
                "- {}",
                java_strip(project.description.as_deref().unwrap_or_default())
            ));
        }
        lines.push(String::new());
    }
}
