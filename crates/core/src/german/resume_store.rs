//! Source-shaped Germany resume writes, immutable bounded admission and the pure content mapping.
//! It holds no SQL: the PostgreSQL adapter
//! implements [`GermanResumeRepository`] in `jobniac_infra`.
//!
//! Every row has private fields and a checked constructor. The constructor first admits the raw
//! text against the preparation budget (UTF-16 units, counted before any normalization), then
//! normalizes it with the source rules (NUL refused, Java strip, ROOT lowercase for the open and
//! closed vocabularies, blank optionals become `None`). The raw unit total is retained, so a
//! stripped value keeps counting against the aggregate budget. Write aggregates count physical
//! rows (10,000 admitted, 10,001 refused) and at most two variants.
//!
//! Rust strings cannot hold lone surrogates and the typed fields cannot hold the wrong
//! type, so those refusals have no counterpart here: a surrogate pair is already one
//! scalar, and a `datetime` cannot be passed where a `Date` is expected.

use std::collections::HashSet;
use std::hash::Hash;

use async_trait::async_trait;
use jobniac_text::java::{BudgetField, PreparationBudget, java_strip, utf16_length};
use time::{Date, Month, OffsetDateTime, UtcOffset};
use uuid::Uuid;

use super::content::{StructuredResumeContent, budget_resume_content};
use crate::errors::{AppError, ApplicationError, validation_error};
use crate::generated_assets::{GeneratedReservation, IntentReservation};

/// The fixed message of every refused resume persistence input.
const INVALID_MESSAGE: &str = "Invalid German resume persistence input";

/// The refusal of every malformed resume persistence input.
#[must_use]
pub fn invalid_resume_input() -> ApplicationError {
    validation_error(INVALID_MESSAGE, None, None)
}

fn invalid() -> ApplicationError {
    invalid_resume_input()
}

/// The persistence admission budget, built with the default `profile` field.
fn new_budget() -> PreparationBudget {
    PreparationBudget::new(BudgetField::Profile)
}

macro_rules! accessors {
    () => {};
    (raw; $($rest:tt)*) => {
        /// The retained raw UTF-16 unit total of the admitted input, before normalization.
        #[must_use]
        pub const fn raw_units(&self) -> usize {
            self.raw.0
        }
        accessors!($($rest)*);
    };
    (copy $name:ident: $ty:ty; $($rest:tt)*) => {
        #[doc = concat!("The `", stringify!($name), "` column.")]
        #[must_use]
        pub const fn $name(&self) -> $ty {
            self.$name
        }
        accessors!($($rest)*);
    };
    (str $name:ident; $($rest:tt)*) => {
        #[doc = concat!("The normalized `", stringify!($name), "` column.")]
        #[must_use]
        pub fn $name(&self) -> &str {
            &self.$name
        }
        accessors!($($rest)*);
    };
    (opt $name:ident; $($rest:tt)*) => {
        #[doc = concat!("The normalized `", stringify!($name), "` column, when present.")]
        #[must_use]
        pub fn $name(&self) -> Option<&str> {
            self.$name.as_deref()
        }
        accessors!($($rest)*);
    };
}

pub(super) use accessors;

/// A stored revision compare-and-set token: an exact integer in `0..=i64::MAX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(i64);

impl Revision {
    /// The revision, when `value` is not negative.
    #[must_use]
    pub const fn new(value: i64) -> Option<Self> {
        if value < 0 { None } else { Some(Self(value)) }
    }

    /// The revision of a wide integer, when it lies in `0..=i64::MAX`.
    #[must_use]
    pub fn from_wide(value: i128) -> Option<Self> {
        i64::try_from(value).ok().and_then(Self::new)
    }

    /// The stored value.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Admits a numeric owner revision precondition.
///
/// # Errors
///
/// The fixed validation error for a value outside `0..=i64::MAX`.
pub fn validate_revision(value: i128) -> Result<Revision, ApplicationError> {
    Revision::from_wide(value).ok_or_else(invalid)
}

/// A stored position: an exact integer in `0..=i32::MAX` (a PostgreSQL non-negative `integer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayOrder(i32);

impl DisplayOrder {
    /// The position, when `value` lies in `0..=i32::MAX`.
    #[must_use]
    pub fn new(value: i64) -> Option<Self> {
        i32::try_from(value)
            .ok()
            .filter(|order| *order >= 0)
            .map(Self)
    }

    /// The stored value.
    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }
}

/// The pair of a profile and a job that serializes German resume and cover-letter writers
/// (advisory lock `jobniac-germany-resume:{profile_id}:{job_id}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PairKey {
    /// The owning profile.
    pub profile_id: Uuid,
    /// The owning job.
    pub job_id: Uuid,
}

impl PairKey {
    /// The text hashed into the transaction advisory lock key, hyphenated lowercase UUIDs.
    #[must_use]
    pub fn lock_name(&self) -> String {
        format!("jobniac-germany-resume:{}:{}", self.profile_id, self.job_id)
    }
}

/// The two stored resume languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResumeLanguage {
    /// English (`en`).
    English,
    /// German (`de`).
    German,
}

