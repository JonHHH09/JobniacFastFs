//! IPC mirror of the profile use-case results.
//!
//! Field names are camelCase. Revisions and every other `i64` cross as decimal strings,
//! so they never need `bigint` in TypeScript. Display orders are JSON numbers: the core
//! keeps them within `i32` (at most 2147483647). Dates are `YYYY-MM-DD` text and
//! instants are the `format_instant` text, kept as produced. Records keep only the fields
//! the screens use; embeddings, resume text and link or document internals never cross IPC.

use jobniac_core::clock::format_instant;
use jobniac_core::profiles::model::{
    EducationRecord, ExperienceRecord, LanguageRecord, LinkRecord, ProfileGraph, ProfileIdentity,
    ProfileRecord, ProjectRecord, ProjectTechnologyRecord, SkillRecord,
};
use jobniac_core::profiles::repository::ProjectPatchResult;
use jobniac_core::profiles::{PersonalDetailsEnvelope, ProfileListPage, ProfileSearchResponse};
use serde::Serialize;
use specta::Type;
use time::Date;

/// The profile identity shown in the list and the search results (personal data, UI text only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileIdentityDto {
    pub id: String,
    pub full_name: String,
    pub email: String,
}

impl From<&ProfileIdentity> for ProfileIdentityDto {
    fn from(identity: &ProfileIdentity) -> Self {
        Self {
            id: identity.id.to_string(),
            full_name: identity.full_name.clone(),
            email: identity.email.clone(),
        }
    }
}

/// One page of the profile list. An empty `nextCursor` (null) means there is no next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePageDto {
    pub profiles: Vec<ProfileIdentityDto>,
    pub next_cursor: Option<String>,
}

impl From<&ProfileListPage> for ProfilePageDto {
    fn from(page: &ProfileListPage) -> Self {
        Self {
            profiles: page.profiles.iter().map(ProfileIdentityDto::from).collect(),
            next_cursor: page.next_cursor.clone(),
        }
    }
}

/// The root row of a profile graph. `revision` is the compare-and-set value as a string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRecordDto {
    pub id: String,
    pub full_name: String,
    pub email: String,
    pub summary: Option<String>,
    pub revision: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A contact row. Write keys: `id`, `contactType`, `contactValue`, `label`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ContactDto {
    pub id: String,
    pub contact_type: String,
    pub contact_value: String,
    pub label: Option<String>,
}

/// A link row. Write keys: `id`, `linkType`, `url`, `label`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LinkDto {
    pub id: String,
    pub link_type: String,
    pub url: String,
    pub label: Option<String>,
}

/// A skill row. Write keys: `id`, `skill`, `normalizedSkill`, `category`, `displayOrder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillDto {
    pub id: String,
    pub skill: String,
    pub normalized_skill: String,
    pub category: Option<String>,
    pub display_order: i32,
}

/// A language row. Write keys: `id`, `language`, `normalizedLanguage`, `proficiency`, `displayOrder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LanguageDto {
    pub id: String,
    pub language: String,
    pub normalized_language: String,
    pub proficiency: Option<String>,
    pub display_order: i32,
}

/// An education row. Write keys: `id`, `institution`, `degree`, `field`, `location`, `startDate`,
/// `endDate`, `relevantFocus`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EducationDto {
    pub id: String,
    pub institution: Option<String>,
    pub degree: Option<String>,
    pub field: Option<String>,
    pub location: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub relevant_focus: Option<String>,
}

/// An experience row. Write keys: `id`, `company`, `title`, `location`, `startDate`, `endDate`,
/// `description`, `displayOrder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExperienceDto {
    pub id: String,
    pub company: Option<String>,
    pub title: Option<String>,
    pub location: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub description: Option<String>,
    pub display_order: i32,
}

/// A technology under a project. Write keys: `id`, `technology`, `normalizedTechnology`,
/// `displayOrder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTechnologyDto {
    pub id: String,
    pub technology: String,
    pub normalized_technology: String,
    pub display_order: i32,
}

