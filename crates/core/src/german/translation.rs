//! Offline English-to-German translation of resume text.
//!
//! The translator applies, in order: whole-phrase lookup of the stripped text (first match in
//! table order, Java-style equals-ignore-case), then every phrase as an ordered, sequential,
//! ASCII case-insensitive replace-all, then the ASCII token glossary, then a collapse of runs of
//! two or more ASCII whitespace characters to one space and a final strip. There is no
//! protected-term, URL or e-mail rule (unlike the French translator).
//!
//! The tables come from a [`Glossary`]. The built-in German tables
//! below are generic: section and contact labels, language levels, a few neutral phrases and
//! neutral words. A user glossary file ([`UserGlossary`](crate::generation::translation::UserGlossary),
//! format described on
//! [`UserGlossary::parse`](crate::generation::translation::UserGlossary::parse)) extends them; its targets are German here.
//!
//! Rust strings cannot hold lone surrogates, so no surrogate handling is needed.

use std::collections::BTreeMap;

use jobniac_text::java::{
    BudgetField, PreparationBudget, PreparationLimitExceeded, java_is_blank, java_strip,
    utf16_length,
};

use super::content::{
    AdditionalEntry, EducationEntry, ExperienceEntry, LanguageEntry, PersonalField, ResumeInput,
    SkillGroup, StructuredResumeContent, raw_strings,
};
use crate::errors::{ApplicationError, validation_error};
use crate::generation::translation::Glossary;

/// Message of the refusal raised for an uppercase original with an empty replacement.
const INVALID_INPUT: &str = "Invalid German translation input";

/// Generic built-in label set: the lower-case source label and its German form.
///
/// Labels are structural resume headings and contact fields, not personal content.
pub const BUILTIN_LABELS: &[(&str, &str)] = &[
    ("contact", "Kontakt"),
    ("links", "Links"),
    ("email", "E-Mail"),
    ("address", "Adresse"),
    ("phone", "Telefon"),
    ("location", "Wohnort"),
    ("website", "Website"),
    ("github", "GitHub"),
    ("linkedin", "LinkedIn"),
    ("portfolio", "Portfolio"),
    ("date of birth", "Geburtsdatum"),
    ("nationality", "Staatsangehörigkeit"),
    ("field of study", "Studienfach"),
    ("technologies", "Technologien"),
    ("backend", "Backend"),
    ("frontend / apps", "Frontend / Apps"),
    ("cloud / ops", "Cloud / Betrieb"),
    ("security", "Sicherheit"),
    ("systems / tools", "Systeme / Tools"),
    ("technical", "Technisch"),
    ("general", "Allgemein"),
    ("fluent", "fließend"),
    ("beginner", "Anfänger"),
    ("intermediate", "Fortgeschritten"),
    ("native", "Muttersprache"),
    ("professional", "verhandlungssicher"),
];

/// Generic built-in phrases, applied in this order (a later phrase sees the output of an earlier
/// one). Longer phrases come before the shorter phrases they contain.
pub const BUILTIN_PHRASES: &[(&str, &str)] = &[
    ("field of study:", "Studienfach:"),
    ("technologies:", "Technologien:"),
];

