//! Immutable source-normalized German content, bounded before copying or scanning.
//!
//! Every record has private fields and a fallible constructor that first admits the raw input
//! against the preparation budget (UTF-16 units and entry counts), then normalizes it. The raw
//! totals are retained, so nested records keep counting their pre-normalization size wherever they
//! are repeated. A refusal is always the fixed safe `Invalid German content input` validation
//! error, or the preparation budget error.

use jobniac_pdf::layout::german as layout;
use jobniac_text::java::{BudgetField, PreparationBudget, java_strip, utf16_length};
use time::{Date, Month};

use crate::errors::{ApplicationError, validation_error};

/// The fixed message of every refused German content input.
const INVALID_MESSAGE: &str = "Invalid German content input";

fn invalid() -> ApplicationError {
    validation_error(INVALID_MESSAGE, None, None)
}

/// A raw, possibly null list of possibly null strings.
pub type RawStrings = Option<Vec<Option<String>>>;

/// Wraps plain strings as a present [`RawStrings`] list without null members.
pub fn raw_strings<I, S>(items: I) -> RawStrings
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    Some(items.into_iter().map(|item| Some(item.into())).collect())
}

/// The retained raw admission totals of a record: never part of equality.
#[derive(Debug, Clone, Copy, Default)]
struct RawTotals {
    units: usize,
    entries: usize,
}

impl PartialEq for RawTotals {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for RawTotals {}

/// The admission totals of one nested record occurrence.
#[derive(Debug, Clone, Copy)]
struct Totals {
    units: usize,
    entries: usize,
}

/// Raw view of an optional string list for admission and normalization.
fn view(items: &RawStrings) -> Option<Vec<Option<&str>>> {
    items
        .as_ref()
        .map(|list| list.iter().map(Option::as_deref).collect())
}

/// View of stored (already normalized) strings.
fn stored(items: &[String]) -> Option<Vec<Option<&str>>> {
    Some(items.iter().map(|item| Some(item.as_str())).collect())
}

/// Admission accumulator.
struct Admission {
    budget: PreparationBudget,
    units: usize,
    entries: usize,
}

impl Admission {
    fn new(row: bool) -> Result<Self, ApplicationError> {
        let entries = usize::from(row);
        let mut budget = PreparationBudget::new(BudgetField::Text);
        budget.add_entries(entries)?;
        Ok(Self {
            budget,
            units: 0,
            entries,
        })
    }

    fn scalar(&mut self, value: Option<&str>, nullable: bool) -> Result<(), ApplicationError> {
        if value.is_none() && !nullable {
            return Err(invalid());
        }
        self.budget.add_text(value)?;
        if let Some(text) = value {
            self.units += utf16_length(text);
        }
        Ok(())
    }

    fn strings(
        &mut self,
        items: Option<&[Option<&str>]>,
        nullable_collection: bool,
        nullable_member: bool,
    ) -> Result<(), ApplicationError> {
        let items = match items {
            Some(items) => items,
            None if nullable_collection => &[],
            None => return Err(invalid()),
        };
        self.budget.add_entries(items.len())?;
        self.entries += items.len();
        for member in items {
            match member {
                None if nullable_member => {}
                None => return Err(invalid()),
                Some(text) => {
                    self.budget.add_text(Some(text))?;
                    self.units += utf16_length(text);
                }
            }
        }
        Ok(())
    }

    fn records(&mut self, members: &[Totals]) -> Result<(), ApplicationError> {
        self.budget.add_entries(members.len())?;
        self.entries += members.len();
        for member in members {
            self.budget.add_units(member.units)?;
            let extra = member.entries.saturating_sub(1);
            self.budget.add_entries(extra)?;
            self.units += member.units;
            self.entries += extra;
        }
        Ok(())
    }

