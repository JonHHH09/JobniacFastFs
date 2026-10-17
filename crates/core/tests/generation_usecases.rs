//! Generation use cases over in-memory ports: file names (golden), the standalone and profile
//! resume paths, the compensation status, the qualification gate and the facade projection.

use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jobniac_core::cleanup::{
    GenerationRootFiles, GenerationRootQualification, GenerationRootRepository, GenerationRootState,
};
use jobniac_core::clock::{FixedClock, format_instant};
use jobniac_core::documents::DocumentMetadata;
use jobniac_core::documents::model::PDF_MEDIA_TYPE;
use jobniac_core::errors::{AppError, ErrorCode};
use jobniac_core::generated_assets::identity::{PublicationFailure, PublicationOutcome};
use jobniac_core::generated_assets::service::sha256_hex;
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedAssetRepository, GeneratedAssetService, GeneratedAssetWriter,
    GeneratedReservation, GenerationDescriptor, GenerationIntent, GenerationPurpose,
    GenerationState, IntentReservation, MaterializationOutcome, PreparationOutcome,
    ProfileResumeDocument, ReplaceResult,
};
use jobniac_core::generation::facade::{DocumentMetadataPort, GenerationService};
use jobniac_core::generation::generic::{
    GenericGenerationService, ProfileGraphPort, ProfileResumeLinkPort,
};
use jobniac_core::generation::standalone::{StandaloneGenerationService, standalone_file_name};
use jobniac_core::generation::translation::FrenchTranslator;
use jobniac_core::profiles::model::{ProfileGraph, ProfileRecord};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const VECTORS: &str = include_str!("golden/generation/standalone-file-names.json");

/// A fixed instant for every clock in the tests.
fn instant() -> Result<OffsetDateTime, Box<dyn Error>> {
    Ok(OffsetDateTime::from_unix_timestamp(1_800_000_000)?)
}

/// The synthetic in-memory state shared by the fakes: claims, names, bytes and publications.
#[derive(Default)]
struct Store {
    claims: Mutex<HashMap<Uuid, IntentReservation>>,
    names: Mutex<HashMap<Uuid, String>>,
    bytes: Mutex<HashMap<Uuid, Vec<u8>>>,
    published: Mutex<Vec<Uuid>>,
    abandoned: Mutex<Vec<Uuid>>,
    fail_materialize: bool,
}

/// The classified repository over [`Store`].
struct StoreRepository(Arc<Store>);

#[async_trait]
impl GeneratedAssetRepository for StoreRepository {
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
        let intent =
            GenerationIntent::pending(identity.asset_id(), descriptor.clone(), lease_token, now)
                .map_err(|_| AppError::Internal)?;
        let claim = IntentReservation::new(
            GeneratedReservation {
                asset: identity.clone(),
                document_id,
            },
            intent,
        )
        .map_err(|_| AppError::Internal)?;
        self.0
            .claims
            .lock()
            .map_err(|_| AppError::Internal)?
            .insert(identity.asset_id(), claim.clone());
        self.0
            .names
            .lock()
            .map_err(|_| AppError::Internal)?
            .insert(identity.asset_id(), original_file_name.to_owned());
        self.0
            .bytes
            .lock()
            .map_err(|_| AppError::Internal)?
            .insert(identity.asset_id(), content.to_vec());
        Ok(claim)
    }

    async fn find_classified_reservation(
        &self,
        asset_id: Uuid,
    ) -> Result<Option<IntentReservation>, AppError> {
        Ok(self
            .0
            .claims
            .lock()
            .map_err(|_| AppError::Internal)?
            .get(&asset_id)
            .cloned())
    }

    async fn materialize_classified(
        &self,
        claim: &IntentReservation,
        writer: &dyn GeneratedAssetWriter,
    ) -> Result<MaterializationOutcome, AppError> {
        if self.0.fail_materialize {
            return Err(AppError::Internal);
        }
        let content = self
            .0
            .bytes
            .lock()
            .map_err(|_| AppError::Internal)?
            .get(&claim.asset_id())
            .cloned()
            .ok_or(AppError::Internal)?;
        writer
            .materialize_owned(&claim.reservation.asset, &content)
            .map_err(|_| AppError::Internal)?;
        let mut claims = self.0.claims.lock().map_err(|_| AppError::Internal)?;
        let stored = claims
            .get_mut(&claim.asset_id())
            .ok_or(AppError::Internal)?;
        if claim.intent.descriptor.purpose == GenerationPurpose::StandalonePdf {
            stored.intent.state = GenerationState::Completed;
            stored.intent.finished_at = Some(stored.intent.created_at);
        }
        self.0
            .published
            .lock()
            .map_err(|_| AppError::Internal)?
            .push(claim.asset_id());
        Ok(MaterializationOutcome::Active)
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
        let mut claims = self.0.claims.lock().map_err(|_| AppError::Internal)?;
        let Some(stored) = claims.get_mut(&claim.asset_id()) else {
            return Ok(PreparationOutcome::Ineligible);
        };
        stored.intent.state = GenerationState::Abandoned;
        self.0
            .abandoned
            .lock()
            .map_err(|_| AppError::Internal)?
            .push(claim.asset_id());
        Ok(PreparationOutcome::Prepared)
    }

    async fn recover_expired_generations(
        &self,
        _root_id: Uuid,
        _limit: usize,
    ) -> Result<Vec<Uuid>, AppError> {
        Ok(Vec::new())
    }
}