/// Generic built-in word glossary: the lower-case ASCII word and its German form. An empty German
/// form drops the word (articles).
pub const BUILTIN_WORDS: &[(&str, &str)] = &[
    ("built", "erstellte"),
    ("build", "erstellen"),
    ("developed", "entwickelte"),
    ("develop", "entwickeln"),
    ("implemented", "implementierte"),
    ("implement", "implementieren"),
    ("integrated", "integrierte"),
    ("managed", "verwaltete"),
    ("supported", "unterstützte"),
    ("automated", "automatisierte"),
    ("migrated", "migrierte"),
    ("created", "erstellte"),
    ("delivered", "lieferte"),
    ("worked", "arbeitete"),
    ("using", "mit"),
    ("with", "mit"),
    ("for", "für"),
    ("and", "und"),
    ("or", "oder"),
    ("in", "in"),
    ("of", "von"),
    ("the", ""),
    ("a", ""),
    ("an", ""),
    ("to", "zu"),
    ("from", "von"),
    ("across", "über"),
    ("through", "durch"),
    ("systems", "Systeme"),
    ("system", "System"),
    ("application", "Anwendung"),
    ("applications", "Anwendungen"),
    ("workflow", "Workflow"),
    ("workflows", "Workflows"),
    ("data", "Daten"),
    ("production", "Produktion"),
    ("customer", "Kunde"),
    ("customers", "Kunden"),
    ("authentication", "Authentifizierung"),
    ("deployment", "Deployment"),
    ("configuration", "Konfiguration"),
    ("issue", "Problem"),
    ("issues", "Probleme"),
    ("diagnosis", "Diagnose"),
    ("support", "Support"),
    ("software", "Software"),
    ("hardware", "Hardware"),
    ("users", "Benutzer"),
    ("user", "Benutzer"),
    ("website", "Website"),
    ("company", "Unternehmen"),
    ("business", "Geschäfts"),
    ("document", "Dokument"),
    ("generation", "Erzeugung"),
    ("process", "Prozess"),
    ("processes", "Prozesse"),
    ("present", "heute"),
    ("unknown", "unbekannt"),
    ("role", "Rolle"),
    ("project", "Projekt"),
    ("degree", "Abschluss"),
    ("institution", "Institution"),
];

/// The generic built-in German glossary.
///
/// Extend it with [`Glossary::merged_with`] and a parsed [`UserGlossary`]
/// (`crate::generation::translation::UserGlossary`).
#[must_use]
pub fn builtin_glossary() -> Glossary {
    Glossary {
        labels: pairs_to_map(BUILTIN_LABELS),
        phrases: BUILTIN_PHRASES
            .iter()
            .map(|(source, target)| ((*source).to_owned(), (*target).to_owned()))
            .collect(),
        words: pairs_to_map(BUILTIN_WORDS),
    }
}

fn pairs_to_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

/// The validation error of an exceeded preparation budget.
fn budget_error(error: PreparationLimitExceeded) -> ApplicationError {
    validation_error(
        error.message(),
        Some(error.field().as_str()),
        Some(error.reason()),
    )
}

/// Counts every text against a fresh `text` budget.
fn check_budget(texts: &[Option<&str>]) -> Result<(), ApplicationError> {
    let mut budget = PreparationBudget::new(BudgetField::Text);
    for text in texts {
        budget.add_text(*text).map_err(budget_error)?;
    }
    Ok(())
}

/// The one-character title-case form of a character whose full uppercase form has several
/// characters. Only the Greek letters with an iota
/// subscript have a one-character title form among those, so the table lists exactly them.
fn single_title(character: char) -> Option<char> {
    let code = u32::from(character);
    let title = match code {
        0x1F80..=0x1F87 | 0x1F90..=0x1F97 | 0x1FA0..=0x1FA7 => code + 8,
        0x1F88..=0x1F8F | 0x1F98..=0x1F9F | 0x1FA8..=0x1FAF | 0x1FBC | 0x1FCC | 0x1FFC => code,
        0x1FB3 => 0x1FBC,
        0x1FC3 => 0x1FCC,
        0x1FF3 => 0x1FFC,
        _ => return None,
    };
    char::from_u32(title)
}

/// The single character a string maps to, or `None` when it has no or several characters.
fn only_char(mut characters: impl Iterator<Item = char>) -> Option<char> {
    let first = characters.next()?;
    characters.next().is_none().then_some(first)
}

/// Java-style character equality for the finite phrase domain: equal, dotted or dotless `i` against
/// ASCII `i`, or equal single-character upper or
/// lower case forms.
fn chars_equal_ignore_case(character: char, expected: char) -> bool {
    if character == expected {
        return true;
    }
    if matches!(character, 'i' | 'I' | '\u{130}' | '\u{131}') && matches!(expected, 'i' | 'I') {
        return true;
    }
    if let Some(upper) = only_char(character.to_uppercase())
        && only_char(expected.to_uppercase()) == Some(upper)
    {
        return true;
    }
    if let Some(lower) = only_char(character.to_lowercase())
        && only_char(expected.to_lowercase()) == Some(lower)
    {
        return true;
    }
    false
}