impl ResumeLanguage {
    /// The stored language id.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::German => "de",
        }
    }

    /// The language of a stored id.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "en" => Some(Self::English),
            "de" => Some(Self::German),
            _ => None,
        }
    }
}

/// The stored section types, in the order the mapper emits them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SectionType {
    /// Contact fields (`personal`).
    Personal,
    /// The profile summary (`summary`).
    Summary,
    /// Professional experience (`experience`).
    Experience,
    /// Projects (`additional`).
    Additional,
    /// Education (`education`).
    Education,
    /// Skill groups (`skills`).
    Skills,
    /// Spoken languages (`languages`).
    Languages,
}

impl SectionType {
    /// Every section type in mapper order.
    pub const ALL: [Self; 7] = [
        Self::Personal,
        Self::Summary,
        Self::Experience,
        Self::Additional,
        Self::Education,
        Self::Skills,
        Self::Languages,
    ];

    /// The stored section type.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Summary => "summary",
            Self::Experience => "experience",
            Self::Additional => "additional",
            Self::Education => "education",
            Self::Skills => "skills",
            Self::Languages => "languages",
        }
    }

    /// The stored section title in `language`.
    #[must_use]
    pub const fn title(self, language: ResumeLanguage) -> &'static str {
        match (self, language) {
            (Self::Personal, ResumeLanguage::English) => "Contact",
            (Self::Personal, ResumeLanguage::German) => "Kontakt",
            (Self::Summary, ResumeLanguage::English) => "Profile",
            (Self::Summary, ResumeLanguage::German) => "Profil",
            (Self::Experience, ResumeLanguage::English) => "Professional Experience",
            (Self::Experience, ResumeLanguage::German) => "Berufserfahrung",
            (Self::Additional, ResumeLanguage::English) => "Projects",
            (Self::Additional, ResumeLanguage::German) => "Projekte",
            (Self::Education, ResumeLanguage::English) => "Education",
            (Self::Education, ResumeLanguage::German) => "Ausbildung",
            (Self::Skills, ResumeLanguage::English) => "Skills",
            (Self::Skills, ResumeLanguage::German) => "Kenntnisse",
            (Self::Languages, ResumeLanguage::English) => "Languages",
            (Self::Languages, ResumeLanguage::German) => "Sprachen",
        }
    }
}

/// The stored entry types written by the mapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryType {
    /// A contact field (`personal_field`).
    PersonalField,
    /// The summary paragraph (`summary`).
    Summary,
    /// A job (`experience`).
    Experience,
    /// A project (`project`).
    Project,
    /// An education entry (`education`).
    Education,
    /// A skill group (`skill_group`).
    SkillGroup,
    /// A spoken language (`language`).
    Language,
}

impl EntryType {
    /// Every entry type.
    pub const ALL: [Self; 7] = [
        Self::PersonalField,
        Self::Summary,
        Self::Experience,
        Self::Project,
        Self::Education,
        Self::SkillGroup,
        Self::Language,
    ];

    /// The stored entry type.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PersonalField => "personal_field",
            Self::Summary => "summary",
            Self::Experience => "experience",
            Self::Project => "project",
            Self::Education => "education",
            Self::SkillGroup => "skill_group",
            Self::Language => "language",
        }
    }
}

/// The retained raw UTF-16 unit total of a row; never part of equality.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct RawUnits(pub(super) usize);

impl PartialEq for RawUnits {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for RawUnits {}

/// The retained raw admission totals of a write wrapper; never part of equality.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct RawTotals(pub(super) Totals);

impl PartialEq for RawTotals {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for RawTotals {}

/// UTF-16 units and physical rows of a subtree.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Totals {
    pub(super) units: usize,
    pub(super) entries: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextKind {
    Required,
    Lower,
    Format,
    Language,
}

/// The shared prefix of every text kind: NUL refused, then Java strip.
fn canonical(value: &str) -> Result<&str, ApplicationError> {
    if value.contains('\0') {
        return Err(invalid());
    }
    Ok(java_strip(value))
}

fn normalize(value: &str, kind: TextKind) -> Result<String, ApplicationError> {
    let stripped = canonical(value)?;
    let result = match kind {
        TextKind::Required => stripped.to_owned(),
        TextKind::Lower | TextKind::Format | TextKind::Language => stripped.to_lowercase(),
    };
    if result.is_empty()
        || (kind == TextKind::Format && result != "germany")
        || (kind == TextKind::Language && ResumeLanguage::parse(&result).is_none())
    {
        return Err(invalid());
    }
    Ok(result)
}

fn normalize_optional(value: Option<&str>) -> Result<Option<String>, ApplicationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let stripped = canonical(value)?;
    Ok((!stripped.is_empty()).then(|| stripped.to_owned()))
}

/// Pass 1 of a row: admits every raw text before any normalized copy exists.
fn admit(texts: &[Option<&str>]) -> Result<usize, ApplicationError> {
    let mut budget = new_budget();
    let mut units = 0;
    for text in texts.iter().flatten() {
        budget.add_text(Some(text))?;
        units += utf16_length(text);
    }
    Ok(units)
}

/// The units a stored row counts: the larger of the retained raw total and the stored texts,
/// the latter admitted again because ROOT lowercase can expand a value.
fn retained(texts: &[Option<&str>], raw: RawUnits) -> Result<usize, ApplicationError> {
    Ok(admit(texts)?.max(raw.0))
}

/// Normalizes an aware instant to UTC.
fn to_utc(value: OffsetDateTime) -> Result<OffsetDateTime, ApplicationError> {
    value.checked_to_offset(UtcOffset::UTC).ok_or_else(invalid)
}

/// Raw inputs of a [`Resume`].
#[derive(Debug, Clone, Copy)]
pub struct ResumeFields<'a> {
    /// Resume id.
    pub id: Uuid,
    /// Owning profile.
    pub profile_id: Uuid,
    /// Owning job.
    pub job_id: Uuid,
    /// Raw format; must normalize to `germany`.
    pub format: &'a str,
    /// Profile revision instant the resume was built from.
    pub profile_revision: OffsetDateTime,
    /// Job revision instant the resume was built from.
    pub job_revision: OffsetDateTime,
    /// Creation instant.
    pub created_at: OffsetDateTime,
    /// Last write instant.
    pub updated_at: OffsetDateTime,
}

