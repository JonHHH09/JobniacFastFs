//! English resume preparation and the German resume content builder.
//!
//! The builder ranks and selects content from a profile graph and a job aggregate, builds the
//! English [`StructuredResumeContent`], translates it with the [`GermanResumeTranslator`] and
//! reviews both variants. Every step is pure: the sentence finder, the translator and the optional
//! personal details are injected.
//!
//! The graph is typed, so the cases that untyped input would tolerate only for missing keys or null
//! members cannot occur here (a contact type is always
//! present, a language always has a name). Optional values that arrive as untyped JSON (the
//! personal-details object and the `includeProjects` flag) go through [`parse_personal_details`]
//! and [`parse_include_projects`], which refuse malformed values.
//!
//! Sentence-boundary offsets are code-point indices, the unit of the shared finder
//! ([`default_sentence_boundaries`](crate::generation::content::default_sentence_boundaries)).
//! A Rust string cannot be split inside a surrogate pair, so no surrogate-pair offset check is
//! needed.

use std::cmp::Reverse;
use std::collections::HashSet;

use jobniac_text::java::{
    BudgetField, PreparationBudget, java_is_blank, java_lines, java_string_key, java_strip,
    utf16_length,
};
use serde_json::Value;
use time::{Date, Month};

use super::content::{
    AdditionalEntry, EducationEntry, ExperienceEntry, LanguageEntry, PersonalField, ResumeInput,
    SkillGroup, StructuredResumeContent, budget_resume_content, raw_strings,
};
use super::translation::GermanResumeTranslator;
use crate::errors::{ApplicationError, ErrorCode, validation_error};
use crate::jobs::model::JobAggregate;
use crate::profiles::model::ProfileGraph;

/// The most bullets one experience keeps after ranking.
const EXPERIENCE_BULLETS: usize = 5;
/// The most bullets one project keeps after ranking (before the technologies line).
const PROJECT_BULLETS: usize = 3;
/// The most projects the resume selects.
const MAX_PROJECTS: usize = 2;
/// The most skills the resume selects.
const MAX_SKILLS: usize = 24;
/// The shortest rendered body, in UTF-16 units, a review accepts.
const MIN_BODY_UNITS: usize = 40;

/// Injected personal details: the raw date of birth and
/// nationality text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PersonalDetails {
    /// Date of birth; must be a canonical ISO `YYYY-MM-DD` date when present.
    pub date_of_birth: Option<String>,
    /// Nationality; kept raw, added only when it has text.
    pub nationality: Option<String>,
}

fn details_error(field: &str, reason: Option<&str>) -> ApplicationError {
    validation_error("Invalid personal details", Some(field), reason)
}

/// Reads the injected personal-details value (`None` or an object). `null` is no details;
/// anything but an object is refused with field `personalDetails`. A date of birth that is neither
/// text nor `null` is refused with field `dateOfBirth`, and a nationality that is neither text nor
/// `null` with field `nationality`.
///
/// # Errors
///
/// The `Invalid personal details` validation error.
pub fn parse_personal_details(value: &Value) -> Result<Option<PersonalDetails>, ApplicationError> {
    let object = match value {
        Value::Null => return Ok(None),
        Value::Object(object) => object,
        _ => return Err(details_error("personalDetails", Some("must be an object"))),
    };
    let text = |key: &str| match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(details_error(key, None)),
    };
    Ok(Some(PersonalDetails {
        date_of_birth: text("dateOfBirth")?,
        nationality: text("nationality")?,
    }))
}

/// Reads the `includeProjects` flag: only a JSON boolean is accepted.
///
/// # Errors
///
/// The `Invalid German resume preparation input` validation error with field `includeProjects`.
pub fn parse_include_projects(value: &Value) -> Result<bool, ApplicationError> {
    match value {
        Value::Bool(flag) => Ok(*flag),
        _ => Err(validation_error(
            "Invalid German resume preparation input",
            Some("includeProjects"),
            Some("must be a boolean"),
        )),
    }
}

fn has_text(value: Option<&str>) -> bool {
    value.is_some_and(|text| !java_is_blank(text))
}

/// The stripped value when it has text, else the fallback.
fn default_text(value: Option<&str>, fallback: &str) -> String {
    match value {
        Some(text) if !java_is_blank(text) => java_strip(text).to_owned(),
        _ => fallback.to_owned(),
    }
}