/// The owned-file writer fake: accepts every identical publication and records it.
struct StoreWriter;

impl GeneratedAssetWriter for StoreWriter {
    fn materialize_owned(
        &self,
        _identity: &AssetIdentity,
        _content: &[u8],
    ) -> Result<PublicationOutcome, PublicationFailure> {
        Ok(PublicationOutcome::Created)
    }
}

/// The document metadata fake: reads the reserved bytes and name of the document's asset.
struct StoreDocuments(Arc<Store>);

#[async_trait]
impl DocumentMetadataPort for StoreDocuments {
    async fn get_document_metadata(&self, document_id: &str) -> Result<DocumentMetadata, AppError> {
        let claims = self.0.claims.lock().map_err(|_| AppError::Internal)?;
        let claim = claims
            .values()
            .find(|claim| claim.reservation.document_id.to_string() == document_id)
            .ok_or(AppError::Internal)?;
        let names = self.0.names.lock().map_err(|_| AppError::Internal)?;
        let bytes = self.0.bytes.lock().map_err(|_| AppError::Internal)?;
        let content = bytes.get(&claim.asset_id()).ok_or(AppError::Internal)?;
        let at = format_instant(instant().map_err(|_| AppError::Internal)?);
        Ok(DocumentMetadata {
            id: claim.reservation.document_id,
            original_file_name: names
                .get(&claim.asset_id())
                .cloned()
                .ok_or(AppError::Internal)?,
            media_type: PDF_MEDIA_TYPE.to_owned(),
            byte_size: content.len() as i64,
            sha256: sha256_hex(content),
            created_at: at.clone(),
            updated_at: at,
        })
    }
}

/// The profile graph fake: one fixed synthetic profile, or none.
struct Graphs(Option<ProfileGraph>);

#[async_trait]
impl ProfileGraphPort for Graphs {
    async fn find_profile(&self, profile_id: Uuid) -> Result<Option<ProfileGraph>, AppError> {
        Ok(self
            .0
            .clone()
            .filter(|graph| graph.profile.id == profile_id))
    }
}

/// The link fake: saves the link it was given and reports no previous link. With `corrupt`, the
/// reported saved link has another identifier, so the result check fails after the save.
#[derive(Default)]
struct Links {
    corrupt: bool,
}

#[async_trait]
impl ProfileResumeLinkPort for Links {
    async fn replace_link(
        &self,
        link: &ProfileResumeDocument,
        _binding: &GeneratedReservation,
        _expected_revision: i64,
        _expected_updated_at: OffsetDateTime,
        _claim: Option<&IntentReservation>,
    ) -> Result<ReplaceResult, AppError> {
        let mut saved = link.clone();
        if self.corrupt {
            saved.id = Uuid::new_v4();
        }
        ReplaceResult::new(saved, None).map_err(|_| AppError::Internal)
    }
}

/// The registry fake for the qualification gate.
#[derive(Debug)]
struct Registry {
    registered: bool,
}

#[async_trait]
impl GenerationRootRepository for Registry {
    async fn read_generation_root_state(
        &self,
        _root_id: Uuid,
        _now: OffsetDateTime,
    ) -> Result<GenerationRootState, AppError> {
        Ok(GenerationRootState {
            registered: self.registered,
            expired_pending: false,
        })
    }
}

/// The owned namespace fake: always qualified.
#[derive(Debug)]
struct Qualified;

