//! German generation orchestration.
//!
//! One call prepares the resume (English and German) or the cover letter from the profile, the job
//! and, for the resume, the optional personal details. The content is built, budgeted and reviewed
//! before any render or asset work, rendered through the [`GermanPdfRenderer`] port, reserved and
//! materialized as classified generated assets, and written through the persistence ports with one
//! compare-and-swap on the numeric profile and job revisions.
//!
//! Every failure after the first reservation compensates every known reservation. A cleanup
//! failure never replaces the original failure; it only clears `compensation_confirmed`. Only an
//! active materialization is linked, and the success value never carries bytes or source rows.
//!
//! Reads and writes sit behind ports. Checks against a port that mutates the
//! descriptor, a binding or a claim after the call are unnecessary: every value is
//! passed by shared reference or cloned, and the proof types are immutable.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use jobniac_pdf::PdfError;
use jobniac_pdf::render::german::{render_cover_letter_pdf, render_resume_pdf};
use serde_json::Value;
use time::{Date, OffsetDateTime, UtcOffset};
use uuid::Uuid;

use super::content::{GermanCoverLetterContent, StructuredResumeContent};
use super::cover_letter::{build_german_cover_letter, review_cover_letter_content};
use super::cover_letter_store::{
    CoverLetter, CoverLetterAggregateWrite, CoverLetterFields, CoverLetterVariant,
    GermanCoverLetterRepository, ReplaceResult as LetterReplaceResult,
    VariantFields as LetterVariantFields, VariantWrite as LetterVariantWrite,
    map_cover_letter_content, validate_cover_letter_aggregate,
};
use super::resume::{PersonalDetails, prepare_german_resume};
use super::resume_store::{
    GermanResumeRepository, ReplaceResult as ResumeReplaceResult, Resume, ResumeAggregateWrite,
    ResumeFields, ResumeVariant, Revision, VariantFields, VariantWrite, to_variant_write,
    validate_resume_aggregate,
};
use super::translation::GermanResumeTranslator;
use crate::clock::Clock;
use crate::errors::{AppError, ApplicationError, ErrorCode, not_found, validation_error};
use crate::generated_assets::service::sha256_hex;
use crate::generated_assets::{
    GeneratedAssetService, GeneratedReservation, GenerationDescriptor, GenerationPurpose,
    GenerationState, IntentReservation, MaterializationOutcome, PreparationOutcome,
    validate_generated_pdf,
};
use crate::generation::content::default_sentence_boundaries;
use crate::generation::generic::{GenerationFailure, ProfileGraphPort, application_error};
use crate::jobs::model::JobAggregate;
use crate::profiles::personal_details::PersonalDetailsEnvelope;

/// The failure of a German generation: the caller-safe error and whether every known reservation
/// was confirmed retired.
pub type GermanGenerationFailure = GenerationFailure;

/// Message of the failed resume render.
const RESUME_RENDER_MESSAGE: &str = "German resume PDF could not be generated";
/// Message of the failed cover-letter render.
const LETTER_RENDER_MESSAGE: &str = "German cover-letter PDF could not be generated";
/// Message of a source row that does not match the identity that was asked for.
const SNAPSHOT_MESSAGE: &str = "Invalid German generation source snapshot";
/// Message of a profile that changed between the profile and the personal-details reads.
const REVISION_CONFLICT_MESSAGE: &str = "German generation profile revision conflict";
/// The most previous variants a replacement may report.
const MAX_PREVIOUS: usize = 10_000;
/// The most raw units a returned row may retain.
const MAX_RETURNED_UNITS: usize = 1_000_000;

/// Read port of the job aggregate. `None` when it is missing.
#[async_trait]
pub trait JobAggregatePort: Send + Sync {
    /// The complete aggregate of `job_id`, or `None` when it is missing.
    async fn find_job(&self, job_id: Uuid) -> Result<Option<JobAggregate>, AppError>;
}

