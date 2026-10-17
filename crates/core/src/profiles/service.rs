//! The profile aggregate use cases.
//!
//! Validation runs before any transaction opens. Every write is a compare-and-set on the profile
//! revision: a mismatch, or a revision at the maximum, is a `conflict` and nothing is written.
//! Reads of the graph and writes are separate transactions.

use std::sync::Arc;

use jobniac_text::str_rules::strip_text;
use serde_json::Value;
use uuid::Uuid;

use crate::clock::Clock;
use crate::errors::{AppError, not_found};

use super::draft::{canonicalize_profile_write, canonicalize_project_patch};
use super::graph::{build_profile_graph, fresh_id};
use super::model::{ProfileGraph, ProjectRecord, ProjectTechnologyRecord};
use super::repository::{
    ProfileRepository, ProjectChange, ProjectPatchResult, READ_ONLY, READ_WRITE, UnitOfWork,
};
use super::revision::{next_revision, persisted_revision, profile_conflict, profile_not_found};
use super::values::{expected_revision, uuid_text, write_invalid};

/// Validates, canonicalizes and persists profile graphs.
#[derive(Debug)]
pub struct ProfileService<U, R> {
    uow: U,
    repo: R,
    clock: Arc<dyn Clock>,
}

impl<U, R> ProfileService<U, R>
where
    U: UnitOfWork,
    R: ProfileRepository<Tx = U::Tx>,
{
    /// A service over a unit of work, a repository and a clock.
    pub fn new(uow: U, repo: R, clock: Arc<dyn Clock>) -> Self {
        Self { uow, repo, clock }
    }

    /// The complete graph of a profile.
    ///
    /// # Errors
    ///
    /// `validation_error` for a malformed `profileId`, `not_found` when the profile is missing.
    pub async fn get_profile(&self, profile_id: &str) -> Result<ProfileGraph, AppError> {
        let parsed = uuid_text(profile_id, "profileId")?;
        self.read(parsed).await
    }

    /// Validates and creates the profile and its complete child graph in one transaction.
    ///
    /// # Errors
    ///
    /// The write-request validation error of the first invalid field, or a persistence error.
    pub async fn create_profile(&self, request: &Value) -> Result<ProfileGraph, AppError> {
        let draft = canonicalize_profile_write(request)?;
        let now = self.clock.now();
        let profile_id = Uuid::new_v4();
        let graph = build_profile_graph(&draft, profile_id, now, now, 0, &mut fresh_id);
        self.uow
            .transaction(READ_WRITE, async |tx| {
                self.repo.create_profile(tx, &graph).await
            })
            .await
    }

    /// Replaces every profile-owned child when `expected_revision` matches the stored revision.
    ///
    /// # Errors
    ///
    /// Validation of `profileId`, the request and `expectedRevision`, `not_found`, or `conflict`
    /// for a stale or maximal revision.
    pub async fn update_profile(
        &self,
        profile_id: &str,
        expected_revision_value: &Value,
        request: &Value,
    ) -> Result<ProfileGraph, AppError> {
        let parsed = uuid_text(profile_id, "profileId")?;
        let draft = canonicalize_profile_write(request)?;
        let expected = expected_revision(Some(expected_revision_value))?;
        let current = self.read(parsed).await?;
        if persisted_revision(current.profile.revision)? != expected {
            return Err(profile_conflict(parsed, expected));
        }
        let new_revision = next_revision(parsed, expected)?;
        let now = self.clock.now();
        let graph = build_profile_graph(
            &draft,
            parsed,
            current.profile.created_at,
            now,
            new_revision,
            &mut fresh_id,
        );
        let persisted = self
            .uow
            .transaction(READ_WRITE, async |tx| {
                self.repo.replace_profile(tx, &graph, expected).await
            })
            .await?;
        persisted.ok_or_else(|| profile_conflict(parsed, expected))
    }

    /// Patches one project. Omitted fields and omitted technologies stay unchanged; an explicit
    /// technology list replaces the project's technologies (an empty list clears them).
    ///
    /// # Errors
    ///
    /// Validation of the patch, `not_found` for a missing profile or project, `conflict` for a
    /// stale revision, or a duplicate project name and URL in the same profile.
    pub async fn update_profile_project(
        &self,
        request: &Value,
    ) -> Result<ProjectPatchResult, AppError> {
        let patch = canonicalize_project_patch(request)?;
        let profile_id = patch.profile_id;
        let project_id = patch.project_id;
        let expected = patch.expected_revision;
        let graph = self.read(profile_id).await?;
        if persisted_revision(graph.profile.revision)? != expected {
            return Err(profile_conflict(profile_id, expected));
        }
        let current = graph
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .cloned()
            .ok_or_else(|| project_not_found(profile_id, project_id))?;
        let new_revision = next_revision(profile_id, expected)?;
        let now = self.clock.now();
        let mut updated = current;
        if let Some(name) = patch.name {
            updated.name = Some(name);
        }
        if let Some(url) = patch.url {
            updated.url = Some(url);
        }
        if let Some(description) = patch.description {
            updated.description = Some(description);
        }
        if let Some(display_order) = patch.display_order {
            updated.display_order = display_order;
        }
        // Replacement project technologies always get a fresh identifier; any identifier the
        // request carries is ignored. Only full profile writes honour supplied ids.
        let replacement = patch.technologies.as_ref().map(|items| {
            items
                .iter()
                .map(|item| ProjectTechnologyRecord {
                    id: fresh_id(),
                    project_id,
                    technology: item.technology.clone(),
                    normalized_technology: item.normalized_technology.clone(),
                    display_order: item.display_order,
                    created_at: now,
                })
                .collect::<Vec<_>>()
        });
        if let Some(technologies) = &replacement {
            updated.technologies = technologies.clone();
        }
        validate_project_key(&graph.projects, &updated)?;
        let change = ProjectChange {
            profile_id,
            expected_revision: expected,
            new_revision,
            updated_at: now,
            project: updated,
            replacement_technologies: replacement,
        };
        let result = self
            .uow
            .transaction(READ_WRITE, async |tx| {
                self.repo.update_project(tx, &change).await
            })
            .await?;
        result.ok_or_else(|| profile_conflict(profile_id, expected))
    }

    /// Reads the graph in a read-only transaction, or reports the missing profile.
    async fn read(&self, profile_id: Uuid) -> Result<ProfileGraph, AppError> {
        let found = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo.get_profile(tx, profile_id).await
            })
            .await?;
        found.ok_or_else(|| profile_not_found(profile_id))
    }
}

/// `Profile project not found: <id>` with the `resource`, `profileId` and `projectId` details.
fn project_not_found(profile_id: Uuid, project_id: Uuid) -> AppError {
    AppError::from(not_found(
        &format!("Profile project not found: {project_id}"),
        [
            ("resource", "project".to_owned()),
            ("profileId", profile_id.to_string()),
            ("projectId", project_id.to_string()),
        ],
    ))
}

/// No other project of the profile has the same name and URL key.
fn validate_project_key(
    projects: &[ProjectRecord],
    updated: &ProjectRecord,
) -> Result<(), AppError> {
    let updated_key = project_key(updated);
    for project in projects {
        if project.id != updated.id && project_key(project) == updated_key {
            return Err(write_invalid(
                "name",
                "duplicates another project name/url in this profile",
            ));
        }
    }
    Ok(())
}

/// The name (stripped, lower-cased) and URL (stripped) that identify a project in its profile.
fn project_key(project: &ProjectRecord) -> (String, String) {
    (
        project
            .name
            .as_deref()
            .map_or_else(String::new, |name| strip_text(name).to_lowercase()),
        project
            .url
            .as_deref()
            .map_or_else(String::new, |url| strip_text(url).to_owned()),
    )
}