/// The ISO `YYYY-MM-DD` text of a date.
fn iso(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

fn iso_option(date: Option<Date>) -> Option<String> {
    date.map(iso)
}

/// Counts the selected inputs against the preparation budget.
fn budget_inputs(
    graph: &ProfileGraph,
    job: &JobAggregate,
    details: Option<&PersonalDetails>,
    projects: bool,
) -> Result<(), ApplicationError> {
    let mut budget = PreparationBudget::new(BudgetField::Profile);
    let profile = &graph.profile;
    budget.add_text(Some(&profile.full_name))?;
    budget.add_text(Some(&profile.email))?;
    budget.add_text(profile.summary.as_deref())?;
    for row in &graph.contacts {
        budget.add_entries(1)?;
        budget.add_text(row.label.as_deref())?;
        budget.add_text(Some(&row.contact_type))?;
        budget.add_text(Some(&row.contact_value))?;
    }
    for row in &graph.links {
        budget.add_entries(1)?;
        budget.add_text(row.label.as_deref())?;
        budget.add_text(Some(&row.link_type))?;
        budget.add_text(Some(&row.url))?;
    }
    for row in &graph.skills {
        budget.add_entries(1)?;
        budget.add_text(Some(&row.skill))?;
        budget.add_text(row.category.as_deref())?;
    }
    for row in &graph.languages {
        budget.add_entries(1)?;
        budget.add_text(Some(&row.language))?;
        budget.add_text(row.proficiency.as_deref())?;
    }
    for row in &graph.experiences {
        budget.add_entries(1)?;
        budget.add_text(row.title.as_deref())?;
        budget.add_text(row.company.as_deref())?;
        budget.add_text(row.location.as_deref())?;
        budget.add_text(iso_option(row.start_date).as_deref())?;
        budget.add_text(iso_option(row.end_date).as_deref())?;
        budget.add_text(row.description.as_deref())?;
    }
    for row in &graph.education {
        budget.add_entries(1)?;
        budget.add_text(row.degree.as_deref())?;
        budget.add_text(row.institution.as_deref())?;
        budget.add_text(row.field.as_deref())?;
        budget.add_text(row.location.as_deref())?;
        budget.add_text(iso_option(row.start_date).as_deref())?;
        budget.add_text(iso_option(row.end_date).as_deref())?;
        budget.add_text(row.relevant_focus.as_deref())?;
    }
    if projects {
        for row in &graph.projects {
            budget.add_entries(1)?;
            budget.add_text(row.name.as_deref())?;
            budget.add_text(row.description.as_deref())?;
            budget.add_text(row.url.as_deref())?;
            for technology in &row.technologies {
                budget.add_entries(1)?;
                budget.add_text(Some(&technology.technology))?;
            }
        }
    }
    if let Some(details) = details {
        budget.add_entries(1)?;
        budget.add_text(details.date_of_birth.as_deref())?;
        budget.add_text(details.nationality.as_deref())?;
    }
    let record = &job.job;
    budget.add_text(Some(&record.title))?;
    budget.add_text(record.company.as_deref())?;
    budget.add_text(Some(&record.description))?;
    budget.add_text(record.experience_requirement.as_deref())?;
    budget.add_text(record.seniority.as_deref())?;
    for skill in &job.skills {
        budget.add_entries(1)?;
        budget.add_text(Some(&skill.skill))?;
    }
    Ok(())
}

/// The lower-cased tokens of at least three characters over `[a-z0-9+#./]`.
fn tokens(value: Option<&str>) -> HashSet<String> {
    value
        .unwrap_or_default()
        .to_lowercase()
        .split(|character: char| {
            !(character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '+' | '#' | '.' | '/'))
        })
        .filter(|token| token.chars().count() >= 3)
        .map(str::to_owned)
        .collect()
}

/// The tokens of the job title, company, description, requirement, seniority and skills.
fn job_tokens(job: &JobAggregate) -> HashSet<String> {
    let record = &job.job;
    let mut values = vec![
        Some(record.title.as_str()),
        record.company.as_deref(),
        Some(record.description.as_str()),
        record.experience_requirement.as_deref(),
        record.seniority.as_deref(),
    ];
    values.extend(job.skills.iter().map(|row| Some(row.skill.as_str())));
    let joined: String = values
        .into_iter()
        .filter(|value| has_text(*value))
        .flatten()
        .flat_map(|value| [" ", value])
        .collect();
    tokens(Some(&joined))
}

