//! Builds the complete profile graph from a canonical draft.
//!
//! Every child is stamped with the profile identifier and the same creation instant. Missing child
//! identifiers come from the `new_id` source, drawn in this order: the project before its
//! technologies, and the children in collection order.

use time::OffsetDateTime;
use uuid::Uuid;

use super::draft::{ProfileDraft, ProjectDraft, TechnologyDraft};
use super::model::{
    ContactRecord, EducationRecord, ExperienceRecord, LanguageRecord, LinkRecord, ProfileGraph,
    ProfileRecord, ProjectRecord, ProjectTechnologyRecord, SkillRecord,
};

/// A fresh random identifier.
pub(crate) fn fresh_id() -> Uuid {
    Uuid::new_v4()
}

/// Builds the graph of a profile. `revision` is 0 on create and the next revision on replace.
pub fn build_profile_graph(
    draft: &ProfileDraft,
    profile_id: Uuid,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    revision: i64,
    new_id: &mut dyn FnMut() -> Uuid,
) -> ProfileGraph {
    let profile = ProfileRecord {
        id: profile_id,
        full_name: draft.full_name.clone(),
        email: draft.email.clone(),
        summary: draft.summary.clone(),
        raw_resume_text: None,
        created_at,
        updated_at,
        embedding: None,
        revision,
    };
    let contacts = draft
        .contacts
        .iter()
        .map(|value| ContactRecord {
            id: value.id.unwrap_or_else(&mut *new_id),
            profile_id,
            contact_type: value.contact_type.clone(),
            contact_value: value.contact_value.clone(),
            label: value.label.clone(),
            created_at,
            updated_at,
        })
        .collect();
    let links = draft
        .links
        .iter()
        .map(|value| LinkRecord {
            id: value.id.unwrap_or_else(&mut *new_id),
            profile_id,
            link_type: value.link_type.clone(),
            url: value.url.clone(),
            label: value.label.clone(),
            created_at,
            updated_at,
        })
        .collect();
    let skills = draft
        .skills
        .iter()
        .map(|value| SkillRecord {
            id: value.id.unwrap_or_else(&mut *new_id),
            profile_id,
            skill: value.skill.clone(),
            normalized_skill: value.normalized_skill.clone(),
            category: value.category.clone(),
            display_order: value.display_order,
            created_at,
        })
        .collect();
    let languages = draft
        .languages
        .iter()
        .map(|value| LanguageRecord {
            id: value.id.unwrap_or_else(&mut *new_id),
            profile_id,
            language: value.language.clone(),
            normalized_language: value.normalized_language.clone(),
            proficiency: value.proficiency.clone(),
            display_order: value.display_order,
            created_at,
        })
        .collect();
    let education = draft
        .education
        .iter()
        .map(|value| EducationRecord {
            id: value.id.unwrap_or_else(&mut *new_id),
            profile_id,
            institution: value.institution.clone(),
            degree: value.degree.clone(),
            field: value.field.clone(),
            location: value.location.clone(),
            start_date: value.start_date,
            end_date: value.end_date,
            relevant_focus: value.relevant_focus.clone(),
            created_at,
        })
        .collect();
    let experiences = draft
        .experiences
        .iter()
        .map(|value| ExperienceRecord {
            id: value.id.unwrap_or_else(&mut *new_id),
            profile_id,
            company: value.company.clone(),
            title: value.title.clone(),
            location: value.location.clone(),
            start_date: value.start_date,
            end_date: value.end_date,
            description: value.description.clone(),
            display_order: value.display_order,
            created_at,
        })
        .collect();
    let mut projects = Vec::with_capacity(draft.projects.len());
    let mut project_technologies = Vec::new();
    for value in &draft.projects {
        let project = project_record(value, profile_id, created_at, new_id);
        project_technologies.extend(project.technologies.iter().cloned());
        projects.push(project);
    }
    ProfileGraph {
        profile,
        contacts,
        links,
        skills,
        languages,
        education,
        experiences,
        projects,
        project_technologies,
    }
}

/// One project record, with its technologies identified after the project.
fn project_record(
    value: &ProjectDraft,
    profile_id: Uuid,
    created_at: OffsetDateTime,
    new_id: &mut dyn FnMut() -> Uuid,
) -> ProjectRecord {
    let id = value.id.unwrap_or_else(&mut *new_id);
    let technologies = value
        .technologies
        .iter()
        .map(|technology| technology_record(technology, id, created_at, new_id))
        .collect();
    ProjectRecord {
        id,
        profile_id,
        name: value.name.clone(),
        url: value.url.clone(),
        description: value.description.clone(),
        technologies,
        display_order: value.display_order,
        created_at,
    }
}

/// One project technology record owned by `project_id`.
pub(crate) fn technology_record(
    value: &TechnologyDraft,
    project_id: Uuid,
    created_at: OffsetDateTime,
    new_id: &mut dyn FnMut() -> Uuid,
) -> ProjectTechnologyRecord {
    ProjectTechnologyRecord {
        id: value.id.unwrap_or_else(&mut *new_id),
        project_id,
        technology: value.technology.clone(),
        normalized_technology: value.normalized_technology.clone(),
        display_order: value.display_order,
        created_at,
    }
}