/// Read port of the personal-details envelope. `None` when
/// the profile is missing.
#[async_trait]
pub trait PersonalDetailsPort: Send + Sync {
    /// The envelope of `profile_id`, or `None` when the profile is missing.
    async fn find_personal_details(
        &self,
        profile_id: Uuid,
    ) -> Result<Option<PersonalDetailsEnvelope>, AppError>;
}

/// The PDF a renderer produced, before it is admitted. The size and
/// the page count are the renderer's claims and are checked against the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedGermanPdf {
    /// The PDF file bytes.
    pub data: Vec<u8>,
    /// The size the renderer reports.
    pub byte_size: usize,
    /// The page count the renderer reports.
    pub page_count: usize,
}

/// Render port of the German documents.
pub trait GermanPdfRenderer: Send + Sync {
    /// Renders resume content.
    ///
    /// # Errors
    ///
    /// The caller-safe error of a refused layout or a failed render.
    fn render_resume(
        &self,
        content: &StructuredResumeContent,
    ) -> Result<RenderedGermanPdf, ApplicationError>;

    /// Renders cover-letter content.
    ///
    /// # Errors
    ///
    /// The caller-safe error of a refused layout or a failed render.
    fn render_cover_letter(
        &self,
        content: &GermanCoverLetterContent,
    ) -> Result<RenderedGermanPdf, ApplicationError>;
}

/// The renderer of `jobniac-pdf`: intentional validation errors keep their envelope and every
/// other failure becomes the fixed internal message of the document.
#[derive(Debug, Clone, Copy, Default)]
pub struct PdfGermanRenderer;

fn render_error(error: PdfError, message: &str) -> ApplicationError {
    match error {
        PdfError::Validation {
            message,
            field,
            reason,
        } => validation_error(message, field, reason),
        PdfError::Internal => ApplicationError::with_message(ErrorCode::InternalError, message),
    }
}

impl GermanPdfRenderer for PdfGermanRenderer {
    fn render_resume(
        &self,
        content: &StructuredResumeContent,
    ) -> Result<RenderedGermanPdf, ApplicationError> {
        let rendered = render_resume_pdf(&content.to_layout_resume())
            .map_err(|error| render_error(error, RESUME_RENDER_MESSAGE))?;
        Ok(RenderedGermanPdf {
            byte_size: rendered.byte_size(),
            page_count: rendered.page_count,
            data: rendered.data,
        })
    }

    fn render_cover_letter(
        &self,
        content: &GermanCoverLetterContent,
    ) -> Result<RenderedGermanPdf, ApplicationError> {
        let rendered = render_cover_letter_pdf(&content.to_layout_cover_letter())
            .map_err(|error| render_error(error, LETTER_RENDER_MESSAGE))?;
        Ok(RenderedGermanPdf {
            byte_size: rendered.byte_size(),
            page_count: rendered.page_count,
            data: rendered.data,
        })
    }
}

/// One language variant of a generated resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedResumeVariant {
    /// The stored variant row.
    pub variant: ResumeVariant,
    /// The committed reservation of its PDF.
    pub reservation: GeneratedReservation,
    /// The byte size of the PDF.
    pub byte_size: usize,
    /// The page count of the PDF.
    pub page_count: usize,
}

/// The safe result of a generated German resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedResumeResult {
    /// The stored parent.
    pub resume: Resume,
    /// The variants in order: English, then German.
    pub variants: Vec<GeneratedResumeVariant>,
    /// Whether a previous resume was replaced.
    pub replaced_existing: bool,
    /// The generation instant.
    pub generated_at: OffsetDateTime,
}

/// The safe result of a generated German cover letter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCoverLetterResult {
    /// The stored parent.
    pub cover_letter: CoverLetter,
    /// The stored German variant.
    pub variant: CoverLetterVariant,
    /// The committed reservation of its PDF.
    pub reservation: GeneratedReservation,
    /// The byte size of the PDF.
    pub byte_size: usize,
    /// The page count of the PDF.
    pub page_count: usize,
    /// Whether a previous cover letter was replaced.
    pub replaced_existing: bool,
    /// The generation instant.
    pub generated_at: OffsetDateTime,
}

