//! The profile aggregate in SQLite.
//!
//! Reads run inside a read-only scope (`BEGIN DEFERRED`), so the root revision and every child
//! SELECT share one WAL snapshot. Writes run on the caller's transaction, which already holds the
//! single writer (`BEGIN IMMEDIATE`), and never open a session of their own. Child rows are deleted
//! and inserted in a fixed order, and the search postings are rebuilt in the same transaction as
//! the graph they index.

use std::collections::HashMap;

use jobniac_core::errors::AppError;
use jobniac_core::profiles::model::{
    ContactRecord, EducationRecord, ExperienceRecord, LanguageRecord, LinkRecord, ProfileGraph,
    ProfileRecord, ProjectRecord, ProjectTechnologyRecord, SkillRecord,
};
use jobniac_core::profiles::repository::{ProjectChange, ProjectPatchResult};
use jobniac_core::profiles::search_terms::profile_search_terms;
use rusqlite::Connection;
use uuid::Uuid;

use super::{col, db, execute, order_value, query_rows, unavailable, weight_value};
use crate::sqlite::codec::{Day, Id, Instant};

/// The root query of the profile load.
const PROFILE_SQL: &str = r#"
        SELECT id, full_name AS "fullName", email, summary, created_at AS "createdAt",
               updated_at AS "updatedAt", revision
        FROM profile_profiles
        WHERE id = ?1
        "#;

/// The contacts query of the profile load.
const CONTACTS_SQL: &str = r#"
            SELECT id, profile_id AS "profileId", contact_type AS "contactType",
                   contact_value AS "contactValue", label, created_at AS "createdAt",
                   updated_at AS "updatedAt"
            FROM profile_profile_contacts
            WHERE profile_id = ?1
            ORDER BY contact_type, contact_value, id
            "#;

/// The links query of the profile load.
const LINKS_SQL: &str = r#"
            SELECT id, profile_id AS "profileId", link_type AS "linkType", url, label,
                   created_at AS "createdAt", updated_at AS "updatedAt"
            FROM profile_profile_links
            WHERE profile_id = ?1
            ORDER BY link_type, url, id
            "#;

/// The skills query of the profile load.
const SKILLS_SQL: &str = r#"
            SELECT id, profile_id AS "profileId", skill,
                   normalized_skill AS "normalizedSkill", category,
                   display_order AS "displayOrder", created_at AS "createdAt"
            FROM profile_profile_skills
            WHERE profile_id = ?1
            ORDER BY display_order, skill, id
            "#;

/// The languages query of the profile load.
const LANGUAGES_SQL: &str = r#"
            SELECT id, profile_id AS "profileId", language,
                   normalized_language AS "normalizedLanguage", proficiency,
                   display_order AS "displayOrder", created_at AS "createdAt"
            FROM profile_profile_languages
            WHERE profile_id = ?1
            ORDER BY display_order, language, id
            "#;

/// The education query of the profile load.
const EDUCATION_SQL: &str = r#"
            SELECT id, profile_id AS "profileId", institution, degree, field, location,
                   start_date AS "startDate", end_date AS "endDate",
                   relevant_focus AS "relevantFocus", created_at AS "createdAt"
            FROM profile_education
            WHERE profile_id = ?1
            ORDER BY start_date NULLS LAST, institution NULLS LAST, id
            "#;

/// The experiences query of the profile load.
const EXPERIENCES_SQL: &str = r#"
            SELECT id, profile_id AS "profileId", company, title, location,
                   start_date AS "startDate", end_date AS "endDate", description,
                   display_order AS "displayOrder", created_at AS "createdAt"
            FROM profile_experiences
            WHERE profile_id = ?1
            ORDER BY display_order, start_date DESC NULLS LAST, id
            "#;

/// The project technologies query of the profile load.
const TECHNOLOGIES_SQL: &str = r#"
        SELECT technology.id, technology.project_id AS "projectId", technology.technology,
               technology.normalized_technology AS "normalizedTechnology",
               technology.display_order AS "displayOrder", technology.created_at AS "createdAt"
        FROM profile_project_technologies AS technology
        JOIN profile_projects AS project ON project.id = technology.project_id
        WHERE project.profile_id = ?1
        ORDER BY project.display_order, technology.display_order, technology.technology,
                 technology.id
        "#;

