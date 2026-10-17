//! Deterministic resume text extraction.
//!
//! The extractor turns the plain text of one resume PDF into a profile write request. The text is
//! untrusted data: only recognizable identity and section fields are materialized, and nothing in
//! it is interpreted, executed or fetched. The module is pure: no IO and no database.
//!
//! The regular expressions and the string
//! rules are in [`pattern`] and [`text`]; the section split is in [`sections`]; the link,
//! phrase and entry builders are in [`entries`].

mod entries;
mod pattern;
mod sections;
mod text;

use jobniac_text::java::utf16_length;
use jobniac_text::search::lower_text;
use jobniac_text::str_rules::{is_text_alpha, is_text_decimal, split_lines, strip_text};
use serde_json::{Value, json};

use crate::errors::{AppError, validation_error};

use self::pattern::{EMAIL, PHONE, guarded_find};
use self::text::{local_part_words, title_case};

/// The message of every extraction validation error.
pub const EXTRACTION_INPUT_MESSAGE: &str = "Invalid profile extraction input";

/// The longest display name in UTF-16 units.
const NAME_LIMIT: usize = 80;

/// Extracts a profile write request from resume text.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeterministicProfileTextExtractor;

impl DeterministicProfileTextExtractor {
    /// Extracts the complete deterministic write-request shape of a resume text.
    ///
    /// The first email address is required, and so is a name: the first plausible line among the
    /// first twelve, or else the title-cased local part of the email when it has separators. The
    /// value is a profile write request, with `id` null on every child and the
    /// same key names, so that `profile_details` canonicalization accepts it unchanged.
    ///
    /// # Errors
    ///
    /// `validation_error` with message `Invalid profile extraction input` and the field `text`,
    /// `email` or `fullName`.
    pub fn extract_profile(&self, text: &str) -> Result<Value, AppError> {
        if strip_text(text).is_empty() {
            return Err(invalid("text", "must not be blank"));
        }
        let email = EMAIL
            .find(text)
            .map(|found| found.as_str())
            .ok_or_else(|| invalid("email", "could not be extracted from PDF text"))?;
        let name = first_likely_name(text, email)
            .ok_or_else(|| invalid("fullName", "could not be extracted from PDF text"))?;
        let mut contacts = vec![json!({
            "id": null,
            "contactType": "email",
            "contactValue": email,
            "label": "extracted resume email",
        })];
        if let Some(phone) = find_phone(text) {
            contacts.push(json!({
                "id": null,
                "contactType": "phone",
                "contactValue": phone,
                "label": "extracted resume phone",
            }));
        }
        let sections = sections::split(text);
        Ok(json!({
            "fullName": name,
            "email": email,
            "summary": sections::summary(text, &sections.summary),
            "contacts": contacts,
            "links": entries::links(text),
            "skills": entries::skill_values(&sections.skills),
            "languages": entries::language_values(&sections.languages),
            "education": sections
                .education
                .iter()
                .map(|entry| entries::education(entry))
                .collect::<Vec<_>>(),
            "experiences": sections
                .experience
                .iter()
                .enumerate()
                .map(|(index, entry)| entries::experience(entry, index))
                .collect::<Vec<_>>(),
            "projects": sections
                .projects
                .iter()
                .enumerate()
                .map(|(index, entry)| entries::project(entry, index))
                .collect::<Vec<_>>(),
        }))
    }
}

/// The first phone number of the text, kept as written.
pub(crate) fn find_phone(text: &str) -> Option<&str> {
    guarded_find(text, &PHONE, |character| {
        character == '+' || character == '(' || is_text_decimal(character)
    })
}

/// The first of the first twelve lines that reads as a name, or else
/// the title-cased email local part when it has separators.
fn first_likely_name(text: &str, email: &str) -> Option<String> {
    for raw_line in split_lines(text).into_iter().take(12) {
        let line = strip_text(raw_line);
        if line.is_empty() || line.contains('@') || lower_text(line).contains("http") {
            continue;
        }
        if utf16_length(line) <= NAME_LIMIT
            && line.chars().count() >= 2
            && line.chars().next().is_some_and(is_text_alpha)
            && line
                .chars()
                .all(|character| is_text_alpha(character) || " .'-".contains(character))
        {
            return Some(line.to_owned());
        }
    }
    let local = email.split_once('@').map_or(email, |(local, _)| local);
    let words = local_part_words(local);
    if words.contains(' ') {
        Some(title_case(&words))
    } else {
        None
    }
}

/// The validation error of an extraction field.
fn invalid(field: &str, reason: &str) -> AppError {
    AppError::from(validation_error(
        EXTRACTION_INPUT_MESSAGE,
        Some(field),
        Some(reason),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_resume_gives_a_name_and_email() -> Result<(), AppError> {
        let value =
            DeterministicProfileTextExtractor.extract_profile("Ada Example\nada@example.com")?;
        assert_eq!(value["fullName"], "Ada Example");
        assert_eq!(value["email"], "ada@example.com");
        assert_eq!(value["contacts"].as_array().map(Vec::len), Some(1));
        Ok(())
    }

    #[test]
    fn blank_text_is_rejected_first() {
        let error = DeterministicProfileTextExtractor.extract_profile("  \n ");
        assert!(error.is_err());
    }

    #[test]
    fn name_falls_back_to_the_email_local_part() -> Result<(), AppError> {
        let value = DeterministicProfileTextExtractor.extract_profile("ada.example@example.com")?;
        assert_eq!(value["fullName"], "Ada Example");
        Ok(())
    }
}