    fn finish(self) -> (usize, usize) {
        (self.units, self.entries)
    }
}

/// Totals of a record collection for the parent admission.
trait Member {
    fn totals(&self) -> Result<Totals, ApplicationError>;
}

fn totals_of<T: Member>(members: &[T]) -> Result<Vec<Totals>, ApplicationError> {
    members.iter().map(Member::totals).collect()
}

fn combine(admitted: (usize, usize), raw: RawTotals) -> Totals {
    Totals {
        units: admitted.0.max(raw.units),
        entries: admitted.1.max(raw.entries),
    }
}

fn normalize_required(value: Option<&str>) -> Result<String, ApplicationError> {
    let stripped = java_strip(value.ok_or_else(invalid)?);
    if stripped.is_empty() {
        return Err(invalid());
    }
    Ok(stripped.to_owned())
}

fn normalize_text(value: Option<&str>) -> Result<String, ApplicationError> {
    Ok(java_strip(value.ok_or_else(invalid)?).to_owned())
}

fn normalize_optional(value: Option<&str>) -> Option<String> {
    value
        .map(java_strip)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// A normalized positive-year ISO `YYYY-MM-DD` date, kept literally; anything else is refused.
fn normalize_date(value: Option<&str>) -> Result<Option<String>, ApplicationError> {
    let Some(text) = value else {
        return Ok(None);
    };
    let bytes = text.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit());
    if !shaped {
        return Err(invalid());
    }
    let number = |range: std::ops::Range<usize>| text[range].parse::<u32>().map_err(|_| invalid());
    let year = i32::try_from(number(0..4)?).map_err(|_| invalid())?;
    let month = u8::try_from(number(5..7)?).map_err(|_| invalid())?;
    let day = u8::try_from(number(8..10)?).map_err(|_| invalid())?;
    if year < 1 {
        return Err(invalid());
    }
    let month = Month::try_from(month).map_err(|_| invalid())?;
    Date::from_calendar_date(year, month, day).map_err(|_| invalid())?;
    Ok(Some(text.to_owned()))
}

/// Strips each non-null member and drops the blank ones (`bullets`, `skills` and
/// `paragraphs` kinds).
fn normalize_strings(
    items: Option<&[Option<&str>]>,
    paragraphs: bool,
    skills: bool,
) -> Result<Vec<String>, ApplicationError> {
    let items = match items {
        Some(items) => items,
        None if !paragraphs => &[],
        None => return Err(invalid()),
    };
    let stripped: Vec<&str> = items
        .iter()
        .flatten()
        .map(|text| java_strip(text))
        .collect();
    if paragraphs && stripped.iter().any(|text| text.is_empty()) {
        return Err(invalid());
    }
    let result: Vec<String> = stripped
        .into_iter()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .collect();
    if skills && result.is_empty() {
        return Err(invalid());
    }
    Ok(result)
}

/// A personal contact field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonalField {
    label: String,
    value: String,
    raw: RawTotals,
}

fn admit_personal(
    label: Option<&str>,
    value: Option<&str>,
) -> Result<(usize, usize), ApplicationError> {
    let mut admission = Admission::new(true)?;
    admission.scalar(label, false)?;
    admission.scalar(value, false)?;
    Ok(admission.finish())
}

impl PersonalField {
    /// Admits and normalizes a field; a missing or blank label or value is refused.
    ///
    /// # Errors
    ///
    /// The fixed validation error, or the preparation budget error.
    pub fn new(label: Option<&str>, value: Option<&str>) -> Result<Self, ApplicationError> {
        let (units, entries) = admit_personal(label, value)?;
        Ok(Self {
            label: normalize_required(label)?,
            value: normalize_required(value)?,
            raw: RawTotals { units, entries },
        })
    }

    /// The stripped label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The stripped value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The retained raw UTF-16 unit total of the admitted input, before normalization.
    #[must_use]
    pub const fn raw_units(&self) -> usize {
        self.raw.units
    }

    /// The retained raw entry count of the admitted input (one for a field).
    #[must_use]
    pub const fn raw_entries(&self) -> usize {
        self.raw.entries
    }

    /// The render input of this field.
    #[must_use]
    pub fn to_layout(&self) -> layout::PersonalField {
        layout::PersonalField {
            label: self.label.clone(),
            value: self.value.clone(),
        }
    }
}

impl Member for PersonalField {
    fn totals(&self) -> Result<Totals, ApplicationError> {
        Ok(combine(
            admit_personal(Some(&self.label), Some(&self.value))?,
            self.raw,
        ))
    }
}

/// The admission shared by the experience and education schemas (text, text, optional, date,
/// date, bullets).
fn admit_dated(
    first: Option<&str>,
    second: Option<&str>,
    location: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
    bullets: Option<&[Option<&str>]>,
) -> Result<(usize, usize), ApplicationError> {
    let mut admission = Admission::new(true)?;
    admission.scalar(first, false)?;
    admission.scalar(second, false)?;
    admission.scalar(location, true)?;
    admission.scalar(start, true)?;
    admission.scalar(end, true)?;
    admission.strings(bullets, true, true)?;
    Ok(admission.finish())
}