/// The projects query of the profile load.
const PROJECTS_SQL: &str = r#"
        SELECT id, profile_id AS "profileId", name, url, description,
               display_order AS "displayOrder", created_at AS "createdAt"
        FROM profile_projects
        WHERE profile_id = ?1
        ORDER BY display_order, name NULLS LAST, id
        "#;

/// The search postings delete.
const DELETE_TERMS_SQL: &str = "DELETE FROM profile_search_terms WHERE profile_id = ?1";

/// One search posting insert.
const INSERT_TERM_SQL: &str = r#"
            INSERT INTO profile_search_terms (profile_id, field_key, term, weight)
            VALUES (?1, ?2, ?3, ?4)
            "#;

/// The root compare-and-set of a replacement.
const REPLACE_ROOT_SQL: &str = r#"
                UPDATE profile_profiles
                SET full_name = ?1,
                    email = ?2,
                    summary = ?3,
                    updated_at = ?4,
                    revision = ?5
                WHERE id = ?6 AND revision = ?7
                "#;

/// The root insert.
const INSERT_PROFILE_SQL: &str = r#"
        INSERT INTO profile_profiles
            (id, full_name, email, avatar_url, summary, created_at, updated_at, revision)
        VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7)
        "#;

/// The child deletes, in a fixed order.
const DELETE_CHILDREN_SQL: [&str; 7] = [
    "DELETE FROM profile_profile_contacts WHERE profile_id = ?1",
    "DELETE FROM profile_profile_links WHERE profile_id = ?1",
    "DELETE FROM profile_profile_skills WHERE profile_id = ?1",
    "DELETE FROM profile_profile_languages WHERE profile_id = ?1",
    "DELETE FROM profile_education WHERE profile_id = ?1",
    "DELETE FROM profile_experiences WHERE profile_id = ?1",
    "DELETE FROM profile_projects WHERE profile_id = ?1",
];

