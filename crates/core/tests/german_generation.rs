//! German generation orchestration over in-memory ports, plus the raw-total accessors the persistence
//! mappers use. Every text is synthetic.
//!
//! Cases that depend on a dynamic type system (a wrong-typed port result, an equal-coercing object,
//! a missing port), on lone surrogates, on mutating frozen records or on a date string outside the
//! typed date record have no Rust counterpart: ports are typed, proofs and rows are immutable behind
//! checked constructors and the ports receive shared references. The test that covers each of them
//! names it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use jobniac_core::clock::FixedClock;
use jobniac_core::errors::{AppError, ApplicationError, ErrorCode, conflict};
use jobniac_core::generated_assets::identity::{PublicationFailure, PublicationOutcome};
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedAssetRepository, GeneratedAssetService, GeneratedAssetWriter,
    GeneratedReservation, GenerationDescriptor, GenerationIntent, GenerationPurpose,
    GenerationState, IntentReservation, MaterializationOutcome, PreparationOutcome,
};
use jobniac_core::generation::generic::{GenerationFailure, ProfileGraphPort};
use jobniac_core::german::content::{
    CoverLetterInput, GermanCoverLetterContent, PersonalField, ResumeInput,
    StructuredResumeContent, raw_strings,
};
use jobniac_core::german::cover_letter_store::{
    CoverLetter, CoverLetterAggregateWrite, CoverLetterFields, CoverLetterVariant,
    GermanCoverLetterRepository, ReplaceResult as LetterReplaceResult,
    VariantFields as LetterVariantFields, VariantWrite as LetterVariantWrite,
    map_cover_letter_content,
};
use jobniac_core::german::generation::{
    GermanGenerationPorts, GermanGenerationService, GermanPdfRenderer, JobAggregatePort,
    PdfGermanRenderer, PersonalDetailsPort, RenderedGermanPdf,
};
use jobniac_core::german::resume_store::{
    GermanResumeRepository, ReplaceResult as ResumeReplaceResult, Resume, ResumeAggregateWrite,
    ResumeFields, ResumeVariant, Revision, VariantFields, VariantWrite, to_variant_write,
};
use jobniac_core::german::translation::GermanResumeTranslator;
use jobniac_core::jobs::model::{JobAggregate, JobRecord};
use jobniac_core::profiles::model::{
    ContactRecord, ExperienceRecord, ProfileGraph, ProfileRecord, ProjectRecord,
};
use jobniac_core::profiles::personal_details::{PersonalDetailsEnvelope, PersonalDetailsRecord};
use serde_json::{Value, json};
use time::macros::datetime;
use time::{Date, Month, OffsetDateTime};
use uuid::Uuid;

const NOW: OffsetDateTime = datetime!(2026-10-03 0:00 UTC);
const PDF: &[u8] = b"%PDF-1.7\nsynthetic unit bytes";

/// How the replace port answers.
#[derive(Clone)]
enum Replace {
    Normal,
    Fail(AppError),
    Parent,
    Variant,
    DuplicatePrevious,
    ValidPrevious,
    InvalidPreviousRow,
}

/// How the reserve port corrupts the stored reservation it returns.
#[derive(Clone, Copy)]
enum Tamper {
    Root,
    Sha,
    Size,
}

#[derive(Default)]
struct State {
    calls: Vec<String>,
    claims: HashMap<Uuid, IntentReservation>,
    bytes: HashMap<Uuid, Vec<u8>>,
    names: Vec<String>,
    known: Vec<Uuid>,
    cleaned: Vec<Uuid>,
    cleaned_tokens: Vec<Option<Uuid>>,
    resumes_rendered: Vec<StructuredResumeContent>,
    letters_rendered: Vec<GermanCoverLetterContent>,
    fail: Option<String>,
    cleanup: VecDeque<Result<PreparationOutcome, ()>>,
    graph: Option<ProfileGraph>,
    job: Option<JobAggregate>,
    personal: Option<PersonalDetailsEnvelope>,
    resume: Option<Resume>,
    render_override: Option<RenderedGermanPdf>,
    outcome: Option<MaterializationOutcome>,
    tamper: Option<Tamper>,
    replace: Option<Replace>,
    resume_write: Option<ResumeAggregateWrite>,
    letter_write: Option<CoverLetterAggregateWrite>,
    cas: Option<(i64, i64)>,
    replace_claims: Vec<IntentReservation>,
    replace_bindings: Vec<GeneratedReservation>,
}

/// One in-memory implementation of every port, recording the calls in order.
struct Harness {
    profile_id: Uuid,
    job_id: Uuid,
    resume_id: Uuid,
    root_id: Uuid,
    state: Mutex<State>,
}

fn date(year: i32, month: Month, day: u8) -> Date {
    Date::from_calendar_date(year, month, day).unwrap()
}

fn profile(id: Uuid) -> ProfileGraph {
    ProfileGraph {
        profile: ProfileRecord {
            id,
            full_name: "Ada Example".to_owned(),
            email: "ada@example.test".to_owned(),
            summary: None,
            raw_resume_text: None,
            created_at: NOW,
            updated_at: NOW,
            embedding: None,
            revision: 7,
        },
        contacts: vec![ContactRecord {
            id: Uuid::nil(),
            profile_id: id,
            contact_type: "phone".to_owned(),
            contact_value: "123".to_owned(),
            label: Some("Phone".to_owned()),
            created_at: NOW,
            updated_at: NOW,
        }],
        links: Vec::new(),
        skills: Vec::new(),
        languages: Vec::new(),
        education: Vec::new(),
        experiences: vec![ExperienceRecord {
            id: Uuid::nil(),
            profile_id: id,
            company: Some("Example Company".to_owned()),
            title: Some("Java Developer".to_owned()),
            location: None,
            start_date: Some(date(2020, Month::January, 1)),
            end_date: None,
            description: Some("- Built Java systems.\n- Created SQL reports.".to_owned()),
            display_order: 0,
            created_at: NOW,
        }],
        projects: vec![ProjectRecord {
            id: Uuid::nil(),
            profile_id: id,
            name: Some("Java Tools".to_owned()),
            url: None,
            description: Some("- Built tools.\n- Tested them.".to_owned()),
            technologies: Vec::new(),
            display_order: 0,
            created_at: NOW,
        }],
        project_technologies: Vec::new(),
    }
}

fn job(id: Uuid) -> JobAggregate {
    JobAggregate {
        job: JobRecord {
            id,
            source_method: "text".to_owned(),
            source_label: None,
            title: "Java Developer".to_owned(),
            company: Some("Example Employer".to_owned()),
            location: None,
            description: String::new(),
            experience_requirement: None,
            employment_type: None,
            seniority: None,
            posted_at: None,
            canonical_fingerprint: String::new(),
            created_at: NOW,
            updated_at: NOW,
            revision: 9,
        },
        skills: Vec::new(),
        link_ingestion: None,
        text_ingestion: None,
    }
}

fn personal(profile_id: Uuid) -> PersonalDetailsEnvelope {
    PersonalDetailsEnvelope {
        profile_id,
        profile_revision: 7,
        profile_updated_at: NOW,
        personal_details: Some(PersonalDetailsRecord {
            profile_id,
            date_of_birth: Some(date(1990, Month::January, 1)),
            nationality: Some("Zelandian".to_owned()),
            photo_document_id: None,
            created_at: NOW,
            updated_at: NOW,
        }),
    }
}

