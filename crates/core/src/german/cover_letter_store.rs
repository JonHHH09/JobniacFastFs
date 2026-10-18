//! Source-shaped German cover-letter writes, bounded admission and the pure content mapping.
//! It holds no SQL: the SQLite
//! adapter implements [`GermanCoverLetterRepository`] in `jobniac_infra`.
//!
//! The aggregate references its resume by id and revision instant; the numeric owner revisions of
//! the profile and the job are the compare-and-set tokens of the replace call and stay separate
//! from the stored instants. Every refusal is the fixed `Invalid German cover-letter persistence
//! input` validation error or the fixed `Cover letter persistence limit exceeded` one.
//!
//! Wrapper-tampering and wrong-type refusals have no counterpart: rows and
//! writes are immutable behind checked constructors and every field is typed (see also
//! [`super::resume_store`] for the surrogate rule).

use std::collections::HashSet;

use async_trait::async_trait;
use jobniac_text::java::{
    MAX_PREPARATION_ENTRIES, MAX_PREPARATION_UNITS, java_strip, utf16_length,
};
use time::{OffsetDateTime, UtcOffset};
use uuid::Uuid;

pub use super::resume_store::{DisplayOrder, PairKey, Revision};
use super::resume_store::{RawTotals, RawUnits, Totals, accessors};

use super::content::GermanCoverLetterContent;
use crate::errors::{AppError, ApplicationError, validation_error};
use crate::generated_assets::{GeneratedReservation, IntentReservation};

/// The fixed message of every refused cover-letter persistence input.
const INVALID_MESSAGE: &str = "Invalid German cover-letter persistence input";

/// The fixed message of every exceeded cover-letter persistence limit.
const LIMIT_MESSAGE: &str = "Cover letter persistence limit exceeded";

/// The refusal of every malformed cover-letter persistence input.
#[must_use]
pub fn invalid_cover_letter_input() -> ApplicationError {
    validation_error(INVALID_MESSAGE, None, None)
}

/// The refusal of an exceeded unit or row limit.
#[must_use]
pub fn cover_letter_limit_exceeded() -> ApplicationError {
    validation_error(LIMIT_MESSAGE, None, None)
}

fn invalid() -> ApplicationError {
    invalid_cover_letter_input()
}

fn limit() -> ApplicationError {
    cover_letter_limit_exceeded()
}

/// Admits a numeric owner revision precondition.
///
/// # Errors
///
/// The fixed validation error for a value outside `0..=i64::MAX`.
pub fn validate_revision(value: i128) -> Result<Revision, ApplicationError> {
    Revision::from_wide(value).ok_or_else(invalid)
}

/// Checks a unit and row total against the safety limits.
fn bounded(units: usize, entries: usize) -> Result<(), ApplicationError> {
    if units > MAX_PREPARATION_UNITS || entries > MAX_PREPARATION_ENTRIES {
        return Err(limit());
    }
    Ok(())
}

/// The UTF-16 units of `value` while `remaining` units are left.
fn text_units(value: &str, remaining: usize) -> Result<usize, ApplicationError> {
    // Reject before encoding if even the scalar count cannot fit.
    if value.chars().count() > remaining {
        return Err(limit());
    }
    let units = utf16_length(value);
    if units > remaining {
        return Err(limit());
    }
    Ok(units)
}

