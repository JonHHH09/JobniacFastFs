//! Resume content preparation: the master, Canadian English
//! and Canadian French resume bodies from one normalized profile graph.
//!
//! The preparation is pure. The caller passes the profile graph, the French translator and a
//! sentence finder; the infrastructure layer supplies none of them. The default finder is the
//! UAX #29 segmenter of `jobniac-text` ([`default_sentence_boundaries`]).
//! Preparation budgets (`PreparationBudget`) refuse oversized profiles with a validation error and
//! never return a partial body.

mod budget;
mod bullets;
mod sections;
mod text;

use std::collections::HashSet;

use jobniac_text::java::{java_is_blank, java_is_whitespace, java_strip, utf16_length};

pub use budget::budget_error;
pub use bullets::{MAX_BULLETS, default_sentence_boundaries, experience_bullets};

use self::budget::{budget_inputs, raw_protected_terms};
use self::sections::{
    French, append_education, append_experiences, append_languages, append_projects, append_skills,
    is_email_contact,
};
use self::text::{default_text, has_text, label_value};
use crate::errors::{ApplicationError, validation_error};
use crate::generation::translation::FrenchTranslator;
use crate::profiles::model::ProfileGraph;

/// The most UTF-16 units one resume body may hold (the inherited generic body limit).
pub const MAX_BODY_UNITS: usize = 50_000;

/// Which resume body is prepared. The wire name is the `resume_type` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeVariant {
    /// The master resume, English, with the projects section.
    MasterResume,
    /// The Canadian English resume.
    CanadianResume,
    /// The Canadian French resume, translated with the French translator.
    CanadianResumeFr,
}

impl ResumeVariant {
    /// The `resume_type` text of the variant.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MasterResume => "master_resume",
            Self::CanadianResume => "canadian_resume",
            Self::CanadianResumeFr => "canadian_resume_fr",
        }
    }
}

/// A prepared resume: the title, the plain-text body and the variant it was prepared as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedResume {
    /// The document title.
    pub title: String,
    /// The body text, stripped, between 1 and [`MAX_BODY_UNITS`] UTF-16 units.
    pub body: String,
    /// The variant the body was prepared as.
    pub resume_type: ResumeVariant,
}

/// The generic body gate: strips the body, then refuses a blank body
/// or one over [`MAX_BODY_UNITS`] UTF-16 units. A missing body is blank.
///
/// # Errors
///
/// Returns a `validation_error` with message `Invalid PDF generation request` and field `body`.
pub fn validate_pdf_body(body: Option<&str>) -> Result<String, ApplicationError> {
    let body = body.map_or("", java_strip);
    let reason = if java_is_blank(body) {
        Some("must not be blank")
    } else if utf16_length(body) > MAX_BODY_UNITS {
        Some("must be less than or equal to 50000 characters")
    } else {
        None
    };
    if let Some(reason) = reason {
        return Err(validation_error(
            "Invalid PDF generation request",
            Some("body"),
            Some(reason),
        ));
    }
    Ok(body.to_owned())
}

/// The French body accepts the Latin-1 printable range and whitespace only.
fn validate_french_font(value: &str, field: &str) -> Result<(), ApplicationError> {
    let supported = value.chars().all(|character| {
        let code = u32::from(character);
        (32..=126).contains(&code) || (160..=255).contains(&code) || java_is_whitespace(character)
    });
    if supported {
        return Ok(());
    }
    Err(validation_error(
        "French Canadian resume contains characters unsupported by the current PDF font",
        Some(field),
        Some("contains unsupported characters"),
    ))
}

/// Joins the lines into a body and applies the French font gate and the body gate.
/// The title is not stripped.
fn finish(
    title: String,
    lines: &[String],
    variant: ResumeVariant,
) -> Result<PreparedResume, ApplicationError> {
    let body = java_strip(&lines.join("\n")).to_owned();
    if variant == ResumeVariant::CanadianResumeFr {
        validate_french_font(&title, "title")?;
        validate_french_font(&body, "body")?;
    }
    Ok(PreparedResume {
        title,
        body: validate_pdf_body(Some(&body))?,
        resume_type: variant,
    })
}

/// The distinct stripped protected terms, in first-seen order.
fn protected_terms(graph: &ProfileGraph) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut terms = Vec::new();
    for term in raw_protected_terms(graph).into_iter().flatten() {
        if has_text(Some(term)) {
            let stripped = java_strip(term);
            if seen.insert(stripped.to_owned()) {
                terms.push(stripped.to_owned());
            }
        }
    }
    terms
}

/// The default finder used by the Canadian bodies when the caller supplies none.
fn no_sentences(_: &str) -> Option<Vec<usize>> {
    None
}

/// Prepares the master resume.
///
/// # Errors
///
/// Returns a validation error when the profile exceeds the preparation budget or the body fails
/// the body gate.
pub fn prepare_master_resume(graph: &ProfileGraph) -> Result<PreparedResume, ApplicationError> {
    budget_inputs(graph, ResumeVariant::MasterResume)?;
    let profile = &graph.profile;
    let mut lines = vec![profile.full_name.clone(), profile.email.clone()];
    if !graph.contacts.is_empty() {
        lines.push(
            graph
                .contacts
                .iter()
                .map(|row| label_value(row.label.as_deref(), &row.contact_type, &row.contact_value))
                .collect::<Vec<_>>()
                .join(" | "),
        );
    }
    if !graph.links.is_empty() {
        lines.push(
            graph
                .links
                .iter()
                .map(|row| label_value(row.label.as_deref(), &row.link_type, &row.url))
                .collect::<Vec<_>>()
                .join(" | "),
        );
    }
    lines.extend([
        String::new(),
        "SUMMARY".to_owned(),
        default_text(profile.summary.as_deref(), "Profile summary not provided."),
        String::new(),
    ]);
    append_skills(&mut lines, graph, "SKILLS", None)?;
    append_languages(&mut lines, graph, None)?;
    append_experiences(&mut lines, graph, false, &no_sentences, None)?;
    append_projects(&mut lines, graph);
    append_education(&mut lines, graph, None)?;
    finish(
        format!("{} - Master Resume", profile.full_name),
        &lines,
        ResumeVariant::MasterResume,
    )
}