fn resume_row(
    id: Uuid,
    profile_id: Uuid,
    job_id: Uuid,
    at: OffsetDateTime,
    jt: OffsetDateTime,
) -> Resume {
    Resume::new(ResumeFields {
        id,
        profile_id,
        job_id,
        format: "germany",
        profile_revision: at,
        job_revision: jt,
        created_at: NOW,
        updated_at: NOW,
    })
    .unwrap()
}

impl Harness {
    fn new() -> Arc<Self> {
        let profile_id = Uuid::new_v4();
        let job_id = Uuid::new_v4();
        let resume_id = Uuid::new_v4();
        let state = State {
            graph: Some(profile(profile_id)),
            job: Some(job(job_id)),
            personal: Some(personal(profile_id)),
            resume: Some(resume_row(resume_id, profile_id, job_id, NOW, NOW)),
            ..State::default()
        };
        Arc::new(Self {
            profile_id,
            job_id,
            resume_id,
            root_id: Uuid::new_v4(),
            state: Mutex::new(state),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    fn stage(&self, name: &str) -> Result<(), AppError> {
        let mut state = self.state();
        state.calls.push(name.to_owned());
        if state.fail.as_deref() == Some(name) {
            return Err(AppError::Internal);
        }
        Ok(())
    }

    fn fail_at(&self, name: &str) {
        self.state().fail = Some(name.to_owned());
    }

    fn calls(&self) -> Vec<String> {
        self.state().calls.clone()
    }

    fn cleaned(&self) -> Vec<Uuid> {
        self.state().cleaned.clone()
    }

    fn known(&self) -> Vec<Uuid> {
        self.state().known.clone()
    }

    fn rendered_count(&self) -> usize {
        let state = self.state();
        state.resumes_rendered.len() + state.letters_rendered.len()
    }

    fn set_replace(&self, replace: Replace) {
        self.state().replace = Some(replace);
    }

    fn service(self: &Arc<Self>) -> GermanGenerationService {
        self.service_with(true, true, Arc::clone(self) as Arc<dyn GermanPdfRenderer>)
    }

    fn service_with(
        self: &Arc<Self>,
        enabled: bool,
        assets_enabled: bool,
        renderer: Arc<dyn GermanPdfRenderer>,
    ) -> GermanGenerationService {
        let clock = Arc::new(FixedClock::new(NOW));
        let assets = Arc::new(GeneratedAssetService::new(
            Arc::clone(self) as Arc<dyn GeneratedAssetRepository>,
            Arc::clone(self) as Arc<dyn GeneratedAssetWriter>,
            self.root_id,
            clock.clone(),
            assets_enabled,
        ));
        GermanGenerationService::new(
            GermanGenerationPorts {
                profiles: Arc::clone(self) as Arc<dyn ProfileGraphPort>,
                jobs: Arc::clone(self) as Arc<dyn JobAggregatePort>,
                personal_details: Arc::clone(self) as Arc<dyn PersonalDetailsPort>,
                resumes: Arc::clone(self) as Arc<dyn GermanResumeRepository>,
                cover_letters: Arc::clone(self) as Arc<dyn GermanCoverLetterRepository>,
                assets,
                renderer,
            },
            Arc::new(GermanResumeTranslator::builtin()),
            self.root_id,
            clock,
            enabled,
        )
    }
}

fn internal_envelope(failure: &GenerationFailure) {
    assert_eq!(failure.error.code(), ErrorCode::InternalError);
    assert_eq!(failure.error.message(), "Unexpected application error");
    assert!(failure.error.details().is_empty());
}

#[async_trait]
impl ProfileGraphPort for Harness {
    async fn find_profile(&self, profile_id: Uuid) -> Result<Option<ProfileGraph>, AppError> {
        assert_eq!(profile_id, self.profile_id);
        self.stage("profile")?;
        Ok(self.state().graph.clone())
    }
}

#[async_trait]
impl JobAggregatePort for Harness {
    async fn find_job(&self, job_id: Uuid) -> Result<Option<JobAggregate>, AppError> {
        assert_eq!(job_id, self.job_id);
        self.stage("job")?;
        Ok(self.state().job.clone())
    }
}

#[async_trait]
impl PersonalDetailsPort for Harness {
    async fn find_personal_details(
        &self,
        profile_id: Uuid,
    ) -> Result<Option<PersonalDetailsEnvelope>, AppError> {
        assert_eq!(profile_id, self.profile_id);
        self.stage("personal")?;
        Ok(self.state().personal.clone())
    }
}

impl GermanPdfRenderer for Harness {
    fn render_resume(
        &self,
        content: &StructuredResumeContent,
    ) -> Result<RenderedGermanPdf, ApplicationError> {
        self.stage(&format!("render-{}", content.language()))
            .map_err(|_| ApplicationError::of(ErrorCode::InternalError))?;
        let mut state = self.state();
        state.resumes_rendered.push(content.clone());
        Ok(state.render_override.clone().unwrap_or(RenderedGermanPdf {
            data: PDF.to_vec(),
            byte_size: PDF.len(),
            page_count: 1,
        }))
    }

    fn render_cover_letter(
        &self,
        content: &GermanCoverLetterContent,
    ) -> Result<RenderedGermanPdf, ApplicationError> {
        self.stage("render-letter")
            .map_err(|_| ApplicationError::of(ErrorCode::InternalError))?;
        let mut state = self.state();
        state.letters_rendered.push(content.clone());
        Ok(state.render_override.clone().unwrap_or(RenderedGermanPdf {
            data: PDF.to_vec(),
            byte_size: PDF.len(),
            page_count: 1,
        }))
    }
}

#[async_trait]
impl GeneratedAssetRepository for Harness {
    async fn reserve_classified_pdf(
        &self,
        identity: &AssetIdentity,
        document_id: Uuid,
        original_file_name: &str,
        content: &[u8],
        descriptor: &GenerationDescriptor,
        lease_token: Uuid,
        now: OffsetDateTime,
    ) -> Result<IntentReservation, AppError> {
        let language = if original_file_name.ends_with("-de.pdf") {
            "de"
        } else {
            "en"
        };
        self.stage(&format!("reserve-{language}"))?;
        assert!(!original_file_name.contains('/') && !original_file_name.contains('\\'));
        let intent =
            GenerationIntent::pending(identity.asset_id(), descriptor.clone(), lease_token, now)
                .map_err(|_| AppError::Internal)?;
        let claim = IntentReservation::new(
            GeneratedReservation {
                asset: identity.clone(),
                document_id,
            },
            intent.clone(),
        )
        .map_err(|_| AppError::Internal)?;
        let mut state = self.state();
        state.claims.insert(identity.asset_id(), claim.clone());
        state.bytes.insert(identity.asset_id(), content.to_vec());
        state.names.push(original_file_name.to_owned());
        state.known.push(identity.asset_id());
        let Some(tamper) = state.tamper else {
            return Ok(claim);
        };
        let asset = match tamper {
            Tamper::Root => AssetIdentity::new(
                identity.asset_id(),
                Uuid::new_v4(),
                identity.sha256(),
                identity.size_bytes(),
            ),
            Tamper::Sha => AssetIdentity::new(
                identity.asset_id(),
                identity.root_id(),
                &"a".repeat(64),
                identity.size_bytes(),
            ),
            Tamper::Size => AssetIdentity::new(
                identity.asset_id(),
                identity.root_id(),
                identity.sha256(),
                identity.size_bytes() + 1,
            ),
        }
        .map_err(|_| AppError::Internal)?;
        IntentReservation::new(GeneratedReservation { asset, document_id }, intent)
            .map_err(|_| AppError::Internal)
    }

    async fn find_classified_reservation(
        &self,
        asset_id: Uuid,
    ) -> Result<Option<IntentReservation>, AppError> {
        Ok(self.state().claims.get(&asset_id).cloned())
    }

    async fn materialize_classified(
        &self,
        claim: &IntentReservation,
        writer: &dyn GeneratedAssetWriter,
    ) -> Result<MaterializationOutcome, AppError> {
        let known = self.state().known.len();
        self.stage(&format!("materialize-{known}"))?;
        let content = self
            .state()
            .bytes
            .get(&claim.asset_id())
            .cloned()
            .ok_or(AppError::Internal)?;
        writer
            .materialize_owned(&claim.reservation.asset, &content)
            .map_err(|_| AppError::Internal)?;
        Ok(self
            .state()
            .outcome
            .unwrap_or(MaterializationOutcome::Active))
    }

    async fn renew_generation(
        &self,
        _claim: &IntentReservation,
        _new_token: Uuid,
    ) -> Result<Option<IntentReservation>, AppError> {
        Ok(None)
    }

    async fn abandon_generation(
        &self,
        claim: &IntentReservation,
    ) -> Result<PreparationOutcome, AppError> {
        let mut state = self.state();
        state.cleaned.push(claim.asset_id());
        state.cleaned_tokens.push(claim.intent.lease_token);
        state.calls.push("cleanup".to_owned());
        match state.cleanup.pop_front() {
            Some(Ok(outcome)) => Ok(outcome),
            Some(Err(())) => Err(AppError::Internal),
            None => Ok(PreparationOutcome::Prepared),
        }
    }

    async fn recover_expired_generations(
        &self,
        _root_id: Uuid,
        _limit: usize,
    ) -> Result<Vec<Uuid>, AppError> {
        Ok(Vec::new())
    }
}

impl GeneratedAssetWriter for Harness {
    fn materialize_owned(
        &self,
        _identity: &AssetIdentity,
        _content: &[u8],
    ) -> Result<PublicationOutcome, PublicationFailure> {
        Ok(PublicationOutcome::Created)
    }
}

#[async_trait]
impl GermanResumeRepository for Harness {
    async fn find_by_id(&self, resume_id: Uuid) -> Result<Option<Resume>, AppError> {
        assert_eq!(resume_id, self.resume_id);
        self.stage("resume")?;
        Ok(self.state().resume.clone())
    }

    async fn find_by_profile_job_format(
        &self,
        _profile_id: Uuid,
        _job_id: Uuid,
        _format: &str,
    ) -> Result<Option<Resume>, AppError> {
        Ok(None)
    }

    async fn find_variants(&self, _resume_id: Uuid) -> Result<Vec<ResumeVariant>, AppError> {
        Ok(Vec::new())
    }

    async fn replace_germany_resume(
        &self,
        write: &ResumeAggregateWrite,
        bindings: &[GeneratedReservation],
        expected_profile_revision: Revision,
        expected_job_revision: Revision,
        generation_claims: Option<&[IntentReservation]>,
    ) -> Result<ResumeReplaceResult, AppError> {
        let claims = generation_claims.unwrap().to_vec();
        assert_eq!(
            claims
                .iter()
                .map(|claim| claim.reservation.clone())
                .collect::<Vec<_>>(),
            bindings
        );
        {
            let mut state = self.state();
            state.resume_write = Some(write.clone());
            state.cas = Some((expected_profile_revision.get(), expected_job_revision.get()));
            state.replace_claims = claims;
            state.replace_bindings = bindings.to_vec();
        }
        self.stage("replace")?;
        let replace = self.state().replace.clone().unwrap_or(Replace::Normal);
        let parent = write.resume().clone();
        let variants: Vec<ResumeVariant> = write
            .variants()
            .iter()
            .map(|item| item.variant().clone())
            .collect();
        let other = |source: &ResumeVariant, created: OffsetDateTime, updated: OffsetDateTime| {
            ResumeVariant::new(VariantFields {
                id: Uuid::new_v4(),
                resume_id: source.resume_id(),
                language: source.language(),
                document_id: Uuid::new_v4(),
                file_path: source.file_path(),
                created_at: created,
                updated_at: updated,
            })
            .unwrap()
        };
        let plain = |parent: Resume, variants: Vec<ResumeVariant>, previous: Vec<ResumeVariant>| {
            Ok(ResumeReplaceResult {
                resume: parent,
                variants,
                previous_variants: previous,
            })
        };
        match replace {
            Replace::Normal => plain(parent, variants, Vec::new()),
            Replace::Fail(error) => Err(error),
            Replace::Parent => plain(
                resume_row(
                    Uuid::new_v4(),
                    parent.profile_id(),
                    parent.job_id(),
                    parent.profile_revision(),
                    parent.job_revision(),
                ),
                variants,
                Vec::new(),
            ),
            Replace::Variant => {
                let mut reversed = variants.clone();
                reversed.reverse();
                plain(parent, reversed, Vec::new())
            }
            Replace::DuplicatePrevious => {
                let previous = vec![variants[0].clone()];
                plain(parent, variants, previous)
            }
            Replace::ValidPrevious => {
                let previous = vec![other(&variants[0], NOW, NOW)];
                plain(parent, variants, previous)
            }
            Replace::InvalidPreviousRow => {
                let previous = vec![other(&variants[0], NOW + time::Duration::seconds(1), NOW)];
                plain(parent, variants, previous)
            }
        }
    }
}

#[async_trait]
impl GermanCoverLetterRepository for Harness {
    async fn find_by_id(&self, _cover_letter_id: Uuid) -> Result<Option<CoverLetter>, AppError> {
        Ok(None)
    }

    async fn find_by_profile_job_resume(
        &self,
        _profile_id: Uuid,
        _job_id: Uuid,
        _resume_id: Uuid,
    ) -> Result<Option<CoverLetter>, AppError> {
        Ok(None)
    }

    async fn find_variants(
        &self,
        _cover_letter_id: Uuid,
    ) -> Result<Vec<CoverLetterVariant>, AppError> {
        Ok(Vec::new())
    }

    async fn find_paragraphs(
        &self,
        _variant_id: Uuid,
    ) -> Result<Vec<jobniac_core::german::cover_letter_store::CoverLetterParagraph>, AppError> {
        Ok(Vec::new())
    }

    async fn replace_german_cover_letter(
        &self,
        write: &CoverLetterAggregateWrite,
        binding: &GeneratedReservation,
        expected_profile_revision: Revision,
        expected_job_revision: Revision,
        generation_claim: Option<&IntentReservation>,
    ) -> Result<LetterReplaceResult, AppError> {
        let claim = generation_claim.unwrap().clone();
        assert_eq!(&claim.reservation, binding);
        {
            let mut state = self.state();
            state.letter_write = Some(write.clone());
            state.cas = Some((expected_profile_revision.get(), expected_job_revision.get()));
            state.replace_claims = vec![claim];
            state.replace_bindings = vec![binding.clone()];
        }
        self.stage("replace")?;
        let replace = self.state().replace.clone().unwrap_or(Replace::Normal);
        let parent = write.cover_letter().clone();
        let variant = write.variant().variant().clone();
        let other = || {
            CoverLetterVariant::new(LetterVariantFields {
                id: Uuid::new_v4(),
                cover_letter_id: variant.cover_letter_id(),
                format: variant.format(),
                language: variant.language(),
                document_id: Uuid::new_v4(),
                file_path: variant.file_path(),
                subject: variant.subject(),
                salutation: variant.salutation(),
                closing: variant.closing(),
                signature: variant.signature(),
                created_at: variant.created_at(),
                updated_at: variant.updated_at(),
            })
            .unwrap()
        };
        let plain = |parent: CoverLetter,
                     variant: CoverLetterVariant,
                     previous: Vec<CoverLetterVariant>| {
            Ok(LetterReplaceResult {
                cover_letter: parent,
                variant,
                previous_variants: previous,
            })
        };
        match replace {
            Replace::Normal => plain(parent.clone(), variant.clone(), Vec::new()),
            Replace::Fail(error) => Err(error),
            Replace::Parent => plain(
                CoverLetter::new(CoverLetterFields {
                    id: Uuid::new_v4(),
                    profile_id: parent.profile_id(),
                    job_id: parent.job_id(),
                    resume_id: parent.resume_id(),
                    profile_revision: parent.profile_revision(),
                    job_revision: parent.job_revision(),
                    resume_revision: parent.resume_revision(),
                    created_at: parent.created_at(),
                    updated_at: parent.updated_at(),
                })
                .unwrap(),
                variant.clone(),
                Vec::new(),
            ),
            Replace::Variant => plain(parent.clone(), other(), Vec::new()),
            Replace::DuplicatePrevious => {
                plain(parent.clone(), variant.clone(), vec![variant.clone()])
            }
            Replace::ValidPrevious => plain(parent.clone(), variant.clone(), vec![other()]),
            Replace::InvalidPreviousRow => {
                let twice = other();
                plain(parent.clone(), variant.clone(), vec![twice.clone(), twice])
            }
        }
    }
}

async fn run_resume(
    harness: &Arc<Harness>,
    include: Value,
) -> Result<jobniac_core::german::generation::GeneratedResumeResult, GenerationFailure> {
    harness
        .service()
        .generate_german_tailored_resume(
            &json!(harness.profile_id.to_string()),
            &json!(harness.job_id.to_string()),
            &include,
        )
        .await
}

async fn run_letter(
    harness: &Arc<Harness>,
) -> Result<jobniac_core::german::generation::GeneratedCoverLetterResult, GenerationFailure> {
    harness
        .service()
        .generate_german_cover_letter(
            &json!(harness.profile_id.to_string()),
            &json!(harness.job_id.to_string()),
            &json!(harness.resume_id.to_string()),
        )
        .await
}

async fn run(harness: &Arc<Harness>, letter: bool) -> Result<(), GenerationFailure> {
    if letter {
        run_letter(harness).await.map(|_| ())
    } else {
        run_resume(harness, Value::Null).await.map(|_| ())
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[tokio::test]
async fn disabled_performs_no_io() {
    let harness = Harness::new();
    let service = harness.service_with(false, true, harness.clone());
    let failure = service
        .generate_german_tailored_resume(
            &json!(harness.profile_id.to_string()),
            &json!(harness.job_id.to_string()),
            &Value::Null,
        )
        .await
        .unwrap_err();
    assert!(failure.error.message().contains("not enabled"));
    assert!(failure.compensation_confirmed);
    assert!(harness.calls().is_empty());
}

#[tokio::test]
async fn invalid_identity_before_disabled_or_io() {
    let long = harness_profile_without_hyphens();
    for value in [
        json!(null),
        json!(1),
        json!(true),
        json!(""),
        json!("bad"),
        json!(long),
    ] {
        let harness = Harness::new();
        let service = harness.service_with(false, true, harness.clone());
        let failure = service
            .generate_german_tailored_resume(
                &value,
                &json!(harness.job_id.to_string()),
                &Value::Null,
            )
            .await
            .unwrap_err();
        assert_eq!(failure.error.code(), ErrorCode::ValidationError);
        assert_eq!(failure.error.details()["field"], "profileId");
        let reason = if value.is_null() {
            "must not be null"
        } else {
            "must be a valid UUID"
        };
        assert_eq!(failure.error.details()["reason"], reason);
        assert!(harness.calls().is_empty());
    }
}

fn harness_profile_without_hyphens() -> String {
    Uuid::new_v4().to_string().replace('-', "")
}

#[tokio::test]
async fn invalid_projects_before_disabled_or_io() {
    for value in [json!(0), json!(1), json!("true"), json!([]), json!({})] {
        let harness = Harness::new();
        let service = harness.service_with(false, true, harness.clone());
        let failure = service
            .generate_german_tailored_resume(
                &json!(harness.profile_id.to_string()),
                &json!(harness.job_id.to_string()),
                &value,
            )
            .await
            .unwrap_err();
        assert_eq!(failure.error.details()["field"], "includeProjects");
        assert!(harness.calls().is_empty());
    }
}

#[tokio::test]
async fn one_bilingual_atomic_write_with_numeric_cas_and_safe_result() {
    let harness = Harness::new();
    let result = run_resume(&harness, Value::Null).await.unwrap();
    assert_eq!(
        harness.calls(),
        strings(&[
            "profile",
            "job",
            "personal",
            "render-en",
            "reserve-en",
            "materialize-1",
            "render-de",
            "reserve-de",
            "materialize-2",
            "replace",
        ])
    );
    let languages: Vec<&str> = result
        .variants
        .iter()
        .map(|item| item.variant.language())
        .collect();
    assert_eq!(languages, ["en", "de"]);
    assert_eq!(result.resume.profile_revision(), NOW);
    assert_eq!(result.resume.job_revision(), NOW);
    let state = harness.state();
    assert_eq!(state.cas, Some((7, 9)));
    assert_eq!(result.generated_at, NOW);
    assert!(!result.replaced_existing);
    assert_eq!(result.variants[0].reservation, state.replace_bindings[0]);
    assert_eq!(result.variants[1].reservation, state.replace_bindings[1]);
    assert!(state.cleaned.is_empty());
    let rendered: Vec<&str> = state
        .resumes_rendered
        .iter()
        .map(StructuredResumeContent::language)
        .collect();
    assert_eq!(rendered, ["en", "de"]);
    assert!(
        state.resumes_rendered[0]
            .personal_fields()
            .iter()
            .any(|field| field.value() == "Zelandian")
    );
    let write = state.resume_write.as_ref().unwrap();
    assert!(!write.variants()[0].sections().is_empty());
    let claims = &state.replace_claims;
    let operations: std::collections::HashSet<Uuid> = claims
        .iter()
        .map(|claim| claim.intent.descriptor.operation_id)
        .collect();
    assert_eq!(operations.len(), 1);
    let scopes: Vec<_> = claims
        .iter()
        .map(|claim| claim.intent.descriptor.output_scope.clone().unwrap())
        .collect();
    assert_eq!(scopes, ["en", "de"]);
    for claim in claims {
        let descriptor = &claim.intent.descriptor;
        assert_eq!(descriptor.purpose, GenerationPurpose::GermanResume);
        assert_eq!(descriptor.resume_id, Some(result.resume.id()));
        assert_eq!(descriptor.profile_id, Some(harness.profile_id));
        assert_eq!(descriptor.job_id, Some(harness.job_id));
        assert_eq!(claim.intent.state, GenerationState::Pending);
    }
    // Reservation names carry the language and the stored path is the asset file name.
    assert!(state.names[0].ends_with("-en.pdf") && state.names[1].ends_with("-de.pdf"));
    for output in &result.variants {
        assert_eq!(
            output.variant.file_path(),
            format!("{}.pdf", output.reservation.asset.asset_id())
        );
        assert_eq!(output.variant.document_id(), output.reservation.document_id);
        assert_eq!(output.byte_size, PDF.len());
        assert_eq!(output.page_count, 1);
    }
}

#[tokio::test]
async fn projects_default_and_isolation() {
    for (option, expected) in [
        (Value::Null, false),
        (json!(false), false),
        (json!(true), true),
    ] {
        let harness = Harness::new();
        let first = run_resume(&harness, option).await.unwrap();
        {
            let state = harness.state();
            assert_eq!(!state.resumes_rendered[0].additional().is_empty(), expected);
            assert_eq!(!state.resumes_rendered[1].additional().is_empty(), expected);
        }
        harness.state().resumes_rendered.clear();
        let second = run_resume(&harness, Value::Null).await.unwrap();
        let state = harness.state();
        assert!(state.resumes_rendered[0].additional().is_empty());
        assert!(state.resumes_rendered[1].additional().is_empty());
        assert_ne!(first.resume.id(), second.resume.id());
        assert_eq!(
            state.graph.as_ref().unwrap().projects[0].name.as_deref(),
            Some("Java Tools")
        );
    }
}

#[tokio::test]
async fn letter_exact_resume_provenance_and_ordered_paragraphs() {
    let harness = Harness::new();
    let result = run_letter(&harness).await.unwrap();
    assert_eq!(
        harness.calls(),
        strings(&[
            "profile",
            "job",
            "resume",
            "render-letter",
            "reserve-de",
            "materialize-1",
            "replace",
        ])
    );
    assert_eq!(result.cover_letter.resume_id(), harness.resume_id);
    assert_eq!(result.cover_letter.resume_revision(), NOW);
    assert_eq!(result.variant.language(), "de");
    let state = harness.state();
    assert_eq!(result.reservation, state.replace_bindings[0]);
    let write = state.letter_write.as_ref().unwrap();
    let orders: Vec<i32> = write
        .variant()
        .paragraphs()
        .iter()
        .map(|paragraph| paragraph.display_order().get())
        .collect();
    let count = state.letters_rendered[0].paragraphs().len();
    assert_eq!(
        orders,
        (0..i32::try_from(count).unwrap()).collect::<Vec<_>>()
    );
    let texts: Vec<&str> = write
        .variant()
        .paragraphs()
        .iter()
        .map(|paragraph| paragraph.text())
        .collect();
    let expected: Vec<&str> = state.letters_rendered[0]
        .paragraphs()
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(texts, expected);
    assert_eq!(state.cas, Some((7, 9)));
    assert!(state.names[0].ends_with("-de.pdf"));
    assert_eq!(
        result.variant.file_path(),
        format!("{}.pdf", result.reservation.asset.asset_id())
    );
}

#[tokio::test]
async fn failures_preserve_known_identities_and_remove_private_context() {
    for letter in [false, true] {
        for stage in ["profile", "job", "replace"] {
            let harness = Harness::new();
            harness.fail_at(stage);
            let failure = run(&harness, letter).await.unwrap_err();
            assert_eq!(harness.cleaned(), harness.known());
            assert!(failure.compensation_confirmed);
            internal_envelope(&failure);
            assert!(
                harness
                    .calls()
                    .iter()
                    .filter(|call| *call == "replace")
                    .count()
                    <= 1
            );
        }
    }
}

#[tokio::test]
async fn each_resume_stage_compensates_every_known_reservation() {
    for (stage, known) in [
        ("personal", 0),
        ("render-en", 0),
        ("reserve-en", 0),
        ("materialize-1", 1),
        ("render-de", 1),
        ("reserve-de", 1),
        ("materialize-2", 2),
    ] {
        let harness = Harness::new();
        harness.fail_at(stage);
        run(&harness, false).await.unwrap_err();
        assert_eq!(harness.known().len(), known, "{stage}");
        assert_eq!(harness.cleaned(), harness.known(), "{stage}");
        assert!(!harness.calls().contains(&"replace".to_owned()), "{stage}");
    }
}

#[tokio::test]
async fn each_letter_stage_compensates_every_known_reservation() {
    for stage in ["resume", "render-letter", "reserve-de", "materialize-1"] {
        let harness = Harness::new();
        harness.fail_at(stage);
        run(&harness, true).await.unwrap_err();
        assert_eq!(harness.cleaned(), harness.known(), "{stage}");
        assert!(!harness.calls().contains(&"replace".to_owned()), "{stage}");
    }
}

#[tokio::test]
async fn cleanup_all_attempted_original_failure_preserved() {
    for (outcome, confirmed) in [
        (Ok(PreparationOutcome::Prepared), true),
        (Ok(PreparationOutcome::AlreadyRetired), true),
        (Ok(PreparationOutcome::Referenced), true),
        (Ok(PreparationOutcome::Ineligible), false),
        (Err(()), false),
    ] {
        let harness = Harness::new();
        harness.fail_at("replace");
        harness.state().cleanup = VecDeque::from([outcome, Ok(PreparationOutcome::Prepared)]);
        let failure = run(&harness, false).await.unwrap_err();
        assert_eq!(failure.compensation_confirmed, confirmed);
        assert_eq!(harness.cleaned().len(), 2);
        internal_envelope(&failure);
    }
}

#[tokio::test]
async fn mixed_personal_snapshot_no_render() {
    for mutate in 0..3 {
        let harness = Harness::new();
        {
            let mut state = harness.state();
            let envelope = state.personal.as_mut().unwrap();
            match mutate {
                0 => envelope.profile_id = Uuid::new_v4(),
                1 => envelope.profile_revision = 8,
                _ => envelope.profile_updated_at = NOW - time::Duration::days(1),
            }
        }
        let failure = run(&harness, false).await.unwrap_err();
        assert_eq!(failure.error.code(), ErrorCode::Conflict);
        assert!(harness.known().is_empty());
        assert_eq!(harness.rendered_count(), 0);
    }
}

#[tokio::test]
async fn invalid_owner_metadata_before_render() {
    for owner in ["profile", "job"] {
        for mutate in 0..2 {
            let harness = Harness::new();
            {
                let mut state = harness.state();
                if owner == "profile" {
                    let row = &mut state.graph.as_mut().unwrap().profile;
                    if mutate == 0 {
                        row.id = Uuid::new_v4();
                    } else {
                        row.revision = -1;
                    }
                } else {
                    let row = &mut state.job.as_mut().unwrap().job;
                    if mutate == 0 {
                        row.id = Uuid::new_v4();
                    } else {
                        row.revision = -1;
                    }
                }
            }
            run(&harness, false).await.unwrap_err();
            assert!(harness.known().is_empty());
            assert_eq!(harness.rendered_count(), 0);
        }
    }
}

#[tokio::test]
async fn invalid_exact_resume_precedes_artifacts() {
    let earlier = NOW - time::Duration::days(365);
    for field in [
        "id",
        "profile_id",
        "job_id",
        "profile_revision",
        "job_revision",
    ] {
        let harness = Harness::new();
        {
            let mut state = harness.state();
            let (mut id, mut profile_id, mut job_id) =
                (harness.resume_id, harness.profile_id, harness.job_id);
            let (mut at, mut job_at) = (NOW, NOW);
            match field {
                "id" => id = Uuid::new_v4(),
                "profile_id" => profile_id = Uuid::new_v4(),
                "job_id" => job_id = Uuid::new_v4(),
                "profile_revision" => at = earlier,
                _ => job_at = earlier,
            }
            state.resume = Some(resume_row(id, profile_id, job_id, at, job_at));
        }
        let failure = run(&harness, true).await.unwrap_err();
        let expected = match field {
            "id" => ErrorCode::InternalError,
            "profile_revision" | "job_revision" => ErrorCode::Conflict,
            _ => ErrorCode::ValidationError,
        };
        assert_eq!(failure.error.code(), expected, "{field}");
        assert!(harness.known().is_empty());
        assert_eq!(harness.rendered_count(), 0);
    }
}

#[tokio::test]
async fn missing_source_not_found_without_artifacts() {
    for field in ["graph", "job", "resume", "personal"] {
        let harness = Harness::new();
        {
            let mut state = harness.state();
            match field {
                "graph" => state.graph = None,
                "personal" => state.personal = None,
                "job" => state.job = None,
                _ => state.resume = None,
            }
        }
        let failure = run(&harness, field == "resume").await.unwrap_err();
        assert_eq!(failure.error.code(), ErrorCode::NotFound, "{field}");
        assert!(harness.known().is_empty());
    }
}

#[tokio::test]
async fn malformed_renderer_output_never_reserved() {
    let cases = [
        (b"bad".to_vec(), 3, 1),
        (PDF.to_vec(), 0, 1),
        (PDF.to_vec(), PDF.len(), 0),
        (PDF.to_vec(), PDF.len(), 3),
    ];
    for (data, byte_size, page_count) in cases {
        let harness = Harness::new();
        harness.state().render_override = Some(RenderedGermanPdf {
            data,
            byte_size,
            page_count,
        });
        run(&harness, false).await.unwrap_err();
        assert!(harness.known().is_empty());
    }
}

#[tokio::test]
async fn classified_replace_error_preserved_with_compensation() {
    for letter in [false, true] {
        let harness = Harness::new();
        let original = conflict("Fixed revision conflict", [("resource", "profile")]);
        harness.set_replace(Replace::Fail(AppError::Application(original.clone())));
        let failure = run(&harness, letter).await.unwrap_err();
        assert_eq!(failure.error.to_envelope(), original.to_envelope());
        assert!(failure.compensation_confirmed);
        assert_eq!(harness.cleaned().len(), if letter { 1 } else { 2 });
    }
}

#[tokio::test]
async fn uncertain_commit_does_not_retry_or_shallow_reconcile() {
    for letter in [false, true] {
        let harness = Harness::new();
        harness.set_replace(Replace::Fail(AppError::Internal));
        harness.state().cleanup = VecDeque::from([
            Ok(PreparationOutcome::Referenced),
            Ok(PreparationOutcome::Referenced),
        ]);
        let failure = run(&harness, letter).await.unwrap_err();
        assert!(failure.compensation_confirmed);
        let calls = harness.calls();
        assert_eq!(calls.iter().filter(|call| *call == "replace").count(), 1);
        assert_eq!(
            calls.iter().filter(|call| *call == "resume").count(),
            usize::from(letter)
        );
        assert_eq!(harness.cleaned().len(), harness.known().len());
    }
}

#[tokio::test]
async fn invalid_authoritative_result_compensated() {
    for letter in [false, true] {
        for substitution in [
            Replace::Parent,
            Replace::Variant,
            Replace::DuplicatePrevious,
        ] {
            let harness = Harness::new();
            harness.set_replace(substitution);
            run(&harness, letter).await.unwrap_err();
            assert_eq!(harness.cleaned().len(), harness.known().len());
            assert!(!harness.known().is_empty());
        }
    }
}

#[tokio::test]
async fn review_before_render_or_reservation() {
    let harness = Harness::new();
    harness.state().graph.as_mut().unwrap().experiences.clear();
    let failure = run(&harness, false).await.unwrap_err();
    assert!(failure.error.message().contains("content review failed"));
    assert_eq!(failure.error.code(), ErrorCode::ValidationError);
    assert!(harness.known().is_empty());
    assert_eq!(harness.rendered_count(), 0);
}

#[tokio::test]
async fn only_active_materialization_linked() {
    for outcome in [
        MaterializationOutcome::CleanupPending,
        MaterializationOutcome::Blocked,
        MaterializationOutcome::Ineligible,
    ] {
        let harness = Harness::new();
        harness.state().outcome = Some(outcome);
        run(&harness, false).await.unwrap_err();
        assert_eq!(harness.cleaned(), vec![harness.known()[0]]);
        assert!(!harness.calls().contains(&"replace".to_owned()));
    }
}

#[tokio::test]
async fn combined_bilingual_budget_precedes_render_and_assets() {
    let harness = Harness::new();
    harness.state().graph.as_mut().unwrap().profile.summary = Some("x".repeat(500_001));
    let failure = run(&harness, false).await.unwrap_err();
    assert!(failure.error.message().contains("limit exceeded"));
    assert_eq!(harness.calls(), strings(&["profile", "job", "personal"]));
    assert!(harness.known().is_empty());
    assert!(harness.cleaned().is_empty());
}

#[tokio::test]
async fn storage_invalid_text_precedes_renderer() {
    let harness = Harness::new();
    harness.state().graph.as_mut().unwrap().profile.email = "private\0@example.test".to_owned();
    run(&harness, false).await.unwrap_err();
    assert_eq!(harness.rendered_count(), 0);
    assert!(harness.known().is_empty());
    assert!(harness.cleaned().is_empty());
}

#[tokio::test]
async fn personal_row_owner_mismatch_precedes_renderer() {
    let harness = Harness::new();
    harness
        .state()
        .personal
        .as_mut()
        .unwrap()
        .personal_details
        .as_mut()
        .unwrap()
        .profile_id = Uuid::new_v4();
    run(&harness, false).await.unwrap_err();
    assert_eq!(harness.rendered_count(), 0);
    assert!(harness.known().is_empty());
}

#[tokio::test]
async fn unproven_reservation_never_authorizes_retirement() {
    for tamper in [Tamper::Root, Tamper::Sha, Tamper::Size] {
        let harness = Harness::new();
        harness.state().tamper = Some(tamper);
        let failure = run(&harness, false).await.unwrap_err();
        assert!(harness.cleaned().is_empty());
        assert!(!failure.compensation_confirmed);
        assert!(!harness.calls().contains(&"materialize-1".to_owned()));
    }
}

#[tokio::test]
async fn valid_previous_variant_sets_source_replaced_flag() {
    let harness = Harness::new();
    harness.set_replace(Replace::ValidPrevious);
    assert!(
        run_resume(&harness, Value::Null)
            .await
            .unwrap()
            .replaced_existing
    );
    assert!(harness.cleaned().is_empty());
    let harness = Harness::new();
    harness.set_replace(Replace::ValidPrevious);
    assert!(run_letter(&harness).await.unwrap().replaced_existing);
    assert!(harness.cleaned().is_empty());
}

#[tokio::test]
async fn invalid_previous_row_rejected() {
    let harness = Harness::new();
    harness.set_replace(Replace::InvalidPreviousRow);
    run(&harness, false).await.unwrap_err();
    assert_eq!(harness.cleaned().len(), harness.known().len());
    let harness = Harness::new();
    harness.set_replace(Replace::InvalidPreviousRow);
    run(&harness, true).await.unwrap_err();
    assert_eq!(harness.cleaned().len(), harness.known().len());
}

#[tokio::test]
async fn actual_qualified_renderers_share_the_core_flow_without_file_io() {
    let harness = Harness::new();
    let service = harness.service_with(true, true, Arc::new(PdfGermanRenderer));
    let profile = json!(harness.profile_id.to_string());
    let job = json!(harness.job_id.to_string());
    let resume = service
        .generate_german_tailored_resume(&profile, &job, &Value::Null)
        .await
        .unwrap();
    let letter = service
        .generate_german_cover_letter(&profile, &job, &json!(harness.resume_id.to_string()))
        .await
        .unwrap();
    assert!(resume.variants.iter().all(|item| item.byte_size > 1000));
    assert!(
        resume
            .variants
            .iter()
            .all(|item| (1..=2).contains(&item.page_count))
    );
    assert!(letter.byte_size > 1000 && (1..=2).contains(&letter.page_count));
    assert!(harness.cleaned().is_empty());
}

#[tokio::test]
async fn compensation_uses_the_fences_handed_to_the_writer() {
    let harness = Harness::new();
    harness.fail_at("replace");
    let failure = run(&harness, false).await.unwrap_err();
    assert!(failure.compensation_confirmed);
    let state = harness.state();
    let written: Vec<_> = state
        .replace_claims
        .iter()
        .map(|claim| claim.intent.lease_token)
        .collect();
    assert_eq!(state.cleaned_tokens, written);
    assert_eq!(written.len(), 2);
    let bound: Vec<_> = state
        .replace_claims
        .iter()
        .map(|claim| claim.reservation.clone())
        .collect();
    assert_eq!(bound, state.replace_bindings);
    assert_eq!(
        state.cleaned,
        state
            .replace_bindings
            .iter()
            .map(|binding| binding.asset.asset_id())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn disabled_asset_lifecycle_refuses_before_any_source_read() {
    // The classified lifecycle is probed before the sources: nothing is read, and
    // no reservation exists, so compensation is confirmed.
    let harness = Harness::new();
    let service = harness.service_with(true, false, harness.clone());
    let failure = service
        .generate_german_tailored_resume(
            &json!(harness.profile_id.to_string()),
            &json!(harness.job_id.to_string()),
            &Value::Null,
        )
        .await
        .unwrap_err();
    internal_envelope(&failure);
    assert!(failure.compensation_confirmed);
    assert!(harness.calls().is_empty());
}

#[tokio::test]
async fn disabled_asset_service_fails_closed_with_the_default_envelope() {
    for letter in [false, true] {
        let harness = Harness::new();
        let service = harness.service_with(true, false, harness.clone());
        let profile = json!(harness.profile_id.to_string());
        let job = json!(harness.job_id.to_string());
        let failure = if letter {
            service
                .generate_german_cover_letter(&profile, &job, &json!(harness.resume_id.to_string()))
                .await
                .map(|_| ())
        } else {
            service
                .generate_german_tailored_resume(&profile, &job, &Value::Null)
                .await
                .map(|_| ())
        }
        .unwrap_err();
        internal_envelope(&failure);
        assert!(harness.cleaned().is_empty());
        assert!(!harness.calls().contains(&"replace".to_owned()));
    }
}

#[tokio::test]
async fn cover_letter_claim_uses_exact_selected_resume_and_fixed_german_language() {
    let harness = Harness::new();
    run_letter(&harness).await.unwrap();
    let state = harness.state();
    let [claim] = state.replace_claims.as_slice() else {
        panic!("exactly one claim");
    };
    let descriptor = &claim.intent.descriptor;
    assert_eq!(descriptor.purpose, GenerationPurpose::GermanCoverLetter);
    assert_eq!(descriptor.profile_id, Some(harness.profile_id));
    assert_eq!(descriptor.job_id, Some(harness.job_id));
    assert_eq!(descriptor.resume_id, Some(harness.resume_id));
    assert_eq!(descriptor.output_scope.as_deref(), Some("de"));
}

// The persistence mappers count the retained raw totals of the content.

#[test]
fn resume_mapper_counts_the_raw_units_of_padded_content() {
    let padded = ResumeInput {
        full_name: Some(format!("   Ada Example{}", " ".repeat(500))),
        language: Some("en".to_owned()),
        ..ResumeInput::default()
    };
    let content = StructuredResumeContent::new(padded).unwrap();
    assert_eq!(content.raw_units(), 3 + 11 + 500 + 2);
    assert!(content.full_name().len() < content.raw_units());
    let variant = ResumeVariant::new(VariantFields {
        id: Uuid::new_v4(),
        resume_id: Uuid::new_v4(),
        language: "en",
        document_id: Uuid::new_v4(),
        file_path: "asset.pdf",
        created_at: NOW,
        updated_at: NOW,
    })
    .unwrap();
    let write = to_variant_write(variant, &content, &mut || Uuid::new_v4()).unwrap();
    // The stored rows are far smaller than the padding, so the raw total decides.
    assert_eq!(write.raw_units(), content.raw_units());
}

#[test]
fn cover_letter_mapper_counts_the_raw_units_of_padded_content() {
    let content = GermanCoverLetterContent::new(CoverLetterInput {
        sender_name: Some(format!("Ada Example{}", " ".repeat(5000))),
        personal_fields: vec![PersonalField::new(Some("Email"), Some("ada@example.test")).unwrap()],
        recipient_company: None,
        recipient_location: None,
        job_title: Some("Developer".to_owned()),
        subject: Some("Bewerbung als Developer".to_owned()),
        salutation: Some("Sehr geehrte Damen und Herren,".to_owned()),
        paragraphs: raw_strings(["Erster Absatz.", "Zweiter Absatz."]),
        closing: Some("Mit freundlichen Grüßen,".to_owned()),
        signature: Some("Ada Example".to_owned()),
    })
    .unwrap();
    assert!(content.raw_units() > 5000);
    assert_eq!(content.personal_fields()[0].raw_entries(), 1);
    let parent = CoverLetter::new(CoverLetterFields {
        id: Uuid::new_v4(),
        profile_id: Uuid::new_v4(),
        job_id: Uuid::new_v4(),
        resume_id: Uuid::new_v4(),
        profile_revision: NOW,
        job_revision: NOW,
        resume_revision: NOW,
        created_at: NOW,
        updated_at: NOW,
    })
    .unwrap();
    let variant = CoverLetterVariant::new(LetterVariantFields {
        id: Uuid::new_v4(),
        cover_letter_id: parent.id(),
        format: "germany",
        language: "de",
        document_id: Uuid::new_v4(),
        file_path: "asset.pdf",
        subject: content.subject(),
        salutation: content.salutation(),
        closing: content.closing(),
        signature: content.signature(),
        created_at: NOW,
        updated_at: NOW,
    })
    .unwrap();
    let write =
        map_cover_letter_content(parent, variant, &content, &mut || Uuid::new_v4()).unwrap();
    assert_eq!(write.raw_units(), content.raw_units());
}

/// Pads the fixture's phone contact so the admitted raw totals exceed what the normalized rows
/// recount to.
fn pad_phone(harness: &Arc<Harness>) {
    let mut state = harness.state();
    let contact = state
        .graph
        .as_mut()
        .and_then(|graph| graph.contacts.first_mut())
        .unwrap();
    contact.contact_value = format!("   123{}   ", " ".repeat(500));
}

#[tokio::test]
async fn rebuilt_resume_variants_keep_the_admitted_raw_totals() {
    let harness = Harness::new();
    pad_phone(&harness);
    run_resume(&harness, Value::Null).await.unwrap();
    let state = harness.state();
    let write = state.resume_write.clone().unwrap();
    assert_eq!(write.variants().len(), state.resumes_rendered.len());
    // The German content is translated from the padded English one, so only English keeps the
    // padding.
    let english = state
        .resumes_rendered
        .iter()
        .find(|content| content.language() == "en")
        .unwrap();
    let variant = write
        .variants()
        .iter()
        .find(|variant| variant.variant().language() == "en")
        .unwrap();
    assert!(english.raw_units() > 500);
    // A plain recount of the stored rows is smaller than the admitted padding.
    let plain = VariantWrite::new(variant.variant().clone(), variant.sections().to_vec()).unwrap();
    assert!(plain.raw_units() < english.raw_units());
    assert!(variant.raw_units() >= english.raw_units());
    assert!(write.raw_units() >= english.raw_units());
}

#[tokio::test]
async fn rebuilt_cover_letter_keeps_the_admitted_raw_totals() {
    let harness = Harness::new();
    pad_phone(&harness);
    run_letter(&harness).await.unwrap();
    let state = harness.state();
    let write = state.letter_write.clone().unwrap();
    let [content] = state.letters_rendered.as_slice() else {
        panic!("exactly one rendered letter");
    };
    assert!(content.raw_units() > 500);
    let plain_variant = LetterVariantWrite::new(
        write.variant().variant().clone(),
        write.variant().paragraphs().to_vec(),
    )
    .unwrap();
    let plain =
        CoverLetterAggregateWrite::new(write.cover_letter().clone(), plain_variant).unwrap();
    // The content total stays on the aggregate only; the variant recount stays smaller.
    assert!(plain.raw_units() < content.raw_units());
    assert!(write.raw_units() >= content.raw_units());
}

// German generation tests. Tests whose subject is object mutation or duck typing do not exist
// in Rust; they are listed after these.

fn sorted(mut ids: Vec<Uuid>) -> Vec<Uuid> {
    ids.sort();
    ids
}

#[tokio::test]
async fn mutated_previous_variant_rejected() {
    for letter in [false, true] {
        let harness = Harness::new();
        harness.set_replace(Replace::InvalidPreviousRow);
        run(&harness, letter).await.unwrap_err();
        assert!(!harness.known().is_empty());
        assert_eq!(sorted(harness.cleaned()), sorted(harness.known()));
    }
}

#[tokio::test]
async fn equal_coercing_result_parent_rejected() {
    for letter in [false, true] {
        let harness = Harness::new();
        harness.set_replace(Replace::Parent);
        run(&harness, letter).await.unwrap_err();
        assert!(!harness.known().is_empty());
        assert_eq!(sorted(harness.cleaned()), sorted(harness.known()));
    }
}

#[tokio::test]
async fn mutated_replacement_binding_cannot_redirect_retirement() {
    for letter in [false, true] {
        let harness = Harness::new();
        harness.set_replace(Replace::Fail(AppError::Internal));
        let failure = run(&harness, letter).await.unwrap_err();
        assert!(failure.compensation_confirmed);
        let state = harness.state();
        let bound: Vec<Uuid> = state
            .replace_bindings
            .iter()
            .map(|binding| binding.asset.asset_id())
            .collect();
        assert!(!bound.is_empty());
        assert_eq!(state.cleaned, bound);
    }
}

#[tokio::test]
async fn replacement_success_mutation_does_not_change_returned_proven_binding() {
    for letter in [false, true] {
        let harness = Harness::new();
        let returned: Vec<GeneratedReservation> = if letter {
            vec![run_letter(&harness).await.unwrap().reservation]
        } else {
            run_resume(&harness, Value::Null)
                .await
                .unwrap()
                .variants
                .into_iter()
                .map(|variant| variant.reservation)
                .collect()
        };
        let dispatched: Vec<Uuid> = harness
            .state()
            .replace_bindings
            .iter()
            .map(|binding| binding.asset.asset_id())
            .collect();
        let returned_ids: Vec<Uuid> = returned
            .iter()
            .map(|binding| binding.asset.asset_id())
            .collect();
        assert_eq!(returned_ids, dispatched);
        for binding in &returned {
            assert_eq!(
                binding.asset.relative_path(),
                format!("{}.pdf", binding.asset.asset_id())
            );
        }
        assert!(harness.cleaned().is_empty());
    }
}

#[tokio::test]
async fn real_asset_readiness_failure_before_all_source_and_artifact_io() {
    // The repository, writer and root cannot vary; Rust takes those as required
    // constructor arguments, so only the disabled lifecycle can be missing here.
    for letter in [false, true] {
        let harness = Harness::new();
        let service = harness.service_with(true, false, harness.clone());
        let profile = json!(harness.profile_id.to_string());
        let job = json!(harness.job_id.to_string());
        let failure = if letter {
            service
                .generate_german_cover_letter(&profile, &job, &json!(harness.resume_id.to_string()))
                .await
                .map(|_| ())
        } else {
            service
                .generate_german_tailored_resume(&profile, &job, &Value::Null)
                .await
                .map(|_| ())
        }
        .unwrap_err();
        internal_envelope(&failure);
        assert!(failure.compensation_confirmed);
        assert!(harness.calls().is_empty());
        assert!(harness.known().is_empty());
        assert!(harness.cleaned().is_empty());
        assert_eq!(harness.rendered_count(), 0);
    }
}

#[tokio::test]
async fn healthy_foundation_assets_cannot_enable_classified_generation() {
    for letter in [false, true] {
        let harness = Harness::new();
        let service = harness.service_with(false, true, harness.clone());
        let profile = json!(harness.profile_id.to_string());
        let job = json!(harness.job_id.to_string());
        let failure = if letter {
            service
                .generate_german_cover_letter(&profile, &job, &json!(harness.resume_id.to_string()))
                .await
                .map(|_| ())
        } else {
            service
                .generate_german_tailored_resume(&profile, &job, &Value::Null)
                .await
                .map(|_| ())
        }
        .unwrap_err();
        assert!(failure.error.message().contains("not enabled"));
        assert!(failure.compensation_confirmed);
        assert!(harness.calls().is_empty());
        assert!(harness.known().is_empty());
        assert!(harness.cleaned().is_empty());
    }
}

#[tokio::test]
async fn mutable_writer_claims_cannot_change_bilingual_compensation_fences() {
    for letter in [false, true] {
        let harness = Harness::new();
        harness.fail_at("replace");
        let failure = run(&harness, letter).await.unwrap_err();
        assert!(failure.compensation_confirmed);
        let state = harness.state();
        let written: Vec<Option<Uuid>> = state
            .replace_claims
            .iter()
            .map(|claim| claim.intent.lease_token)
            .collect();
        assert_eq!(state.cleaned_tokens, written);
        assert_eq!(written.len(), if letter { 1 } else { 2 });
    }
}

// Cases without a Rust test, because the type system already rules out what they probe:
// - test_missing_common_ports_no_io: ports are required `Arc<dyn ...>` constructor fields, so a
//   missing profile, job, resume or asset port cannot be expressed.
// - test_mutated_materialization_cannot_redirect_retirement: `IntentReservation` and
//   `GeneratedReservation` are owned, immutable values; the port cannot rewrite the asset id or
//   path that compensation later retires.
// - test_reservation_port_cannot_mutate_requested_descriptor_authority: the descriptor is passed
//   as `&GenerationDescriptor` and has no setters, so a port cannot change its authority fields.
// - The attribute-mutation variants of the claim and binding tests above: the same
//   guarantee holds by construction, and the tests assert the observable fences instead.
