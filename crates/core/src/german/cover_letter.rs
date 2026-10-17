//! Conservative German cover-letter content and the review-only text projection.
//!
//! The template text is deterministic, including the
//! Java-style concatenation quirk where a missing title, company, degree or institution reads
//! `null`. The typed profile and job views make a missing contact type or value
//! unrepresentable; see the note on [`build_german_cover_letter`].

use std::collections::HashSet;

use jobniac_text::java::{
    BudgetField, PreparationBudget, java_compare, java_is_blank, java_strip, utf16_length,
};
use time::Date;

use super::content::{
    CoverLetterInput, GermanCoverLetterContent, PersonalField, budget_cover_letter_content,
    raw_strings,
};
use crate::errors::{ApplicationError, ErrorCode};
use crate::jobs::JobAggregate;
use crate::profiles::ProfileGraph;
use crate::profiles::model::{ExperienceRecord, SkillRecord};

/// The most personal fields of a cover letter (the e-mail field, contacts, then links).
const MAX_PERSONAL_FIELDS: usize = 6;
/// The most contact rows kept before links are added.
const MAX_CONTACTS: usize = 5;
/// The most skills named in the skills paragraph.
const MAX_SKILLS: usize = 5;
/// The minimum UTF-16 length of the review subject.
const MIN_SUBJECT_UNITS: usize = 12;
/// The minimum UTF-16 length of the review body projection.
const MIN_BODY_UNITS: usize = 140;
/// The safe message of a failed content review.
const REVIEW_MESSAGE: &str = "German cover-letter content review failed";

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

/// Counts the selected profile and job inputs against the preparation budget; unselected private
/// text is outside the budget.
fn budget_inputs(graph: &ProfileGraph, job: &JobAggregate) -> Result<(), ApplicationError> {
    let mut budget = PreparationBudget::new(BudgetField::Profile);
    budget.add_text(Some(&graph.profile.full_name))?;
    budget.add_text(Some(&graph.profile.email))?;
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
    for row in &graph.experiences {
        budget.add_entries(1)?;
        budget.add_text(row.title.as_deref())?;
        budget.add_text(row.company.as_deref())?;
        budget.add_text(iso_text(row.start_date).as_deref())?;
        budget.add_text(iso_text(row.end_date).as_deref())?;
    }
    for row in &graph.education {
        budget.add_entries(1)?;
        budget.add_text(row.degree.as_deref())?;
        budget.add_text(row.institution.as_deref())?;
    }
    for row in &graph.skills {
        budget.add_entries(1)?;
        budget.add_text(Some(&row.skill))?;
    }
    budget.add_text(Some(&job.job.title))?;
    budget.add_text(job.job.company.as_deref())?;
    budget.add_text(job.job.location.as_deref())?;
    budget.add_text(Some(&job.job.description))?;
    for row in &job.skills {
        budget.add_entries(1)?;
        budget.add_text(Some(&row.skill))?;
    }
    Ok(())
}

/// A present, non-blank value.
fn has_text(value: Option<&str>) -> bool {
    value.is_some_and(|text| !java_is_blank(text))
}

/// The stripped value, or none when it is missing or blank.
fn optional(value: Option<&str>) -> Option<String> {
    value
        .filter(|text| !java_is_blank(text))
        .map(|text| java_strip(text).to_owned())
}

/// Java string concatenation of a nullable value: none reads `null`.
fn java_concat(value: Option<&str>) -> &str {
    value.unwrap_or("null")
}

/// The lower-cased ASCII tokens of at least two characters; every run of characters outside
/// `[a-z0-9+#./]` delimits.
fn tokens(value: &str) -> HashSet<String> {
    value
        .to_lowercase()
        .split(|character: char| {
            !(character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '+' | '#' | '.' | '/'))
        })
        .filter(|token| token.chars().count() >= 2)
        .map(str::to_owned)
        .collect()
}

/// The label of a contact or link: the stripped label if it has text, else the type.
fn field_label(label: Option<&str>, fallback: &str) -> String {
    let chosen = if has_text(label) {
        label.unwrap_or(fallback)
    } else {
        fallback
    };
    java_strip(chosen).to_owned()
}