/// The German resume parent row. Instants are UTC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resume {
    id: Uuid,
    profile_id: Uuid,
    job_id: Uuid,
    format: String,
    profile_revision: OffsetDateTime,
    job_revision: OffsetDateTime,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    raw: RawUnits,
}

impl Resume {
    /// Admits and normalizes a resume row.
    ///
    /// # Errors
    ///
    /// The fixed validation error, or the preparation budget error.
    pub fn new(fields: ResumeFields<'_>) -> Result<Self, ApplicationError> {
        let units = admit(&[Some(fields.format)])?;
        Ok(Self {
            id: fields.id,
            profile_id: fields.profile_id,
            job_id: fields.job_id,
            format: normalize(fields.format, TextKind::Format)?,
            profile_revision: to_utc(fields.profile_revision)?,
            job_revision: to_utc(fields.job_revision)?,
            created_at: to_utc(fields.created_at)?,
            updated_at: to_utc(fields.updated_at)?,
            raw: RawUnits(units),
        })
    }

    accessors! {
        raw;
        copy id: Uuid;
        copy profile_id: Uuid;
        copy job_id: Uuid;
        str format;
        copy profile_revision: OffsetDateTime;
        copy job_revision: OffsetDateTime;
        copy created_at: OffsetDateTime;
        copy updated_at: OffsetDateTime;
    }

    /// The profile and job pair that serializes the writers.
    #[must_use]
    pub const fn pair_key(&self) -> PairKey {
        PairKey {
            profile_id: self.profile_id,
            job_id: self.job_id,
        }
    }

    fn retained_units(&self) -> Result<usize, ApplicationError> {
        retained(&[Some(&self.format)], self.raw)
    }
}

/// Raw inputs of a [`ResumeVariant`].
#[derive(Debug, Clone, Copy)]
pub struct VariantFields<'a> {
    /// Variant id.
    pub id: Uuid,
    /// Owning resume.
    pub resume_id: Uuid,
    /// Raw language; must normalize to `en` or `de`.
    pub language: &'a str,
    /// The document that owns the PDF.
    pub document_id: Uuid,
    /// Relative file path of the generated PDF.
    pub file_path: &'a str,
    /// Creation instant.
    pub created_at: OffsetDateTime,
    /// Last write instant.
    pub updated_at: OffsetDateTime,
}

/// One language variant of a resume with its PDF document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeVariant {
    id: Uuid,
    resume_id: Uuid,
    language: String,
    document_id: Uuid,
    file_path: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    raw: RawUnits,
}

impl ResumeVariant {
    /// Admits and normalizes a variant row.
    ///
    /// # Errors
    ///
    /// The fixed validation error, or the preparation budget error.
    pub fn new(fields: VariantFields<'_>) -> Result<Self, ApplicationError> {
        let units = admit(&[Some(fields.language), Some(fields.file_path)])?;
        Ok(Self {
            id: fields.id,
            resume_id: fields.resume_id,
            language: normalize(fields.language, TextKind::Language)?,
            document_id: fields.document_id,
            file_path: normalize(fields.file_path, TextKind::Required)?,
            created_at: to_utc(fields.created_at)?,
            updated_at: to_utc(fields.updated_at)?,
            raw: RawUnits(units),
        })
    }

    accessors! {
        raw;
        copy id: Uuid;
        copy resume_id: Uuid;
        str language;
        copy document_id: Uuid;
        str file_path;
        copy created_at: OffsetDateTime;
        copy updated_at: OffsetDateTime;
    }

    fn retained_units(&self) -> Result<usize, ApplicationError> {
        retained(&[Some(&self.language), Some(&self.file_path)], self.raw)
    }
}