impl GenerationRootFiles for Qualified {
    fn qualify_owned_root(&self) -> Result<(), jobniac_core::cleanup::RemovalFailure> {
        Ok(())
    }
}

/// A synthetic profile with a name, an email and one skill.
fn profile(id: Uuid) -> Result<ProfileGraph, Box<dyn Error>> {
    let at = instant()?;
    Ok(ProfileGraph {
        profile: ProfileRecord {
            id,
            full_name: "Ada Example".to_owned(),
            email: "ada@example.com".to_owned(),
            summary: Some("Synthetic summary.".to_owned()),
            raw_resume_text: None,
            created_at: at,
            updated_at: at,
            embedding: None,
            revision: 3,
        },
        contacts: Vec::new(),
        links: Vec::new(),
        skills: Vec::new(),
        languages: Vec::new(),
        education: Vec::new(),
        experiences: Vec::new(),
        projects: Vec::new(),
        project_technologies: Vec::new(),
    })
}

/// The wired use cases over one store, with the given opt-in and registration.
struct Wiring {
    root: Uuid,
    standalone: StandaloneGenerationService,
    generic: GenericGenerationService,
    facade: GenerationService,
}

fn wire(
    store: Arc<Store>,
    graph: Option<ProfileGraph>,
    enabled: bool,
    registered: bool,
) -> Result<Wiring, Box<dyn Error>> {
    wire_with(store, graph, enabled, registered, Links::default())
}

fn wire_with(
    store: Arc<Store>,
    graph: Option<ProfileGraph>,
    enabled: bool,
    registered: bool,
    links: Links,
) -> Result<Wiring, Box<dyn Error>> {
    let root = Uuid::new_v4();
    let clock = Arc::new(FixedClock::new(instant()?));
    let assets = Arc::new(GeneratedAssetService::new(
        Arc::new(StoreRepository(Arc::clone(&store))),
        Arc::new(StoreWriter),
        root,
        clock.clone(),
        enabled,
    ));
    let standalone =
        StandaloneGenerationService::new(Arc::clone(&assets), root, clock.clone(), enabled);
    let generic = GenericGenerationService::new(
        Arc::new(Graphs(graph)),
        Arc::new(links),
        Arc::clone(&assets),
        Arc::new(FrenchTranslator::builtin()),
        root,
        clock.clone(),
        enabled,
    );
    let qualification = Arc::new(GenerationRootQualification::new(
        Some(Arc::new(Registry { registered })),
        Some(Arc::new(Qualified)),
        Some(root),
        enabled,
    ));
    let facade = GenerationService::new(
        StandaloneGenerationService::new(Arc::clone(&assets), root, clock.clone(), enabled),
        GenericGenerationService::new(
            Arc::new(Graphs(None)),
            Arc::new(Links::default()),
            Arc::clone(&assets),
            Arc::new(FrenchTranslator::builtin()),
            root,
            clock.clone(),
            enabled,
        ),
        Arc::new(StoreDocuments(Arc::clone(&store))),
        qualification,
        clock,
    );
    Ok(Wiring {
        root,
        standalone,
        generic,
        facade,
    })
}

#[test]
fn standalone_file_names_match_the_golden_vectors() -> TestResult {
    let vectors: Value = serde_json::from_str(VECTORS)?;
    let cases = vectors.as_array().ok_or("vectors must be a list")?;
    assert_eq!(cases.len(), 16);
    for case in cases {
        let input = case["input"].as_str();
        match standalone_file_name(input) {
            Ok(name) => {
                assert_eq!(case["ok"], json!(true), "input {input:?}");
                assert_eq!(Value::String(name), case["value"], "input {input:?}");
            }
            Err(error) => {
                assert_eq!(case["ok"], json!(false), "input {input:?}");
                assert_eq!(
                    json!(error.code().code()),
                    case["error"]["code"],
                    "input {input:?}"
                );
                assert_eq!(
                    json!(error.message()),
                    case["error"]["message"],
                    "input {input:?}"
                );
                for (key, value) in case["error"]["details"].as_object().ok_or("details")? {
                    assert_eq!(
                        error.details().get(key).map(String::as_str),
                        value.as_str(),
                        "input {input:?} detail {key}"
                    );
                }
            }
        }
    }
    Ok(())
}

#[test]
fn file_name_refuses_a_nul_and_an_oversized_raw_name() {
    assert!(standalone_file_name(Some("a\u{0}.pdf")).is_err());
    let raw = "x".repeat(65_537);
    assert!(standalone_file_name(Some(&raw)).is_err());
}