/// The summed units of `texts`, each bounded by what the earlier ones left.
fn sum_units(texts: &[&str]) -> Result<usize, ApplicationError> {
    let mut units = 0;
    for text in texts {
        units += text_units(text, MAX_PREPARATION_UNITS - units)?;
    }
    Ok(units)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextKind {
    Required,
    Format,
    Language,
}

/// Normalizes a stored text: NUL refused, Java strip, lowercase for the format
/// and language kinds, empty refused, format `germany`, language `de` (German only).
fn normalize(value: &str, kind: TextKind) -> Result<String, ApplicationError> {
    if value.contains('\0') {
        return Err(invalid());
    }
    let stripped = java_strip(value);
    let result = match kind {
        TextKind::Required => stripped.to_owned(),
        TextKind::Format | TextKind::Language => stripped.to_lowercase(),
    };
    if result.is_empty()
        || (kind == TextKind::Format && result != "germany")
        || (kind == TextKind::Language && result != "de")
    {
        return Err(invalid());
    }
    Ok(result)
}

/// Normalizes an aware instant to UTC.
fn to_utc(value: OffsetDateTime) -> Result<OffsetDateTime, ApplicationError> {
    value.checked_to_offset(UtcOffset::UTC).ok_or_else(invalid)
}

/// The units a stored row counts: the stored texts or the retained raw total, whichever is
/// larger, bounded as one row.
fn row_units(texts: &[&str], raw: RawUnits) -> Result<usize, ApplicationError> {
    let units = sum_units(texts)?.max(raw.0);
    bounded(units, 1)?;
    Ok(units)
}

/// Raw inputs of a [`CoverLetter`].
#[derive(Debug, Clone, Copy)]
pub struct CoverLetterFields {
    /// Cover letter id.
    pub id: Uuid,
    /// Owning profile.
    pub profile_id: Uuid,
    /// Owning job.
    pub job_id: Uuid,
    /// The resume the letter is written against.
    pub resume_id: Uuid,
    /// Profile revision instant the letter was built from.
    pub profile_revision: OffsetDateTime,
    /// Job revision instant the letter was built from.
    pub job_revision: OffsetDateTime,
    /// Revision instant of the resume the letter was built from.
    pub resume_revision: OffsetDateTime,
    /// Creation instant.
    pub created_at: OffsetDateTime,
    /// Last write instant; never before `created_at`.
    pub updated_at: OffsetDateTime,
}

/// The German cover-letter parent row. Instants are UTC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverLetter {
    id: Uuid,
    profile_id: Uuid,
    job_id: Uuid,
    resume_id: Uuid,
    profile_revision: OffsetDateTime,
    job_revision: OffsetDateTime,
    resume_revision: OffsetDateTime,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    raw: RawUnits,
}

impl CoverLetter {
    /// Admits and normalizes a cover-letter row.
    ///
    /// # Errors
    ///
    /// The fixed validation error (also for `updated_at` before `created_at`).
    pub fn new(fields: CoverLetterFields) -> Result<Self, ApplicationError> {
        let row = Self {
            id: fields.id,
            profile_id: fields.profile_id,
            job_id: fields.job_id,
            resume_id: fields.resume_id,
            profile_revision: to_utc(fields.profile_revision)?,
            job_revision: to_utc(fields.job_revision)?,
            resume_revision: to_utc(fields.resume_revision)?,
            created_at: to_utc(fields.created_at)?,
            updated_at: to_utc(fields.updated_at)?,
            raw: RawUnits(0),
        };
        if row.updated_at < row.created_at {
            return Err(invalid());
        }
        Ok(row)
    }

    accessors! {
        raw;
        copy id: Uuid;
        copy profile_id: Uuid;
        copy job_id: Uuid;
        copy resume_id: Uuid;
        copy profile_revision: OffsetDateTime;
        copy job_revision: OffsetDateTime;
        copy resume_revision: OffsetDateTime;
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

    fn units(&self) -> Result<usize, ApplicationError> {
        row_units(&[], self.raw)
    }
}

/// Raw inputs of a [`CoverLetterVariant`].
#[derive(Debug, Clone, Copy)]
pub struct VariantFields<'a> {
    /// Variant id.
    pub id: Uuid,
    /// Owning cover letter.
    pub cover_letter_id: Uuid,
    /// Raw format; must normalize to `germany`.
    pub format: &'a str,
    /// Raw language; must normalize to `de`.
    pub language: &'a str,
    /// The document that owns the PDF.
    pub document_id: Uuid,
    /// Relative file path of the generated PDF.
    pub file_path: &'a str,
    /// Subject line.
    pub subject: &'a str,
    /// Salutation line.
    pub salutation: &'a str,
    /// Closing line.
    pub closing: &'a str,
    /// Signature line.
    pub signature: &'a str,
    /// Creation instant.
    pub created_at: OffsetDateTime,
    /// Last write instant; never before `created_at`.
    pub updated_at: OffsetDateTime,
}

