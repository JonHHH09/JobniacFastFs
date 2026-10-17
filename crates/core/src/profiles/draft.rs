//! Write-request canonicalization.
//!
//! Each collection is checked in a fixed order: a field's required text, then its
//! normalized value, then the duplicate key, then its identifier, optional text and display order.
//! The first failure wins, and nothing is written before the whole request is canonical.

use std::collections::HashSet;

use jobniac_text::str_rules::{is_text_whitespace, strip_text};
use serde_json::Value;
use time::Date;
use uuid::Uuid;

use crate::errors::AppError;

use super::link_url::canonical_profile_link_url;
use super::values::{
    REQUEST_MESSAGE, array, display_order, expected_revision, invalid, iso_date, lowercase_text,
    mapping, normalized_text, optional_date, optional_text, optional_uuid, parse_uuid, present,
    required_text, write_invalid,
};

/// A canonical profile write request.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileDraft {
    /// Stripped full name.
    pub full_name: String,
    /// Stripped, lower-cased email.
    pub email: String,
    /// Optional stripped summary.
    pub summary: Option<String>,
    /// Canonical contacts.
    pub contacts: Vec<ContactDraft>,
    /// Canonical links.
    pub links: Vec<LinkDraft>,
    /// Canonical skills.
    pub skills: Vec<SkillDraft>,
    /// Canonical languages.
    pub languages: Vec<LanguageDraft>,
    /// Canonical education entries.
    pub education: Vec<EducationDraft>,
    /// Canonical experiences.
    pub experiences: Vec<ExperienceDraft>,
    /// Canonical projects.
    pub projects: Vec<ProjectDraft>,
}

/// A canonical contact. `id` is `None` when the request omitted it.
#[derive(Debug, Clone, PartialEq)]
pub struct ContactDraft {
    /// Requested identifier, if any.
    pub id: Option<Uuid>,
    /// Lower-cased contact type.
    pub contact_type: String,
    /// Stripped contact value.
    pub contact_value: String,
    /// Optional label.
    pub label: Option<String>,
}

/// A canonical link.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkDraft {
    /// Requested identifier, if any.
    pub id: Option<Uuid>,
    /// Lower-cased link type.
    pub link_type: String,
    /// Canonical URL.
    pub url: String,
    /// Optional label.
    pub label: Option<String>,
}

/// A canonical skill.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillDraft {
    /// Requested identifier, if any.
    pub id: Option<Uuid>,
    /// Stripped skill text.
    pub skill: String,
    /// Lower-cased normalized skill.
    pub normalized_skill: String,
    /// Optional category.
    pub category: Option<String>,
    /// Display order.
    pub display_order: i64,
}

/// A canonical language.
#[derive(Debug, Clone, PartialEq)]
pub struct LanguageDraft {
    /// Requested identifier, if any.
    pub id: Option<Uuid>,
    /// Stripped language text.
    pub language: String,
    /// Lower-cased normalized language.
    pub normalized_language: String,
    /// Optional proficiency.
    pub proficiency: Option<String>,
    /// Display order.
    pub display_order: i64,
}

/// A canonical education entry.
#[derive(Debug, Clone, PartialEq)]
pub struct EducationDraft {
    /// Requested identifier, if any.
    pub id: Option<Uuid>,
    /// Optional institution.
    pub institution: Option<String>,
    /// Optional degree.
    pub degree: Option<String>,
    /// Optional field of study.
    pub field: Option<String>,
    /// Optional location.
    pub location: Option<String>,
    /// Optional relevant focus.
    pub relevant_focus: Option<String>,
    /// Optional start date.
    pub start_date: Option<Date>,
    /// Optional end date.
    pub end_date: Option<Date>,
}