/// A dated experience.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperienceEntry {
    title: String,
    company: String,
    location: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
    bullets: Vec<String>,
    raw: RawTotals,
}

impl ExperienceEntry {
    /// Admits and normalizes an experience; dates must be normalized positive-year ISO dates.
    ///
    /// # Errors
    ///
    /// The fixed validation error, or the preparation budget error.
    pub fn new(
        title: Option<&str>,
        company: Option<&str>,
        location: Option<&str>,
        start_date: Option<&str>,
        end_date: Option<&str>,
        bullets: RawStrings,
    ) -> Result<Self, ApplicationError> {
        let bullets = view(&bullets);
        let (units, entries) = admit_dated(
            title,
            company,
            location,
            start_date,
            end_date,
            bullets.as_deref(),
        )?;
        Ok(Self {
            title: normalize_text(title)?,
            company: normalize_text(company)?,
            location: normalize_optional(location),
            start_date: normalize_date(start_date)?,
            end_date: normalize_date(end_date)?,
            bullets: normalize_strings(bullets.as_deref(), false, false)?,
            raw: RawTotals { units, entries },
        })
    }

    /// The stripped title (may be empty).
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The stripped company (may be empty).
    #[must_use]
    pub fn company(&self) -> &str {
        &self.company
    }

    /// The stripped location, when not blank.
    #[must_use]
    pub fn location(&self) -> Option<&str> {
        self.location.as_deref()
    }

    /// The ISO start date, when given.
    #[must_use]
    pub fn start_date(&self) -> Option<&str> {
        self.start_date.as_deref()
    }

    /// The ISO end date, when given.
    #[must_use]
    pub fn end_date(&self) -> Option<&str> {
        self.end_date.as_deref()
    }

    /// The stripped, non-blank bullets.
    #[must_use]
    pub fn bullets(&self) -> &[String] {
        &self.bullets
    }

    /// The render input of this entry.
    #[must_use]
    pub fn to_layout(&self) -> layout::ExperienceEntry {
        layout::ExperienceEntry {
            title: self.title.clone(),
            company: self.company.clone(),
            location: self.location.clone(),
            start_date: self.start_date.clone(),
            end_date: self.end_date.clone(),
            bullets: self.bullets.clone(),
        }
    }
}

impl Member for ExperienceEntry {
    fn totals(&self) -> Result<Totals, ApplicationError> {
        let bullets = stored(&self.bullets);
        Ok(combine(
            admit_dated(
                Some(&self.title),
                Some(&self.company),
                self.location.as_deref(),
                self.start_date.as_deref(),
                self.end_date.as_deref(),
                bullets.as_deref(),
            )?,
            self.raw,
        ))
    }
}

/// A dated education entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EducationEntry {
    degree: String,
    institution: String,
    location: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
    bullets: Vec<String>,
    raw: RawTotals,
}

impl EducationEntry {
    /// Admits and normalizes an education entry; dates must be normalized positive-year ISO dates.
    ///
    /// # Errors
    ///
    /// The fixed validation error, or the preparation budget error.
    pub fn new(
        degree: Option<&str>,
        institution: Option<&str>,
        location: Option<&str>,
        start_date: Option<&str>,
        end_date: Option<&str>,
        bullets: RawStrings,
    ) -> Result<Self, ApplicationError> {
        let bullets = view(&bullets);
        let (units, entries) = admit_dated(
            degree,
            institution,
            location,
            start_date,
            end_date,
            bullets.as_deref(),
        )?;
        Ok(Self {
            degree: normalize_text(degree)?,
            institution: normalize_text(institution)?,
            location: normalize_optional(location),
            start_date: normalize_date(start_date)?,
            end_date: normalize_date(end_date)?,
            bullets: normalize_strings(bullets.as_deref(), false, false)?,
            raw: RawTotals { units, entries },
        })
    }

    /// The stripped degree (may be empty).
    #[must_use]
    pub fn degree(&self) -> &str {
        &self.degree
    }

    /// The stripped institution (may be empty).
    #[must_use]
    pub fn institution(&self) -> &str {
        &self.institution
    }

    /// The stripped location, when not blank.
    #[must_use]
    pub fn location(&self) -> Option<&str> {
        self.location.as_deref()
    }

