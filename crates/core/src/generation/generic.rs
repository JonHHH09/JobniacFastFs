//! Profile-only resume PDF generation.
//!
//! The master, Canadian and Canadian French resumes are prepared from the profile graph, laid out
//! and rendered with the generic renderer, admitted as classified generated assets, and linked to
//! the profile under the revision and timestamp the graph was read at. Nothing here does IO: the
//! graph read and the link replacement are ports, the asset lifecycle is [`GeneratedAssetService`].
//! Committed outputs are never compensated after the link is saved; a failure before that point
//! abandons the claim and reports whether the abandonment was confirmed.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_pdf::PdfError;
use jobniac_pdf::layout::{PageNumberLocale, build_pdf_layout};
use jobniac_pdf::render::render_pdf;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::clock::Clock;
use crate::errors::{AppError, ApplicationError, ErrorCode, not_found, validation_error};
use crate::generated_assets::service::sha256_hex;
use crate::generated_assets::{
    ClassifiedAssetResult, GeneratedAssetService, GeneratedReservation, GenerationDescriptor,
    GenerationPurpose, GenerationState, IntentReservation, MaterializationOutcome,
    PreparationOutcome, ProfileResumeDocument, ReplaceResult, ReserveError,
    admit_profile_resume_link, validate_generated_pdf,
};
use crate::generation::content::{
    PreparedResume, ResumeVariant, default_sentence_boundaries, prepare_canadian_resume,
    prepare_french_canadian_resume, prepare_master_resume,
};
use crate::generation::translation::FrenchTranslator;
use crate::profiles::model::ProfileGraph;

/// Read port of the profile graph. `None` when the
/// profile does not exist.
#[async_trait]
pub trait ProfileGraphPort: Send + Sync {
    /// The complete graph of `profile_id`, or `None` when it is missing.
    async fn find_profile(&self, profile_id: Uuid) -> Result<Option<ProfileGraph>, AppError>;
}

/// Write port of the profile resume link.
#[async_trait]
pub trait ProfileResumeLinkPort: Send + Sync {
    /// Saves `link` for its profile and resume type when the profile still has
    /// `expected_revision` and `expected_updated_at`, binding it to `binding` and, when given, to
    /// the generation `claim`. Returns the saved link and the previous one, if any.
    async fn replace_link(
        &self,
        link: &ProfileResumeDocument,
        binding: &GeneratedReservation,
        expected_revision: i64,
        expected_updated_at: OffsetDateTime,
        claim: Option<&IntentReservation>,
    ) -> Result<ReplaceResult, AppError>;
}

/// A generated profile resume: the saved link, its asset and the file metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedProfileResumePdf {
    /// The saved link.
    pub link: ProfileResumeDocument,
    /// The committed reservation of the PDF.
    pub reservation: GeneratedReservation,
    /// The generated logical file name.
    pub file_name: String,
    /// The byte size of the PDF.
    pub byte_size: usize,
    /// The page count of the PDF.
    pub page_count: usize,
    /// The generation instant.
    pub generated_at: OffsetDateTime,
    /// Whether a previous link was replaced.
    pub replaced_existing_link: bool,
}

/// A failed generation. `compensation_confirmed` tells whether the abandoned claim, if any, was
/// confirmed retired. The safe error never carries paths, bytes or database text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{error}")]
pub struct GenerationFailure {
    /// The caller-safe error.
    pub error: ApplicationError,
    /// Whether the compensation of the claim was confirmed.
    pub compensation_confirmed: bool,
}

/// State of one publication attempt used for compensation.
#[derive(Debug, Default)]
pub(crate) struct Publication {
    /// The reserved claim, once the reservation is dispatched.
    pub(crate) claim: Option<IntentReservation>,
    /// Whether a reservation was dispatched but not yet proven.
    pub(crate) unproven: bool,
}

/// Maps a port error to the caller-safe application error; anything but an application error is
/// internal.
pub(crate) fn application_error(error: AppError) -> ApplicationError {
    match error {
        AppError::Application(error) => error,
        _ => ApplicationError::of(ErrorCode::InternalError),
    }
}