/// A canonical experience.
#[derive(Debug, Clone, PartialEq)]
pub struct ExperienceDraft {
    /// Requested identifier, if any.
    pub id: Option<Uuid>,
    /// Optional company.
    pub company: Option<String>,
    /// Optional title.
    pub title: Option<String>,
    /// Optional location.
    pub location: Option<String>,
    /// Optional start date.
    pub start_date: Option<Date>,
    /// Optional end date.
    pub end_date: Option<Date>,
    /// Optional description.
    pub description: Option<String>,
    /// Display order.
    pub display_order: i64,
}

/// A canonical project. Its URL is stripped but not canonicalized.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectDraft {
    /// Requested identifier, if any.
    pub id: Option<Uuid>,
    /// Optional name.
    pub name: Option<String>,
    /// Optional URL text.
    pub url: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Display order.
    pub display_order: i64,
    /// Canonical technologies.
    pub technologies: Vec<TechnologyDraft>,
}

/// A canonical project technology.
#[derive(Debug, Clone, PartialEq)]
pub struct TechnologyDraft {
    /// Requested identifier, if any.
    pub id: Option<Uuid>,
    /// Stripped technology text.
    pub technology: String,
    /// Lower-cased normalized technology.
    pub normalized_technology: String,
    /// Display order.
    pub display_order: i64,
}

/// A canonical project patch. A `None` field is unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectPatch {
    /// Profile that owns the project.
    pub profile_id: Uuid,
    /// Project to patch.
    pub project_id: Uuid,
    /// Revision the caller read.
    pub expected_revision: i64,
    /// Replacement name, when supplied.
    pub name: Option<String>,
    /// Replacement URL, when supplied.
    pub url: Option<String>,
    /// Replacement description, when supplied.
    pub description: Option<String>,
    /// Replacement display order, when supplied.
    pub display_order: Option<i64>,
    /// Replacement technologies, when supplied (an empty list clears them).
    pub technologies: Option<Vec<TechnologyDraft>>,
}

/// Canonicalizes a profile create or replace request.
///
/// # Errors
///
/// Returns a `validation_error` with the field and reason for the first invalid value.
pub fn canonicalize_profile_write(request: &Value) -> Result<ProfileDraft, AppError> {
    let source = mapping(request, "request")?;
    let full_name = strip_text(required_text(source.get("fullName"), "fullName")?).to_owned();
    let email_text = strip_text(required_text(source.get("email"), "email")?);
    if !is_basic_email(email_text) {
        return Err(write_invalid("email", "must be a valid email address"));
    }
    Ok(ProfileDraft {
        full_name,
        email: lowercase_text(email_text),
        summary: optional_text(source.get("summary"), "summary")?,
        contacts: contacts(source.get("contacts"))?,
        links: links(source.get("links"))?,
        skills: skills(source.get("skills"))?,
        languages: languages(source.get("languages"))?,
        education: education(source.get("education"))?,
        experiences: experiences(source.get("experiences"))?,
        projects: projects(source.get("projects"))?,
    })
}

/// Canonicalizes a project patch request. At least one of the five patch fields must be non-null.
///
/// # Errors
///
/// Returns the `Invalid profile request` or `Invalid profile write request` validation error of
/// the first invalid field.
pub fn canonicalize_project_patch(request: &Value) -> Result<ProjectPatch, AppError> {
    let source = mapping(request, "request")?;
    let profile_id = parse_uuid(source.get("profileId"), "profileId")?;
    let project_id = parse_uuid(source.get("projectId"), "projectId")?;
    let expected = expected_revision(source.get("expectedRevision"))?;
    let fields = ["name", "url", "description", "displayOrder", "technologies"];
    if !fields
        .iter()
        .any(|field| present(source.get(*field)).is_some())
    {
        return Err(invalid(
            REQUEST_MESSAGE,
            "request",
            "must specify at least one project field",
        ));
    }
    let text = |field: &str| -> Result<Option<String>, AppError> {
        match present(source.get(field)) {
            Some(value) => Ok(Some(
                strip_text(required_text(Some(value), field)?).to_owned(),
            )),
            None => Ok(None),
        }
    };
    let name = text("name")?;
    let url = text("url")?;
    let description = text("description")?;
    let display_order = match present(source.get("displayOrder")) {
        Some(value) => Some(super::values::non_negative_order(value, "displayOrder")?),
        None => None,
    };
    let technologies = match present(source.get("technologies")) {
        Some(value) => Some(technologies(Some(value), "technologies", true)?),
        None => None,
    };
    Ok(ProjectPatch {
        profile_id,
        project_id,
        expected_revision: expected,
        name,
        url,
        description,
        display_order,
        technologies,
    })
}