fn overlap(value: Option<&str>, tokens_: &HashSet<String>) -> usize {
    tokens(value).intersection(tokens_).count()
}

/// Orders the bullets by descending overlap (stable) and keeps the first `limit`.
fn rank_bullets(bullets: Vec<String>, tokens_: &HashSet<String>, limit: usize) -> Vec<String> {
    let mut keyed: Vec<(usize, String)> = bullets
        .into_iter()
        .map(|bullet| (overlap(Some(&bullet), tokens_), bullet))
        .collect();
    keyed.sort_by_key(|(score, _)| Reverse(*score));
    keyed
        .into_iter()
        .take(limit)
        .map(|(_, bullet)| bullet)
        .collect()
}

/// Strips a numbered prefix `^\d+[.)]\s+` (ASCII digits, ASCII whitespace) once.
fn strip_numbered_prefix(line: &str) -> &str {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return line;
    }
    let mut characters = line[digits..].chars();
    if !matches!(characters.next(), Some('.' | ')')) {
        return line;
    }
    let rest = characters.as_str();
    let spaces = rest
        .find(|c: char| !matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{0B}' | '\u{0C}'))
        .unwrap_or(rest.len());
    if spaces == 0 {
        return line;
    }
    &rest[spaces..]
}

fn internal(message: &str) -> ApplicationError {
    ApplicationError::with_message(ErrorCode::InternalError, message)
}

/// Splits a description into bullets.
///
/// Each non-blank line is stripped and loses a leading `- `, `* ` or `• `, else a numbered prefix.
/// More than one surviving line gives those lines; exactly one is split into sentences with
/// `sentences` (code-point offsets) and every non-blank fragment is kept. There is no bullet limit.
///
/// # Errors
///
/// The text budget error, or an internal error when the finder fails, returns invalid offsets, or
/// the description has no candidate line.
pub fn split_german_bullets(
    description: Option<&str>,
    sentences: &dyn Fn(&str) -> Option<Vec<usize>>,
) -> Result<Vec<String>, ApplicationError> {
    PreparationBudget::new(BudgetField::Text).add_text(description)?;
    let Some(description) = description.filter(|text| !java_is_blank(text)) else {
        return Ok(Vec::new());
    };
    let mut candidates: Vec<String> = Vec::new();
    for line in java_lines(java_strip(description)) {
        let line = java_strip(line);
        if java_is_blank(line) {
            continue;
        }
        let bullet = match line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("• "))
        {
            Some(rest) => java_strip(rest),
            None => java_strip(strip_numbered_prefix(line)),
        };
        if !java_is_blank(bullet) {
            candidates.push(bullet.to_owned());
        }
    }
    match candidates.len() {
        0 => {
            return Err(internal("German description has no content candidates"));
        }
        1 => {}
        _ => return Ok(candidates),
    }
    let text = candidates[0].as_str();
    let offsets =
        sentences(text).ok_or_else(|| internal("German sentence boundary backend failed"))?;
    let code_points = text.chars().count();
    let valid = offsets.len() >= 2
        && offsets.len() <= code_points + 1
        && offsets.first() == Some(&0)
        && offsets.last() == Some(&code_points)
        && offsets.windows(2).all(|pair| pair[0] < pair[1]);
    if !valid {
        return Err(internal(
            "German sentence boundary backend returned invalid offsets",
        ));
    }
    let byte_offsets: Vec<usize> = text
        .char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(text.len()))
        .collect();
    Ok(offsets
        .windows(2)
        .map(|pair| java_strip(&text[byte_offsets[pair[0]]..byte_offsets[pair[1]]]))
        .filter(|fragment| !java_is_blank(fragment))
        .map(str::to_owned)
        .collect())
}