/// The German variant of a cover letter with its PDF document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverLetterVariant {
    id: Uuid,
    cover_letter_id: Uuid,
    format: String,
    language: String,
    document_id: Uuid,
    file_path: String,
    subject: String,
    salutation: String,
    closing: String,
    signature: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    raw: RawUnits,
}

impl CoverLetterVariant {
    /// Admits and normalizes a variant row.
    ///
    /// # Errors
    ///
    /// The fixed validation error or the fixed limit error.
    pub fn new(fields: VariantFields<'_>) -> Result<Self, ApplicationError> {
        let units = sum_units(&[
            fields.format,
            fields.language,
            fields.file_path,
            fields.subject,
            fields.salutation,
            fields.closing,
            fields.signature,
        ])?;
        bounded(units, 1)?;
        let row = Self {
            id: fields.id,
            cover_letter_id: fields.cover_letter_id,
            format: normalize(fields.format, TextKind::Format)?,
            language: normalize(fields.language, TextKind::Language)?,
            document_id: fields.document_id,
            file_path: normalize(fields.file_path, TextKind::Required)?,
            subject: normalize(fields.subject, TextKind::Required)?,
            salutation: normalize(fields.salutation, TextKind::Required)?,
            closing: normalize(fields.closing, TextKind::Required)?,
            signature: normalize(fields.signature, TextKind::Required)?,
            created_at: to_utc(fields.created_at)?,
            updated_at: to_utc(fields.updated_at)?,
            raw: RawUnits(units),
        };
        if row.updated_at < row.created_at {
            return Err(invalid());
        }
        Ok(row)
    }

    accessors! {
        raw;
        copy id: Uuid;
        copy cover_letter_id: Uuid;
        str format;
        str language;
        copy document_id: Uuid;
        str file_path;
        str subject;
        str salutation;
        str closing;
        str signature;
        copy created_at: OffsetDateTime;
        copy updated_at: OffsetDateTime;
    }

    fn units(&self) -> Result<usize, ApplicationError> {
        row_units(
            &[
                &self.format,
                &self.language,
                &self.file_path,
                &self.subject,
                &self.salutation,
                &self.closing,
                &self.signature,
            ],
            self.raw,
        )
    }
}

/// Raw inputs of a [`CoverLetterParagraph`].
#[derive(Debug, Clone, Copy)]
pub struct ParagraphFields<'a> {
    /// Paragraph id.
    pub id: Uuid,
    /// Owning variant.
    pub variant_id: Uuid,
    /// Position within the variant; unique per variant, not required to be contiguous.
    pub display_order: DisplayOrder,
    /// Paragraph text.
    pub text: &'a str,
}

/// A paragraph of a variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverLetterParagraph {
    id: Uuid,
    variant_id: Uuid,
    display_order: DisplayOrder,
    text: String,
    raw: RawUnits,
}

impl CoverLetterParagraph {
    /// Admits and normalizes a paragraph row.
    ///
    /// # Errors
    ///
    /// The fixed validation error or the fixed limit error.
    pub fn new(fields: ParagraphFields<'_>) -> Result<Self, ApplicationError> {
        let units = sum_units(&[fields.text])?;
        bounded(units, 1)?;
        Ok(Self {
            id: fields.id,
            variant_id: fields.variant_id,
            display_order: fields.display_order,
            text: normalize(fields.text, TextKind::Required)?,
            raw: RawUnits(units),
        })
    }

    accessors! {
        raw;
        copy id: Uuid;
        copy variant_id: Uuid;
        copy display_order: DisplayOrder;
        str text;
    }

    fn units(&self) -> Result<usize, ApplicationError> {
        row_units(&[&self.text], self.raw)
    }
}

/// A variant with its paragraphs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantWrite {
    variant: CoverLetterVariant,
    paragraphs: Vec<CoverLetterParagraph>,
    raw: RawTotals,
}