/// Case-preserving replacement of a matched token.
///
/// An all-caps original longer than one UTF-16 unit gives the uppercase replacement. Otherwise,
/// when the original's first code point is at most U+FFFF and uppercase, the replacement's first
/// character is uppercased as a character mapping (never an expansion; a first character that
/// would expand uses its one-character title form, else stays) and the rest is kept. A
/// replacement whose first code point is above U+FFFF is kept unchanged. Anything else keeps the
/// replacement unchanged.
fn preserve_case_unchecked(original: &str, replacement: &str) -> Result<String, ApplicationError> {
    if utf16_length(original) > 1 && original == original.to_uppercase() {
        return Ok(replacement.to_uppercase());
    }
    if let Some(first) = original.chars().next()
        && u32::from(first) <= 0xFFFF
        && first.is_uppercase()
    {
        let mut characters = replacement.chars();
        let Some(head) = characters.next() else {
            return Err(validation_error(INVALID_INPUT, None, None));
        };
        if u32::from(head) > 0xFFFF {
            return Ok(replacement.to_owned());
        }
        let upper =
            only_char(head.to_uppercase()).unwrap_or_else(|| single_title(head).unwrap_or(head));
        let mut result = String::with_capacity(replacement.len());
        result.push(upper);
        result.push_str(characters.as_str());
        return Ok(result);
    }
    Ok(replacement.to_owned())
}

/// Case-preserving replacement.
///
/// # Errors
///
/// Returns the preparation-budget validation error when either text exceeds the budget, and
/// `Invalid German translation input` when the original starts with an uppercase letter and the
/// replacement is empty.
pub fn preserve_case(original: &str, replacement: &str) -> Result<String, ApplicationError> {
    check_budget(&[Some(original), Some(replacement)])?;
    preserve_case_unchecked(original, replacement)
}

/// Replaces every non-overlapping ASCII case-insensitive match of `pattern`, left to right.
fn replace_ascii_insensitive(text: &[char], pattern: &[char], replacement: &str) -> Vec<char> {
    let length = pattern.len();
    if length == 0 {
        return text.to_vec();
    }
    let mut output = Vec::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let matches = index + length <= text.len()
            && pattern
                .iter()
                .enumerate()
                .all(|(offset, expected)| eq_ascii_ci(text[index + offset], *expected));
        if matches {
            output.extend(replacement.chars());
            index += length;
        } else {
            output.push(text[index]);
            index += 1;
        }
    }
    output
}

/// ASCII-only case-insensitive character equality.
fn eq_ascii_ci(left: char, right: char) -> bool {
    left == right
        || (left.is_ascii_alphabetic()
            && right.is_ascii_alphabetic()
            && left.eq_ignore_ascii_case(&right))
}

/// True for ASCII whitespace: space, tab, line feed, vertical tab, form
/// feed and carriage return.
fn is_ascii_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r')
}

/// Replaces each run of two or more ASCII whitespace characters with one space; a single
/// whitespace character is kept as is.
fn collapse_space_runs(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < characters.len() {
        if is_ascii_space(characters[index]) {
            let mut end = index + 1;
            while end < characters.len() && is_ascii_space(characters[end]) {
                end += 1;
            }
            if end - index >= 2 {
                output.push(' ');
            } else {
                output.push(characters[index]);
            }
            index = end;
        } else {
            output.push(characters[index]);
            index += 1;
        }
    }
    output
}

/// The German translator built from one glossary.
///
/// Build it once per glossary and reuse it; it holds no mutable state.
#[derive(Debug, Clone)]
pub struct GermanResumeTranslator {
    labels: BTreeMap<String, String>,
    phrases: Vec<(Vec<char>, String)>,
    words: BTreeMap<String, String>,
    max_phrase_length: usize,
}