/// Raw inputs of a [`ResumeSection`].
#[derive(Debug, Clone, Copy)]
pub struct SectionFields<'a> {
    /// Section id.
    pub id: Uuid,
    /// Owning variant.
    pub variant_id: Uuid,
    /// Raw section type; stored lowercase and open (see [`SectionType`]).
    pub section_type: &'a str,
    /// Section title.
    pub title: &'a str,
    /// Position within the variant.
    pub display_order: DisplayOrder,
}

/// A section of a variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeSection {
    id: Uuid,
    variant_id: Uuid,
    section_type: String,
    title: String,
    display_order: DisplayOrder,
    raw: RawUnits,
}

impl ResumeSection {
    /// Admits and normalizes a section row.
    ///
    /// # Errors
    ///
    /// The fixed validation error, or the preparation budget error.
    pub fn new(fields: SectionFields<'_>) -> Result<Self, ApplicationError> {
        let units = admit(&[Some(fields.section_type), Some(fields.title)])?;
        Ok(Self {
            id: fields.id,
            variant_id: fields.variant_id,
            section_type: normalize(fields.section_type, TextKind::Lower)?,
            title: normalize(fields.title, TextKind::Required)?,
            display_order: fields.display_order,
            raw: RawUnits(units),
        })
    }

    accessors! {
        raw;
        copy id: Uuid;
        copy variant_id: Uuid;
        str section_type;
        str title;
        copy display_order: DisplayOrder;
    }

    fn retained_units(&self) -> Result<usize, ApplicationError> {
        retained(&[Some(&self.section_type), Some(&self.title)], self.raw)
    }
}

/// Raw inputs of a [`ResumeEntry`].
#[derive(Debug, Clone, Copy)]
pub struct EntryFields<'a> {
    /// Entry id.
    pub id: Uuid,
    /// Owning section.
    pub section_id: Uuid,
    /// Raw entry type; stored lowercase and open (see [`EntryType`]).
    pub entry_type: &'a str,
    /// Position within the section; duplicates are source-compatible.
    pub display_order: DisplayOrder,
    /// Optional title.
    pub title: Option<&'a str>,
    /// Optional organization.
    pub organization: Option<&'a str>,
    /// Optional location.
    pub location: Option<&'a str>,
    /// Optional start date.
    pub start_date: Option<Date>,
    /// Optional end date.
    pub end_date: Option<Date>,
    /// Optional opaque metadata text (not JSON).
    pub metadata: Option<&'a str>,
}

/// An entry of a section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeEntry {
    id: Uuid,
    section_id: Uuid,
    entry_type: String,
    display_order: DisplayOrder,
    title: Option<String>,
    organization: Option<String>,
    location: Option<String>,
    start_date: Option<Date>,
    end_date: Option<Date>,
    metadata: Option<String>,
    raw: RawUnits,
}

impl ResumeEntry {
    /// Admits and normalizes an entry row.
    ///
    /// # Errors
    ///
    /// The fixed validation error, or the preparation budget error.
    pub fn new(fields: EntryFields<'_>) -> Result<Self, ApplicationError> {
        let units = admit(&[
            Some(fields.entry_type),
            fields.title,
            fields.organization,
            fields.location,
            fields.metadata,
        ])?;
        Ok(Self {
            id: fields.id,
            section_id: fields.section_id,
            entry_type: normalize(fields.entry_type, TextKind::Lower)?,
            display_order: fields.display_order,
            title: normalize_optional(fields.title)?,
            organization: normalize_optional(fields.organization)?,
            location: normalize_optional(fields.location)?,
            start_date: fields.start_date,
            end_date: fields.end_date,
            metadata: normalize_optional(fields.metadata)?,
            raw: RawUnits(units),
        })
    }

    accessors! {
        raw;
        copy id: Uuid;
        copy section_id: Uuid;
        str entry_type;
        copy display_order: DisplayOrder;
        opt title;
        opt organization;
        opt location;
        copy start_date: Option<Date>;
        copy end_date: Option<Date>;
        opt metadata;
    }

    fn retained_units(&self) -> Result<usize, ApplicationError> {
        retained(
            &[
                Some(&self.entry_type),
                self.title.as_deref(),
                self.organization.as_deref(),
                self.location.as_deref(),
                self.metadata.as_deref(),
            ],
            self.raw,
        )
    }
}

/// Raw inputs of a [`ResumeEntryBullet`].
#[derive(Debug, Clone, Copy)]
pub struct BulletFields<'a> {
    /// Bullet id.
    pub id: Uuid,
    /// Owning entry.
    pub entry_id: Uuid,
    /// Position within the entry; duplicates are source-compatible.
    pub display_order: DisplayOrder,
    /// Bullet text.
    pub text: &'a str,
}

/// A bullet of an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeEntryBullet {
    id: Uuid,
    entry_id: Uuid,
    display_order: DisplayOrder,
    text: String,
    raw: RawUnits,
}