impl VariantWrite {
    /// Admits a variant and its paragraphs (zero paragraphs are valid).
    ///
    /// # Errors
    ///
    /// The fixed limit error.
    pub fn new(
        variant: CoverLetterVariant,
        paragraphs: Vec<CoverLetterParagraph>,
    ) -> Result<Self, ApplicationError> {
        let raw = Self::totals(&variant, &paragraphs, None)?;
        Ok(Self {
            variant,
            paragraphs,
            raw: RawTotals(raw),
        })
    }

    /// Rebuilds a variant write around `variant` and `paragraphs` and keeps the admitted raw totals
    /// of `original`: each total is the larger of the original and the
    /// recomputed one.
    ///
    /// # Errors
    ///
    /// The fixed limit error, or the fixed validation error.
    pub(crate) fn retained(
        variant: CoverLetterVariant,
        paragraphs: Vec<CoverLetterParagraph>,
        original: &Self,
    ) -> Result<Self, ApplicationError> {
        let raw = Self::totals(&variant, &paragraphs, Some(original.raw.0))?;
        Ok(Self {
            variant,
            paragraphs,
            raw: RawTotals(raw),
        })
    }

    /// The totals of a variant write; `stored` is the retained total of an already
    /// admitted wrapper.
    fn totals(
        variant: &CoverLetterVariant,
        paragraphs: &[CoverLetterParagraph],
        stored: Option<Totals>,
    ) -> Result<Totals, ApplicationError> {
        let mut entries = 1 + paragraphs.len();
        // Before touching the members.
        bounded(0, entries)?;
        let mut units = variant.units()?;
        for paragraph in paragraphs {
            units += paragraph.units()?;
            bounded(units, entries)?;
        }
        if let Some(stored) = stored {
            if stored.entries < entries {
                return Err(invalid());
            }
            units = units.max(stored.units);
            entries = entries.max(stored.entries);
        }
        bounded(units, entries)?;
        Ok(Totals { units, entries })
    }

    fn recompute(&self) -> Result<Totals, ApplicationError> {
        Self::totals(&self.variant, &self.paragraphs, Some(self.raw.0))
    }

    /// The variant row.
    #[must_use]
    pub const fn variant(&self) -> &CoverLetterVariant {
        &self.variant
    }

    /// The paragraphs in stored order.
    #[must_use]
    pub fn paragraphs(&self) -> &[CoverLetterParagraph] {
        &self.paragraphs
    }

    /// The physical row count of the subtree.
    #[must_use]
    pub const fn raw_entries(&self) -> usize {
        self.raw.0.entries
    }

    /// The admitted UTF-16 unit total.
    #[must_use]
    pub const fn raw_units(&self) -> usize {
        self.raw.0.units
    }
}

/// The full cover-letter aggregate: the parent and its single variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverLetterAggregateWrite {
    cover_letter: CoverLetter,
    variant: VariantWrite,
    raw: RawTotals,
}

impl CoverLetterAggregateWrite {
    /// Admits a cover letter and its variant write.
    ///
    /// # Errors
    ///
    /// The fixed limit error.
    pub fn new(cover_letter: CoverLetter, variant: VariantWrite) -> Result<Self, ApplicationError> {
        let raw = Self::totals(&cover_letter, &variant, None)?;
        Ok(Self {
            cover_letter,
            variant,
            raw: RawTotals(raw),
        })
    }

    /// Rebuilds the aggregate around `cover_letter` and `variant` and keeps the admitted raw totals
    /// of `original`: each total is the larger of the original and the
    /// recomputed one.
    ///
    /// # Errors
    ///
    /// The fixed limit error, or the fixed validation error.
    pub(crate) fn retained(
        cover_letter: CoverLetter,
        variant: VariantWrite,
        original: &Self,
    ) -> Result<Self, ApplicationError> {
        let raw = Self::totals(&cover_letter, &variant, Some(original.raw.0))?;
        Ok(Self {
            cover_letter,
            variant,
            raw: RawTotals(raw),
        })
    }