/// True when the stripped address fully matches `[^@\s]+@[^@\s]+\.[^@\s]+`.
fn is_basic_email(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    let plain = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|character| character != '@' && !is_text_whitespace(character))
    };
    if !plain(local) || !plain(domain) {
        return false;
    }
    let characters: Vec<char> = domain.chars().collect();
    characters.len() >= 3 && characters[1..characters.len() - 1].contains(&'.')
}

fn contacts(values: Option<&Value>) -> Result<Vec<ContactDraft>, AppError> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for (index, value) in array(values, "contacts")?.iter().enumerate() {
        let field = format!("contacts[{index}]");
        let item = mapping(value, &field)?;
        let contact_type = lowercase_text(strip_text(required_text(
            item.get("contactType"),
            &format!("{field}.contactType"),
        )?));
        let contact_value = strip_text(required_text(
            item.get("contactValue"),
            &format!("{field}.contactValue"),
        )?)
        .to_owned();
        let key = format!("{contact_type}^@{contact_value}");
        if !seen.insert(key) {
            return Err(write_invalid(
                &format!("{field}.contactValue"),
                "duplicates another contact type/value in this request",
            ));
        }
        result.push(ContactDraft {
            id: optional_uuid(item.get("id"), &format!("{field}.id"))?,
            contact_type,
            contact_value,
            label: optional_text(item.get("label"), &format!("{field}.label"))?,
        });
    }
    Ok(result)
}

fn links(values: Option<&Value>) -> Result<Vec<LinkDraft>, AppError> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for (index, value) in array(values, "links")?.iter().enumerate() {
        let field = format!("links[{index}]");
        let item = mapping(value, &field)?;
        let link_type = lowercase_text(strip_text(required_text(
            item.get("linkType"),
            &format!("{field}.linkType"),
        )?));
        let raw_url = required_text(item.get("url"), &format!("{field}.url"))?;
        let url = canonical_profile_link_url(raw_url);
        let key = format!("{link_type}^@{url}");
        if !seen.insert(key) {
            return Err(write_invalid(
                &format!("{field}.url"),
                "duplicates another link type/url in this request",
            ));
        }
        result.push(LinkDraft {
            id: optional_uuid(item.get("id"), &format!("{field}.id"))?,
            link_type,
            url,
            label: optional_text(item.get("label"), &format!("{field}.label"))?,
        });
    }
    Ok(result)
}

fn skills(values: Option<&Value>) -> Result<Vec<SkillDraft>, AppError> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for (index, value) in array(values, "skills")?.iter().enumerate() {
        let field = format!("skills[{index}]");
        let item = mapping(value, &field)?;
        let skill =
            strip_text(required_text(item.get("skill"), &format!("{field}.skill"))?).to_owned();
        let normalized = normalized_text(
            item.get("normalizedSkill"),
            &skill,
            &format!("{field}.normalizedSkill"),
        )?;
        if !seen.insert(normalized.clone()) {
            return Err(write_invalid(
                &format!("{field}.normalizedSkill"),
                "duplicates another normalized skill in this request",
            ));
        }
        result.push(SkillDraft {
            id: optional_uuid(item.get("id"), &format!("{field}.id"))?,
            skill,
            normalized_skill: normalized,
            category: optional_text(item.get("category"), &format!("{field}.category"))?,
            display_order: display_order(item, &format!("{field}.displayOrder"))?,
        });
    }
    Ok(result)
}