impl GermanResumeTranslator {
    /// A translator for `glossary`.
    #[must_use]
    pub fn new(glossary: &Glossary) -> Self {
        let phrases: Vec<(Vec<char>, String)> = glossary
            .phrases
            .iter()
            .map(|(source, target)| (source.chars().collect(), target.clone()))
            .collect();
        let max_phrase_length = phrases
            .iter()
            .map(|(source, _)| source.len())
            .max()
            .unwrap_or(0);
        Self {
            labels: glossary.labels.clone(),
            phrases,
            words: glossary.words.clone(),
            max_phrase_length,
        }
    }

    /// A translator for the generic built-in German glossary ([`builtin_glossary`]).
    #[must_use]
    pub fn builtin() -> Self {
        Self::new(&builtin_glossary())
    }

    /// Translates free text. `None` and blank text are returned
    /// unchanged.
    ///
    /// # Errors
    ///
    /// Returns the preparation-budget validation error when the text exceeds the budget.
    pub fn translate_text(&self, text: Option<&str>) -> Result<Option<String>, ApplicationError> {
        check_budget(&[text])?;
        Ok(self.text_value(text))
    }

    /// Translates a label: the stripped, lower-cased label is looked up
    /// in the label map first (an empty map value falls through), otherwise the label is
    /// translated as text. `None` and blank labels are returned unchanged.
    ///
    /// # Errors
    ///
    /// Returns the preparation-budget validation error when the label exceeds the budget.
    pub fn translate_label(&self, label: Option<&str>) -> Result<Option<String>, ApplicationError> {
        check_budget(&[label])?;
        Ok(self.label_value(label))
    }

    /// Translates a personal-field value by its original label: only the labels `address`,
    /// `location` and `nationality`
    /// (stripped, case-insensitive) translate the value; any other, `None` or blank label returns
    /// the value unchanged.
    ///
    /// # Errors
    ///
    /// Returns the preparation-budget validation error when the label and value together exceed
    /// the budget.
    pub fn translate_personal_field_value(
        &self,
        label: Option<&str>,
        value: Option<&str>,
    ) -> Result<Option<String>, ApplicationError> {
        check_budget(&[label, value])?;
        Ok(self.personal_value(label, value))
    }

    /// Translates admitted English resume content to German: the content is
    /// budgeted first, then the summary, titles, locations, bullets, labels, personal-field values,
    /// category names, languages and levels are translated; names, companies, institutions, dates,
    /// skills and organizations are copied. The result is admitted again by the content models.
    ///
    /// # Errors
    ///
    /// The preparation-budget error, or the content-model refusal (for example a translated label
    /// that becomes blank).
    pub fn to_german(
        &self,
        english: &StructuredResumeContent,
    ) -> Result<StructuredResumeContent, ApplicationError> {
        english.budget()?;
        let text = |value: Option<&str>| self.text_value(value);
        let bullets = |items: &[String]| {
            raw_strings(
                items
                    .iter()
                    .map(|bullet| self.text_value(Some(bullet)).unwrap_or_default()),
            )
        };
        let mut personal_fields = Vec::new();
        for field in english.personal_fields() {
            personal_fields.push(PersonalField::new(
                self.label_value(Some(field.label())).as_deref(),
                self.personal_value(Some(field.label()), Some(field.value()))
                    .as_deref(),
            )?);
        }
        let mut experiences = Vec::new();
        for row in english.experiences() {
            experiences.push(ExperienceEntry::new(
                text(Some(row.title())).as_deref(),
                Some(row.company()),
                text(row.location()).as_deref(),
                row.start_date(),
                row.end_date(),
                bullets(row.bullets()),
            )?);
        }
        let mut education = Vec::new();
        for row in english.education() {
            education.push(EducationEntry::new(
                text(Some(row.degree())).as_deref(),
                Some(row.institution()),
                text(row.location()).as_deref(),
                row.start_date(),
                row.end_date(),
                bullets(row.bullets()),
            )?);
        }
        let mut skill_groups = Vec::new();
        for group in english.skill_groups() {
            skill_groups.push(SkillGroup::new(
                self.label_value(Some(group.category())).as_deref(),
                raw_strings(group.skills().iter().cloned()),
            )?);
        }
        let mut languages = Vec::new();
        for row in english.languages() {
            languages.push(LanguageEntry::new(
                text(Some(row.language())).as_deref(),
                self.label_value(row.proficiency()).as_deref(),
            )?);
        }
        let mut additional = Vec::new();
        for row in english.additional() {
            additional.push(AdditionalEntry::new(
                text(Some(row.title())).as_deref(),
                row.organization(),
                bullets(row.bullets()),
            )?);
        }
        StructuredResumeContent::new(ResumeInput {
            full_name: Some(english.full_name().to_owned()),
            language: Some("de".to_owned()),
            summary: text(english.summary()),
            personal_fields,
            experiences,
            education,
            skill_groups,
            languages,
            additional,
        })
    }