    fn totals(
        cover_letter: &CoverLetter,
        variant: &VariantWrite,
        stored: Option<Totals>,
    ) -> Result<Totals, ApplicationError> {
        let child = variant.recompute()?;
        let mut units = cover_letter.units()? + child.units;
        let mut entries = 1 + child.entries;
        if let Some(stored) = stored {
            if stored.entries < entries {
                return Err(invalid());
            }
            units = units.max(stored.units);
            entries = entries.max(stored.entries);
        }
        bounded(units, entries)?;
        Ok(Totals { units, entries })
    }

    /// The parent row.
    #[must_use]
    pub const fn cover_letter(&self) -> &CoverLetter {
        &self.cover_letter
    }

    /// The variant write.
    #[must_use]
    pub const fn variant(&self) -> &VariantWrite {
        &self.variant
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
        self.cover_letter.pair_key()
    }
}

/// Re-admits the whole tree, then checks parentage and paragraph identity and position
/// uniqueness. Positions need not be contiguous.
///
/// # Errors
///
/// The fixed validation error or the fixed limit error.
pub fn validate_cover_letter_aggregate(
    write: &CoverLetterAggregateWrite,
) -> Result<(), ApplicationError> {
    // Admit the entire physical tree first.
    CoverLetterAggregateWrite::totals(&write.cover_letter, &write.variant, Some(write.raw.0))?;
    let variant = &write.variant.variant;
    if variant.cover_letter_id != write.cover_letter.id {
        return Err(invalid());
    }
    let mut ids = HashSet::new();
    let mut positions = HashSet::new();
    for paragraph in &write.variant.paragraphs {
        if paragraph.variant_id != variant.id
            || !ids.insert(paragraph.id)
            || !positions.insert(paragraph.display_order)
        {
            return Err(invalid());
        }
    }
    Ok(())
}

/// The stored result of replacing a cover letter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceResult {
    /// The stored parent.
    pub cover_letter: CoverLetter,
    /// The variant as stored.
    pub variant: CoverLetterVariant,
    /// The variants the replacement removed.
    pub previous_variants: Vec<CoverLetterVariant>,
}

/// Persistence port of the German cover letter. The
/// SQLite adapter is `jobniac_infra::sqlite::german`; the replace transaction runs on the single
/// writer ([`PairKey::lock_name`] names the pair), then writes the rows in the fixed order.
#[async_trait]
pub trait GermanCoverLetterRepository: Send + Sync {
    /// Reads a cover letter by id.
    async fn find_by_id(&self, cover_letter_id: Uuid) -> Result<Option<CoverLetter>, AppError>;

    /// Reads the cover letter of one profile, job and resume.
    async fn find_by_profile_job_resume(
        &self,
        profile_id: Uuid,
        job_id: Uuid,
        resume_id: Uuid,
    ) -> Result<Option<CoverLetter>, AppError>;

    /// Reads the variants of a cover letter.
    async fn find_variants(
        &self,
        cover_letter_id: Uuid,
    ) -> Result<Vec<CoverLetterVariant>, AppError>;

    /// Reads the paragraphs of a variant in stored order.
    async fn find_paragraphs(
        &self,
        variant_id: Uuid,
    ) -> Result<Vec<CoverLetterParagraph>, AppError>;

    /// Replaces the cover-letter aggregate under the owner revision preconditions. `binding` is
    /// the reserved asset of the variant and `generation_claim`, when present, its pending intent
    /// (the unclassified path passes `None`).
    async fn replace_german_cover_letter(
        &self,
        write: &CoverLetterAggregateWrite,
        binding: &GeneratedReservation,
        expected_profile_revision: Revision,
        expected_job_revision: Revision,
        generation_claim: Option<&IntentReservation>,
    ) -> Result<ReplaceResult, AppError>;
}