/// A project with its technologies. Write keys: `id`, `name`, `url`, `description`,
/// `displayOrder`, `technologies`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDto {
    pub id: String,
    pub name: Option<String>,
    pub url: Option<String>,
    pub description: Option<String>,
    pub display_order: i32,
    pub technologies: Vec<ProjectTechnologyDto>,
}

/// The complete profile graph returned by get, create and update. Every collection key is present
/// on a read, so the editor treats each collection as provided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileGraphDto {
    pub profile: ProfileRecordDto,
    pub contacts: Vec<ContactDto>,
    pub links: Vec<LinkDto>,
    pub skills: Vec<SkillDto>,
    pub languages: Vec<LanguageDto>,
    pub education: Vec<EducationDto>,
    pub experiences: Vec<ExperienceDto>,
    pub projects: Vec<ProjectDto>,
}

/// The result of a project-only patch: the project after the patch and the new profile revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPatchDto {
    pub project: ProjectDto,
    pub profile_revision: String,
}

/// One search result. `score` is on the wire but the screen does not show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSearchMatchDto {
    pub profile: ProfileIdentityDto,
    pub matched_fields: Vec<String>,
    pub score: String,
}

/// The bounded search response. `totalMatches` is null only when the total is unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSearchDto {
    pub query: String,
    pub query_tokens: Vec<String>,
    pub total_matches: Option<String>,
    pub matched_count: String,
    pub has_more: bool,
    pub returned_count: u32,
    pub profiles: Vec<ProfileSearchMatchDto>,
}

/// The personal details of a profile (no photo bytes; the photo id is only a presence marker).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PersonalDetailsDto {
    pub profile_id: String,
    pub date_of_birth: Option<String>,
    pub nationality: Option<String>,
    pub photo_document_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// The personal-details envelope. `profileRevision` is the one revision check for both forms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PersonalDetailsEnvelopeDto {
    pub profile_id: String,
    pub profile_revision: String,
    pub profile_updated_at: String,
    pub personal_details: Option<PersonalDetailsDto>,
}

