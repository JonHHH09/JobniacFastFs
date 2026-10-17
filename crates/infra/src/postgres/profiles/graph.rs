//! The profile aggregate in PostgreSQL.
//!
//! Reads run inside a read-only scope at REPEATABLE READ, so the root revision and every child
//! SELECT share one snapshot. Writes run on the caller's transaction and never open a session of
//! their own. Child rows are deleted and inserted in a fixed order, and the search postings are
//! rebuilt in the same transaction as the graph they index.

use std::collections::HashMap;

use jobniac_core::errors::AppError;
use jobniac_core::profiles::model::{
    ContactRecord, EducationRecord, ExperienceRecord, LanguageRecord, LinkRecord, ProfileGraph,
    ProfileRecord, ProjectRecord, ProjectTechnologyRecord, SkillRecord,
};
use jobniac_core::profiles::repository::{ProjectChange, ProjectPatchResult};
use jobniac_core::profiles::search_terms::profile_search_terms;
use tokio_postgres::Client;
use uuid::Uuid;

use super::{col, db, map_rows, order_value, unavailable, weight_value};

/// The root query of the profile load.
const PROFILE_SQL: &str = r#"
        SELECT id, full_name AS "fullName", email, summary, created_at AS "createdAt",
               updated_at AS "updatedAt", revision
        FROM profile.profiles
        WHERE id = $1
        "#;

/// The contacts query of the profile load.
const CONTACTS_SQL: &str = r#"
            SELECT id, profile_id AS "profileId", contact_type AS "contactType",
                   contact_value AS "contactValue", label, created_at AS "createdAt",
                   updated_at AS "updatedAt"
            FROM profile.profile_contacts
            WHERE profile_id = $1
            ORDER BY contact_type, contact_value, id
            "#;

/// The links query of the profile load.
const LINKS_SQL: &str = r#"
            SELECT id, profile_id AS "profileId", link_type AS "linkType", url, label,
                   created_at AS "createdAt", updated_at AS "updatedAt"
            FROM profile.profile_links
            WHERE profile_id = $1
            ORDER BY link_type, url, id
            "#;

/// The skills query of the profile load.
const SKILLS_SQL: &str = r#"
            SELECT id, profile_id AS "profileId", skill,
                   normalized_skill AS "normalizedSkill", category,
                   display_order AS "displayOrder", created_at AS "createdAt"
            FROM profile.profile_skills
            WHERE profile_id = $1
            ORDER BY display_order, skill, id
            "#;

/// The languages query of the profile load.
const LANGUAGES_SQL: &str = r#"
            SELECT id, profile_id AS "profileId", language,
                   normalized_language AS "normalizedLanguage", proficiency,
                   display_order AS "displayOrder", created_at AS "createdAt"
            FROM profile.profile_languages
            WHERE profile_id = $1
            ORDER BY display_order, language, id
            "#;

/// The education query of the profile load.
const EDUCATION_SQL: &str = r#"
            SELECT id, profile_id AS "profileId", institution, degree, field, location,
                   start_date AS "startDate", end_date AS "endDate",
                   relevant_focus AS "relevantFocus", created_at AS "createdAt"
            FROM profile.education
            WHERE profile_id = $1
            ORDER BY start_date NULLS LAST, institution, id
            "#;

/// The experiences query of the profile load.
const EXPERIENCES_SQL: &str = r#"
            SELECT id, profile_id AS "profileId", company, title, location,
                   start_date AS "startDate", end_date AS "endDate", description,
                   display_order AS "displayOrder", created_at AS "createdAt"
            FROM profile.experiences
            WHERE profile_id = $1
            ORDER BY display_order, start_date DESC NULLS LAST, id
            "#;

/// The project technologies query of the profile load.
const TECHNOLOGIES_SQL: &str = r#"
        SELECT technology.id, technology.project_id AS "projectId", technology.technology,
               technology.normalized_technology AS "normalizedTechnology",
               technology.display_order AS "displayOrder", technology.created_at AS "createdAt"
        FROM profile.project_technologies AS technology
        JOIN profile.projects AS project ON project.id = technology.project_id
        WHERE project.profile_id = $1
        ORDER BY project.display_order, technology.display_order, technology.technology,
                 technology.id
        "#;