/// Maps a pure PDF error to the caller-safe application error.
pub(crate) fn pdf_error(error: PdfError) -> ApplicationError {
    match error {
        PdfError::Validation {
            message,
            field,
            reason,
        } => validation_error(message, field, reason),
        PdfError::Internal => ApplicationError::with_message(
            ErrorCode::InternalError,
            jobniac_pdf::error::INTERNAL_MESSAGE,
        ),
    }
}

/// Maps a reservation failure: invalid input and unproven state are both internal.
pub(crate) fn reserve_error(error: ReserveError) -> ApplicationError {
    match error {
        ReserveError::Invalid(_) => ApplicationError::of(ErrorCode::InternalError),
        ReserveError::Unavailable(error) => {
            ApplicationError::with_message(ErrorCode::InternalError, error.0)
        }
    }
}

/// Maps an asset-service refusal to an internal application error with its fixed message.
pub(crate) fn unavailable(message: &'static str) -> ApplicationError {
    ApplicationError::with_message(ErrorCode::InternalError, message)
}

/// Whether a compensation outcome confirms that the claim is no longer live.
pub(crate) async fn compensate(assets: &GeneratedAssetService, known: &Publication) -> bool {
    match &known.claim {
        None => !known.unproven,
        Some(claim) => matches!(
            assets.abandon_generation(claim).await,
            Ok(PreparationOutcome::Prepared
                | PreparationOutcome::AlreadyRetired
                | PreparationOutcome::Referenced)
        ),
    }
}

/// Checks that a reservation binds to `root_id`, `data` and its size and digest.
pub(crate) fn check_reservation(
    reservation: &GeneratedReservation,
    data: &[u8],
    root_id: Uuid,
) -> Result<(), ApplicationError> {
    if reservation.asset.root_id() != root_id
        || reservation.asset.size_bytes() != data.len() as u64
        || reservation.asset.sha256() != sha256_hex(data)
    {
        return Err(unavailable("Invalid profile generated reservation"));
    }
    Ok(())
}

/// Reserves and materializes the classified PDF `data`. The claim is recorded
/// in `known` before materialization so that a failure can abandon it.
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors the publication step; the arguments are all distinct inputs"
)]
pub(crate) async fn publish_classified(
    assets: &GeneratedAssetService,
    root_id: Uuid,
    data: &[u8],
    name: &str,
    pages: usize,
    rendered_pages: usize,
    known: &mut Publication,
    descriptor: &GenerationDescriptor,
) -> Result<IntentReservation, ApplicationError> {
    validate_generated_pdf(data)
        .map_err(|_| unavailable("Invalid profile generated PDF metadata"))?;
    if rendered_pages != pages {
        return Err(unavailable("Invalid profile generated PDF metadata"));
    }
    known.unproven = true;
    let binding = assets
        .reserve_classified_pdf(data, name, descriptor)
        .await
        .map_err(reserve_error)?;
    check_reservation(&binding.reservation, data, root_id)?;
    if binding.intent.descriptor != *descriptor || binding.intent.state != GenerationState::Pending
    {
        return Err(unavailable("Invalid profile generated intent"));
    }
    let retained = binding.clone();
    known.claim = Some(binding.clone());
    known.unproven = false;
    let active = assets
        .materialize_classified(&binding)
        .await
        .map_err(|error| ApplicationError::with_message(ErrorCode::InternalError, error.0))?;
    let ClassifiedAssetResult { claim, outcome } = active;
    if claim != retained || outcome != MaterializationOutcome::Active {
        return Err(unavailable("Profile generated PDF is not active"));
    }
    check_reservation(&claim.reservation, data, root_id)?;
    Ok(retained)
}