impl ResumeEntryBullet {
    /// Admits and normalizes a bullet row.
    ///
    /// # Errors
    ///
    /// The fixed validation error, or the preparation budget error.
    pub fn new(fields: BulletFields<'_>) -> Result<Self, ApplicationError> {
        let units = admit(&[Some(fields.text)])?;
        Ok(Self {
            id: fields.id,
            entry_id: fields.entry_id,
            display_order: fields.display_order,
            text: normalize(fields.text, TextKind::Required)?,
            raw: RawUnits(units),
        })
    }

    accessors! {
        raw;
        copy id: Uuid;
        copy entry_id: Uuid;
        copy display_order: DisplayOrder;
        str text;
    }

    fn retained_units(&self) -> Result<usize, ApplicationError> {
        retained(&[Some(&self.text)], self.raw)
    }
}

/// Totals of one write level: the row, then its children, each bounded
/// by the preparation budget. `stored` is the retained total of an already admitted wrapper.
fn level_totals(
    row_units: impl FnOnce() -> Result<usize, ApplicationError>,
    children: usize,
    child_totals: impl FnOnce() -> Result<Vec<Totals>, ApplicationError>,
    max_children: Option<usize>,
    stored: Option<Totals>,
) -> Result<Totals, ApplicationError> {
    let mut budget = new_budget();
    budget.add_entries(1 + children)?;
    if max_children.is_some_and(|max| children > max) {
        return Err(invalid());
    }
    let mut units = row_units()?;
    let mut entries = 1;
    budget.add_units(units)?;
    for child in child_totals()? {
        budget.add_units(child.units)?;
        budget.add_entries(child.entries.saturating_sub(1))?;
        units += child.units;
        entries += child.entries;
    }
    if let Some(stored) = stored {
        if stored.entries < entries {
            return Err(invalid());
        }
        units = units.max(stored.units);
        entries = entries.max(stored.entries);
        let mut budget = new_budget();
        budget.add_units(units)?;
        budget.add_entries(entries)?;
    }
    Ok(Totals { units, entries })
}

/// An entry with its bullets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryWrite {
    entry: ResumeEntry,
    bullets: Vec<ResumeEntryBullet>,
    raw: RawTotals,
}

impl EntryWrite {
    /// Admits an entry and its bullets.
    ///
    /// # Errors
    ///
    /// The preparation budget error.
    pub fn new(
        entry: ResumeEntry,
        bullets: Vec<ResumeEntryBullet>,
    ) -> Result<Self, ApplicationError> {
        let raw = Self::totals(&entry, &bullets, None)?;
        Ok(Self {
            entry,
            bullets,
            raw: RawTotals(raw),
        })
    }

    fn totals(
        entry: &ResumeEntry,
        bullets: &[ResumeEntryBullet],
        stored: Option<Totals>,
    ) -> Result<Totals, ApplicationError> {
        level_totals(
            || entry.retained_units(),
            bullets.len(),
            || {
                bullets
                    .iter()
                    .map(|bullet| {
                        Ok(Totals {
                            units: bullet.retained_units()?,
                            entries: 1,
                        })
                    })
                    .collect()
            },
            None,
            stored,
        )
    }

    fn recompute(&self) -> Result<Totals, ApplicationError> {
        Self::totals(&self.entry, &self.bullets, Some(self.raw.0))
    }

    /// The entry row.
    #[must_use]
    pub const fn entry(&self) -> &ResumeEntry {
        &self.entry
    }

    /// The bullets in stored order.
    #[must_use]
    pub fn bullets(&self) -> &[ResumeEntryBullet] {
        &self.bullets
    }
}

/// A section with its entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionWrite {
    section: ResumeSection,
    entries: Vec<EntryWrite>,
    raw: RawTotals,
}

impl SectionWrite {
    /// Admits a section and its entries.
    ///
    /// # Errors
    ///
    /// The preparation budget error.
    pub fn new(section: ResumeSection, entries: Vec<EntryWrite>) -> Result<Self, ApplicationError> {
        let raw = Self::totals(&section, &entries, None)?;
        Ok(Self {
            section,
            entries,
            raw: RawTotals(raw),
        })
    }

    fn totals(
        section: &ResumeSection,
        entries: &[EntryWrite],
        stored: Option<Totals>,
    ) -> Result<Totals, ApplicationError> {
        level_totals(
            || section.retained_units(),
            entries.len(),
            || entries.iter().map(EntryWrite::recompute).collect(),
            None,
            stored,
        )
    }

    fn recompute(&self) -> Result<Totals, ApplicationError> {
        Self::totals(&self.section, &self.entries, Some(self.raw.0))
    }

    /// The section row.
    #[must_use]
    pub const fn section(&self) -> &ResumeSection {
        &self.section
    }

    /// The entries in stored order.
    #[must_use]
    pub fn entries(&self) -> &[EntryWrite] {
        &self.entries
    }
}

/// A variant with its sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantWrite {
    variant: ResumeVariant,
    sections: Vec<SectionWrite>,
    raw: RawTotals,
}

impl VariantWrite {
    /// Admits a variant and its sections.
    ///
    /// # Errors
    ///
    /// The preparation budget error.
    pub fn new(
        variant: ResumeVariant,
        sections: Vec<SectionWrite>,
    ) -> Result<Self, ApplicationError> {
        let raw = Self::totals(&variant, &sections, None)?;
        Ok(Self {
            variant,
            sections,
            raw: RawTotals(raw),
        })
    }