    /// The ISO start date, when given.
    #[must_use]
    pub fn start_date(&self) -> Option<&str> {
        self.start_date.as_deref()
    }

    /// The ISO end date, when given.
    #[must_use]
    pub fn end_date(&self) -> Option<&str> {
        self.end_date.as_deref()
    }

    /// The stripped, non-blank bullets.
    #[must_use]
    pub fn bullets(&self) -> &[String] {
        &self.bullets
    }

    /// The render input of this entry.
    #[must_use]
    pub fn to_layout(&self) -> layout::EducationEntry {
        layout::EducationEntry {
            degree: self.degree.clone(),
            institution: self.institution.clone(),
            location: self.location.clone(),
            start_date: self.start_date.clone(),
            end_date: self.end_date.clone(),
            bullets: self.bullets.clone(),
        }
    }
}

impl Member for EducationEntry {
    fn totals(&self) -> Result<Totals, ApplicationError> {
        let bullets = stored(&self.bullets);
        Ok(combine(
            admit_dated(
                Some(&self.degree),
                Some(&self.institution),
                self.location.as_deref(),
                self.start_date.as_deref(),
                self.end_date.as_deref(),
                bullets.as_deref(),
            )?,
            self.raw,
        ))
    }
}

/// A group of skills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillGroup {
    category: String,
    skills: Vec<String>,
    raw: RawTotals,
}

fn admit_skill_group(
    category: Option<&str>,
    skills: Option<&[Option<&str>]>,
) -> Result<(usize, usize), ApplicationError> {
    let mut admission = Admission::new(true)?;
    admission.scalar(category, false)?;
    admission.strings(skills, true, true)?;
    Ok(admission.finish())
}

impl SkillGroup {
    /// Admits and normalizes a group; the category must not be blank and at least one non-blank
    /// skill must remain.
    ///
    /// # Errors
    ///
    /// The fixed validation error, or the preparation budget error.
    pub fn new(category: Option<&str>, skills: RawStrings) -> Result<Self, ApplicationError> {
        let skills = view(&skills);
        let (units, entries) = admit_skill_group(category, skills.as_deref())?;
        Ok(Self {
            category: normalize_required(category)?,
            skills: normalize_strings(skills.as_deref(), false, true)?,
            raw: RawTotals { units, entries },
        })
    }

    /// The stripped category.
    #[must_use]
    pub fn category(&self) -> &str {
        &self.category
    }

    /// The stripped, non-blank skills (never empty).
    #[must_use]
    pub fn skills(&self) -> &[String] {
        &self.skills
    }

    /// The render input of this group.
    #[must_use]
    pub fn to_layout(&self) -> layout::SkillGroup {
        layout::SkillGroup {
            category: self.category.clone(),
            skills: self.skills.clone(),
        }
    }
}

impl Member for SkillGroup {
    fn totals(&self) -> Result<Totals, ApplicationError> {
        let skills = stored(&self.skills);
        Ok(combine(
            admit_skill_group(Some(&self.category), skills.as_deref())?,
            self.raw,
        ))
    }
}

/// A spoken language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageEntry {
    language: String,
    proficiency: Option<String>,
    raw: RawTotals,
}

fn admit_language(
    language: Option<&str>,
    proficiency: Option<&str>,
) -> Result<(usize, usize), ApplicationError> {
    let mut admission = Admission::new(true)?;
    admission.scalar(language, false)?;
    admission.scalar(proficiency, true)?;
    Ok(admission.finish())
}

impl LanguageEntry {
    /// Admits and normalizes a language entry; a missing language is refused, a blank one is
    /// kept empty.
    ///
    /// # Errors
    ///
    /// The fixed validation error, or the preparation budget error.
    pub fn new(
        language: Option<&str>,
        proficiency: Option<&str>,
    ) -> Result<Self, ApplicationError> {
        let (units, entries) = admit_language(language, proficiency)?;
        Ok(Self {
            language: normalize_text(language)?,
            proficiency: normalize_optional(proficiency),
            raw: RawTotals { units, entries },
        })
    }

    /// The stripped language (may be empty).
    #[must_use]
    pub fn language(&self) -> &str {
        &self.language
    }

    /// The stripped proficiency, when not blank.
    #[must_use]
    pub fn proficiency(&self) -> Option<&str> {
        self.proficiency.as_deref()
    }