#[tokio::test]
async fn standalone_pdf_is_published_once_with_the_logical_name() -> TestResult {
    let store = Arc::new(Store::default());
    let wiring = wire(Arc::clone(&store), None, true, true)?;
    let generated = wiring
        .standalone
        .generate_pdf_file(
            Some("Ada Example.pdf"),
            Some("Ada CV"),
            Some("Summary line"),
        )
        .await
        .map_err(|failure| failure.error)?;
    assert_eq!(generated.file_name, "Ada_Example.pdf");
    assert_eq!(generated.page_count, 1);
    assert!(generated.byte_size > 0);
    assert_eq!(generated.reservation.asset.root_id(), wiring.root);
    let published = store.published.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(published, vec![generated.reservation.asset.asset_id()]);
    Ok(())
}

#[tokio::test]
async fn disabled_standalone_generation_refuses_before_publication() -> TestResult {
    let store = Arc::new(Store::default());
    let wiring = wire(Arc::clone(&store), None, false, true)?;
    let failure = wiring
        .standalone
        .generate_pdf_file(None, None, Some("Body"))
        .await
        .err()
        .ok_or("disabled generation must fail")?;
    assert_eq!(failure.error.code(), ErrorCode::InternalError);
    assert_eq!(
        failure.error.message(),
        "Standalone generation is not enabled"
    );
    let claims = store.claims.lock().map_err(|_| "poisoned")?.len();
    assert_eq!(claims, 0);
    Ok(())
}

#[tokio::test]
async fn blank_body_is_refused_before_any_root_io() -> TestResult {
    let store = Arc::new(Store::default());
    let wiring = wire(Arc::clone(&store), None, true, true)?;
    let error = wiring
        .standalone
        .generate_pdf_file(None, None, Some("   "))
        .await
        .err()
        .ok_or("a blank body must fail")?
        .error;
    assert_eq!(error.code(), ErrorCode::ValidationError);
    assert_eq!(
        error.details().get("field").map(String::as_str),
        Some("body")
    );
    let claims = store.claims.lock().map_err(|_| "poisoned")?.len();
    assert_eq!(claims, 0);
    Ok(())
}

#[tokio::test]
async fn master_resume_links_the_profile_to_the_committed_pdf() -> TestResult {
    let id = Uuid::new_v4();
    let wiring = wire(Arc::new(Store::default()), Some(profile(id)?), true, true)?;
    let generated = wiring
        .generic
        .generate_pdf_resume(id)
        .await
        .map_err(|failure| failure.error)?;
    assert!(generated.file_name.starts_with("master-resume-"));
    assert_eq!(generated.link.resume_type, "master_resume");
    assert_eq!(
        generated.link.document_id,
        generated.reservation.document_id
    );
    assert_eq!(
        generated.link.file_path,
        generated.reservation.asset.relative_path()
    );
    assert!(!generated.replaced_existing_link);
    Ok(())
}

#[tokio::test]
async fn missing_profile_is_not_found_with_the_documented_message() -> TestResult {
    let id = Uuid::new_v4();
    let wiring = wire(Arc::new(Store::default()), None, true, true)?;
    let error = wiring
        .generic
        .generate_pdf_resume(id)
        .await
        .err()
        .ok_or("a missing profile must fail")?
        .error;
    assert_eq!(error.code(), ErrorCode::NotFound);
    assert_eq!(error.message(), format!("Profile not found: {id}"));
    Ok(())
}

#[tokio::test]
async fn failed_materialization_confirms_the_abandonment() -> TestResult {
    let id = Uuid::new_v4();
    let store = Arc::new(Store {
        fail_materialize: true,
        ..Store::default()
    });
    let wiring = wire(Arc::clone(&store), Some(profile(id)?), true, true)?;
    let failure = wiring
        .generic
        .generate_pdf_resume(id)
        .await
        .err()
        .ok_or("a failed materialization must fail")?;
    assert!(failure.compensation_confirmed);
    let abandoned = store.abandoned.lock().map_err(|_| "poisoned")?.len();
    assert_eq!(abandoned, 1);
    Ok(())
}