    /// Rebuilds a variant write around `variant` and `sections` and keeps the admitted raw totals
    /// of `original`: each total is the larger of the original and the
    /// recomputed one.
    ///
    /// # Errors
    ///
    /// The preparation budget error, or the fixed validation error.
    pub(crate) fn retained(
        variant: ResumeVariant,
        sections: Vec<SectionWrite>,
        original: &Self,
    ) -> Result<Self, ApplicationError> {
        let raw = Self::totals(&variant, &sections, Some(original.raw.0))?;
        Ok(Self {
            variant,
            sections,
            raw: RawTotals(raw),
        })
    }

    fn totals(
        variant: &ResumeVariant,
        sections: &[SectionWrite],
        stored: Option<Totals>,
    ) -> Result<Totals, ApplicationError> {
        level_totals(
            || variant.retained_units(),
            sections.len(),
            || sections.iter().map(SectionWrite::recompute).collect(),
            None,
            stored,
        )
    }

    fn recompute(&self) -> Result<Totals, ApplicationError> {
        Self::totals(&self.variant, &self.sections, Some(self.raw.0))
    }

    /// The variant row.
    #[must_use]
    pub const fn variant(&self) -> &ResumeVariant {
        &self.variant
    }

    /// The sections in stored order.
    #[must_use]
    pub fn sections(&self) -> &[SectionWrite] {
        &self.sections
    }

    /// The admitted UTF-16 unit total (at least the stored texts).
    #[must_use]
    pub const fn raw_units(&self) -> usize {
        self.raw.0.units
    }

    /// The physical row count of the subtree.
    #[must_use]
    pub const fn raw_entries(&self) -> usize {
        self.raw.0.entries
    }
}

/// The full resume aggregate: the parent with at most two variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeAggregateWrite {
    resume: Resume,
    variants: Vec<VariantWrite>,
    raw: RawTotals,
}

/// The most variants one resume aggregate may carry (English and German).
pub const MAX_VARIANTS: usize = 2;

impl ResumeAggregateWrite {
    /// Admits a resume and its variants; more than two variants are refused.
    ///
    /// # Errors
    ///
    /// The preparation budget error, or the fixed validation error.
    pub fn new(resume: Resume, variants: Vec<VariantWrite>) -> Result<Self, ApplicationError> {
        let raw = Self::totals(&resume, &variants, None)?;
        Ok(Self {
            resume,
            variants,
            raw: RawTotals(raw),
        })
    }

    /// Rebuilds the aggregate around `resume` and `variants` and keeps the admitted raw totals of
    /// `original`: each total is the larger of the original and the
    /// recomputed one.
    ///
    /// # Errors
    ///
    /// The preparation budget error, or the fixed validation error.
    pub(crate) fn retained(
        resume: Resume,
        variants: Vec<VariantWrite>,
        original: &Self,
    ) -> Result<Self, ApplicationError> {
        let raw = Self::totals(&resume, &variants, Some(original.raw.0))?;
        Ok(Self {
            resume,
            variants,
            raw: RawTotals(raw),
        })
    }

    fn totals(
        resume: &Resume,
        variants: &[VariantWrite],
        stored: Option<Totals>,
    ) -> Result<Totals, ApplicationError> {
        level_totals(
            || resume.retained_units(),
            variants.len(),
            || variants.iter().map(VariantWrite::recompute).collect(),
            Some(MAX_VARIANTS),
            stored,
        )
    }

    /// The parent row.
    #[must_use]
    pub const fn resume(&self) -> &Resume {
        &self.resume
    }

    /// The variants in stored order.
    #[must_use]
    pub fn variants(&self) -> &[VariantWrite] {
        &self.variants
    }

    /// The physical row count of the aggregate.
    #[must_use]
    pub const fn raw_entries(&self) -> usize {
        self.raw.0.entries
    }

    /// The admitted UTF-16 unit total.
    #[must_use]
    pub const fn raw_units(&self) -> usize {
        self.raw.0.units
    }

    /// The profile and job pair whose advisory lock serializes the writer.
    #[must_use]
    pub const fn pair_key(&self) -> PairKey {
        self.resume.pair_key()
    }
}

fn unique<T: Hash + Eq>(seen: &mut HashSet<T>, value: T) -> Result<(), ApplicationError> {
    if seen.insert(value) {
        Ok(())
    } else {
        Err(invalid())
    }
}