/// Checks the saved link against the expected link and the retained reservation.
/// Returns the saved link and whether a previous link was replaced.
fn check_replacement(
    result: &ReplaceResult,
    expected: &ProfileResumeDocument,
    retained: &GeneratedReservation,
) -> Result<(ProfileResumeDocument, bool), ApplicationError> {
    let invalid = || unavailable("Invalid profile resume replacement result");
    let saved = &result.saved;
    saved.validate_incoming().map_err(|_| invalid())?;
    admit_profile_resume_link(saved, retained).map_err(|_| invalid())?;
    if saved.profile_id != expected.profile_id
        || saved.document_id != expected.document_id
        || saved.file_path != expected.file_path
        || saved.resume_type != expected.resume_type
        || saved.updated_at != expected.updated_at
    {
        return Err(invalid());
    }
    match &result.previous {
        None => {
            if saved.id != expected.id || saved.created_at != expected.created_at {
                return Err(invalid());
            }
        }
        Some(previous) => {
            previous.validate_incoming().map_err(|_| invalid())?;
            if previous.profile_id != expected.profile_id
                || previous.resume_type != expected.resume_type
                || previous.id != saved.id
                || previous.created_at != saved.created_at
                || previous.updated_at > saved.updated_at
                || previous.document_id == saved.document_id
            {
                return Err(invalid());
            }
        }
    }
    Ok((saved.clone(), result.previous.is_some()))
}

/// Profile-only resume generation over the graph, link, asset and clock ports.
pub struct GenericGenerationService {
    profiles: Arc<dyn ProfileGraphPort>,
    links: Arc<dyn ProfileResumeLinkPort>,
    assets: Arc<GeneratedAssetService>,
    translator: Arc<FrenchTranslator>,
    root_id: Uuid,
    clock: Arc<dyn Clock>,
    enabled: bool,
}