#[tokio::test]
async fn a_failure_after_the_link_is_saved_never_abandons_the_claim() -> TestResult {
    let id = Uuid::new_v4();
    let store = Arc::new(Store::default());
    let wiring = wire_with(
        Arc::clone(&store),
        Some(profile(id)?),
        true,
        true,
        Links { corrupt: true },
    )?;
    let failure = wiring
        .generic
        .generate_pdf_resume(id)
        .await
        .err()
        .ok_or("a corrupt replacement result must fail")?;
    assert!(failure.compensation_confirmed);
    assert_eq!(store.abandoned.lock().map_err(|_| "poisoned")?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn unregistered_root_refuses_every_facade_operation() -> TestResult {
    let wiring = wire(Arc::new(Store::default()), None, true, false)?;
    let error = wiring
        .facade
        .generate_pdf_file(&json!({"body": "Body"}))
        .await
        .err()
        .ok_or("an unregistered root must refuse")?;
    assert_eq!(error.code(), ErrorCode::InternalError);
    assert_eq!(error.message(), "Generation is not available");
    Ok(())
}

#[tokio::test]
async fn facade_projects_the_standalone_metadata_with_the_documented_keys() -> TestResult {
    let wiring = wire(Arc::new(Store::default()), None, true, true)?;
    let value = wiring
        .facade
        .generate_pdf_file(&json!({"fileName": "cv.pdf", "body": "Body"}))
        .await
        .map_err(|error| error.message().to_owned())?;
    assert_eq!(value["fileName"], json!("cv.pdf"));
    assert_eq!(value["pageCount"], json!(1));
    assert!(value.get("documentId").is_none());
    let desktop = wiring
        .facade
        .generate_pdf_file_for_desktop(&json!({"fileName": "cv.pdf", "body": "Body"}))
        .await
        .map_err(|error| error.message().to_owned())?;
    assert!(desktop["documentId"].is_string());
    Ok(())
}

#[tokio::test]
async fn facade_projects_the_profile_resume_and_refuses_bad_identifiers() -> TestResult {
    let id = Uuid::new_v4();
    let wiring = wire(Arc::new(Store::default()), None, true, true)?;
    let error = wiring
        .facade
        .generate_pdf_resume(&json!({"profileId": "not-a-uuid"}))
        .await
        .err()
        .ok_or("a bad identifier must fail")?;
    assert_eq!(error.code(), ErrorCode::ValidationError);
    assert_eq!(
        error.details().get("reason").map(String::as_str),
        Some("must be a valid UUID")
    );
    let error = wiring
        .facade
        .generate_pdf_resume(&json!("text"))
        .await
        .err()
        .ok_or("a non-object request must fail")?;
    assert_eq!(error.message(), "Invalid profile resume generation request");
    // A missing profile passes the generic not-found error through the facade unchanged.
    let error = wiring
        .facade
        .generate_pdf_resume(&json!({"profileId": id.to_string()}))
        .await
        .err()
        .ok_or("a missing profile must fail")?;
    assert_eq!(error.code(), ErrorCode::NotFound);
    Ok(())
}

#[tokio::test]
async fn facade_profile_projection_carries_the_link_and_document() -> TestResult {
    let id = Uuid::new_v4();
    let store = Arc::new(Store::default());
    let root = Uuid::new_v4();
    let clock = Arc::new(FixedClock::new(instant()?));
    let assets = Arc::new(GeneratedAssetService::new(
        Arc::new(StoreRepository(Arc::clone(&store))),
        Arc::new(StoreWriter),
        root,
        clock.clone(),
        true,
    ));
    let facade = GenerationService::new(
        StandaloneGenerationService::new(Arc::clone(&assets), root, clock.clone(), true),
        GenericGenerationService::new(
            Arc::new(Graphs(Some(profile(id)?))),
            Arc::new(Links::default()),
            Arc::clone(&assets),
            Arc::new(FrenchTranslator::builtin()),
            root,
            clock.clone(),
            true,
        ),
        Arc::new(StoreDocuments(Arc::clone(&store))),
        Arc::new(GenerationRootQualification::new(
            Some(Arc::new(Registry { registered: true })),
            Some(Arc::new(Qualified)),
            Some(root),
            true,
        )),
        clock,
    );
    let value = facade
        .generate_pdf_resume(&json!({"profileId": id.to_string()}))
        .await
        .map_err(|error| error.message().to_owned())?;
    assert_eq!(value["profileId"], json!(id.to_string()));
    assert_eq!(value["resumeType"], json!("master_resume"));
    assert_eq!(value["replacedExistingLink"], json!(false));
    assert_eq!(value["document"]["mediaType"], json!(PDF_MEDIA_TYPE));
    assert_eq!(value["generatedFile"]["pageCount"], json!(1));
    assert!(value["linkedAt"].is_string());
    Ok(())
}