/// Re-admits the whole tree, then checks chronology, parentage and per-table uniqueness. Duplicate
/// entry and bullet positions are accepted; section
/// positions are unique within a variant.
///
/// # Errors
///
/// The preparation budget error, or the fixed validation error.
pub fn validate_resume_aggregate(write: &ResumeAggregateWrite) -> Result<(), ApplicationError> {
    ResumeAggregateWrite::totals(&write.resume, &write.variants, Some(write.raw.0))?;
    if write.resume.updated_at < write.resume.created_at {
        return Err(invalid());
    }
    let mut variants = HashSet::new();
    let mut sections = HashSet::new();
    let mut entries = HashSet::new();
    let mut bullets = HashSet::new();
    let mut documents = HashSet::new();
    let mut languages = HashSet::new();
    for variant_write in &write.variants {
        let variant = &variant_write.variant;
        if variant.resume_id != write.resume.id || variant.updated_at < variant.created_at {
            return Err(invalid());
        }
        unique(&mut variants, variant.id)?;
        unique(&mut documents, variant.document_id)?;
        unique(&mut languages, variant.language.as_str())?;
        let mut positions = HashSet::new();
        for section_write in &variant_write.sections {
            let section = &section_write.section;
            if section.variant_id != variant.id || positions.contains(&section.display_order) {
                return Err(invalid());
            }
            unique(&mut sections, section.id)?;
            positions.insert(section.display_order);
            for entry_write in &section_write.entries {
                let entry = &entry_write.entry;
                if entry.section_id != section.id {
                    return Err(invalid());
                }
                unique(&mut entries, entry.id)?;
                for bullet in &entry_write.bullets {
                    if bullet.entry_id != entry.id {
                        return Err(invalid());
                    }
                    unique(&mut bullets, bullet.id)?;
                }
            }
        }
    }
    Ok(())
}

/// The stored result of replacing a resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceResult {
    /// The stored parent.
    pub resume: Resume,
    /// The variants as stored.
    pub variants: Vec<ResumeVariant>,
    /// The variants the replacement removed.
    pub previous_variants: Vec<ResumeVariant>,
}

/// Persistence port of the German resume. The PostgreSQL
/// adapter is `jobniac_infra::postgres::german`; the replace transaction takes the pair advisory
/// lock ([`PairKey::lock_name`]), then the rows in the fixed order.
#[async_trait]
pub trait GermanResumeRepository: Send + Sync {
    /// Reads a resume by id.
    async fn find_by_id(&self, resume_id: Uuid) -> Result<Option<Resume>, AppError>;

    /// Reads the resume of one profile, job and format.
    async fn find_by_profile_job_format(
        &self,
        profile_id: Uuid,
        job_id: Uuid,
        format: &str,
    ) -> Result<Option<Resume>, AppError>;

    /// Reads the variants of a resume.
    async fn find_variants(&self, resume_id: Uuid) -> Result<Vec<ResumeVariant>, AppError>;

    /// Replaces the resume aggregate under the owner revision preconditions. `bindings` are the
    /// reserved assets of the variants and `generation_claims`, when present, their pending
    /// intents (the unclassified path passes `None`).
    async fn replace_germany_resume(
        &self,
        write: &ResumeAggregateWrite,
        bindings: &[GeneratedReservation],
        expected_profile_revision: Revision,
        expected_job_revision: Revision,
        generation_claims: Option<&[IntentReservation]>,
    ) -> Result<ReplaceResult, AppError>;
}

/// A normalized positive-year ISO date of already admitted content.
fn parse_iso_date(text: &str) -> Result<Date, ApplicationError> {
    let number = |range: std::ops::Range<usize>| {
        text.get(range)
            .and_then(|part| part.parse::<u32>().ok())
            .ok_or_else(invalid)
    };
    let year = i32::try_from(number(0..4)?).map_err(|_| invalid())?;
    let month = Month::try_from(u8::try_from(number(5..7)?).map_err(|_| invalid())?)
        .map_err(|_| invalid())?;
    let day = u8::try_from(number(8..10)?).map_err(|_| invalid())?;
    Date::from_calendar_date(year, month, day).map_err(|_| invalid())
}

fn optional_date(text: Option<&str>) -> Result<Option<Date>, ApplicationError> {
    text.map(parse_iso_date).transpose()
}

/// One source row of a section before identities are allocated.
struct RowSpec<'a> {
    entry_type: EntryType,
    title: Option<&'a str>,
    organization: Option<&'a str>,
    location: Option<&'a str>,
    start_date: Option<&'a str>,
    end_date: Option<&'a str>,
    metadata: Option<&'a str>,
    bullets: &'a [String],
}

impl<'a> RowSpec<'a> {
    const fn plain(entry_type: EntryType, title: Option<&'a str>) -> Self {
        Self {
            entry_type,
            title,
            organization: None,
            location: None,
            start_date: None,
            end_date: None,
            metadata: None,
            bullets: &[],
        }
    }
}