fn languages(values: Option<&Value>) -> Result<Vec<LanguageDraft>, AppError> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for (index, value) in array(values, "languages")?.iter().enumerate() {
        let field = format!("languages[{index}]");
        let item = mapping(value, &field)?;
        let language = strip_text(required_text(
            item.get("language"),
            &format!("{field}.language"),
        )?)
        .to_owned();
        let normalized = normalized_text(
            item.get("normalizedLanguage"),
            &language,
            &format!("{field}.normalizedLanguage"),
        )?;
        if !seen.insert(normalized.clone()) {
            return Err(write_invalid(
                &format!("{field}.normalizedLanguage"),
                "duplicates another normalized language in this request",
            ));
        }
        result.push(LanguageDraft {
            id: optional_uuid(item.get("id"), &format!("{field}.id"))?,
            language,
            normalized_language: normalized,
            proficiency: optional_text(item.get("proficiency"), &format!("{field}.proficiency"))?,
            display_order: display_order(item, &format!("{field}.displayOrder"))?,
        });
    }
    Ok(result)
}

/// The ISO date or `None`, used in the duplicate keys of education and experiences.
fn date_key(date: Option<Date>) -> String {
    date.map_or_else(|| "None".to_owned(), iso_date)
}

/// Lower-cased text, or the empty string for an absent value.
fn optional_normalized(value: &Option<String>) -> String {
    value.as_deref().map_or_else(String::new, lowercase_text)
}

fn education(values: Option<&Value>) -> Result<Vec<EducationDraft>, AppError> {
    let mut result = Vec::new();
    let mut seen: HashSet<(String, String, String, String, String)> = HashSet::new();
    for (index, value) in array(values, "education")?.iter().enumerate() {
        let field = format!("education[{index}]");
        let item = mapping(value, &field)?;
        let start = optional_date(item.get("startDate"), &format!("{field}.startDate"))?;
        let end = optional_date(item.get("endDate"), &format!("{field}.endDate"))?;
        if let (Some(start), Some(end)) = (start, end)
            && end < start
        {
            return Err(write_invalid(
                &format!("{field}.endDate"),
                "must not be before startDate",
            ));
        }
        let institution = optional_text(item.get("institution"), &format!("{field}.institution"))?;
        let degree = optional_text(item.get("degree"), &format!("{field}.degree"))?;
        let study_field = optional_text(item.get("field"), &format!("{field}.field"))?;
        let location = optional_text(item.get("location"), &format!("{field}.location"))?;
        let relevant_focus =
            optional_text(item.get("relevantFocus"), &format!("{field}.relevantFocus"))?;
        let key = (
            optional_normalized(&institution),
            optional_normalized(&degree),
            optional_normalized(&study_field),
            date_key(start),
            date_key(end),
        );
        if !seen.insert(key) {
            return Err(write_invalid(
                &format!("{field}.institution"),
                "duplicates another education entry in this request",
            ));
        }
        result.push(EducationDraft {
            id: optional_uuid(item.get("id"), &format!("{field}.id"))?,
            institution,
            degree,
            field: study_field,
            location,
            relevant_focus,
            start_date: start,
            end_date: end,
        });
    }
    Ok(result)
}