/// `YYYY-MM-DD` text of a calendar date.
fn iso_date(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

fn opt_date(date: Option<Date>) -> Option<String> {
    date.map(iso_date)
}

/// The core keeps display orders within `i32` (at most 2147483647), so the value
/// always fits; the clamp only keeps the conversion total.
fn order(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

impl From<&ProfileRecord> for ProfileRecordDto {
    fn from(record: &ProfileRecord) -> Self {
        Self {
            id: record.id.to_string(),
            full_name: record.full_name.clone(),
            email: record.email.clone(),
            summary: record.summary.clone(),
            revision: record.revision.to_string(),
            created_at: format_instant(record.created_at),
            updated_at: format_instant(record.updated_at),
        }
    }
}

impl From<&LinkRecord> for LinkDto {
    fn from(record: &LinkRecord) -> Self {
        Self {
            id: record.id.to_string(),
            link_type: record.link_type.clone(),
            url: record.url.clone(),
            label: record.label.clone(),
        }
    }
}

impl From<&SkillRecord> for SkillDto {
    fn from(record: &SkillRecord) -> Self {
        Self {
            id: record.id.to_string(),
            skill: record.skill.clone(),
            normalized_skill: record.normalized_skill.clone(),
            category: record.category.clone(),
            display_order: order(record.display_order),
        }
    }
}

impl From<&LanguageRecord> for LanguageDto {
    fn from(record: &LanguageRecord) -> Self {
        Self {
            id: record.id.to_string(),
            language: record.language.clone(),
            normalized_language: record.normalized_language.clone(),
            proficiency: record.proficiency.clone(),
            display_order: order(record.display_order),
        }
    }
}

impl From<&EducationRecord> for EducationDto {
    fn from(record: &EducationRecord) -> Self {
        Self {
            id: record.id.to_string(),
            institution: record.institution.clone(),
            degree: record.degree.clone(),
            field: record.field.clone(),
            location: record.location.clone(),
            start_date: opt_date(record.start_date),
            end_date: opt_date(record.end_date),
            relevant_focus: record.relevant_focus.clone(),
        }
    }
}

impl From<&ExperienceRecord> for ExperienceDto {
    fn from(record: &ExperienceRecord) -> Self {
        Self {
            id: record.id.to_string(),
            company: record.company.clone(),
            title: record.title.clone(),
            location: record.location.clone(),
            start_date: opt_date(record.start_date),
            end_date: opt_date(record.end_date),
            description: record.description.clone(),
            display_order: order(record.display_order),
        }
    }
}

impl From<&ProjectTechnologyRecord> for ProjectTechnologyDto {
    fn from(record: &ProjectTechnologyRecord) -> Self {
        Self {
            id: record.id.to_string(),
            technology: record.technology.clone(),
            normalized_technology: record.normalized_technology.clone(),
            display_order: order(record.display_order),
        }
    }
}

impl From<&ProjectRecord> for ProjectDto {
    fn from(record: &ProjectRecord) -> Self {
        Self {
            id: record.id.to_string(),
            name: record.name.clone(),
            url: record.url.clone(),
            description: record.description.clone(),
            display_order: order(record.display_order),
            technologies: record
                .technologies
                .iter()
                .map(ProjectTechnologyDto::from)
                .collect(),
        }
    }
}

impl From<&ProfileGraph> for ProfileGraphDto {
    fn from(graph: &ProfileGraph) -> Self {
        Self {
            profile: ProfileRecordDto::from(&graph.profile),
            contacts: graph
                .contacts
                .iter()
                .map(|record| ContactDto {
                    id: record.id.to_string(),
                    contact_type: record.contact_type.clone(),
                    contact_value: record.contact_value.clone(),
                    label: record.label.clone(),
                })
                .collect(),
            links: graph.links.iter().map(LinkDto::from).collect(),
            skills: graph.skills.iter().map(SkillDto::from).collect(),
            languages: graph.languages.iter().map(LanguageDto::from).collect(),
            education: graph.education.iter().map(EducationDto::from).collect(),
            experiences: graph.experiences.iter().map(ExperienceDto::from).collect(),
            projects: graph.projects.iter().map(ProjectDto::from).collect(),
        }
    }
}

impl From<&ProjectPatchResult> for ProjectPatchDto {
    fn from(result: &ProjectPatchResult) -> Self {
        Self {
            project: ProjectDto::from(&result.project),
            profile_revision: result.profile_revision.to_string(),
        }
    }
}

impl From<&ProfileSearchResponse> for ProfileSearchDto {
    fn from(response: &ProfileSearchResponse) -> Self {
        Self {
            query: response.query.clone(),
            query_tokens: response.query_tokens.clone(),
            total_matches: response.total_matches.map(|total| total.to_string()),
            matched_count: response.matched_count.to_string(),
            has_more: response.has_more,
            returned_count: u32::try_from(response.returned_count).unwrap_or(u32::MAX),
            profiles: response
                .profiles
                .iter()
                .map(|matched| ProfileSearchMatchDto {
                    profile: ProfileIdentityDto::from(&matched.profile),
                    matched_fields: matched.matched_fields.clone(),
                    score: matched.score.to_string(),
                })
                .collect(),
        }
    }
}

impl From<&PersonalDetailsEnvelope> for PersonalDetailsEnvelopeDto {
    fn from(envelope: &PersonalDetailsEnvelope) -> Self {
        Self {
            profile_id: envelope.profile_id.to_string(),
            profile_revision: envelope.profile_revision.to_string(),
            profile_updated_at: format_instant(envelope.profile_updated_at),
            personal_details: envelope
                .personal_details
                .as_ref()
                .map(|record| PersonalDetailsDto {
                    profile_id: record.profile_id.to_string(),
                    date_of_birth: opt_date(record.date_of_birth),
                    nationality: record.nationality.clone(),
                    photo_document_id: record.photo_document_id.map(|id| id.to_string()),
                    created_at: format_instant(record.created_at),
                    updated_at: format_instant(record.updated_at),
                }),
        }
    }
}