/// Maps admitted content to the storage rows of one variant.
///
/// The content budget is checked first, then the variant, then identities are allocated in the
/// source order: the section id, then for each row its entry id and its bullet ids in order. The
/// variant language is kept as supplied; the content language only selects the section titles.
/// Sections follow the fixed order personal (always), summary (when present), experience,
/// additional, education, skills and languages (each only with rows); positions restart at zero
/// in every parent.
///
/// # Errors
///
/// The preparation budget error, or the fixed validation error.
pub fn to_variant_write(
    variant: ResumeVariant,
    content: &StructuredResumeContent,
    uuid_factory: &mut dyn FnMut() -> Uuid,
) -> Result<VariantWrite, ApplicationError> {
    budget_resume_content(content)?;
    variant.retained_units()?;
    let language = if content.language() == "de" {
        ResumeLanguage::German
    } else {
        ResumeLanguage::English
    };
    let mut sections: Vec<SectionWrite> = Vec::new();
    let mut add_section = |kind: SectionType, rows: Vec<RowSpec<'_>>| {
        let order = i64::try_from(sections.len()).map_err(|_| invalid())?;
        let section = ResumeSection::new(SectionFields {
            id: uuid_factory(),
            variant_id: variant.id,
            section_type: kind.as_str(),
            title: kind.title(language),
            display_order: DisplayOrder::new(order).ok_or_else(invalid)?,
        })?;
        let mut entries = Vec::with_capacity(rows.len());
        for (position, row) in rows.iter().enumerate() {
            let entry = ResumeEntry::new(EntryFields {
                id: uuid_factory(),
                section_id: section.id,
                entry_type: row.entry_type.as_str(),
                display_order: position_order(position)?,
                title: row.title,
                organization: row.organization,
                location: row.location,
                start_date: optional_date(row.start_date)?,
                end_date: optional_date(row.end_date)?,
                metadata: row.metadata,
            })?;
            let mut bullets = Vec::with_capacity(row.bullets.len());
            for (index, text) in row.bullets.iter().enumerate() {
                bullets.push(ResumeEntryBullet::new(BulletFields {
                    id: uuid_factory(),
                    entry_id: entry.id,
                    display_order: position_order(index)?,
                    text,
                })?);
            }
            entries.push(EntryWrite::new(entry, bullets)?);
        }
        sections.push(SectionWrite::new(section, entries)?);
        Ok::<(), ApplicationError>(())
    };

    add_section(
        SectionType::Personal,
        content
            .personal_fields()
            .iter()
            .map(|row| RowSpec {
                metadata: Some(row.value()),
                ..RowSpec::plain(EntryType::PersonalField, Some(row.label()))
            })
            .collect(),
    )?;
    if let Some(summary) = content.summary() {
        let title = SectionType::Summary.title(language);
        add_section(
            SectionType::Summary,
            vec![RowSpec {
                metadata: Some(summary),
                ..RowSpec::plain(EntryType::Summary, Some(title))
            }],
        )?;
    }
    if !content.experiences().is_empty() {
        add_section(
            SectionType::Experience,
            content
                .experiences()
                .iter()
                .map(|row| RowSpec {
                    organization: Some(row.company()),
                    location: row.location(),
                    start_date: row.start_date(),
                    end_date: row.end_date(),
                    bullets: row.bullets(),
                    ..RowSpec::plain(EntryType::Experience, Some(row.title()))
                })
                .collect(),
        )?;
    }
    if !content.additional().is_empty() {
        add_section(
            SectionType::Additional,
            content
                .additional()
                .iter()
                .map(|row| RowSpec {
                    organization: row.organization(),
                    bullets: row.bullets(),
                    ..RowSpec::plain(EntryType::Project, Some(row.title()))
                })
                .collect(),
        )?;
    }
    if !content.education().is_empty() {
        add_section(
            SectionType::Education,
            content
                .education()
                .iter()
                .map(|row| RowSpec {
                    organization: Some(row.institution()),
                    location: row.location(),
                    start_date: row.start_date(),
                    end_date: row.end_date(),
                    bullets: row.bullets(),
                    ..RowSpec::plain(EntryType::Education, Some(row.degree()))
                })
                .collect(),
        )?;
    }
    let joined: Vec<String> = content
        .skill_groups()
        .iter()
        .map(|row| row.skills().join(", "))
        .collect();
    if !content.skill_groups().is_empty() {
        add_section(
            SectionType::Skills,
            content
                .skill_groups()
                .iter()
                .zip(&joined)
                .map(|(row, skills)| RowSpec {
                    metadata: Some(skills),
                    ..RowSpec::plain(EntryType::SkillGroup, Some(row.category()))
                })
                .collect(),
        )?;
    }
    if !content.languages().is_empty() {
        add_section(
            SectionType::Languages,
            content
                .languages()
                .iter()
                .map(|row| RowSpec {
                    metadata: row.proficiency(),
                    ..RowSpec::plain(EntryType::Language, Some(row.language()))
                })
                .collect(),
        )?;
    }

    let mut result = VariantWrite::new(variant, sections)?;
    // Input entries were admitted before mapping; the storage aggregate counts physical rows,
    // not source skills collapsed into metadata.
    let units = result.raw.0.units.max(content.raw_units());
    let entries = result.raw.0.entries;
    let mut budget = new_budget();
    budget.add_units(units)?;
    budget.add_entries(entries)?;
    result.raw = RawTotals(Totals { units, entries });
    Ok(result)
}

fn position_order(position: usize) -> Result<DisplayOrder, ApplicationError> {
    i64::try_from(position)
        .ok()
        .and_then(DisplayOrder::new)
        .ok_or_else(invalid)
}