/// Re-admits every source text of the content and returns its units.
/// The retained raw totals of the content are counted as received, so padded input keeps its
/// raw size; the stored budget is also checked through [`GermanCoverLetterContent::budget`].
fn content_units(content: &GermanCoverLetterContent) -> Result<usize, ApplicationError> {
    let retained_units = content.raw_units();
    let retained_entries = content.raw_entries();
    bounded(retained_units, retained_entries)?;
    content.budget().map_err(|_| limit())?;
    let entries = content.personal_fields().len() + content.paragraphs().len();
    bounded(0, entries)?;
    if retained_entries < entries {
        return Err(invalid());
    }
    let mut units = 0;
    let required = [
        content.sender_name(),
        content.job_title(),
        content.subject(),
        content.salutation(),
        content.closing(),
        content.signature(),
    ];
    for text in required
        .into_iter()
        .chain(content.recipient_company())
        .chain(content.recipient_location())
    {
        units += text_units(text, MAX_PREPARATION_UNITS - units)?;
    }
    for personal in content.personal_fields() {
        if personal.raw_entries() != 1 {
            return Err(invalid());
        }
        let mut current = text_units(personal.label(), MAX_PREPARATION_UNITS - units)?;
        current += text_units(personal.value(), MAX_PREPARATION_UNITS - units - current)?;
        units += current.max(personal.raw_units());
        bounded(units, entries)?;
    }
    for text in content.paragraphs() {
        units += text_units(text, MAX_PREPARATION_UNITS - units)?;
    }
    units = units.max(retained_units);
    bounded(units, entries)?;
    // Content construction already applied Java strip, so every source text must be canonical.
    let canonical = |text: &str| {
        if java_strip(text) != text {
            return Err(invalid());
        }
        normalize(text, TextKind::Required).map(|_| ())
    };
    for text in required
        .into_iter()
        .chain(content.recipient_company())
        .chain(content.recipient_location())
    {
        canonical(text)?;
    }
    for personal in content.personal_fields() {
        canonical(personal.label())?;
        canonical(personal.value())?;
    }
    for text in content.paragraphs() {
        canonical(text)?;
    }
    Ok(units)
}

/// Maps admitted content to the storage rows of one cover letter.
///
/// The content and the output are admitted, and the variant fields must equal the content, before
/// any paragraph identity is allocated. Paragraphs keep the content order with positions
/// `0..n`, and `id_factory` is called once per paragraph in order. Zero paragraphs are valid.
///
/// # Errors
///
/// The fixed validation error or the fixed limit error.
pub fn map_cover_letter_content(
    parent: CoverLetter,
    variant: CoverLetterVariant,
    content: &GermanCoverLetterContent,
    id_factory: &mut dyn FnMut() -> Uuid,
) -> Result<CoverLetterAggregateWrite, ApplicationError> {
    let content_units = content_units(content)?;
    // Physical output has one parent, one variant and one record per paragraph.
    let entries = 2 + content.paragraphs().len();
    let mut units = parent.units()? + variant.units()?;
    for text in content.paragraphs() {
        units += text_units(text, MAX_PREPARATION_UNITS)?;
    }
    bounded(units.max(content_units), entries)?;
    if variant.cover_letter_id != parent.id {
        return Err(invalid());
    }
    let pairs = [
        (&variant.subject, content.subject()),
        (&variant.salutation, content.salutation()),
        (&variant.closing, content.closing()),
        (&variant.signature, content.signature()),
    ];
    for (stored, source) in pairs {
        if *stored != normalize(source, TextKind::Required)? {
            return Err(invalid());
        }
    }
    // Admit and normalize all paragraphs before allocating any generated identity.
    let texts = content
        .paragraphs()
        .iter()
        .map(|text| normalize(text, TextKind::Required))
        .collect::<Result<Vec<_>, _>>()?;
    let mut paragraphs = Vec::with_capacity(texts.len());
    for (order, text) in texts.iter().enumerate() {
        let order = i64::try_from(order)
            .ok()
            .and_then(DisplayOrder::new)
            .ok_or_else(invalid)?;
        paragraphs.push(CoverLetterParagraph::new(ParagraphFields {
            id: id_factory(),
            variant_id: variant.id,
            display_order: order,
            text,
        })?);
    }
    let mut result =
        CoverLetterAggregateWrite::new(parent, VariantWrite::new(variant, paragraphs)?)?;
    result.raw.0.units = result.raw.0.units.max(content_units).max(units);
    validate_cover_letter_aggregate(&result)?;
    Ok(result)
}