/// The projects query of the profile load.
const PROJECTS_SQL: &str = r#"
        SELECT id, profile_id AS "profileId", name, url, description,
               display_order AS "displayOrder", created_at AS "createdAt"
        FROM profile.projects
        WHERE profile_id = $1
        ORDER BY display_order, name, id
        "#;

/// The search postings delete.
const DELETE_TERMS_SQL: &str = "DELETE FROM profile.search_terms WHERE profile_id = $1";

/// The search postings insert, every posting in one statement. The columns are parallel
/// arrays (the first carries the per-posting entity id), inserted in posting order.
const INSERT_TERMS_SQL: &str = r#"
            INSERT INTO profile.search_terms (profile_id, field_key, term, weight)
            SELECT profile_id, field_key, term, weight
            FROM unnest($1::uuid[], $2::text[], $3::text[], $4::smallint[])
                 WITH ORDINALITY AS t(profile_id, field_key, term, weight, ord)
            ORDER BY ord
            "#;

/// The root compare-and-set of a replacement.
const REPLACE_ROOT_SQL: &str = r#"
                UPDATE profile.profiles
                SET full_name = $1,
                    email = $2,
                    summary = $3,
                    updated_at = $4,
                    revision = $5
                WHERE id = $6 AND revision = $7
                "#;

/// The root insert.
const INSERT_PROFILE_SQL: &str = r#"
        INSERT INTO profile.profiles
            (id, full_name, email, avatar_url, summary, created_at, updated_at, revision)
        VALUES ($1, $2, $3, NULL, $4, $5, $6, $7)
        "#;

/// The child deletes, in a fixed order.
const DELETE_CHILDREN_SQL: [&str; 7] = [
    "DELETE FROM profile.profile_contacts WHERE profile_id = $1",
    "DELETE FROM profile.profile_links WHERE profile_id = $1",
    "DELETE FROM profile.profile_skills WHERE profile_id = $1",
    "DELETE FROM profile.profile_languages WHERE profile_id = $1",
    "DELETE FROM profile.education WHERE profile_id = $1",
    "DELETE FROM profile.experiences WHERE profile_id = $1",
    "DELETE FROM profile.projects WHERE profile_id = $1",
];

