//! Wire-shaped records of the profile aggregate.
//!
//! Field declaration order is the JSON key order, so serialized output is stable
//! byte for byte. Instants serialize with `format_instant` (UTC `Z` and 0, 3 or 6 fraction digits) and dates as `YYYY-MM-DD`. The embedding and raw resume text are
//! never stored, so both serialize as `null`.

use serde::Serialize;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::clock::format_instant;

use super::values::iso_date;

/// The profile root row of an aggregate (`profile` key of the graph).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRecord {
    /// Profile identifier.
    pub id: Uuid,
    /// Full name, stripped.
    pub full_name: String,
    /// Email, stripped and lower-cased.
    pub email: String,
    /// Optional summary, stripped; absent when blank.
    pub summary: Option<String>,
    /// Always `null`: the resume text is never stored.
    pub raw_resume_text: Option<String>,
    /// Creation instant.
    #[serde(serialize_with = "instant")]
    pub created_at: OffsetDateTime,
    /// Last update instant.
    #[serde(serialize_with = "instant")]
    pub updated_at: OffsetDateTime,
    /// Always `null`: the embedding is never stored.
    pub embedding: Option<Vec<f64>>,
    /// Profile revision, from 0 at creation.
    pub revision: i64,
}

/// A contact row (`contacts` of the graph).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactRecord {
    /// Contact identifier.
    pub id: Uuid,
    /// Owning profile.
    pub profile_id: Uuid,
    /// Lower-cased contact type.
    pub contact_type: String,
    /// Stripped contact value.
    pub contact_value: String,
    /// Optional label.
    pub label: Option<String>,
    /// Creation instant.
    #[serde(serialize_with = "instant")]
    pub created_at: OffsetDateTime,
    /// Last update instant.
    #[serde(serialize_with = "instant")]
    pub updated_at: OffsetDateTime,
}

/// A link row (`links` of the graph).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkRecord {
    /// Link identifier.
    pub id: Uuid,
    /// Owning profile.
    pub profile_id: Uuid,
    /// Lower-cased link type.
    pub link_type: String,
    /// Canonical URL (see `link_url`).
    pub url: String,
    /// Optional label.
    pub label: Option<String>,
    /// Creation instant.
    #[serde(serialize_with = "instant")]
    pub created_at: OffsetDateTime,
    /// Last update instant.
    #[serde(serialize_with = "instant")]
    pub updated_at: OffsetDateTime,
}

/// A skill row (`skills` of the graph).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRecord {
    /// Skill identifier.
    pub id: Uuid,
    /// Owning profile.
    pub profile_id: Uuid,
    /// Stripped skill text.
    pub skill: String,
    /// Lower-cased normalized skill.
    pub normalized_skill: String,
    /// Optional category.
    pub category: Option<String>,
    /// Display order, 0 to 2147483647.
    pub display_order: i64,
    /// Creation instant.
    #[serde(serialize_with = "instant")]
    pub created_at: OffsetDateTime,
}

/// A language row (`languages` of the graph).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageRecord {
    /// Language identifier.
    pub id: Uuid,
    /// Owning profile.
    pub profile_id: Uuid,
    /// Stripped language text.
    pub language: String,
    /// Lower-cased normalized language.
    pub normalized_language: String,
    /// Optional proficiency.
    pub proficiency: Option<String>,
    /// Display order, 0 to 2147483647.
    pub display_order: i64,
    /// Creation instant.
    #[serde(serialize_with = "instant")]
    pub created_at: OffsetDateTime,
}

/// An education row (`education` of the graph).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EducationRecord {
    /// Education identifier.
    pub id: Uuid,
    /// Owning profile.
    pub profile_id: Uuid,
    /// Optional institution.
    pub institution: Option<String>,
    /// Optional degree.
    pub degree: Option<String>,
    /// Optional field of study.
    pub field: Option<String>,
    /// Optional location.
    pub location: Option<String>,
    /// Optional start date.
    #[serde(serialize_with = "opt_date")]
    pub start_date: Option<Date>,
    /// Optional end date, not before the start date.
    #[serde(serialize_with = "opt_date")]
    pub end_date: Option<Date>,
    /// Optional relevant focus.
    pub relevant_focus: Option<String>,
    /// Creation instant.
    #[serde(serialize_with = "instant")]
    pub created_at: OffsetDateTime,
}