/// The personal fields: the e-mail, up to five non-mail contacts, then links up to six in all.
fn personal_fields(graph: &ProfileGraph) -> Result<Vec<PersonalField>, ApplicationError> {
    let mut fields = vec![PersonalField::new(
        Some("E-Mail"),
        Some(&graph.profile.email),
    )?];
    let mut contacts: Vec<_> = graph
        .contacts
        .iter()
        .filter(|row| {
            !row.contact_value.contains('@') && !row.contact_type.to_lowercase().contains("mail")
        })
        .collect();
    contacts.sort_by(|left, right| java_compare(&left.contact_type, &right.contact_type));
    for row in contacts.into_iter().take(MAX_CONTACTS) {
        fields.push(PersonalField::new(
            Some(&field_label(row.label.as_deref(), &row.contact_type)),
            Some(&row.contact_value),
        )?);
    }
    let mut links: Vec<_> = graph.links.iter().collect();
    links.sort_by(|left, right| java_compare(&left.link_type, &right.link_type));
    let room = MAX_PERSONAL_FIELDS.saturating_sub(fields.len());
    for row in links.into_iter().take(room) {
        fields.push(PersonalField::new(
            Some(&field_label(row.label.as_deref(), &row.link_type)),
            Some(&row.url),
        )?);
    }
    Ok(fields)
}

/// The evidence paragraph: the latest experience, else the first education row.
fn evidence(graph: &ProfileGraph) -> Option<String> {
    let mut experiences: Vec<_> = graph.experiences.iter().collect();
    let key =
        |row: &&ExperienceRecord| iso_text(row.end_date.or(row.start_date)).unwrap_or_default();
    // Stable descending order: equal keys keep the input order.
    experiences.sort_by_key(|row| std::cmp::Reverse(key(row)));
    if let Some(row) = experiences.first() {
        return Some(format!(
            "In meiner bisherigen Tätigkeit als {} bei {} habe ich praktische Erfahrung gesammelt.",
            java_concat(row.title.as_deref()),
            java_concat(row.company.as_deref()),
        ));
    }
    graph.education.first().map(|row| {
        format!(
            "Mein Abschluss {} an {} bildet eine fachliche Grundlage für diese Aufgabe.",
            java_concat(row.degree.as_deref()),
            java_concat(row.institution.as_deref()),
        )
    })
}

/// The up-to-five profile skills most relevant to the job.
fn skills(graph: &ProfileGraph, job: &JobAggregate) -> Vec<String> {
    let job_skills: Vec<&str> = job.skills.iter().map(|row| row.skill.as_str()).collect();
    let wanted = tokens(&format!(
        "{} {} {}",
        job.job.title,
        job.job.description,
        job_skills.join(" ")
    ));
    let mut ranked: Vec<(usize, &SkillRecord)> = graph
        .skills
        .iter()
        .map(|row| (tokens(&row.skill).intersection(&wanted).count(), row))
        .collect();
    ranked.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then(left.display_order.cmp(&right.display_order))
            .then_with(|| java_compare(&left.skill, &right.skill))
    });
    let relevant: Vec<_> = ranked.iter().filter(|(score, _)| *score > 0).collect();
    let chosen: Vec<_> = if relevant.is_empty() {
        ranked.iter().collect()
    } else {
        relevant
    };
    chosen
        .into_iter()
        .take(MAX_SKILLS)
        .map(|(_, row)| row.skill.clone())
        .collect()
}

/// Builds the deterministic German cover letter from a profile graph and a job.
///
/// A `None` contact type or value in the cover-letter path cannot occur.
/// The typed [`ProfileGraph`] holds both as required strings, so
/// that input cannot be represented and no crash or extra validation is needed; the typed view
/// is the validation point.
///
/// # Errors
///
/// The preparation budget error, or the fixed content validation error from the records.
pub fn build_german_cover_letter(
    graph: &ProfileGraph,
    job: &JobAggregate,
) -> Result<GermanCoverLetterContent, ApplicationError> {
    budget_inputs(graph, job)?;
    let title = java_strip(&job.job.title).to_owned();
    let company = optional(job.job.company.as_deref());
    let company_suffix = company
        .as_deref()
        .map(|name| format!(" bei {name}"))
        .unwrap_or_default();
    let mut paragraphs = vec![format!(
        "Mit großem Interesse bewerbe ich mich als {title}{company_suffix}."
    )];
    if let Some(text) = evidence(graph) {
        paragraphs.push(text);
    }
    let named = skills(graph, job);
    if !named.is_empty() {
        paragraphs.push(format!(
            "Für diese Position bringe ich insbesondere Kenntnisse in {} mit.",
            named.join(", ")
        ));
    }
    paragraphs.push(
        "Gern erläutere ich Ihnen in einem persönlichen Gespräch, \
         wie ich meine Erfahrung in Ihr Team einbringen kann."
            .to_owned(),
    );
    GermanCoverLetterContent::new(CoverLetterInput {
        sender_name: Some(graph.profile.full_name.clone()),
        personal_fields: personal_fields(graph)?,
        recipient_company: company,
        recipient_location: optional(job.job.location.as_deref()),
        job_title: Some(title.clone()),
        subject: Some(format!("Bewerbung als {title}{company_suffix}")),
        salutation: Some("Sehr geehrte Damen und Herren,".to_owned()),
        paragraphs: raw_strings(paragraphs),
        closing: Some("Mit freundlichen Grüßen,".to_owned()),
        signature: Some(graph.profile.full_name.clone()),
    })
}