/// The contact insert.
const INSERT_CONTACT_SQL: &str = r#"
            INSERT INTO profile_profile_contacts
                (id, profile_id, contact_type, contact_value, label, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#;

/// The link insert.
const INSERT_LINK_SQL: &str = r#"
            INSERT INTO profile_profile_links
                (id, profile_id, link_type, url, label, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#;

/// The skill insert.
const INSERT_SKILL_SQL: &str = r#"
            INSERT INTO profile_profile_skills
                (id, profile_id, skill, normalized_skill, category, display_order, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#;

/// The language insert.
const INSERT_LANGUAGE_SQL: &str = r#"
            INSERT INTO profile_profile_languages
                (id, profile_id, language, normalized_language, proficiency, display_order,
                 created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#;

/// The education insert.
const INSERT_EDUCATION_SQL: &str = r#"
            INSERT INTO profile_education
                (id, profile_id, institution, degree, field, location, start_date, end_date,
                 description, relevant_focus, created_at, institution_key, degree_key, field_key)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?10, ?11, ?12, ?13)
            "#;

/// The experience insert.
const INSERT_EXPERIENCE_SQL: &str = r#"
            INSERT INTO profile_experiences
                (id, profile_id, company, title, location, start_date, end_date, description,
                 display_order, created_at, company_key, title_key)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            "#;

/// The project insert.
const INSERT_PROJECT_SQL: &str = r#"
            INSERT INTO profile_projects
                (id, profile_id, name, start_date, end_date, url, description, display_order,
                 created_at, name_key)
            VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?5, ?6, ?7, ?8)
            "#;

/// The project technology insert.
const INSERT_TECHNOLOGY_SQL: &str = r#"
            INSERT INTO profile_project_technologies
                (id, project_id, technology, normalized_technology, display_order, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#;

/// The profile compare-and-set of a project patch (the project must belong to the profile).
const UPDATE_PROJECT_PROFILE_SQL: &str = r#"
                UPDATE profile_profiles
                SET updated_at = ?1, revision = ?2
                WHERE id = ?3
                  AND revision = ?4
                  AND EXISTS (
                      SELECT 1 FROM profile_projects
                      WHERE id = ?5 AND profile_id = ?6
                  )
                "#;

/// The project existence check of a project patch (the writer lock replaces the row lock).
const LOCK_PROJECT_SQL: &str = r#"
                SELECT id FROM profile_projects
                WHERE id = ?1 AND profile_id = ?2
                "#;

/// The project update of a project patch.
const UPDATE_PROJECT_SQL: &str = r#"
                UPDATE profile_projects
                SET name = ?1, url = ?2, description = ?3, display_order = ?4, name_key = ?5
                WHERE id = ?6 AND profile_id = ?7
                "#;

/// The technology replacement delete of a project patch.
const DELETE_PROJECT_TECHNOLOGIES_SQL: &str =
    "DELETE FROM profile_project_technologies WHERE project_id = ?1";

/// Savepoint name of the project patch.
const PROJECT_SAVEPOINT: &str = "profile_project_update";

/// The canonical key of a free-text column: trimmed and lower-cased with full Unicode rules.
///
/// SQLite `lower()` folds ASCII only, so the case-insensitive unique indexes of education,
/// experiences and projects sit on keys computed here (the PostgreSQL schema lower-cased in UTF-8).
fn canonical_key(text: Option<&str>) -> Option<String> {
    text.map(|value| value.trim().to_lowercase())
}

/// Loads the complete graph of a profile in a read-only scope.
///
/// The scope begins `BEGIN DEFERRED` on a read-only connection, so the root row and every child
/// SELECT read one snapshot.
pub(crate) fn load_read_only(
    conn: &Connection,
    profile_id: Uuid,
) -> Result<Option<ProfileGraph>, AppError> {
    load(conn, profile_id)
}

/// Loads the complete graph of a profile, or `None` when the root row is missing.
pub(crate) fn load(conn: &Connection, profile_id: Uuid) -> Result<Option<ProfileGraph>, AppError> {
    let id = Id(profile_id);
    let mut roots = query_rows(conn, PROFILE_SQL, &[&id], |root| {
        Ok(ProfileRecord {
            id: col::<Id>(root, "id")?.0,
            full_name: col(root, "fullName")?,
            email: col(root, "email")?,
            summary: col(root, "summary")?,
            raw_resume_text: None,
            created_at: col::<Instant>(root, "createdAt")?.0,
            updated_at: col::<Instant>(root, "updatedAt")?.0,
            embedding: None,
            revision: col(root, "revision")?,
        })
    })?;
    let Some(profile) = roots.pop() else {
        return Ok(None);
    };
    let contacts = query_rows(conn, CONTACTS_SQL, &[&id], |row| {
        Ok(ContactRecord {
            id: col::<Id>(row, "id")?.0,
            profile_id: col::<Id>(row, "profileId")?.0,
            contact_type: col(row, "contactType")?,
            contact_value: col(row, "contactValue")?,
            label: col(row, "label")?,
            created_at: col::<Instant>(row, "createdAt")?.0,
            updated_at: col::<Instant>(row, "updatedAt")?.0,
        })
    })?;
    let links = query_rows(conn, LINKS_SQL, &[&id], |row| {
        Ok(LinkRecord {
            id: col::<Id>(row, "id")?.0,
            profile_id: col::<Id>(row, "profileId")?.0,
            link_type: col(row, "linkType")?,
            url: col(row, "url")?,
            label: col(row, "label")?,
            created_at: col::<Instant>(row, "createdAt")?.0,
            updated_at: col::<Instant>(row, "updatedAt")?.0,
        })
    })?;
    let skills = query_rows(conn, SKILLS_SQL, &[&id], |row| {
        Ok(SkillRecord {
            id: col::<Id>(row, "id")?.0,
            profile_id: col::<Id>(row, "profileId")?.0,
            skill: col(row, "skill")?,
            normalized_skill: col(row, "normalizedSkill")?,
            category: col(row, "category")?,
            display_order: col(row, "displayOrder")?,
            created_at: col::<Instant>(row, "createdAt")?.0,
        })
    })?;
    let languages = query_rows(conn, LANGUAGES_SQL, &[&id], |row| {
        Ok(LanguageRecord {
            id: col::<Id>(row, "id")?.0,
            profile_id: col::<Id>(row, "profileId")?.0,
            language: col(row, "language")?,
            normalized_language: col(row, "normalizedLanguage")?,
            proficiency: col(row, "proficiency")?,
            display_order: col(row, "displayOrder")?,
            created_at: col::<Instant>(row, "createdAt")?.0,
        })
    })?;
    let education = query_rows(conn, EDUCATION_SQL, &[&id], |row| {
        Ok(EducationRecord {
            id: col::<Id>(row, "id")?.0,
            profile_id: col::<Id>(row, "profileId")?.0,
            institution: col(row, "institution")?,
            degree: col(row, "degree")?,
            field: col(row, "field")?,
            location: col(row, "location")?,
            start_date: col::<Option<Day>>(row, "startDate")?.map(|day| day.0),
            end_date: col::<Option<Day>>(row, "endDate")?.map(|day| day.0),
            relevant_focus: col(row, "relevantFocus")?,
            created_at: col::<Instant>(row, "createdAt")?.0,
        })
    })?;
    let experiences = query_rows(conn, EXPERIENCES_SQL, &[&id], |row| {
        Ok(ExperienceRecord {
            id: col::<Id>(row, "id")?.0,
            profile_id: col::<Id>(row, "profileId")?.0,
            company: col(row, "company")?,
            title: col(row, "title")?,
            location: col(row, "location")?,
            start_date: col::<Option<Day>>(row, "startDate")?.map(|day| day.0),
            end_date: col::<Option<Day>>(row, "endDate")?.map(|day| day.0),
            description: col(row, "description")?,
            display_order: col(row, "displayOrder")?,
            created_at: col::<Instant>(row, "createdAt")?.0,
        })
    })?;
    let project_technologies = query_rows(conn, TECHNOLOGIES_SQL, &[&id], |row| {
        Ok(ProjectTechnologyRecord {
            id: col::<Id>(row, "id")?.0,
            project_id: col::<Id>(row, "projectId")?.0,
            technology: col(row, "technology")?,
            normalized_technology: col(row, "normalizedTechnology")?,
            display_order: col(row, "displayOrder")?,
            created_at: col::<Instant>(row, "createdAt")?.0,
        })
    })?;
    let mut technologies_by_project: HashMap<Uuid, Vec<ProjectTechnologyRecord>> = HashMap::new();
    for technology in &project_technologies {
        technologies_by_project
            .entry(technology.project_id)
            .or_default()
            .push(technology.clone());
    }
    let projects = query_rows(conn, PROJECTS_SQL, &[&id], |row| {
        let project_id = col::<Id>(row, "id")?.0;
        Ok(ProjectRecord {
            id: project_id,
            profile_id: col::<Id>(row, "profileId")?.0,
            name: col(row, "name")?,
            url: col(row, "url")?,
            description: col(row, "description")?,
            technologies: technologies_by_project
                .get(&project_id)
                .cloned()
                .unwrap_or_default(),
            display_order: col(row, "displayOrder")?,
            created_at: col::<Instant>(row, "createdAt")?.0,
        })
    })?;
    Ok(Some(ProfileGraph {
        profile,
        contacts,
        links,
        skills,
        languages,
        education,
        experiences,
        projects,
        project_technologies,
    }))
}

/// Inserts a new graph and its search postings. Returns the persisted graph.
pub(crate) fn create(conn: &Connection, graph: &ProfileGraph) -> Result<ProfileGraph, AppError> {
    insert_profile(conn, graph)?;
    rebuild_search_terms(conn, graph)?;
    load(conn, graph.profile.id)?.ok_or_else(unavailable)
}

/// Replaces every child and the root when the stored revision is `expected_revision`. Returns
/// `None` on a revision mismatch.
pub(crate) fn replace(
    conn: &Connection,
    graph: &ProfileGraph,
    expected_revision: i64,
) -> Result<Option<ProfileGraph>, AppError> {
    let profile = &graph.profile;
    let updated = execute(
        conn,
        REPLACE_ROOT_SQL,
        &[
            &profile.full_name,
            &profile.email,
            &profile.summary,
            &Instant(profile.updated_at),
            &profile.revision,
            &Id(profile.id),
            &expected_revision,
        ],
    )?;
    if updated != 1 {
        return Ok(None);
    }
    delete_children(conn, profile.id)?;
    insert_children(conn, graph, profile.id)?;
    rebuild_search_terms(conn, graph)?;
    load(conn, profile.id)?.ok_or_else(unavailable).map(Some)
}

/// Patches one project under the profile revision check, with a savepoint around the project
/// rows. Returns `None` on a mismatch, with nothing left written.
pub(crate) fn update_project(
    conn: &Connection,
    change: &ProjectChange,
) -> Result<Option<ProjectPatchResult>, AppError> {
    let profile_id = change.profile_id;
    let project = &change.project;
    let project_id = project.id;
    db(conn.execute_batch(&format!("SAVEPOINT {PROJECT_SAVEPOINT}")))?;
    let profile_rows = execute(
        conn,
        UPDATE_PROJECT_PROFILE_SQL,
        &[
            &Instant(change.updated_at),
            &change.new_revision,
            &Id(profile_id),
            &change.expected_revision,
            &Id(project_id),
            &Id(profile_id),
        ],
    )?;
    if profile_rows != 1 {
        return Ok(None);
    }
    let present = query_rows(
        conn,
        LOCK_PROJECT_SQL,
        &[&Id(project_id), &Id(profile_id)],
        |_| Ok(()),
    )?;
    if present.is_empty() {
        rollback_to_savepoint(conn)?;
        return Ok(None);
    }
    let project_rows = execute(
        conn,
        UPDATE_PROJECT_SQL,
        &[
            &project.name,
            &project.url,
            &project.description,
            &order_value(project.display_order)?,
            &canonical_key(project.name.as_deref()),
            &Id(project_id),
            &Id(profile_id),
        ],
    )?;
    if project_rows != 1 {
        rollback_to_savepoint(conn)?;
        return Ok(None);
    }
    if let Some(technologies) = &change.replacement_technologies {
        execute(conn, DELETE_PROJECT_TECHNOLOGIES_SQL, &[&Id(project_id)])?;
        insert_technologies(conn, technologies, project_id)?;
    }
    let graph = load(conn, profile_id)?.ok_or_else(unavailable)?;
    rebuild_search_terms(conn, &graph)?;
    let persisted = graph
        .projects
        .into_iter()
        .find(|item| item.id == project_id)
        .ok_or(AppError::Internal)?;
    Ok(Some(ProjectPatchResult {
        project: persisted,
        profile_revision: change.new_revision,
    }))
}

/// `ROLLBACK TO SAVEPOINT` of the project patch.
fn rollback_to_savepoint(conn: &Connection) -> Result<(), AppError> {
    db(conn.execute_batch(&format!("ROLLBACK TO SAVEPOINT {PROJECT_SAVEPOINT}")))
}

/// Inserts the root row and every child.
fn insert_profile(conn: &Connection, graph: &ProfileGraph) -> Result<(), AppError> {
    let profile = &graph.profile;
    execute(
        conn,
        INSERT_PROFILE_SQL,
        &[
            &Id(profile.id),
            &profile.full_name,
            &profile.email,
            &profile.summary,
            &Instant(profile.created_at),
            &Instant(profile.updated_at),
            &profile.revision,
        ],
    )?;
    insert_children(conn, graph, profile.id)
}

/// Deletes every profile-owned child, in a fixed order.
fn delete_children(conn: &Connection, profile_id: Uuid) -> Result<(), AppError> {
    for statement in DELETE_CHILDREN_SQL {
        execute(conn, statement, &[&Id(profile_id)])?;
    }
    Ok(())
}

/// Inserts every child of the graph under `profile_id` (`_insert_profile_children`).
fn insert_children(
    conn: &Connection,
    graph: &ProfileGraph,
    profile_id: Uuid,
) -> Result<(), AppError> {
    for value in &graph.contacts {
        execute(
            conn,
            INSERT_CONTACT_SQL,
            &[
                &Id(value.id),
                &Id(profile_id),
                &value.contact_type,
                &value.contact_value,
                &value.label,
                &Instant(value.created_at),
                &Instant(value.updated_at),
            ],
        )?;
    }
    for value in &graph.links {
        execute(
            conn,
            INSERT_LINK_SQL,
            &[
                &Id(value.id),
                &Id(profile_id),
                &value.link_type,
                &value.url,
                &value.label,
                &Instant(value.created_at),
                &Instant(value.updated_at),
            ],
        )?;
    }
    for value in &graph.skills {
        execute(
            conn,
            INSERT_SKILL_SQL,
            &[
                &Id(value.id),
                &Id(profile_id),
                &value.skill,
                &value.normalized_skill,
                &value.category,
                &order_value(value.display_order)?,
                &Instant(value.created_at),
            ],
        )?;
    }
    for value in &graph.languages {
        execute(
            conn,
            INSERT_LANGUAGE_SQL,
            &[
                &Id(value.id),
                &Id(profile_id),
                &value.language,
                &value.normalized_language,
                &value.proficiency,
                &order_value(value.display_order)?,
                &Instant(value.created_at),
            ],
        )?;
    }
    for value in &graph.education {
        execute(
            conn,
            INSERT_EDUCATION_SQL,
            &[
                &Id(value.id),
                &Id(profile_id),
                &value.institution,
                &value.degree,
                &value.field,
                &value.location,
                &value.start_date.map(Day),
                &value.end_date.map(Day),
                &value.relevant_focus,
                &Instant(value.created_at),
                &canonical_key(value.institution.as_deref()),
                &canonical_key(value.degree.as_deref()),
                &canonical_key(value.field.as_deref()),
            ],
        )?;
    }
    for value in &graph.experiences {
        execute(
            conn,
            INSERT_EXPERIENCE_SQL,
            &[
                &Id(value.id),
                &Id(profile_id),
                &value.company,
                &value.title,
                &value.location,
                &value.start_date.map(Day),
                &value.end_date.map(Day),
                &value.description,
                &order_value(value.display_order)?,
                &Instant(value.created_at),
                &canonical_key(value.company.as_deref()),
                &canonical_key(value.title.as_deref()),
            ],
        )?;
    }
    for value in &graph.projects {
        execute(
            conn,
            INSERT_PROJECT_SQL,
            &[
                &Id(value.id),
                &Id(profile_id),
                &value.name,
                &value.url,
                &value.description,
                &order_value(value.display_order)?,
                &Instant(value.created_at),
                &canonical_key(value.name.as_deref()),
            ],
        )?;
    }
    for value in &graph.project_technologies {
        execute(
            conn,
            INSERT_TECHNOLOGY_SQL,
            &[
                &Id(value.id),
                &Id(value.project_id),
                &value.technology,
                &value.normalized_technology,
                &order_value(value.display_order)?,
                &Instant(value.created_at),
            ],
        )?;
    }
    Ok(())
}

/// Inserts a project's technologies under `project_id`.
fn insert_technologies(
    conn: &Connection,
    values: &[ProjectTechnologyRecord],
    project_id: Uuid,
) -> Result<(), AppError> {
    for value in values {
        execute(
            conn,
            INSERT_TECHNOLOGY_SQL,
            &[
                &Id(value.id),
                &Id(project_id),
                &value.technology,
                &value.normalized_technology,
                &order_value(value.display_order)?,
                &Instant(value.created_at),
            ],
        )?;
    }
    Ok(())
}

/// Replaces the profile's search postings with those of `graph`.
fn rebuild_search_terms(conn: &Connection, graph: &ProfileGraph) -> Result<(), AppError> {
    execute(conn, DELETE_TERMS_SQL, &[&Id(graph.profile.id)])?;
    for term in profile_search_terms(graph) {
        execute(
            conn,
            INSERT_TERM_SQL,
            &[
                &Id(term.entity_id),
                &term.field_key,
                &term.term,
                &weight_value(term.weight)?,
            ],
        )?;
    }
    Ok(())
}