/// Prepares the Canadian English resume.
///
/// `sentences` finds the sentence boundaries of a single experience line; use
/// [`default_sentence_boundaries`] for the built-in finder.
///
/// # Errors
///
/// Returns a validation error for an oversized profile or a refused body, and an internal error
/// when the sentence finder fails or returns invalid offsets.
pub fn prepare_canadian_resume(
    graph: &ProfileGraph,
    sentences: &dyn Fn(&str) -> Option<Vec<usize>>,
) -> Result<PreparedResume, ApplicationError> {
    budget_inputs(graph, ResumeVariant::CanadianResume)?;
    let profile = &graph.profile;
    let mut contacts = vec![java_strip(&profile.email).to_owned()];
    for row in &graph.contacts {
        if !is_email_contact(row) {
            contacts.push(label_value(
                row.label.as_deref(),
                &row.contact_type,
                &row.contact_value,
            ));
        }
    }
    let mut lines = vec![format!("Contact: {}", contacts.join(" | "))];
    let links = graph
        .links
        .iter()
        .map(|row| label_value(row.label.as_deref(), &row.link_type, &row.url))
        .collect::<Vec<_>>()
        .join(" | ");
    if has_text(Some(links.as_str())) {
        lines.push(format!("Links: {links}"));
    }
    lines.extend([
        String::new(),
        "PROFESSIONAL SUMMARY".to_owned(),
        default_text(profile.summary.as_deref(), "Profile summary not provided."),
        String::new(),
    ]);
    append_skills(&mut lines, graph, "TECHNICAL SKILLS", None)?;
    append_experiences(&mut lines, graph, true, sentences, None)?;
    append_education(&mut lines, graph, None)?;
    append_languages(&mut lines, graph, None)?;
    finish(
        profile.full_name.clone(),
        &lines,
        ResumeVariant::CanadianResume,
    )
}

/// Prepares the Canadian French resume.
///
/// The English content is read from `graph`, split into bullets with `sentences`, and translated
/// with `translator`. Protected terms (names, companies, institutions, projects, skills and
/// technologies) are copied unchanged.
///
/// # Errors
///
/// Returns a validation error for an oversized profile, a translation budget refusal, a French
/// font refusal or a refused body, and an internal error for a failing sentence finder.
pub fn prepare_french_canadian_resume(
    graph: &ProfileGraph,
    translator: &FrenchTranslator,
    sentences: &dyn Fn(&str) -> Option<Vec<usize>>,
) -> Result<PreparedResume, ApplicationError> {
    budget_inputs(graph, ResumeVariant::CanadianResumeFr)?;
    let terms = protected_terms(graph);
    let term_refs: Vec<&str> = terms.iter().map(String::as_str).collect();
    let protected = translator.prepare_protected_terms(&term_refs)?;
    let french = French {
        translator,
        protected: &protected,
    };
    let profile = &graph.profile;
    let mut contacts = vec![translator.translate_opaque_value(java_strip(&profile.email))?];
    for row in &graph.contacts {
        if is_email_contact(row) {
            continue;
        }
        let label = default_text(row.label.as_deref(), java_strip(&row.contact_type));
        let identity = format!(
            "{label} {}",
            default_text(
                Some(row.contact_type.as_str()),
                row.label.as_deref().unwrap_or_default()
            )
        )
        .to_lowercase();
        let value = if identity.contains("location") || identity.contains("address") {
            french.translate_text(&row.contact_value)?
        } else {
            translator.translate_opaque_value(&row.contact_value)?
        };
        contacts.push(format!(
            "{}: {}",
            translator.translate_label(&label)?,
            java_strip(&value)
        ));
    }
    let mut lines = vec![format!(
        "{}: {}",
        translator.translate_label("Contact")?,
        contacts.join(" | ")
    )];
    let mut link_parts = Vec::with_capacity(graph.links.len());
    for row in &graph.links {
        let label = default_text(row.label.as_deref(), java_strip(&row.link_type));
        let url = translator.translate_opaque_value(&row.url)?;
        link_parts.push(format!(
            "{}: {}",
            translator.translate_label(&label)?,
            java_strip(&url)
        ));
    }
    let links = link_parts.join(" | ");
    if has_text(Some(links.as_str())) {
        lines.push(format!("{}: {links}", translator.translate_label("Links")?));
    }
    let summary = match profile.summary.as_deref() {
        Some(summary) if has_text(Some(summary)) => french.translate_text(summary)?,
        _ => "Résumé du profil non fourni.".to_owned(),
    };
    lines.extend([
        String::new(),
        "PROFIL PROFESSIONNEL".to_owned(),
        summary,
        String::new(),
    ]);
    append_skills(&mut lines, graph, "COMPÉTENCES TECHNIQUES", Some(&french))?;
    append_experiences(&mut lines, graph, true, sentences, Some(&french))?;
    append_education(&mut lines, graph, Some(&french))?;
    append_languages(&mut lines, graph, Some(&french))?;
    finish(
        profile.full_name.clone(),
        &lines,
        ResumeVariant::CanadianResumeFr,
    )
}