    /// `_translate_text` without budget accounting.
    fn text_value(&self, text: Option<&str>) -> Option<String> {
        let text = text?;
        if java_is_blank(text) {
            return Some(text.to_owned());
        }
        let stripped = java_strip(text);
        let characters: Vec<char> = stripped.chars().collect();
        if characters.len() <= self.max_phrase_length {
            for (source, replacement) in &self.phrases {
                if source.len() == characters.len()
                    && characters
                        .iter()
                        .zip(source)
                        .all(|(character, expected)| chars_equal_ignore_case(*character, *expected))
                {
                    return Some(replacement.clone());
                }
            }
        }
        let mut current = characters;
        for (pattern, replacement) in &self.phrases {
            current = replace_ascii_insensitive(&current, pattern, replacement);
        }
        let replaced = self.replace_words(&current);
        Some(java_strip(&collapse_space_runs(&replaced)).to_owned())
    }

    /// The word glossary over `[A-Za-z][A-Za-z+/.-]*` tokens.
    fn replace_words(&self, current: &[char]) -> String {
        let mut output = String::with_capacity(current.len());
        let mut index = 0;
        while index < current.len() {
            if current[index].is_ascii_alphabetic() {
                let mut end = index + 1;
                while end < current.len()
                    && (current[end].is_ascii_alphabetic()
                        || matches!(current[end], '+' | '.' | '/' | '-'))
                {
                    end += 1;
                }
                let token: String = current[index..end].iter().collect();
                match self.words.get(&token.to_ascii_lowercase()) {
                    // Empty glossary values drop the token; others carry the token's case. The
                    // empty-replacement refusal cannot occur because the value is non-empty.
                    Some(word) if !word.is_empty() => match preserve_case_unchecked(&token, word) {
                        Ok(cased) => output.push_str(&cased),
                        Err(_) => output.push_str(word),
                    },
                    Some(_) => {}
                    None => output.push_str(&token),
                }
                index = end;
            } else {
                output.push(current[index]);
                index += 1;
            }
        }
        output
    }

    /// `_translate_label` without budget accounting.
    fn label_value(&self, label: Option<&str>) -> Option<String> {
        let label = label?;
        if java_is_blank(label) {
            return Some(label.to_owned());
        }
        let key = java_strip(label).to_lowercase();
        match self.labels.get(&key) {
            Some(translated) if !translated.is_empty() => Some(translated.clone()),
            _ => self.text_value(Some(label)),
        }
    }

    /// `_translate_personal` without budget accounting.
    fn personal_value(&self, label: Option<&str>, value: Option<&str>) -> Option<String> {
        let unchanged = || value.map(str::to_owned);
        let Some(label) = label else {
            return unchanged();
        };
        if java_is_blank(label) {
            return unchanged();
        }
        let key = java_strip(label).to_lowercase();
        if matches!(key.as_str(), "address" | "location" | "nationality") {
            self.text_value(value)
        } else {
            unchanged()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_translates_a_generic_sentence() {
        let translator = GermanResumeTranslator::builtin();
        assert_eq!(
            translator
                .translate_text(Some("Built the data systems"))
                .expect("translate"),
            Some("Erstellte Daten Systeme".to_owned())
        );
    }

    #[test]
    fn builtin_label_map_wins() {
        let translator = GermanResumeTranslator::builtin();
        assert_eq!(
            translator.translate_label(Some(" Email ")).expect("label"),
            Some("E-Mail".to_owned())
        );
    }
}