    /// The render input of this entry.
    #[must_use]
    pub fn to_layout(&self) -> layout::LanguageEntry {
        layout::LanguageEntry {
            language: self.language.clone(),
            proficiency: self.proficiency.clone(),
        }
    }
}

impl Member for LanguageEntry {
    fn totals(&self) -> Result<Totals, ApplicationError> {
        Ok(combine(
            admit_language(Some(&self.language), self.proficiency.as_deref())?,
            self.raw,
        ))
    }
}

/// A project or other entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditionalEntry {
    title: String,
    organization: Option<String>,
    bullets: Vec<String>,
    raw: RawTotals,
}

fn admit_additional(
    title: Option<&str>,
    organization: Option<&str>,
    bullets: Option<&[Option<&str>]>,
) -> Result<(usize, usize), ApplicationError> {
    let mut admission = Admission::new(true)?;
    admission.scalar(title, false)?;
    admission.scalar(organization, true)?;
    admission.strings(bullets, true, true)?;
    Ok(admission.finish())
}

impl AdditionalEntry {
    /// Admits and normalizes an additional entry.
    ///
    /// # Errors
    ///
    /// The fixed validation error, or the preparation budget error.
    pub fn new(
        title: Option<&str>,
        organization: Option<&str>,
        bullets: RawStrings,
    ) -> Result<Self, ApplicationError> {
        let bullets = view(&bullets);
        let (units, entries) = admit_additional(title, organization, bullets.as_deref())?;
        Ok(Self {
            title: normalize_text(title)?,
            organization: normalize_optional(organization),
            bullets: normalize_strings(bullets.as_deref(), false, false)?,
            raw: RawTotals { units, entries },
        })
    }

    /// The stripped title (may be empty).
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The stripped organization or link, when not blank.
    #[must_use]
    pub fn organization(&self) -> Option<&str> {
        self.organization.as_deref()
    }

    /// The stripped, non-blank bullets.
    #[must_use]
    pub fn bullets(&self) -> &[String] {
        &self.bullets
    }

    /// The render input of this entry.
    #[must_use]
    pub fn to_layout(&self) -> layout::AdditionalEntry {
        layout::AdditionalEntry {
            title: self.title.clone(),
            organization: self.organization.clone(),
            bullets: self.bullets.clone(),
        }
    }
}

impl Member for AdditionalEntry {
    fn totals(&self) -> Result<Totals, ApplicationError> {
        let bullets = stored(&self.bullets);
        Ok(combine(
            admit_additional(
                Some(&self.title),
                self.organization.as_deref(),
                bullets.as_deref(),
            )?,
            self.raw,
        ))
    }
}

/// The raw input of [`StructuredResumeContent::new`]; absent collections are empty vectors.
#[derive(Debug, Clone, Default)]
pub struct ResumeInput {
    /// Full name (required, non-blank).
    pub full_name: Option<String>,
    /// Language id, `en` or `de` after stripping and lower-casing.
    pub language: Option<String>,
    /// Optional summary.
    pub summary: Option<String>,
    /// Contact fields.
    pub personal_fields: Vec<PersonalField>,
    /// Experiences.
    pub experiences: Vec<ExperienceEntry>,
    /// Education entries.
    pub education: Vec<EducationEntry>,
    /// Skill groups.
    pub skill_groups: Vec<SkillGroup>,
    /// Spoken languages.
    pub languages: Vec<LanguageEntry>,
    /// Additional entries.
    pub additional: Vec<AdditionalEntry>,
}

/// Admitted, normalized resume content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredResumeContent {
    full_name: String,
    language: String,
    summary: Option<String>,
    personal_fields: Vec<PersonalField>,
    experiences: Vec<ExperienceEntry>,
    education: Vec<EducationEntry>,
    skill_groups: Vec<SkillGroup>,
    languages: Vec<LanguageEntry>,
    additional: Vec<AdditionalEntry>,
    raw: RawTotals,
}

/// The borrowed fields of a resume for admission.
struct ResumeView<'a> {
    full_name: Option<&'a str>,
    language: Option<&'a str>,
    summary: Option<&'a str>,
    personal_fields: &'a [PersonalField],
    experiences: &'a [ExperienceEntry],
    education: &'a [EducationEntry],
    skill_groups: &'a [SkillGroup],
    languages: &'a [LanguageEntry],
    additional: &'a [AdditionalEntry],
}