/// The ports of the German generation.
#[derive(Clone)]
pub struct GermanGenerationPorts {
    /// Read port of the profile graph.
    pub profiles: Arc<dyn ProfileGraphPort>,
    /// Read port of the job aggregate.
    pub jobs: Arc<dyn JobAggregatePort>,
    /// Read port of the personal details.
    pub personal_details: Arc<dyn PersonalDetailsPort>,
    /// Persistence port of the German resume.
    pub resumes: Arc<dyn GermanResumeRepository>,
    /// Persistence port of the German cover letter.
    pub cover_letters: Arc<dyn GermanCoverLetterRepository>,
    /// The classified generated-asset lifecycle.
    pub assets: Arc<GeneratedAssetService>,
    /// The PDF renderer.
    pub renderer: Arc<dyn GermanPdfRenderer>,
}

impl std::fmt::Debug for GermanGenerationPorts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GermanGenerationPorts")
            .finish_non_exhaustive()
    }
}

/// The reservations of one operation, kept for compensation.
#[derive(Debug, Default)]
struct Known {
    /// The proven claims, in reservation order.
    claims: Vec<IntentReservation>,
    /// Whether a reservation was dispatched but not yet proven.
    unproven: bool,
}

/// The validated source rows of one request.
struct Sources {
    graph: crate::profiles::model::ProfileGraph,
    job: JobAggregate,
    profile_revision: i64,
    profile_at: OffsetDateTime,
    job_revision: i64,
    job_at: OffsetDateTime,
}

/// German tailored resume and cover-letter generation.
pub struct GermanGenerationService {
    ports: GermanGenerationPorts,
    translator: Arc<GermanResumeTranslator>,
    root_id: Uuid,
    clock: Arc<dyn Clock>,
    enabled: bool,
}

impl std::fmt::Debug for GermanGenerationService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GermanGenerationService")
            .field("root_id", &self.root_id)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

/// The fixed internal error of every unclassified failure.
fn internal() -> ApplicationError {
    ApplicationError::of(ErrorCode::InternalError)
}

/// The refusal of a source row that does not match its identity.
fn invalid_snapshot() -> ApplicationError {
    ApplicationError::with_message(ErrorCode::InternalError, SNAPSHOT_MESSAGE)
}

/// Whether `value` is a canonical hyphenated UUID text (any hex case).
fn is_hyphenated_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

/// Reads a request identity.
fn request_uuid(value: &Value, field: &str, message: &str) -> Result<Uuid, ApplicationError> {
    let invalid = |reason: &str| validation_error(message, Some(field), Some(reason));
    match value {
        Value::Null => Err(invalid("must not be null")),
        Value::String(text) if is_hyphenated_uuid(text) => {
            Uuid::parse_str(text).map_err(|_| invalid("must be a valid UUID"))
        }
        _ => Err(invalid("must be a valid UUID")),
    }
}