/// The contact insert.
const INSERT_CONTACT_SQL: &str = r#"
            INSERT INTO profile.profile_contacts
                (id, profile_id, contact_type, contact_value, label, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#;

/// The link insert.
const INSERT_LINK_SQL: &str = r#"
            INSERT INTO profile.profile_links
                (id, profile_id, link_type, url, label, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#;

/// The skill insert.
const INSERT_SKILL_SQL: &str = r#"
            INSERT INTO profile.profile_skills
                (id, profile_id, skill, normalized_skill, category, display_order, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#;

/// The language insert.
const INSERT_LANGUAGE_SQL: &str = r#"
            INSERT INTO profile.profile_languages
                (id, profile_id, language, normalized_language, proficiency, display_order,
                 created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#;

/// The education insert.
const INSERT_EDUCATION_SQL: &str = r#"
            INSERT INTO profile.education
                (id, profile_id, institution, degree, field, location, start_date, end_date,
                 description, relevant_focus, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, $9, $10)
            "#;

/// The experience insert.
const INSERT_EXPERIENCE_SQL: &str = r#"
            INSERT INTO profile.experiences
                (id, profile_id, company, title, location, start_date, end_date, description,
                 display_order, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#;

/// The project insert.
const INSERT_PROJECT_SQL: &str = r#"
            INSERT INTO profile.projects
                (id, profile_id, name, start_date, end_date, url, description, display_order,
                 created_at)
            VALUES ($1, $2, $3, NULL, NULL, $4, $5, $6, $7)
            "#;

/// The project technology insert.
const INSERT_TECHNOLOGY_SQL: &str = r#"
            INSERT INTO profile.project_technologies
                (id, project_id, technology, normalized_technology, display_order, created_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#;

/// The profile compare-and-set of a project patch (the project must belong to the profile).
const UPDATE_PROJECT_PROFILE_SQL: &str = r#"
                UPDATE profile.profiles
                SET updated_at = $1, revision = $2
                WHERE id = $3
                  AND revision = $4
                  AND EXISTS (
                      SELECT 1 FROM profile.projects
                      WHERE id = $5 AND profile_id = $6
                  )
                "#;

/// The project row lock of a project patch.
const LOCK_PROJECT_SQL: &str = r#"
                SELECT id FROM profile.projects
                WHERE id = $1 AND profile_id = $2
                FOR UPDATE
                "#;

/// The project update of a project patch.
const UPDATE_PROJECT_SQL: &str = r#"
                UPDATE profile.projects
                SET name = $1, url = $2, description = $3, display_order = $4
                WHERE id = $5 AND profile_id = $6
                "#;

/// The technology replacement delete of a project patch.
const DELETE_PROJECT_TECHNOLOGIES_SQL: &str =
    "DELETE FROM profile.project_technologies WHERE project_id = $1";

/// Savepoint name of the project patch.
const PROJECT_SAVEPOINT: &str = "profile_project_update";

/// Loads the complete graph of a profile in a read-only scope.
///
/// The scope begins at REPEATABLE READ and read-only (`BEGIN ISOLATION LEVEL REPEATABLE READ READ
/// ONLY`), so the root row and every child SELECT read one snapshot.
pub(crate) async fn load_read_only(
    client: &Client,
    profile_id: Uuid,
) -> Result<Option<ProfileGraph>, AppError> {
    load(client, profile_id).await
}

/// Loads the complete graph of a profile, or `None` when the root row is missing.
pub(crate) async fn load(
    client: &Client,
    profile_id: Uuid,
) -> Result<Option<ProfileGraph>, AppError> {
    let root_rows = db(client.query(PROFILE_SQL, &[&profile_id]).await)?;
    let Some(root) = root_rows.first() else {
        return Ok(None);
    };
    let profile = ProfileRecord {
        id: col(root, "id")?,
        full_name: col(root, "fullName")?,
        email: col(root, "email")?,
        summary: col(root, "summary")?,
        raw_resume_text: None,
        created_at: col(root, "createdAt")?,
        updated_at: col(root, "updatedAt")?,
        embedding: None,
        revision: col(root, "revision")?,
    };
    let contacts = map_rows(
        &db(client.query(CONTACTS_SQL, &[&profile_id]).await)?,
        |row| {
            Ok(ContactRecord {
                id: col(row, "id")?,
                profile_id: col(row, "profileId")?,
                contact_type: col(row, "contactType")?,
                contact_value: col(row, "contactValue")?,
                label: col(row, "label")?,
                created_at: col(row, "createdAt")?,
                updated_at: col(row, "updatedAt")?,
            })
        },
    )?;
    let links = map_rows(&db(client.query(LINKS_SQL, &[&profile_id]).await)?, |row| {
        Ok(LinkRecord {
            id: col(row, "id")?,
            profile_id: col(row, "profileId")?,
            link_type: col(row, "linkType")?,
            url: col(row, "url")?,
            label: col(row, "label")?,
            created_at: col(row, "createdAt")?,
            updated_at: col(row, "updatedAt")?,
        })
    })?;
    let skills = map_rows(
        &db(client.query(SKILLS_SQL, &[&profile_id]).await)?,
        |row| {
            Ok(SkillRecord {
                id: col(row, "id")?,
                profile_id: col(row, "profileId")?,
                skill: col(row, "skill")?,
                normalized_skill: col(row, "normalizedSkill")?,
                category: col(row, "category")?,
                display_order: i64::from(col::<i32>(row, "displayOrder")?),
                created_at: col(row, "createdAt")?,
            })
        },
    )?;
    let languages = map_rows(
        &db(client.query(LANGUAGES_SQL, &[&profile_id]).await)?,
        |row| {
            Ok(LanguageRecord {
                id: col(row, "id")?,
                profile_id: col(row, "profileId")?,
                language: col(row, "language")?,
                normalized_language: col(row, "normalizedLanguage")?,
                proficiency: col(row, "proficiency")?,
                display_order: i64::from(col::<i32>(row, "displayOrder")?),
                created_at: col(row, "createdAt")?,
            })
        },
    )?;
    let education = map_rows(
        &db(client.query(EDUCATION_SQL, &[&profile_id]).await)?,
        |row| {
            Ok(EducationRecord {
                id: col(row, "id")?,
                profile_id: col(row, "profileId")?,
                institution: col(row, "institution")?,
                degree: col(row, "degree")?,
                field: col(row, "field")?,
                location: col(row, "location")?,
                start_date: col(row, "startDate")?,
                end_date: col(row, "endDate")?,
                relevant_focus: col(row, "relevantFocus")?,
                created_at: col(row, "createdAt")?,
            })
        },
    )?;
    let experiences = map_rows(
        &db(client.query(EXPERIENCES_SQL, &[&profile_id]).await)?,
        |row| {
            Ok(ExperienceRecord {
                id: col(row, "id")?,
                profile_id: col(row, "profileId")?,
                company: col(row, "company")?,
                title: col(row, "title")?,
                location: col(row, "location")?,
                start_date: col(row, "startDate")?,
                end_date: col(row, "endDate")?,
                description: col(row, "description")?,
                display_order: i64::from(col::<i32>(row, "displayOrder")?),
                created_at: col(row, "createdAt")?,
            })
        },
    )?;
    let project_technologies = map_rows(
        &db(client.query(TECHNOLOGIES_SQL, &[&profile_id]).await)?,
        |row| {
            Ok(ProjectTechnologyRecord {
                id: col(row, "id")?,
                project_id: col(row, "projectId")?,
                technology: col(row, "technology")?,
                normalized_technology: col(row, "normalizedTechnology")?,
                display_order: i64::from(col::<i32>(row, "displayOrder")?),
                created_at: col(row, "createdAt")?,
            })
        },
    )?;
    let mut technologies_by_project: HashMap<Uuid, Vec<ProjectTechnologyRecord>> = HashMap::new();
    for technology in &project_technologies {
        technologies_by_project
            .entry(technology.project_id)
            .or_default()
            .push(technology.clone());
    }
    let projects = map_rows(
        &db(client.query(PROJECTS_SQL, &[&profile_id]).await)?,
        |row| {
            let id: Uuid = col(row, "id")?;
            Ok(ProjectRecord {
                id,
                profile_id: col(row, "profileId")?,
                name: col(row, "name")?,
                url: col(row, "url")?,
                description: col(row, "description")?,
                technologies: technologies_by_project
                    .get(&id)
                    .cloned()
                    .unwrap_or_default(),
                display_order: i64::from(col::<i32>(row, "displayOrder")?),
                created_at: col(row, "createdAt")?,
            })
        },
    )?;
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
pub(crate) async fn create(
    client: &Client,
    graph: &ProfileGraph,
) -> Result<ProfileGraph, AppError> {
    insert_profile(client, graph).await?;
    rebuild_search_terms(client, graph).await?;
    load(client, graph.profile.id)
        .await?
        .ok_or_else(unavailable)
}

/// Replaces every child and the root when the stored revision is `expected_revision`. Returns
/// `None` on a revision mismatch.
pub(crate) async fn replace(
    client: &Client,
    graph: &ProfileGraph,
    expected_revision: i64,
) -> Result<Option<ProfileGraph>, AppError> {
    let profile = &graph.profile;
    let updated = db(client
        .execute(
            REPLACE_ROOT_SQL,
            &[
                &profile.full_name,
                &profile.email,
                &profile.summary,
                &profile.updated_at,
                &profile.revision,
                &profile.id,
                &expected_revision,
            ],
        )
        .await)?;
    if updated != 1 {
        return Ok(None);
    }
    delete_children(client, profile.id).await?;
    insert_children(client, graph, profile.id).await?;
    rebuild_search_terms(client, graph).await?;
    load(client, profile.id)
        .await?
        .ok_or_else(unavailable)
        .map(Some)
}

/// Patches one project under the profile revision check, with a savepoint around the project
/// rows. Returns `None` on a mismatch, with nothing left written.
pub(crate) async fn update_project(
    client: &Client,
    change: &ProjectChange,
) -> Result<Option<ProjectPatchResult>, AppError> {
    let profile_id = change.profile_id;
    let project = &change.project;
    let project_id = project.id;
    db(client
        .batch_execute(&format!("SAVEPOINT {PROJECT_SAVEPOINT}"))
        .await)?;
    let profile_rows = db(client
        .execute(
            UPDATE_PROJECT_PROFILE_SQL,
            &[
                &change.updated_at,
                &change.new_revision,
                &profile_id,
                &change.expected_revision,
                &project_id,
                &profile_id,
            ],
        )
        .await)?;
    if profile_rows != 1 {
        return Ok(None);
    }
    let locked = db(client
        .query_opt(LOCK_PROJECT_SQL, &[&project_id, &profile_id])
        .await)?;
    if locked.is_none() {
        rollback_to_savepoint(client).await?;
        return Ok(None);
    }
    let project_rows = db(client
        .execute(
            UPDATE_PROJECT_SQL,
            &[
                &project.name,
                &project.url,
                &project.description,
                &order_value(project.display_order)?,
                &project_id,
                &profile_id,
            ],
        )
        .await)?;
    if project_rows != 1 {
        rollback_to_savepoint(client).await?;
        return Ok(None);
    }
    if let Some(technologies) = &change.replacement_technologies {
        db(client
            .execute(DELETE_PROJECT_TECHNOLOGIES_SQL, &[&project_id])
            .await)?;
        insert_technologies(client, technologies, project_id).await?;
    }
    let graph = load(client, profile_id).await?.ok_or_else(unavailable)?;
    rebuild_search_terms(client, &graph).await?;
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
async fn rollback_to_savepoint(client: &Client) -> Result<(), AppError> {
    db(client
        .batch_execute(&format!("ROLLBACK TO SAVEPOINT {PROJECT_SAVEPOINT}"))
        .await)
}

/// Inserts the root row and every child.
async fn insert_profile(client: &Client, graph: &ProfileGraph) -> Result<(), AppError> {
    let profile = &graph.profile;
    db(client
        .execute(
            INSERT_PROFILE_SQL,
            &[
                &profile.id,
                &profile.full_name,
                &profile.email,
                &profile.summary,
                &profile.created_at,
                &profile.updated_at,
                &profile.revision,
            ],
        )
        .await)?;
    insert_children(client, graph, profile.id).await
}

/// Deletes every profile-owned child, in a fixed order.
async fn delete_children(client: &Client, profile_id: Uuid) -> Result<(), AppError> {
    for statement in DELETE_CHILDREN_SQL {
        db(client.execute(statement, &[&profile_id]).await)?;
    }
    Ok(())
}

/// Inserts every child of the graph under `profile_id` (`_insert_profile_children`).
async fn insert_children(
    client: &Client,
    graph: &ProfileGraph,
    profile_id: Uuid,
) -> Result<(), AppError> {
    for value in &graph.contacts {
        db(client
            .execute(
                INSERT_CONTACT_SQL,
                &[
                    &value.id,
                    &profile_id,
                    &value.contact_type,
                    &value.contact_value,
                    &value.label,
                    &value.created_at,
                    &value.updated_at,
                ],
            )
            .await)?;
    }
    for value in &graph.links {
        db(client
            .execute(
                INSERT_LINK_SQL,
                &[
                    &value.id,
                    &profile_id,
                    &value.link_type,
                    &value.url,
                    &value.label,
                    &value.created_at,
                    &value.updated_at,
                ],
            )
            .await)?;
    }
    for value in &graph.skills {
        db(client
            .execute(
                INSERT_SKILL_SQL,
                &[
                    &value.id,
                    &profile_id,
                    &value.skill,
                    &value.normalized_skill,
                    &value.category,
                    &order_value(value.display_order)?,
                    &value.created_at,
                ],
            )
            .await)?;
    }
    for value in &graph.languages {
        db(client
            .execute(
                INSERT_LANGUAGE_SQL,
                &[
                    &value.id,
                    &profile_id,
                    &value.language,
                    &value.normalized_language,
                    &value.proficiency,
                    &order_value(value.display_order)?,
                    &value.created_at,
                ],
            )
            .await)?;
    }
    for value in &graph.education {
        db(client
            .execute(
                INSERT_EDUCATION_SQL,
                &[
                    &value.id,
                    &profile_id,
                    &value.institution,
                    &value.degree,
                    &value.field,
                    &value.location,
                    &value.start_date,
                    &value.end_date,
                    &value.relevant_focus,
                    &value.created_at,
                ],
            )
            .await)?;
    }
    for value in &graph.experiences {
        db(client
            .execute(
                INSERT_EXPERIENCE_SQL,
                &[
                    &value.id,
                    &profile_id,
                    &value.company,
                    &value.title,
                    &value.location,
                    &value.start_date,
                    &value.end_date,
                    &value.description,
                    &order_value(value.display_order)?,
                    &value.created_at,
                ],
            )
            .await)?;
    }
    for value in &graph.projects {
        db(client
            .execute(
                INSERT_PROJECT_SQL,
                &[
                    &value.id,
                    &profile_id,
                    &value.name,
                    &value.url,
                    &value.description,
                    &order_value(value.display_order)?,
                    &value.created_at,
                ],
            )
            .await)?;
    }
    for value in &graph.project_technologies {
        db(client
            .execute(
                INSERT_TECHNOLOGY_SQL,
                &[
                    &value.id,
                    &value.project_id,
                    &value.technology,
                    &value.normalized_technology,
                    &order_value(value.display_order)?,
                    &value.created_at,
                ],
            )
            .await)?;
    }
    Ok(())
}

/// Inserts a project's technologies under `project_id`.
async fn insert_technologies(
    client: &Client,
    values: &[ProjectTechnologyRecord],
    project_id: Uuid,
) -> Result<(), AppError> {
    if values.is_empty() {
        return Ok(());
    }
    let statement = db(client.prepare(INSERT_TECHNOLOGY_SQL).await)?;
    for value in values {
        db(client
            .execute(
                &statement,
                &[
                    &value.id,
                    &project_id,
                    &value.technology,
                    &value.normalized_technology,
                    &order_value(value.display_order)?,
                    &value.created_at,
                ],
            )
            .await)?;
    }
    Ok(())
}

/// Replaces the profile's search postings with those of `graph`.
async fn rebuild_search_terms(client: &Client, graph: &ProfileGraph) -> Result<(), AppError> {
    let profile_id = graph.profile.id;
    db(client.execute(DELETE_TERMS_SQL, &[&profile_id]).await)?;
    let terms = profile_search_terms(graph);
    if terms.is_empty() {
        return Ok(());
    }
    let mut ids = Vec::with_capacity(terms.len());
    let mut keys = Vec::with_capacity(terms.len());
    let mut texts = Vec::with_capacity(terms.len());
    let mut weights = Vec::with_capacity(terms.len());
    for term in terms {
        weights.push(weight_value(term.weight)?);
        ids.push(term.entity_id);
        keys.push(term.field_key);
        texts.push(term.term);
    }
    db(client
        .execute(INSERT_TERMS_SQL, &[&ids, &keys, &texts, &weights])
        .await)?;
    Ok(())
}