fn experiences(values: Option<&Value>) -> Result<Vec<ExperienceDraft>, AppError> {
    let mut result = Vec::new();
    let mut seen: HashSet<(String, String, String, String)> = HashSet::new();
    for (index, value) in array(values, "experiences")?.iter().enumerate() {
        let field = format!("experiences[{index}]");
        let item = mapping(value, &field)?;
        let start = optional_date(item.get("startDate"), &format!("{field}.startDate"))?;
        let end = optional_date(item.get("endDate"), &format!("{field}.endDate"))?;
        if let (Some(start), Some(end)) = (start, end)
            && end < start
        {
            return Err(write_invalid(
                &format!("{field}.endDate"),
                "must not be before startDate",
            ));
        }
        let company = optional_text(item.get("company"), &format!("{field}.company"))?;
        let title = optional_text(item.get("title"), &format!("{field}.title"))?;
        let location = optional_text(item.get("location"), &format!("{field}.location"))?;
        let description = optional_text(item.get("description"), &format!("{field}.description"))?;
        let order = display_order(item, &format!("{field}.displayOrder"))?;
        let key = (
            optional_normalized(&company),
            optional_normalized(&title),
            date_key(start),
            date_key(end),
        );
        if !seen.insert(key) {
            return Err(write_invalid(
                &format!("{field}.company"),
                "duplicates another experience entry in this request",
            ));
        }
        result.push(ExperienceDraft {
            id: optional_uuid(item.get("id"), &format!("{field}.id"))?,
            company,
            title,
            location,
            start_date: start,
            end_date: end,
            description,
            display_order: order,
        });
    }
    Ok(result)
}

fn projects(values: Option<&Value>) -> Result<Vec<ProjectDraft>, AppError> {
    let mut result = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for (index, value) in array(values, "projects")?.iter().enumerate() {
        let field = format!("projects[{index}]");
        let item = mapping(value, &field)?;
        let name = optional_text(item.get("name"), &format!("{field}.name"))?;
        let url = optional_text(item.get("url"), &format!("{field}.url"))?;
        let description = optional_text(item.get("description"), &format!("{field}.description"))?;
        let order = display_order(item, &format!("{field}.displayOrder"))?;
        let key = (optional_normalized(&name), url.clone().unwrap_or_default());
        if !seen.insert(key) {
            return Err(write_invalid(
                &format!("{field}.name"),
                "duplicates another project entry in this request",
            ));
        }
        let technologies = technologies(
            item.get("technologies"),
            &format!("{field}.technologies"),
            false,
        )?;
        result.push(ProjectDraft {
            id: optional_uuid(item.get("id"), &format!("{field}.id"))?,
            name,
            url,
            description,
            display_order: order,
            technologies,
        });
    }
    Ok(result)
}

/// Canonicalizes technologies. In a project patch an explicit normalized value must be text
/// that is not blank; otherwise a blank value is refused with the same reason.
fn technologies(
    values: Option<&Value>,
    field: &str,
    project_patch: bool,
) -> Result<Vec<TechnologyDraft>, AppError> {
    let mut result = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (index, value) in array(values, field)?.iter().enumerate() {
        let item_field = format!("{field}[{index}]");
        let item = mapping(value, &item_field)?;
        let technology = strip_text(required_text(
            item.get("technology"),
            &format!("{item_field}.technology"),
        )?)
        .to_owned();
        let normalized_field = format!("{item_field}.normalizedTechnology");
        let normalized = match item.get("normalizedTechnology") {
            None | Some(Value::Null) => lowercase_text(&technology),
            Some(supplied) => {
                let blank =
                    !matches!(supplied, Value::String(text) if !strip_text(text).is_empty());
                if project_patch && blank {
                    return Err(write_invalid(
                        &normalized_field,
                        "must not be blank when supplied",
                    ));
                }
                lowercase_text(strip_text(required_text(
                    Some(supplied),
                    &normalized_field,
                )?))
            }
        };
        if !seen.insert(normalized.clone()) {
            return Err(write_invalid(
                &normalized_field,
                "duplicates another project technology in this request",
            ));
        }
        result.push(TechnologyDraft {
            id: optional_uuid(item.get("id"), &format!("{item_field}.id"))?,
            technology,
            normalized_technology: normalized,
            display_order: display_order(item, &format!("{item_field}.displayOrder"))?,
        });
    }
    Ok(result)
}