/// The `YYYY-MM-DD` text of a date.
fn iso_date(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// The `name-<id>-<language>.pdf` logical file name of a reservation (no path separators).
fn reservation_name(prefix: &str, id: Uuid, language: &str) -> String {
    format!("{prefix}-{id}-{language}.pdf")
}

/// Whether the reservation binds to the root and to the exact bytes.
fn reservation_matches(reservation: &GeneratedReservation, data: &[u8], root_id: Uuid) -> bool {
    reservation.asset.root_id() == root_id
        && reservation.asset.size_bytes() == data.len() as u64
        && reservation.asset.sha256() == sha256_hex(data)
}

/// The checked numeric revision of an admitted source row.
fn revision_of(value: i64) -> Result<Revision, ApplicationError> {
    Revision::new(value).ok_or_else(invalid_snapshot)
}

/// Checks the previous rows of a replacement: bounded, unique, none of them
/// an incoming row, each a valid returned row.
fn check_previous<T>(
    previous: &[T],
    incoming: &HashSet<Uuid>,
    row: impl Fn(&T) -> (Uuid, bool),
) -> Result<(), ApplicationError> {
    if previous.len() > MAX_PREVIOUS {
        return Err(internal());
    }
    let mut seen = HashSet::new();
    for item in previous {
        let (id, valid) = row(item);
        if !valid || !seen.insert(id) || incoming.contains(&id) {
            return Err(internal());
        }
    }
    Ok(())
}

/// Whether a returned resume variant is a valid row.
fn resume_variant_row(variant: &ResumeVariant) -> (Uuid, bool) {
    (
        variant.id(),
        variant.updated_at() >= variant.created_at() && variant.raw_units() <= MAX_RETURNED_UNITS,
    )
}

/// Whether a returned cover-letter variant is a valid row.
fn letter_variant_row(variant: &CoverLetterVariant) -> (Uuid, bool) {
    (
        variant.id(),
        variant.updated_at() >= variant.created_at() && variant.raw_units() <= MAX_RETURNED_UNITS,
    )
}

impl GermanGenerationService {
    /// Builds the service. `enabled` must be the exact opt-in value resolved by the runtime, and
    /// `root_id` the registered root of the asset service.
    #[must_use]
    pub fn new(
        ports: GermanGenerationPorts,
        translator: Arc<GermanResumeTranslator>,
        root_id: Uuid,
        clock: Arc<dyn Clock>,
        enabled: bool,
    ) -> Self {
        Self {
            ports,
            translator,
            root_id,
            clock,
            enabled,
        }
    }

    /// Generates the English and German tailored resume of a profile for a job, as one atomic
    /// bilingual write. `include_projects` is `null` or a JSON boolean; projects are only read
    /// when it is `true`.
    ///
    /// # Errors
    ///
    /// The caller-safe error of the first failing step. `compensation_confirmed` tells whether
    /// every reservation made before the failure was confirmed retired.
    pub async fn generate_german_tailored_resume(
        &self,
        profile_id: &Value,
        job_id: &Value,
        include_projects: &Value,
    ) -> Result<GeneratedResumeResult, GermanGenerationFailure> {
        let mut known = Known::default();
        match self
            .resume_steps(profile_id, job_id, include_projects, &mut known)
            .await
        {
            Ok(result) => Ok(result),
            Err(error) => Err(self.failure(error, &known).await),
        }
    }

    /// Generates the German cover letter of a profile for a job against a selected German resume.
    ///
    /// # Errors
    ///
    /// The caller-safe error of the first failing step, with the compensation status.
    pub async fn generate_german_cover_letter(
        &self,
        profile_id: &Value,
        job_id: &Value,
        resume_id: &Value,
    ) -> Result<GeneratedCoverLetterResult, GermanGenerationFailure> {
        let mut known = Known::default();
        match self
            .letter_steps(profile_id, job_id, resume_id, &mut known)
            .await
        {
            Ok(result) => Ok(result),
            Err(error) => Err(self.failure(error, &known).await),
        }
    }

    /// Compensates every known reservation and wraps the original error.
    async fn failure(&self, error: ApplicationError, known: &Known) -> GermanGenerationFailure {
        GermanGenerationFailure {
            error,
            compensation_confirmed: self.compensate(known).await,
        }
    }

    /// Abandons every known claim. A refused or failed abandonment clears the confirmation but
    /// never stops the others.
    async fn compensate(&self, known: &Known) -> bool {
        let mut confirmed = !known.unproven;
        for claim in &known.claims {
            match self.ports.assets.abandon_generation(claim).await {
                Ok(
                    PreparationOutcome::Prepared
                    | PreparationOutcome::AlreadyRetired
                    | PreparationOutcome::Referenced,
                ) => {}
                Ok(PreparationOutcome::Ineligible) | Err(_) => confirmed = false,
            }
        }
        confirmed
    }

    /// Refuses a disabled service: the flag first, then the classified
    /// lifecycle probe, whose refusal is the generic internal error. Nothing is read or reserved
    /// before either check, and no reservation is known yet, so nothing is compensated.
    fn required(&self) -> Result<(), ApplicationError> {
        if !self.enabled {
            return Err(ApplicationError::with_message(
                ErrorCode::InternalError,
                "German generation is not enabled",
            ));
        }
        self.ports
            .assets
            .ensure_classified_available()
            .map_err(|_| internal())
    }

    /// Reads and checks the profile and the job.
    async fn sources(&self, profile_id: Uuid, job_id: Uuid) -> Result<Sources, ApplicationError> {
        let graph = self
            .ports
            .profiles
            .find_profile(profile_id)
            .await
            .map_err(application_error)?
            .ok_or_else(|| {
                not_found(
                    &format!("Profile not found: {profile_id}"),
                    [
                        ("resource", "profile".to_owned()),
                        ("profileId", profile_id.to_string()),
                    ],
                )
            })?;
        if graph.profile.id != profile_id {
            return Err(invalid_snapshot());
        }
        let profile_revision = revision_of(graph.profile.revision)?.get();
        let profile_at = graph.profile.updated_at;
        let job = self
            .ports
            .jobs
            .find_job(job_id)
            .await
            .map_err(application_error)?
            .ok_or_else(|| {
                not_found(
                    &format!("Job not found: {job_id}"),
                    [
                        ("resource", "job".to_owned()),
                        ("jobId", job_id.to_string()),
                    ],
                )
            })?;
        if job.job.id != job_id {
            return Err(invalid_snapshot());
        }
        let job_revision = revision_of(job.job.revision)?.get();
        let job_at = job.job.updated_at;
        Ok(Sources {
            graph,
            job,
            profile_revision,
            profile_at,
            job_revision,
            job_at,
        })
    }

    /// Reads the personal details and proves they belong to the same profile snapshot. Only the
    /// reviewed fields are returned.
    async fn personal(
        &self,
        profile_id: Uuid,
        revision: i64,
        instant: OffsetDateTime,
    ) -> Result<Option<PersonalDetails>, ApplicationError> {
        let envelope = self
            .ports
            .personal_details
            .find_personal_details(profile_id)
            .await
            .map_err(application_error)?
            .ok_or_else(|| {
                not_found(
                    &format!("Profile not found: {profile_id}"),
                    [
                        ("resource", "profile".to_owned()),
                        ("profileId", profile_id.to_string()),
                    ],
                )
            })?;
        let conflict =
            || ApplicationError::with_message(ErrorCode::Conflict, REVISION_CONFLICT_MESSAGE);
        if envelope.profile_id != profile_id || envelope.profile_revision != revision {
            return Err(conflict());
        }
        if envelope.profile_updated_at != instant {
            return Err(conflict());
        }
        let Some(details) = envelope.personal_details else {
            return Ok(None);
        };
        if details.profile_id != profile_id {
            return Err(invalid_snapshot());
        }
        // Keep only reviewed fields; never forward private metadata to rendering.
        Ok(Some(PersonalDetails {
            date_of_birth: details.date_of_birth.map(iso_date),
            nationality: details.nationality,
        }))
    }

    /// The current instant in UTC.
    fn now(&self) -> OffsetDateTime {
        self.clock.now().to_offset(UtcOffset::UTC)
    }

    /// Admits the rendered PDF, reserves it as a classified asset and materializes it. The claim
    /// is recorded in `known` as soon as it is proven, so that any later
    /// failure abandons it. Returns the claim, the byte size and the page count.
    async fn publish(
        &self,
        rendered: RenderedGermanPdf,
        name: &str,
        known: &mut Known,
        descriptor: &GenerationDescriptor,
    ) -> Result<(IntentReservation, usize, usize), ApplicationError> {
        let RenderedGermanPdf {
            data,
            byte_size,
            page_count,
        } = rendered;
        validate_generated_pdf(&data).map_err(|_| internal())?;
        if byte_size != data.len() || !(1..=2).contains(&page_count) {
            return Err(internal());
        }
        known.unproven = true;
        let binding = self
            .ports
            .assets
            .reserve_classified_pdf(&data, name, descriptor)
            .await
            .map_err(|_| internal())?;
        if !reservation_matches(&binding.reservation, &data, self.root_id)
            || binding.intent.descriptor != *descriptor
            || binding.intent.state != GenerationState::Pending
        {
            return Err(internal());
        }
        let retained = binding.clone();
        known.claims.push(binding.clone());
        known.unproven = false;
        let assets: HashSet<Uuid> = known
            .claims
            .iter()
            .map(|claim| claim.reservation.asset.asset_id())
            .collect();
        let documents: HashSet<Uuid> = known
            .claims
            .iter()
            .map(|claim| claim.reservation.document_id)
            .collect();
        if assets.len() != known.claims.len() || documents.len() != known.claims.len() {
            return Err(internal());
        }
        let active = self
            .ports
            .assets
            .materialize_classified(&binding)
            .await
            .map_err(|_| internal())?;
        if active.claim != retained || active.outcome != MaterializationOutcome::Active {
            return Err(internal());
        }
        if !reservation_matches(&active.claim.reservation, &data, self.root_id) {
            return Err(internal());
        }
        Ok((retained, byte_size, page_count))
    }

    /// The resume steps.
    async fn resume_steps(
        &self,
        profile_id: &Value,
        job_id: &Value,
        include_projects: &Value,
        known: &mut Known,
    ) -> Result<GeneratedResumeResult, ApplicationError> {
        let message = "Invalid German tailored resume request";
        let profile_key = request_uuid(profile_id, "profileId", message)?;
        let job_key = request_uuid(job_id, "jobId", message)?;
        let include = match include_projects {
            Value::Null => false,
            Value::Bool(flag) => *flag,
            _ => {
                return Err(validation_error(
                    message,
                    Some("includeProjects"),
                    Some("must be a boolean"),
                ));
            }
        };
        self.required()?;
        let sources = self.sources(profile_key, job_key).await?;
        let personal = self
            .personal(profile_key, sources.profile_revision, sources.profile_at)
            .await?;
        let (english, german) = prepare_german_resume(
            &sources.graph,
            &sources.job,
            personal.as_ref(),
            include,
            &default_sentence_boundaries,
            &self.translator,
        )?;
        let contents = [english, german];
        let now = self.now();
        let parent = Resume::new(ResumeFields {
            id: Uuid::new_v4(),
            profile_id: profile_key,
            job_id: job_key,
            format: "germany",
            profile_revision: sources.profile_at,
            job_revision: sources.job_at,
            created_at: now,
            updated_at: now,
        })?;
        let mut planned = Vec::with_capacity(contents.len());
        for content in &contents {
            let variant = ResumeVariant::new(VariantFields {
                id: Uuid::new_v4(),
                resume_id: parent.id(),
                language: content.language(),
                document_id: Uuid::new_v4(),
                file_path: &format!("{}.pdf", Uuid::new_v4()),
                created_at: now,
                updated_at: now,
            })?;
            planned.push(to_variant_write(variant, content, &mut || Uuid::new_v4())?);
        }
        let admitted = ResumeAggregateWrite::new(parent.clone(), planned.clone())?;
        validate_resume_aggregate(&admitted)?;
        let operation_id = Uuid::new_v4();
        let mut outputs = Vec::with_capacity(contents.len());
        let mut writes = Vec::with_capacity(contents.len());
        for (content, planned_write) in contents.iter().zip(&planned) {
            let rendered = self.ports.renderer.render_resume(content)?;
            let descriptor = GenerationDescriptor {
                operation_id,
                purpose: GenerationPurpose::GermanResume,
                profile_id: Some(profile_key),
                job_id: Some(job_key),
                resume_id: Some(parent.id()),
                output_scope: Some(content.language().to_owned()),
            };
            let name = reservation_name("resume", parent.id(), content.language());
            let (claim, size, pages) = self.publish(rendered, &name, known, &descriptor).await?;
            let old = planned_write.variant();
            let variant = ResumeVariant::new(VariantFields {
                id: old.id(),
                resume_id: old.resume_id(),
                language: old.language(),
                document_id: claim.reservation.document_id,
                file_path: claim.reservation.asset.relative_path(),
                created_at: old.created_at(),
                updated_at: old.updated_at(),
            })?;
            // Retains the admitted raw budget of the planned write.
            writes.push(VariantWrite::retained(
                variant.clone(),
                planned_write.sections().to_vec(),
                planned_write,
            )?);
            outputs.push(GeneratedResumeVariant {
                variant,
                reservation: claim.reservation,
                byte_size: size,
                page_count: pages,
            });
        }
        // Retains the admitted raw budget of the aggregate.
        let aggregate = ResumeAggregateWrite::retained(parent.clone(), writes, &admitted)?;
        validate_resume_aggregate(&aggregate)?;
        let bindings: Vec<GeneratedReservation> = outputs
            .iter()
            .map(|output| output.reservation.clone())
            .collect();
        let result = self
            .ports
            .resumes
            .replace_germany_resume(
                &aggregate,
                &bindings,
                revision_of(sources.profile_revision)?,
                revision_of(sources.job_revision)?,
                Some(&known.claims),
            )
            .await
            .map_err(application_error)?;
        let expected: Vec<ResumeVariant> = outputs
            .iter()
            .map(|output| output.variant.clone())
            .collect();
        let replaced = check_resume_result(&result, &parent, &expected)?;
        validate_resume_aggregate(&aggregate)?;
        Ok(GeneratedResumeResult {
            resume: parent,
            variants: outputs,
            replaced_existing: replaced,
            generated_at: now,
        })
    }

    /// The cover-letter steps.
    async fn letter_steps(
        &self,
        profile_id: &Value,
        job_id: &Value,
        resume_id: &Value,
        known: &mut Known,
    ) -> Result<GeneratedCoverLetterResult, ApplicationError> {
        let message = "Invalid German cover-letter request";
        let profile_key = request_uuid(profile_id, "profileId", message)?;
        let job_key = request_uuid(job_id, "jobId", message)?;
        let resume_key = request_uuid(resume_id, "resumeId", message)?;
        self.required()?;
        let sources = self.sources(profile_key, job_key).await?;
        let resume = self
            .ports
            .resumes
            .find_by_id(resume_key)
            .await
            .map_err(application_error)?
            .ok_or_else(|| {
                not_found(
                    "Selected resume was not found",
                    [("resumeId", resume_key.to_string())],
                )
            })?;
        if resume.id() != resume_key {
            return Err(invalid_snapshot());
        }
        if resume.profile_id() != profile_key || resume.job_id() != job_key {
            return Err(validation_error(
                "Selected resume is not linked to the supplied profile and job",
                Some("resumeId"),
                Some("must belong to profileId and jobId"),
            ));
        }
        if resume.format() != "germany" {
            return Err(validation_error(
                "Selected resume is not a Germany-format resume",
                Some("resumeId"),
                Some("format must be germany"),
            ));
        }
        if resume.profile_revision() != sources.profile_at
            || resume.job_revision() != sources.job_at
        {
            return Err(ApplicationError::new(
                ErrorCode::Conflict,
                Some("Selected resume is stale"),
                [
                    ("field", "resumeId"),
                    (
                        "reason",
                        "regenerate the resume from the current profile and job",
                    ),
                ],
            ));
        }
        // Read admission includes chronology and immutable parent validation.
        validate_resume_aggregate(&ResumeAggregateWrite::new(resume.clone(), Vec::new())?)?;
        let content = build_german_cover_letter(&sources.graph, &sources.job)?;
        review_cover_letter_content(&content)?;
        let now = self.now();
        let parent = CoverLetter::new(CoverLetterFields {
            id: Uuid::new_v4(),
            profile_id: profile_key,
            job_id: job_key,
            resume_id: resume_key,
            profile_revision: sources.profile_at,
            job_revision: sources.job_at,
            resume_revision: resume.updated_at(),
            created_at: now,
            updated_at: now,
        })?;
        let planned = CoverLetterVariant::new(LetterVariantFields {
            id: Uuid::new_v4(),
            cover_letter_id: parent.id(),
            format: "germany",
            language: "de",
            document_id: Uuid::new_v4(),
            file_path: &format!("{}.pdf", Uuid::new_v4()),
            subject: content.subject(),
            salutation: content.salutation(),
            closing: content.closing(),
            signature: content.signature(),
            created_at: now,
            updated_at: now,
        })?;
        let admitted =
            map_cover_letter_content(parent.clone(), planned.clone(), &content, &mut || {
                Uuid::new_v4()
            })?;
        validate_cover_letter_aggregate(&admitted)?;
        let rendered = self.ports.renderer.render_cover_letter(&content)?;
        let descriptor = GenerationDescriptor {
            operation_id: Uuid::new_v4(),
            purpose: GenerationPurpose::GermanCoverLetter,
            profile_id: Some(profile_key),
            job_id: Some(job_key),
            resume_id: Some(resume_key),
            output_scope: Some("de".to_owned()),
        };
        let name = reservation_name("cover-letter", parent.id(), "de");
        let (claim, size, pages) = self.publish(rendered, &name, known, &descriptor).await?;
        let variant = CoverLetterVariant::new(LetterVariantFields {
            id: planned.id(),
            cover_letter_id: planned.cover_letter_id(),
            format: planned.format(),
            language: planned.language(),
            document_id: claim.reservation.document_id,
            file_path: claim.reservation.asset.relative_path(),
            subject: planned.subject(),
            salutation: planned.salutation(),
            closing: planned.closing(),
            signature: planned.signature(),
            created_at: planned.created_at(),
            updated_at: planned.updated_at(),
        })?;
        // Retains the admitted raw budget of the variant and of the aggregate.
        let variant_write = LetterVariantWrite::retained(
            variant.clone(),
            admitted.variant().paragraphs().to_vec(),
            admitted.variant(),
        )?;
        let aggregate =
            CoverLetterAggregateWrite::retained(parent.clone(), variant_write, &admitted)?;
        validate_cover_letter_aggregate(&aggregate)?;
        let result = self
            .ports
            .cover_letters
            .replace_german_cover_letter(
                &aggregate,
                &claim.reservation,
                revision_of(sources.profile_revision)?,
                revision_of(sources.job_revision)?,
                Some(&claim),
            )
            .await
            .map_err(application_error)?;
        let replaced = check_letter_result(&result, &parent, &variant)?;
        validate_cover_letter_aggregate(&aggregate)?;
        Ok(GeneratedCoverLetterResult {
            cover_letter: parent,
            variant,
            reservation: claim.reservation,
            byte_size: size,
            page_count: pages,
            replaced_existing: replaced,
            generated_at: now,
        })
    }
}

/// Checks the stored resume against the written one and returns whether a previous
/// resume was replaced.
fn check_resume_result(
    result: &ResumeReplaceResult,
    parent: &Resume,
    expected: &[ResumeVariant],
) -> Result<bool, ApplicationError> {
    if result.resume != *parent || result.variants != expected {
        return Err(internal());
    }
    if result.resume.updated_at() < result.resume.created_at()
        || result.resume.raw_units() > MAX_RETURNED_UNITS
        || !result
            .variants
            .iter()
            .all(|variant| resume_variant_row(variant).1)
    {
        return Err(internal());
    }
    let incoming: HashSet<Uuid> = expected.iter().map(ResumeVariant::id).collect();
    check_previous(&result.previous_variants, &incoming, resume_variant_row)?;
    Ok(!result.previous_variants.is_empty())
}

/// Checks the stored cover letter against the written one and returns whether a previous cover
/// letter was replaced.
fn check_letter_result(
    result: &LetterReplaceResult,
    parent: &CoverLetter,
    variant: &CoverLetterVariant,
) -> Result<bool, ApplicationError> {
    if result.cover_letter != *parent || result.variant != *variant {
        return Err(internal());
    }
    if result.cover_letter.updated_at() < result.cover_letter.created_at()
        || result.cover_letter.raw_units() > MAX_RETURNED_UNITS
        || !letter_variant_row(&result.variant).1
    {
        return Err(internal());
    }
    let incoming = HashSet::from([variant.id()]);
    check_previous(&result.previous_variants, &incoming, letter_variant_row)?;
    Ok(!result.previous_variants.is_empty())
}