fn admit_resume(view: &ResumeView<'_>) -> Result<(usize, usize), ApplicationError> {
    let mut admission = Admission::new(false)?;
    admission.scalar(view.full_name, false)?;
    admission.scalar(view.language, false)?;
    admission.scalar(view.summary, true)?;
    admission.records(&totals_of(view.personal_fields)?)?;
    admission.records(&totals_of(view.experiences)?)?;
    admission.records(&totals_of(view.education)?)?;
    admission.records(&totals_of(view.skill_groups)?)?;
    admission.records(&totals_of(view.languages)?)?;
    admission.records(&totals_of(view.additional)?)?;
    Ok(admission.finish())
}

fn normalize_language(value: Option<&str>) -> Result<String, ApplicationError> {
    let stripped = java_strip(value.ok_or_else(invalid)?);
    if stripped.is_empty() {
        return Err(invalid());
    }
    let language = stripped.to_lowercase();
    if language != "en" && language != "de" {
        return Err(invalid());
    }
    Ok(language)
}

impl StructuredResumeContent {
    /// Admits and normalizes a resume against the preparation budget.
    ///
    /// # Errors
    ///
    /// The fixed validation error, or the preparation budget error.
    pub fn new(input: ResumeInput) -> Result<Self, ApplicationError> {
        let (units, entries) = admit_resume(&ResumeView {
            full_name: input.full_name.as_deref(),
            language: input.language.as_deref(),
            summary: input.summary.as_deref(),
            personal_fields: &input.personal_fields,
            experiences: &input.experiences,
            education: &input.education,
            skill_groups: &input.skill_groups,
            languages: &input.languages,
            additional: &input.additional,
        })?;
        Ok(Self {
            full_name: normalize_required(input.full_name.as_deref())?,
            language: normalize_language(input.language.as_deref())?,
            summary: normalize_optional(input.summary.as_deref()),
            personal_fields: input.personal_fields,
            experiences: input.experiences,
            education: input.education,
            skill_groups: input.skill_groups,
            languages: input.languages,
            additional: input.additional,
            raw: RawTotals { units, entries },
        })
    }

    /// The stripped full name.
    #[must_use]
    pub fn full_name(&self) -> &str {
        &self.full_name
    }

    /// The language id, `en` or `de`.
    #[must_use]
    pub fn language(&self) -> &str {
        &self.language
    }

    /// The stripped summary, when not blank.
    #[must_use]
    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    /// The contact fields.
    #[must_use]
    pub fn personal_fields(&self) -> &[PersonalField] {
        &self.personal_fields
    }

    /// The experiences.
    #[must_use]
    pub fn experiences(&self) -> &[ExperienceEntry] {
        &self.experiences
    }

    /// The education entries.
    #[must_use]
    pub fn education(&self) -> &[EducationEntry] {
        &self.education
    }

    /// The skill groups.
    #[must_use]
    pub fn skill_groups(&self) -> &[SkillGroup] {
        &self.skill_groups
    }

    /// The spoken languages.
    #[must_use]
    pub fn languages(&self) -> &[LanguageEntry] {
        &self.languages
    }

    /// The additional entries.
    #[must_use]
    pub fn additional(&self) -> &[AdditionalEntry] {
        &self.additional
    }

    /// The retained raw UTF-16 unit total of the admitted input.
    #[must_use]
    pub const fn raw_units(&self) -> usize {
        self.raw.units
    }

    /// The retained raw entry count of the admitted input.
    #[must_use]
    pub const fn raw_entries(&self) -> usize {
        self.raw.entries
    }

    /// Recomputes the admission and checks the larger of the recomputed and retained totals
    /// against the budget.
    ///
    /// # Errors
    ///
    /// The preparation budget error.
    pub fn budget(&self) -> Result<(), ApplicationError> {
        let admitted = admit_resume(&ResumeView {
            full_name: Some(&self.full_name),
            language: Some(&self.language),
            summary: self.summary.as_deref(),
            personal_fields: &self.personal_fields,
            experiences: &self.experiences,
            education: &self.education,
            skill_groups: &self.skill_groups,
            languages: &self.languages,
            additional: &self.additional,
        })?;
        check_totals(combine(admitted, self.raw))
    }