/// The pinned deterministic review projection; not the separately laid-out PDF body.
///
/// # Errors
///
/// The preparation budget error.
pub fn render_cover_letter_body(
    content: &GermanCoverLetterContent,
) -> Result<String, ApplicationError> {
    budget_cover_letter_content(content)?;
    let mut lines: Vec<String> = vec![content.sender_name().to_owned()];
    lines.extend(
        content
            .personal_fields()
            .iter()
            .map(|field| format!("{}: {}", field.label(), field.value())),
    );
    if let Some(company) = content.recipient_company() {
        lines.push(company.to_owned());
    }
    if let Some(location) = content.recipient_location() {
        lines.push(location.to_owned());
    }
    lines.push(content.subject().to_owned());
    lines.push(content.salutation().to_owned());
    for paragraph in content.paragraphs() {
        lines.push(paragraph.clone());
        lines.push(String::new());
    }
    lines.push(content.closing().to_owned());
    lines.push(content.signature().to_owned());
    Ok(java_strip(&lines.join("\n")).to_owned())
}

/// Whether a character is an ISO control character (`U+0000..=U+001F`, `U+007F..=U+009F`).
fn is_control(character: char) -> bool {
    let code = u32::from(character);
    code <= 0x1F || (0x7F..=0x9F).contains(&code)
}

/// Whether any text field of the content holds an ISO control character.
fn contains_controls(content: &GermanCoverLetterContent) -> bool {
    let mut values: Vec<&str> = vec![
        content.sender_name(),
        content.job_title(),
        content.subject(),
        content.salutation(),
        content.closing(),
        content.signature(),
    ];
    values.extend(content.recipient_company());
    values.extend(content.recipient_location());
    values.extend(content.paragraphs().iter().map(String::as_str));
    for field in content.personal_fields() {
        values.push(field.label());
        values.push(field.value());
    }
    values
        .into_iter()
        .any(|value| value.chars().any(is_control))
}

/// Reviews cover-letter content; every defect is collected and reported in a fixed order.
///
/// # Errors
///
/// The preparation budget error, or a `validation_error` with the `defects` detail joined by `; `.
pub fn review_cover_letter_content(
    content: &GermanCoverLetterContent,
) -> Result<(), ApplicationError> {
    budget_cover_letter_content(content)?;
    let mut defects: Vec<&str> = Vec::new();
    if !content
        .personal_fields()
        .iter()
        .any(|field| field.value().contains('@') || field.label().to_lowercase().contains("mail"))
    {
        defects.push("contact email is required");
    }
    if content.paragraphs().len() < 2 {
        defects.push("at least two ordered paragraphs are required");
    }
    if utf16_length(content.subject()) < MIN_SUBJECT_UNITS {
        defects.push("subject is too short");
    }
    if !content
        .subject()
        .to_lowercase()
        .contains(&content.job_title().to_lowercase())
    {
        defects.push("subject must identify the job title");
    }
    if contains_controls(content) {
        defects.push("content must not contain control characters");
    }
    if utf16_length(&render_cover_letter_body(content)?) < MIN_BODY_UNITS {
        defects.push("rendered body is too short to be a usable cover letter");
    }
    if defects.is_empty() {
        return Ok(());
    }
    Err(ApplicationError::new(
        ErrorCode::ValidationError,
        Some(REVIEW_MESSAGE),
        [("defects", defects.join("; "))],
    ))
}