/// An experience row (`experiences` of the graph).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperienceRecord {
    /// Experience identifier.
    pub id: Uuid,
    /// Owning profile.
    pub profile_id: Uuid,
    /// Optional company.
    pub company: Option<String>,
    /// Optional title.
    pub title: Option<String>,
    /// Optional location.
    pub location: Option<String>,
    /// Optional start date.
    #[serde(serialize_with = "opt_date")]
    pub start_date: Option<Date>,
    /// Optional end date, not before the start date.
    #[serde(serialize_with = "opt_date")]
    pub end_date: Option<Date>,
    /// Optional description.
    pub description: Option<String>,
    /// Display order, 0 to 2147483647.
    pub display_order: i64,
    /// Creation instant.
    #[serde(serialize_with = "instant")]
    pub created_at: OffsetDateTime,
}

/// A project row (`projects` of the graph) with its nested technologies.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    /// Project identifier.
    pub id: Uuid,
    /// Owning profile.
    pub profile_id: Uuid,
    /// Optional name.
    pub name: Option<String>,
    /// Optional canonical URL (project URLs are stored stripped, not canonicalized).
    pub url: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// The project's technologies.
    pub technologies: Vec<ProjectTechnologyRecord>,
    /// Display order, 0 to 2147483647.
    pub display_order: i64,
    /// Creation instant.
    #[serde(serialize_with = "instant")]
    pub created_at: OffsetDateTime,
}

/// A project technology row (`projectTechnologies` of the graph and `technologies` of a project).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTechnologyRecord {
    /// Technology identifier.
    pub id: Uuid,
    /// Owning project.
    pub project_id: Uuid,
    /// Stripped technology text.
    pub technology: String,
    /// Lower-cased normalized technology.
    pub normalized_technology: String,
    /// Display order, 0 to 2147483647.
    pub display_order: i64,
    /// Creation instant.
    #[serde(serialize_with = "instant")]
    pub created_at: OffsetDateTime,
}

/// A complete normalized profile aggregate.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileGraph {
    /// The root row.
    pub profile: ProfileRecord,
    /// Contacts.
    pub contacts: Vec<ContactRecord>,
    /// Links.
    pub links: Vec<LinkRecord>,
    /// Skills.
    pub skills: Vec<SkillRecord>,
    /// Languages.
    pub languages: Vec<LanguageRecord>,
    /// Education.
    pub education: Vec<EducationRecord>,
    /// Experiences.
    pub experiences: Vec<ExperienceRecord>,
    /// Projects, each with its technologies.
    pub projects: Vec<ProjectRecord>,
    /// Every project technology, in project order.
    pub project_technologies: Vec<ProjectTechnologyRecord>,
}

/// Safe identity of a profile, as listed and searched (`profile` of the list and search results).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileIdentity {
    /// Profile identifier.
    pub id: Uuid,
    /// Full name.
    pub full_name: String,
    /// Email.
    pub email: String,
    /// Optional summary.
    pub summary: Option<String>,
    /// Always `null`: resume text is never returned by identity reads.
    pub raw_resume_text: Option<String>,
    /// Creation instant.
    #[serde(serialize_with = "instant")]
    pub created_at: OffsetDateTime,
    /// Last update instant.
    #[serde(serialize_with = "instant")]
    pub updated_at: OffsetDateTime,
    /// Always `null`: embeddings are never returned by identity reads.
    pub embedding: Option<Vec<f64>>,
    /// Profile revision.
    pub revision: i64,
}

impl ProfileIdentity {
    /// Builds an identity with the always-null resume text and embedding.
    pub fn new(
        id: Uuid,
        full_name: String,
        email: String,
        summary: Option<String>,
        created_at: OffsetDateTime,
        updated_at: OffsetDateTime,
        revision: i64,
    ) -> Self {
        Self {
            id,
            full_name,
            email,
            summary,
            raw_resume_text: None,
            created_at,
            updated_at,
            embedding: None,
            revision,
        }
    }

    /// The identity with resume text and embedding forced to null, whatever the repository
    /// supplied. Identity readers apply it so
    /// the privacy invariant holds in core, not only in the adapter.
    #[must_use]
    pub fn without_private_fields(mut self) -> Self {
        self.raw_resume_text = None;
        self.embedding = None;
        self
    }
}

/// Serializes an instant as `format_instant` text.
pub(crate) fn instant<S: serde::Serializer>(
    value: &OffsetDateTime,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format_instant(*value))
}

/// Serializes an optional date as `YYYY-MM-DD` text, or null.
pub(crate) fn opt_date<S: serde::Serializer>(
    value: &Option<Date>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(date) => serializer.serialize_str(&iso_date(*date)),
        None => serializer.serialize_none(),
    }
}