/// Sorts rows by a nullable text, a missing text after every present one, by UTF-16 order
///; the sort is stable.
fn sorted_nullable<T>(rows: &[T], text: impl Fn(&T) -> Option<&str>) -> Vec<&T> {
    let mut keyed: Vec<((bool, Vec<u8>), &T)> = rows
        .iter()
        .map(|row| {
            let key = match text(row) {
                Some(value) => (false, java_string_key(value)),
                None => (true, Vec::new()),
            };
            (key, row)
        })
        .collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    keyed.into_iter().map(|(_, row)| row).collect()
}

fn email_like(contact_type: &str, contact_value: &str) -> bool {
    [contact_value, contact_type].into_iter().any(|value| {
        has_text(Some(value)) && {
            let lowered = java_strip(value).to_lowercase();
            lowered.contains("email") || lowered.contains('@')
        }
    })
}

/// True for a canonical ISO `YYYY-MM-DD` date of a positive year (it survives a parse and
/// format round trip unchanged).
fn canonical_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit());
    if !shaped {
        return false;
    }
    let number = |range: std::ops::Range<usize>| value[range].parse::<u8>().ok();
    let year = value[0..4].parse::<i32>().ok();
    let (Some(year), Some(month), Some(day)) = (year, number(5..7), number(8..10)) else {
        return false;
    };
    let Ok(month) = Month::try_from(month) else {
        return false;
    };
    year >= 1 && Date::from_calendar_date(year, month, day).is_ok()
}

fn personal_fields(
    graph: &ProfileGraph,
    details: Option<&PersonalDetails>,
) -> Result<Vec<PersonalField>, ApplicationError> {
    let mut fields = vec![PersonalField::new(
        Some("Email"),
        Some(&graph.profile.email),
    )?];
    for row in sorted_nullable(&graph.contacts, |row| Some(row.contact_type.as_str())) {
        if !email_like(&row.contact_type, &row.contact_value) {
            let label = if has_text(row.label.as_deref()) {
                row.label.as_deref()
            } else {
                Some(row.contact_type.as_str())
            };
            fields.push(PersonalField::new(label, Some(&row.contact_value))?);
        }
    }
    for row in sorted_nullable(&graph.links, |row| Some(row.link_type.as_str())) {
        let label = if has_text(row.label.as_deref()) {
            row.label.as_deref()
        } else {
            Some(row.link_type.as_str())
        };
        fields.push(PersonalField::new(label, Some(&row.url))?);
    }
    if let Some(details) = details {
        if let Some(birth) = details.date_of_birth.as_deref() {
            if !canonical_date(birth) {
                return Err(details_error("dateOfBirth", None));
            }
            fields.push(PersonalField::new(Some("Date of birth"), Some(birth))?);
        }
        if has_text(details.nationality.as_deref()) {
            fields.push(PersonalField::new(
                Some("Nationality"),
                details.nationality.as_deref(),
            )?);
        }
    }
    Ok(fields)
}

/// The category priority by substring, first rule first.
fn category_priority(value: Option<&str>) -> u8 {
    let value = value.unwrap_or_default().to_lowercase();
    let rules: [(&[&str], u8); 5] = [
        (&["machine learning", "mlops"], 5),
        (&["architecture"], 4),
        (&["cloud", "ops"], 3),
        (&["backend", "data"], 2),
        (&["security"], 1),
    ];
    for (signals, priority) in rules {
        if signals.iter().any(|signal| value.contains(signal)) {
            return priority;
        }
    }
    0
}

fn skill_groups(
    graph: &ProfileGraph,
    tokens_: &HashSet<String>,
) -> Result<Vec<SkillGroup>, ApplicationError> {
    type Key = (Reverse<usize>, Reverse<u8>, i64, Vec<u8>);
    let mut ranked: Vec<(Key, usize, &str, Option<&str>)> = graph
        .skills
        .iter()
        .map(|row| {
            let score = overlap(Some(&row.skill), tokens_);
            (
                (
                    Reverse(score),
                    Reverse(category_priority(row.category.as_deref())),
                    row.display_order,
                    java_string_key(&row.skill),
                ),
                score,
                row.skill.as_str(),
                row.category.as_deref(),
            )
        })
        .collect();
    ranked.sort_by(|left, right| left.0.cmp(&right.0));
    let relevant: Vec<_> = ranked.iter().filter(|row| row.1 > 0).collect();
    let chosen: Vec<&(Key, usize, &str, Option<&str>)> = if relevant.is_empty() {
        ranked.iter().collect()
    } else {
        relevant
    };
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for (_, _, skill, category) in chosen.into_iter().take(MAX_SKILLS) {
        let category = default_text(*category, "Technical");
        match groups.iter_mut().find(|(name, _)| *name == category) {
            Some((_, skills)) => skills.push((*skill).to_owned()),
            None => groups.push((category, vec![(*skill).to_owned()])),
        }
    }
    groups
        .into_iter()
        .map(|(category, skills)| SkillGroup::new(Some(&category), raw_strings(skills)))
        .collect()
}