    /// The render input of the layout crate, taken after admission and normalization.
    #[must_use]
    pub fn to_layout_resume(&self) -> layout::StructuredResumeContent {
        layout::StructuredResumeContent {
            full_name: self.full_name.clone(),
            language: self.language.clone(),
            summary: self.summary.clone(),
            personal_fields: self
                .personal_fields
                .iter()
                .map(PersonalField::to_layout)
                .collect(),
            experiences: self
                .experiences
                .iter()
                .map(ExperienceEntry::to_layout)
                .collect(),
            additional: self
                .additional
                .iter()
                .map(AdditionalEntry::to_layout)
                .collect(),
            education: self
                .education
                .iter()
                .map(EducationEntry::to_layout)
                .collect(),
            skill_groups: self
                .skill_groups
                .iter()
                .map(SkillGroup::to_layout)
                .collect(),
            languages: self
                .languages
                .iter()
                .map(LanguageEntry::to_layout)
                .collect(),
        }
    }
}

/// The raw input of [`GermanCoverLetterContent::new`]; absent personal fields are an empty vector.
#[derive(Debug, Clone, Default)]
pub struct CoverLetterInput {
    /// Sender name (required, non-blank).
    pub sender_name: Option<String>,
    /// Contact fields.
    pub personal_fields: Vec<PersonalField>,
    /// Recipient company; blank becomes none.
    pub recipient_company: Option<String>,
    /// Recipient location; blank becomes none.
    pub recipient_location: Option<String>,
    /// Job title (required, non-blank).
    pub job_title: Option<String>,
    /// Subject line (required, non-blank).
    pub subject: Option<String>,
    /// Salutation (required, non-blank).
    pub salutation: Option<String>,
    /// Paragraphs; the list and its members must be present and non-blank.
    pub paragraphs: RawStrings,
    /// Closing line (required, non-blank).
    pub closing: Option<String>,
    /// Signature line (required, non-blank).
    pub signature: Option<String>,
}

/// Admitted, normalized German cover-letter content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GermanCoverLetterContent {
    sender_name: String,
    personal_fields: Vec<PersonalField>,
    recipient_company: Option<String>,
    recipient_location: Option<String>,
    job_title: String,
    subject: String,
    salutation: String,
    paragraphs: Vec<String>,
    closing: String,
    signature: String,
    raw: RawTotals,
}

/// The borrowed fields of a cover letter for admission.
struct CoverView<'a> {
    sender_name: Option<&'a str>,
    personal_fields: &'a [PersonalField],
    recipient_company: Option<&'a str>,
    recipient_location: Option<&'a str>,
    job_title: Option<&'a str>,
    subject: Option<&'a str>,
    salutation: Option<&'a str>,
    paragraphs: Option<&'a [Option<&'a str>]>,
    closing: Option<&'a str>,
    signature: Option<&'a str>,
}

fn admit_cover(view: &CoverView<'_>) -> Result<(usize, usize), ApplicationError> {
    let mut admission = Admission::new(false)?;
    admission.scalar(view.sender_name, false)?;
    admission.records(&totals_of(view.personal_fields)?)?;
    admission.scalar(view.recipient_company, true)?;
    admission.scalar(view.recipient_location, true)?;
    admission.scalar(view.job_title, false)?;
    admission.scalar(view.subject, false)?;
    admission.scalar(view.salutation, false)?;
    admission.strings(view.paragraphs, false, false)?;
    admission.scalar(view.closing, false)?;
    admission.scalar(view.signature, false)?;
    Ok(admission.finish())
}

impl GermanCoverLetterContent {
    /// Admits and normalizes a cover letter against the preparation budget.
    ///
    /// # Errors
    ///
    /// The fixed validation error, or the preparation budget error.
    pub fn new(input: CoverLetterInput) -> Result<Self, ApplicationError> {
        let paragraphs = view(&input.paragraphs);
        let (units, entries) = admit_cover(&CoverView {
            sender_name: input.sender_name.as_deref(),
            personal_fields: &input.personal_fields,
            recipient_company: input.recipient_company.as_deref(),
            recipient_location: input.recipient_location.as_deref(),
            job_title: input.job_title.as_deref(),
            subject: input.subject.as_deref(),
            salutation: input.salutation.as_deref(),
            paragraphs: paragraphs.as_deref(),
            closing: input.closing.as_deref(),
            signature: input.signature.as_deref(),
        })?;
        Ok(Self {
            sender_name: normalize_required(input.sender_name.as_deref())?,
            personal_fields: input.personal_fields,
            recipient_company: normalize_optional(input.recipient_company.as_deref()),
            recipient_location: normalize_optional(input.recipient_location.as_deref()),
            job_title: normalize_required(input.job_title.as_deref())?,
            subject: normalize_required(input.subject.as_deref())?,
            salutation: normalize_required(input.salutation.as_deref())?,
            paragraphs: normalize_strings(paragraphs.as_deref(), true, false)?,
            closing: normalize_required(input.closing.as_deref())?,
            signature: normalize_required(input.signature.as_deref())?,
            raw: RawTotals { units, entries },
        })
    }