impl std::fmt::Debug for GenericGenerationService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GenericGenerationService")
            .field("root_id", &self.root_id)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl GenericGenerationService {
    /// Builds the service. `enabled` must be the exact opt-in value resolved by the runtime, and
    /// `root_id` the registered root of `assets`.
    #[must_use]
    pub fn new(
        profiles: Arc<dyn ProfileGraphPort>,
        links: Arc<dyn ProfileResumeLinkPort>,
        assets: Arc<GeneratedAssetService>,
        translator: Arc<FrenchTranslator>,
        root_id: Uuid,
        clock: Arc<dyn Clock>,
        enabled: bool,
    ) -> Self {
        Self {
            profiles,
            links,
            assets,
            translator,
            root_id,
            clock,
            enabled,
        }
    }

    /// Generates the master resume of a profile.
    ///
    /// # Errors
    ///
    /// Fails with the not-found, preparation, rendering, publication or link errors,
    /// and with the compensation status of any abandoned claim.
    pub async fn generate_pdf_resume(
        &self,
        profile_id: Uuid,
    ) -> Result<GeneratedProfileResumePdf, GenerationFailure> {
        self.generate(profile_id, ResumeVariant::MasterResume, "master-resume-")
            .await
    }

    /// Generates the Canadian English resume of a profile.
    ///
    /// # Errors
    ///
    /// The same refusals as [`Self::generate_pdf_resume`].
    pub async fn generate_canadian_pdf_resume(
        &self,
        profile_id: Uuid,
    ) -> Result<GeneratedProfileResumePdf, GenerationFailure> {
        self.generate(
            profile_id,
            ResumeVariant::CanadianResume,
            "canadian-resume-",
        )
        .await
    }

    /// Generates the Canadian French resume of a profile.
    ///
    /// # Errors
    ///
    /// The same refusals as [`Self::generate_pdf_resume`], plus the translation budget.
    pub async fn generate_canadian_french_pdf_resume(
        &self,
        profile_id: Uuid,
    ) -> Result<GeneratedProfileResumePdf, GenerationFailure> {
        self.generate(
            profile_id,
            ResumeVariant::CanadianResumeFr,
            "canadian-resume-fr-",
        )
        .await
    }

    /// Prepares the body of `variant` from the graph.
    fn prepare(
        &self,
        variant: ResumeVariant,
        graph: &ProfileGraph,
    ) -> Result<PreparedResume, ApplicationError> {
        match variant {
            ResumeVariant::MasterResume => prepare_master_resume(graph),
            ResumeVariant::CanadianResume => {
                prepare_canadian_resume(graph, &default_sentence_boundaries)
            }
            ResumeVariant::CanadianResumeFr => prepare_french_canadian_resume(
                graph,
                &self.translator,
                &default_sentence_boundaries,
            ),
        }
    }

    /// The shared generation path.
    async fn generate(
        &self,
        profile_id: Uuid,
        variant: ResumeVariant,
        prefix: &str,
    ) -> Result<GeneratedProfileResumePdf, GenerationFailure> {
        let mut known = Publication::default();
        match self.run(profile_id, variant, prefix, &mut known).await {
            Ok(generated) => Ok(generated),
            Err(error) => Err(GenerationFailure {
                error,
                compensation_confirmed: compensate(&self.assets, &known).await,
            }),
        }
    }

    /// The generation steps; any error is returned to [`Self::generate`] for compensation.
    async fn run(
        &self,
        profile_id: Uuid,
        variant: ResumeVariant,
        prefix: &str,
        known: &mut Publication,
    ) -> Result<GeneratedProfileResumePdf, ApplicationError> {
        if !self.enabled {
            return Err(unavailable("Profile generation is not enabled"));
        }
        let graph = self
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
        let revision = graph.profile.revision;
        if graph.profile.id != profile_id || revision < 0 {
            return Err(unavailable("Invalid profile generation source snapshot"));
        }
        let profile_at = graph.profile.updated_at;
        let content = self.prepare(variant, &graph)?;
        let french = variant == ResumeVariant::CanadianResumeFr;
        let locale = if french {
            PageNumberLocale::CanadianFrench
        } else {
            PageNumberLocale::English
        };
        let layout = build_pdf_layout(
            Some(content.title.as_str()),
            Some(content.body.as_str()),
            false,
            locale,
        )
        .map_err(pdf_error)?;
        let name = format!("{prefix}{profile_id}-{}.pdf", Uuid::new_v4());
        let rendered = render_pdf(
            Some(content.title.as_str()),
            Some(content.body.as_str()),
            false,
            locale,
        )
        .map_err(pdf_error)?;
        let now = self.clock.now().to_offset(time::UtcOffset::UTC);
        let descriptor = GenerationDescriptor::new(
            Uuid::new_v4(),
            GenerationPurpose::ProfileResume,
            Some(profile_id),
            None,
            None,
            Some(content_resume_type(variant).to_owned()),
        )
        .map_err(|_| unavailable("Invalid profile generated intent"))?;
        let size = rendered.data.len();
        let claim = publish_classified(
            &self.assets,
            self.root_id,
            &rendered.data,
            &name,
            layout.pages.len(),
            rendered.page_count,
            known,
            &descriptor,
        )
        .await?;
        let retained = claim.reservation.clone();
        let link = ProfileResumeDocument::new(
            Uuid::new_v4(),
            profile_id,
            retained.document_id,
            retained.asset.relative_path(),
            content_resume_type(variant),
            now,
            now,
        )
        .map_err(|_| unavailable("Invalid profile resume link"))?;
        admit_profile_resume_link(&link, &retained)
            .map_err(|_| unavailable("Invalid profile resume link"))?;
        let expected = link.clone();
        let result = self
            .links
            .replace_link(&link, &retained, revision, profile_at, Some(&claim))
            .await
            .map_err(application_error)?;
        // The claim is now bound to a saved link: a later failure must not abandon it.
        known.claim = None;
        let (saved, replaced) = check_replacement(&result, &expected, &retained)?;
        Ok(GeneratedProfileResumePdf {
            link: saved,
            reservation: retained,
            file_name: name,
            byte_size: size,
            page_count: layout.pages.len(),
            generated_at: now,
            replaced_existing_link: replaced,
        })
    }
}

/// The `resume_type` text of a variant.
fn content_resume_type(variant: ResumeVariant) -> &'static str {
    variant.as_str()
}