fn project_score(row: &crate::profiles::model::ProjectRecord, tokens_: &HashSet<String>) -> usize {
    let mut values = vec![row.name.as_deref(), row.description.as_deref()];
    values.extend(
        row.technologies
            .iter()
            .map(|technology| Some(technology.technology.as_str())),
    );
    let joined: String = values
        .into_iter()
        .filter(|value| has_text(*value))
        .flatten()
        .flat_map(|value| [" ", value])
        .collect();
    overlap(Some(&joined), tokens_)
}

/// Builds the English resume content from the profile graph and the job.
///
/// Projects are neither read nor budgeted unless `include_projects` is set. Experiences are sorted
/// by display order, then (stable) by end, else start, date descending; education by end date
/// descending; bullets are ranked across all candidates before the limit.
///
/// # Errors
///
/// A preparation-budget or `Invalid personal details` validation error, an internal error from the
/// sentence finder, or the content-model refusal.
pub fn build_english_resume(
    graph: &ProfileGraph,
    job: &JobAggregate,
    personal_details: Option<&PersonalDetails>,
    include_projects: bool,
    sentences: &dyn Fn(&str) -> Option<Vec<usize>>,
) -> Result<StructuredResumeContent, ApplicationError> {
    budget_inputs(graph, job, personal_details, include_projects)?;
    let tokens_ = job_tokens(job);
    let mut rows: Vec<&crate::profiles::model::ExperienceRecord> =
        graph.experiences.iter().collect();
    rows.sort_by_key(|row| row.display_order);
    rows.sort_by(|left, right| {
        let key = |row: &crate::profiles::model::ExperienceRecord| {
            row.end_date.or(row.start_date).map(iso).unwrap_or_default()
        };
        key(right).cmp(&key(left))
    });
    let mut experiences = Vec::with_capacity(rows.len());
    for row in rows {
        let bullets = rank_bullets(
            split_german_bullets(row.description.as_deref(), sentences)?,
            &tokens_,
            EXPERIENCE_BULLETS,
        );
        experiences.push(ExperienceEntry::new(
            Some(&default_text(row.title.as_deref(), "Role")),
            Some(&default_text(row.company.as_deref(), "Company")),
            row.location.as_deref(),
            iso_option(row.start_date).as_deref(),
            iso_option(row.end_date).as_deref(),
            raw_strings(bullets),
        )?);
    }
    let mut education_rows: Vec<_> = graph.education.iter().collect();
    education_rows.sort_by(|left, right| {
        let key = |date: Option<Date>| date.map(iso).unwrap_or_default();
        key(right.end_date).cmp(&key(left.end_date))
    });
    let mut education = Vec::with_capacity(education_rows.len());
    for row in education_rows {
        let mut bullets = Vec::new();
        if let Some(field) = row.field.as_deref().filter(|text| !java_is_blank(text)) {
            bullets.push(format!("Field of study: {}", java_strip(field)));
        }
        if let Some(focus) = row
            .relevant_focus
            .as_deref()
            .filter(|text| !java_is_blank(text))
        {
            bullets.push(java_strip(focus).to_owned());
        }
        if let Some(location) = row.location.as_deref().filter(|text| !java_is_blank(text)) {
            bullets.push(format!(
                "International degree program ({}).",
                java_strip(location)
            ));
        }
        education.push(EducationEntry::new(
            Some(&default_text(row.degree.as_deref(), "Degree")),
            Some(&default_text(row.institution.as_deref(), "Institution")),
            row.location.as_deref(),
            iso_option(row.start_date).as_deref(),
            iso_option(row.end_date).as_deref(),
            raw_strings(bullets),
        )?);
    }
    let mut language_rows: Vec<_> = graph.languages.iter().collect();
    language_rows.sort_by_key(|row| (row.display_order, java_string_key(&row.language)));
    let languages = language_rows
        .into_iter()
        .map(|row| LanguageEntry::new(Some(&row.language), row.proficiency.as_deref()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut additional = Vec::new();
    if include_projects {
        let mut project_rows: Vec<_> = graph.projects.iter().collect();
        project_rows.sort_by_key(|row| (Reverse(project_score(row, &tokens_)), row.display_order));
        for row in project_rows.into_iter().take(MAX_PROJECTS) {
            let mut bullets = rank_bullets(
                split_german_bullets(row.description.as_deref(), sentences)?,
                &tokens_,
                PROJECT_BULLETS,
            );
            let technologies = row
                .technologies
                .iter()
                .filter(|technology| has_text(Some(&technology.technology)))
                .map(|technology| technology.technology.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            if has_text(Some(&technologies)) {
                bullets.push(format!("Technologies: {technologies}"));
            }
            additional.push(AdditionalEntry::new(
                Some(&default_text(row.name.as_deref(), "Project")),
                row.url.as_deref(),
                raw_strings(bullets),
            )?);
        }
    }
    StructuredResumeContent::new(ResumeInput {
        full_name: Some(graph.profile.full_name.clone()),
        language: Some("en".to_owned()),
        summary: graph.profile.summary.clone(),
        personal_fields: personal_fields(graph, personal_details)?,
        experiences,
        education,
        skill_groups: skill_groups(graph, &tokens_)?,
        languages,
        additional,
    })
}

/// `MM/YYYY - MM/YYYY` with the unknown and present placeholders.
fn period(start: Option<&str>, end: Option<&str>, german: bool) -> String {
    let (unknown, present) = if german {
        ("unbekannt", "heute")
    } else {
        ("Unknown", "Present")
    };
    if start.is_none() && end.is_none() {
        return unknown.to_owned();
    }
    let month_year = |date: &str| format!("{}/{}", &date[5..7], &date[..4]);
    format!(
        "{} - {}",
        start.map_or_else(|| unknown.to_owned(), month_year),
        end.map_or_else(|| present.to_owned(), month_year)
    )
}

/// The review projection of the content as plain text.
///
/// # Errors
///
/// The preparation-budget error of the content.
pub fn render_resume_body(content: &StructuredResumeContent) -> Result<String, ApplicationError> {
    budget_resume_content(content)?;
    let german = content.language() == "de";
    let pick = |de: &'static str, en: &'static str| if german { de } else { en };
    let mut lines: Vec<String> = vec![content.full_name().to_owned()];
    lines.extend(
        content
            .personal_fields()
            .iter()
            .map(|field| format!("{}: {}", field.label(), field.value())),
    );
    lines.push(String::new());
    if let Some(summary) = content.summary().filter(|text| !java_is_blank(text)) {
        lines.extend([pick("PROFIL", "PROFILE").to_owned(), summary.to_owned()]);
        lines.push(String::new());
    }
    if !content.experiences().is_empty() {
        lines.push(pick("BERUFSERFAHRUNG", "PROFESSIONAL EXPERIENCE").to_owned());
        for row in content.experiences() {
            lines.push(format!("{} | {}", row.title(), row.company()));
            lines.push(format!(
                "{}{}",
                period(row.start_date(), row.end_date(), german),
                location_suffix(row.location())
            ));
            lines.extend(row.bullets().iter().map(|bullet| format!("- {bullet}")));
            lines.push(String::new());
        }
    }
    if !content.additional().is_empty() {
        lines.push(pick("PROJEKTE", "PROJECTS").to_owned());
        for row in content.additional() {
            lines.push(format!(
                "{}{}",
                row.title(),
                location_suffix(row.organization())
            ));
            lines.extend(row.bullets().iter().map(|bullet| format!("- {bullet}")));
            lines.push(String::new());
        }
    }
    if !content.education().is_empty() {
        lines.push(pick("AUSBILDUNG", "EDUCATION").to_owned());
        for row in content.education() {
            lines.push(format!("{} | {}", row.degree(), row.institution()));
            lines.push(format!(
                "{}{}",
                period(row.start_date(), row.end_date(), german),
                location_suffix(row.location())
            ));
            lines.extend(row.bullets().iter().map(|bullet| format!("- {bullet}")));
            lines.push(String::new());
        }
    }
    if !content.skill_groups().is_empty() {
        lines.push(pick("KENNTNISSE", "SKILLS").to_owned());
        lines.extend(
            content
                .skill_groups()
                .iter()
                .map(|row| format!("{}: {}", row.category(), row.skills().join(", "))),
        );
        lines.push(String::new());
    }
    if !content.languages().is_empty() {
        lines.push(pick("SPRACHEN", "LANGUAGES").to_owned());
        lines.push(
            content
                .languages()
                .iter()
                .map(|row| match row.proficiency() {
                    Some(level) if !java_is_blank(level) => {
                        format!("{} ({level})", row.language())
                    }
                    _ => row.language().to_owned(),
                })
                .collect::<Vec<_>>()
                .join(", "),
        );
        lines.push(String::new());
    }
    Ok(java_strip(&lines.join("\n")).to_owned())
}

/// ` | value` when the value has text, else nothing.
fn location_suffix(value: Option<&str>) -> String {
    match value {
        Some(text) if !java_is_blank(text) => format!(" | {text}"),
        _ => String::new(),
    }
}

/// Reviews the content and reports every defect in one validation error; defects are appended per
/// row and may repeat.
///
/// # Errors
///
/// The budget error, or the `German tailored resume content review failed` validation error whose
/// `defects` detail joins the defects with `; `.
pub fn review_resume_content(content: &StructuredResumeContent) -> Result<(), ApplicationError> {
    budget_resume_content(content)?;
    let mut defects: Vec<&str> = Vec::new();
    if content.experiences().is_empty() && content.education().is_empty() {
        defects.push("at least one experience or education entry is required");
    }
    if !content
        .personal_fields()
        .iter()
        .any(|field| field.value().contains('@') || field.label().to_lowercase().contains("mail"))
    {
        defects.push("contact email is required");
    }
    for row in content.experiences() {
        if java_is_blank(row.title()) || java_is_blank(row.company()) {
            defects.push("experience title and company must not be blank");
        }
    }
    for row in content.education() {
        if java_is_blank(row.degree()) || java_is_blank(row.institution()) {
            defects.push("education degree and institution must not be blank");
        }
    }
    if utf16_length(&render_resume_body(content)?) < MIN_BODY_UNITS {
        defects.push("rendered body is too short to be a usable resume");
    }
    if defects.is_empty() {
        return Ok(());
    }
    Err(ApplicationError::new(
        ErrorCode::ValidationError,
        Some("German tailored resume content review failed"),
        [("defects", defects.join("; "))],
    ))
}

/// Prepares the paired English and German resume content: builds
/// the English content, translates it with `translator`, then reviews the English and the German
/// variant in that order.
///
/// # Errors
///
/// Any error of [`build_english_resume`], of the translation, or of the review.
pub fn prepare_german_resume(
    graph: &ProfileGraph,
    job: &JobAggregate,
    personal_details: Option<&PersonalDetails>,
    include_projects: bool,
    sentences: &dyn Fn(&str) -> Option<Vec<usize>>,
    translator: &GermanResumeTranslator,
) -> Result<(StructuredResumeContent, StructuredResumeContent), ApplicationError> {
    let english = build_english_resume(graph, job, personal_details, include_projects, sentences)?;
    let german = translator.to_german(&english)?;
    review_resume_content(&english)?;
    review_resume_content(&german)?;
    Ok((english, german))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_text_sorts_after_every_present_text_and_ties_are_stable() {
        let rows = [
            (None, 0),
            (Some("phone"), 1),
            (Some("\u{e000}"), 2),
            (Some("😀"), 3),
            (None, 4),
            (Some("phone"), 5),
        ];
        let order: Vec<i32> = sorted_nullable(&rows, |row| row.0)
            .into_iter()
            .map(|row| row.1)
            .collect();
        assert_eq!(order, vec![1, 5, 3, 2, 0, 4]);
    }

    #[test]
    fn canonical_dates() {
        assert!(canonical_date("2020-02-29"));
        assert!(!canonical_date("2020-02-31"));
        assert!(!canonical_date("0000-01-01"));
        assert!(!canonical_date("2020-1-1"));
        assert!(!canonical_date("20200101"));
    }
}