    /// The stripped sender name.
    #[must_use]
    pub fn sender_name(&self) -> &str {
        &self.sender_name
    }

    /// The contact fields.
    #[must_use]
    pub fn personal_fields(&self) -> &[PersonalField] {
        &self.personal_fields
    }

    /// The stripped recipient company, when not blank.
    #[must_use]
    pub fn recipient_company(&self) -> Option<&str> {
        self.recipient_company.as_deref()
    }

    /// The stripped recipient location, when not blank.
    #[must_use]
    pub fn recipient_location(&self) -> Option<&str> {
        self.recipient_location.as_deref()
    }

    /// The stripped job title.
    #[must_use]
    pub fn job_title(&self) -> &str {
        &self.job_title
    }

    /// The stripped subject.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// The stripped salutation.
    #[must_use]
    pub fn salutation(&self) -> &str {
        &self.salutation
    }

    /// The stripped paragraphs (each non-blank).
    #[must_use]
    pub fn paragraphs(&self) -> &[String] {
        &self.paragraphs
    }

    /// The stripped closing.
    #[must_use]
    pub fn closing(&self) -> &str {
        &self.closing
    }

    /// The stripped signature.
    #[must_use]
    pub fn signature(&self) -> &str {
        &self.signature
    }

    /// The retained raw UTF-16 unit total of the admitted input.
    #[must_use]
    pub const fn raw_units(&self) -> usize {
        self.raw.units
    }

    /// The retained raw entry count of the admitted input.
    #[must_use]
    pub const fn raw_entries(&self) -> usize {
        self.raw.entries
    }

    /// Recomputes the admission and checks the larger of the recomputed and retained totals
    /// against the budget.
    ///
    /// # Errors
    ///
    /// The preparation budget error.
    pub fn budget(&self) -> Result<(), ApplicationError> {
        let paragraphs = stored(&self.paragraphs);
        let admitted = admit_cover(&CoverView {
            sender_name: Some(&self.sender_name),
            personal_fields: &self.personal_fields,
            recipient_company: self.recipient_company.as_deref(),
            recipient_location: self.recipient_location.as_deref(),
            job_title: Some(&self.job_title),
            subject: Some(&self.subject),
            salutation: Some(&self.salutation),
            paragraphs: paragraphs.as_deref(),
            closing: Some(&self.closing),
            signature: Some(&self.signature),
        })?;
        check_totals(combine(admitted, self.raw))
    }

    /// The render input of the layout crate; the layout input has no job-title field.
    #[must_use]
    pub fn to_layout_cover_letter(&self) -> layout::GermanCoverLetterContent {
        layout::GermanCoverLetterContent {
            sender_name: self.sender_name.clone(),
            personal_fields: self
                .personal_fields
                .iter()
                .map(PersonalField::to_layout)
                .collect(),
            recipient_company: self.recipient_company.clone(),
            recipient_location: self.recipient_location.clone(),
            subject: self.subject.clone(),
            salutation: self.salutation.clone(),
            paragraphs: self.paragraphs.clone(),
            closing: self.closing.clone(),
            signature: self.signature.clone(),
        }
    }
}

fn check_totals(totals: Totals) -> Result<(), ApplicationError> {
    let mut budget = PreparationBudget::new(BudgetField::Text);
    budget.add_units(totals.units)?;
    budget.add_entries(totals.entries)?;
    Ok(())
}

/// Checks the retained and recomputed totals of resume content.
///
/// # Errors
///
/// The preparation budget error.
pub fn budget_resume_content(content: &StructuredResumeContent) -> Result<(), ApplicationError> {
    content.budget()
}

/// Checks the retained and recomputed totals of cover-letter content.
///
/// # Errors
///
/// The preparation budget error.
pub fn budget_cover_letter_content(
    content: &GermanCoverLetterContent,
) -> Result<(), ApplicationError> {
    content.budget()
}
