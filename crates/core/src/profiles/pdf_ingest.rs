//! Profile ingestion from a stored PDF.
//!
//! The use case reads the stored extraction through the document port, answers reuse and
//! already-linked outcomes without mutating anything, and otherwise runs the deterministic
//! extractor outside any transaction. The persistence port repeats every identity check under
//! locks and performs the canonical profile write and the one-to-one source link in one
//! transaction, in a fixed lock order.
//!
//! The use case holds the orchestration and the decisions. The SQL and the advisory locks are in
//! `jobniac_infra::postgres::profiles::pdf`, and the composition is in `jobniac_runtime`.

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::clock::{Clock, format_instant};
use crate::documents::{
    DocumentMetadata, DocumentRepository, DocumentService, MAX_CHARACTERS_LIMIT, StoredExtraction,
    safe_basename, sha256_hex,
};
use crate::errors::{AppError, ApplicationError, ErrorCode, not_found, validation_error};
use crate::ids::UuidRefusal;

use super::draft::{ProfileDraft, canonicalize_profile_write};
use super::extract::DeterministicProfileTextExtractor;
use super::graph::build_profile_graph;
use super::repository::{ProfileRepository, READ_ONLY, READ_WRITE, UnitOfWork};
use super::revision::{next_revision, persisted_revision, profile_conflict, profile_not_found};
use super::values::expected_revision;

/// The message of every ingestion request validation error.
pub const INGEST_VALIDATION_MESSAGE: &str = "Invalid profile ingestion request";
/// The message of a `maxCharacters` validation error.
pub const MAX_CHARACTERS_MESSAGE: &str = "Invalid PDF text extraction request";
/// The message of an `expectedRevision` validation error.
pub const REVISION_MESSAGE: &str = "Invalid profile request";
/// The message of a missing source.
pub const SOURCE_NOT_FOUND_MESSAGE: &str = "Profile PDF source was not found";
/// The `status` of a reused source.
pub const STATUS_REUSED: &str = "REUSED_EXISTING_SOURCE";
/// The `status` of a duplicate profile candidate.
pub const STATUS_DUPLICATE: &str = "DUPLICATE_PROFILE_CANDIDATE";
/// The `status` of several profile candidates.
pub const STATUS_AMBIGUOUS: &str = "AMBIGUOUS_PROFILE_CANDIDATES";
/// The `status` of a created profile.
pub const STATUS_CREATED: &str = "CREATED_PROFILE";
/// The `status` of an overwritten profile.
pub const STATUS_UPDATED: &str = "UPDATED_PROFILE";
/// The recommended action of an identity candidate.
pub const RECOMMEND_OVERWRITE: &str =
    "rerun with existingProfileId and overwriteExistingProfile=true to replace";
/// The recommended action of a profile that already has a source.
pub const RECOMMEND_NO_MUTATION: &str =
    "profile already has a linked PDF source; no mutation was performed";
/// The conflict message of a source linked to another profile.
pub const ALREADY_LINKED_MESSAGE: &str = "Profile is already linked to a PDF extraction";
/// The validation message of an existing profile without the overwrite opt-in.
pub const OVERWRITE_REQUIRED_MESSAGE: &str =
    "Existing profile ingestion requires overwriteExistingProfile=true";
/// The internal error when the document port returns no extraction identifier.
pub const PROVENANCE_MISSING_MESSAGE: &str = "Stored PDF extraction provenance is missing";

/// The stored-document port the ingestion needs: the extraction of a stored PDF with provenance.
/// The document use case implements it below.
pub trait StoredPdfDocuments: Send + Sync {
    /// Reads the safe metadata of a stored document without extracting or persisting anything.
    /// The ingestion checks the file name through it before the
    /// extraction is stored.
    fn stored_document_metadata(
        &self,
        document_id: &str,
    ) -> impl Future<Output = Result<DocumentMetadata, AppError>>;

    /// Extracts the stored document and persists the extraction (`persistExtraction`).
    fn extract_stored_pdf_text(
        &self,
        request: &Value,
    ) -> impl Future<Output = Result<StoredExtraction, AppError>>;
}

impl<U, R> StoredPdfDocuments for DocumentService<U, R>
where
    U: crate::profiles::repository::UnitOfWork,
    R: DocumentRepository<Tx = U::Tx>,
{
    fn stored_document_metadata(
        &self,
        document_id: &str,
    ) -> impl Future<Output = Result<DocumentMetadata, AppError>> {
        DocumentService::get_document_metadata(self, document_id)
    }

    fn extract_stored_pdf_text(
        &self,
        request: &Value,
    ) -> impl Future<Output = Result<StoredExtraction, AppError>> {
        DocumentService::extract_stored_pdf_text(self, request)
    }
}

impl<T: StoredPdfDocuments> StoredPdfDocuments for Arc<T> {
    fn stored_document_metadata(
        &self,
        document_id: &str,
    ) -> impl Future<Output = Result<DocumentMetadata, AppError>> {
        (**self).stored_document_metadata(document_id)
    }

    fn extract_stored_pdf_text(
        &self,
        request: &Value,
    ) -> impl Future<Output = Result<StoredExtraction, AppError>> {
        (**self).extract_stored_pdf_text(request)
    }
}

/// The base name of a stored document for the ingestion. A name that reduces to a
/// relative marker (`.` or `..`) is not a base name, so it is refused with the request's
/// validation error. The check runs before any extraction is stored.
fn checked_stored_name(original_file_name: &str) -> Result<String, AppError> {
    let base = safe_basename(original_file_name);
    if base == "." || base == ".." {
        return Err(AppError::from(validation_error(
            INGEST_VALIDATION_MESSAGE,
            Some("originalFileName"),
            Some("must be a file name"),
        )));
    }
    Ok(base)
}

/// A source link that already exists for the stored document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedSource {
    /// The one-to-one source link identifier.
    pub source_id: Uuid,
    /// The profile the source belongs to.
    pub profile_id: Uuid,
    /// The stored document of the extraction.
    pub document_id: Uuid,
    /// The extraction the link points at.
    pub pdf_extraction_id: Uuid,
    /// The stored original file name (reduced to a base name by the use case).
    pub original_file_name: String,
    /// The page count of the extraction.
    pub page_count: i64,
    /// The character count of the extraction.
    pub character_count: i64,
    /// Whether the extraction text was cut.
    pub truncated: bool,
}

/// The one-to-one PDF source of a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilePdfSource {
    /// The source link identifier.
    pub id: Uuid,
    /// The profile.
    pub profile_id: Uuid,
    /// The extraction the profile was ingested from.
    pub pdf_extraction_id: Uuid,
    /// The source type, `resume_pdf` by default.
    pub source_type: String,
    /// The creation instant of the link.
    pub created_at: OffsetDateTime,
}

/// The new source link inserted by a successful ingestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSourceLink {
    /// The identifier of the link.
    pub id: Uuid,
    /// The profile the link belongs to.
    pub profile_id: Uuid,
    /// The extraction the link points at.
    pub extraction_id: Uuid,
    /// The creation instant.
    pub created_at: OffsetDateTime,
}

/// Persistence port of the stored-PDF ingestion. Every call takes the transaction handle that the use
/// case opens with the unit of work. The PostgreSQL adapter implements it over the same connection
/// as the profile repository, so the graph write and the link are one transaction.
#[async_trait]
pub trait ProfilePdfRepository: Send + Sync {
    /// The open transaction handle that repository calls take.
    type Tx: Send;

    /// Takes a transaction-scoped advisory lock on a hashed key (SQL `hashtextextended`).
    async fn lock(&self, tx: &mut Self::Tx, hashed_key: &str) -> Result<(), AppError>;

    /// Locks the profile root row (`FOR UPDATE`); a missing row is not an error here.
    async fn lock_profile_root(&self, tx: &mut Self::Tx, profile_id: Uuid) -> Result<(), AppError>;

    /// The newest source linked to a PDF extraction.
    async fn linked_by_extraction(
        &self,
        tx: &mut Self::Tx,
        extraction_id: Uuid,
    ) -> Result<Option<LinkedSource>, AppError>;

    /// The newest source linked to any extraction of a document with this SHA-256 (second
    /// predicate).
    async fn linked_by_document_hash(
        &self,
        tx: &mut Self::Tx,
        sha256: &str,
    ) -> Result<Option<LinkedSource>, AppError>;

    /// Whether the profile already has a PDF source.
    async fn profile_has_source(
        &self,
        tx: &mut Self::Tx,
        profile_id: Uuid,
    ) -> Result<bool, AppError>;

    /// The profiles whose email matches, at most two, in a fixed order.
    async fn email_candidates(&self, tx: &mut Self::Tx, email: &str)
    -> Result<Vec<Uuid>, AppError>;

    /// The profiles with a link of this type and URL, at most two, in a fixed order.
    async fn link_candidates(
        &self,
        tx: &mut Self::Tx,
        link_type: &str,
        url: &str,
    ) -> Result<Vec<Uuid>, AppError>;

    /// Whether the profile has a link of this type and URL (the complete-reason check).
    async fn profile_has_link(
        &self,
        tx: &mut Self::Tx,
        profile_id: Uuid,
        link_type: &str,
        url: &str,
    ) -> Result<bool, AppError>;

    /// Whether the extraction still belongs to the document and the document still has the
    /// stored SHA-256 (the triple check before a write).
    async fn extraction_matches(
        &self,
        tx: &mut Self::Tx,
        extraction_id: Uuid,
        document_id: Uuid,
        sha256: &str,
    ) -> Result<bool, AppError>;

    /// `SAVEPOINT profile_pdf_write`.
    async fn savepoint(&self, tx: &mut Self::Tx) -> Result<(), AppError>;

    /// `ROLLBACK TO SAVEPOINT profile_pdf_write`.
    async fn rollback_to_savepoint(&self, tx: &mut Self::Tx) -> Result<(), AppError>;

    /// Inserts the source link with `ON CONFLICT DO NOTHING`. Returns `false` when a link already
    /// exists for the extraction or the profile.
    async fn insert_source_link(
        &self,
        tx: &mut Self::Tx,
        link: &NewSourceLink,
    ) -> Result<bool, AppError>;

    /// The PDF source of a profile, when one exists.
    async fn source_of_profile(
        &self,
        tx: &mut Self::Tx,
        profile_id: Uuid,
    ) -> Result<Option<ProfilePdfSource>, AppError>;
}

/// The lock key of a raw key string: its SHA-256 hex, which is what the database receives.
#[must_use]
pub fn lock_key(raw: &str) -> String {
    sha256_hex(raw.as_bytes())
}

/// The validated `ProfilePdfRequest`, with the bounds applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestRequest {
    /// The stored document to ingest.
    pub document_id: Uuid,
    /// The profile to replace or link, when one is given.
    pub existing_profile_id: Option<Uuid>,
    /// The explicit overwrite opt-in, when given.
    pub overwrite_existing_profile: Option<bool>,
    /// The display budget of the extraction, between 1 and 250000.
    pub max_characters: Option<i64>,
    /// The revision the caller read, between 0 and the signed 64-bit maximum.
    pub expected_revision: Option<i64>,
}

impl IngestRequest {
    /// Validates a request object in this order: document, existing profile,
    /// overwrite flag, character budget, expected revision.
    ///
    /// # Errors
    ///
    /// A `validation_error` for the first invalid field.
    pub fn parse(value: &Value) -> Result<Self, AppError> {
        let object = value.as_object().ok_or_else(|| {
            AppError::from(validation_error(
                INGEST_VALIDATION_MESSAGE,
                Some("request"),
                Some("must not be null"),
            ))
        })?;
        let document_id = uuid_field(object.get("documentId"), "documentId")?;
        let existing_profile_id = match object.get("existingProfileId") {
            None | Some(Value::Null) => None,
            value => Some(uuid_field(value, "existingProfileId")?),
        };
        let overwrite_existing_profile = match object.get("overwriteExistingProfile") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(flag)) => Some(*flag),
            Some(_) => {
                return Err(AppError::from(validation_error(
                    INGEST_VALIDATION_MESSAGE,
                    Some("overwriteExistingProfile"),
                    Some("must be a boolean"),
                )));
            }
        };
        let max_characters = match object.get("maxCharacters") {
            None | Some(Value::Null) => None,
            Some(value) => Some(max_characters_value(value)?),
        };
        let expected_revision = match object.get("expectedRevision") {
            None | Some(Value::Null) => None,
            Some(value) => Some(revision_value(value)?),
        };
        Ok(Self {
            document_id,
            existing_profile_id,
            overwrite_existing_profile,
            max_characters,
            expected_revision,
        })
    }
}

/// An integer of a request: a JSON number with no fraction. `Above` and `Below` are integers beyond the signed 64-bit range, positive and
/// negative.
enum IntegerValue {
    Int(i64),
    Above,
    Below,
}

/// The magnitude of `2^63` as an `f64`, which is exact.
const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;

/// Classifies a JSON number as an integer. Without `serde_json`'s `arbitrary_precision` feature an
/// integer literal beyond the 64-bit range is stored as a float, so an integral float of magnitude
/// at least `2^63` is treated as an out-of-range integer.
fn integer_value(value: &Value) -> Option<IntegerValue> {
    let Value::Number(number) = value else {
        return None;
    };
    if let Some(signed) = number.as_i64() {
        return Some(IntegerValue::Int(signed));
    }
    if number.is_u64() {
        return Some(IntegerValue::Above);
    }
    match number.as_f64() {
        Some(float) if float.fract() == 0.0 && float >= TWO_POW_63 => Some(IntegerValue::Above),
        Some(float) if float.fract() == 0.0 && float <= -TWO_POW_63 => Some(IntegerValue::Below),
        _ => None,
    }
}

/// `maxCharacters`: an integer from 1 to [`MAX_CHARACTERS_LIMIT`], never clamped.
fn max_characters_value(value: &Value) -> Result<i64, AppError> {
    let reason = match integer_value(value) {
        None => "must be an integer",
        Some(IntegerValue::Int(number)) if number <= 0 => "must be greater than zero",
        Some(IntegerValue::Below) => "must be greater than zero",
        Some(IntegerValue::Int(number)) if number <= limit_i64() => return Ok(number),
        Some(_) => "must be less than or equal to 250000",
    };
    Err(AppError::from(validation_error(
        MAX_CHARACTERS_MESSAGE,
        Some("maxCharacters"),
        Some(reason),
    )))
}

/// The character limit as a signed integer for comparison.
fn limit_i64() -> i64 {
    i64::try_from(MAX_CHARACTERS_LIMIT).unwrap_or(i64::MAX)
}

/// `expectedRevision`: a non-negative integer no larger than the signed 64-bit maximum.
fn revision_value(value: &Value) -> Result<i64, AppError> {
    let reason = match integer_value(value) {
        None => "must be an integer",
        Some(IntegerValue::Int(number)) if number < 0 => "must not be negative",
        // Every signed 64-bit value is within the supported range; a larger positive integer is
        // `Above`, which is refused below.
        Some(IntegerValue::Int(number)) => return Ok(number),
        Some(IntegerValue::Below) => "must not be negative",
        Some(IntegerValue::Above) => "exceeds the supported revision range",
    };
    Err(AppError::from(validation_error(
        REVISION_MESSAGE,
        Some("expectedRevision"),
        Some(reason),
    )))
}

/// A UUID field of a request: a string in the admitted form, otherwise the
/// malformed reason, including for an absent value.
fn uuid_field(value: Option<&Value>, field: &str) -> Result<Uuid, AppError> {
    let Some(Value::String(text)) = value else {
        return Err(UuidRefusal::Malformed.validation(INGEST_VALIDATION_MESSAGE, field));
    };
    crate::ids::admit_uuid(text)
        .map_err(|refusal| refusal.validation(INGEST_VALIDATION_MESSAGE, field))
}

/// The provenance of the stored extraction a request works on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Provenance {
    document_id: Uuid,
    document_sha256: String,
    original_file_name: String,
    extraction_id: Uuid,
    page_count: i64,
    character_count: i64,
    truncated: bool,
}

/// The result of an ingestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestOutcome {
    status: Option<&'static str>,
    profile_id: Option<Uuid>,
    document_id: Uuid,
    pdf_extraction_id: Uuid,
    source_link_id: Option<Uuid>,
    original_file_name: String,
    page_count: i64,
    character_count: i64,
    truncated: bool,
    created_profile: bool,
    existing_profile_link: bool,
    candidate_profile_id: Option<Uuid>,
    matched_on: Vec<String>,
    recommended_action: Option<String>,
}

impl IngestOutcome {
    /// The base result of a stored extraction.
    fn base(provenance: &Provenance) -> Self {
        Self {
            status: None,
            profile_id: None,
            document_id: provenance.document_id,
            pdf_extraction_id: provenance.extraction_id,
            source_link_id: None,
            original_file_name: provenance.original_file_name.clone(),
            page_count: provenance.page_count,
            character_count: provenance.character_count,
            truncated: provenance.truncated,
            created_profile: false,
            existing_profile_link: false,
            candidate_profile_id: None,
            matched_on: Vec::new(),
            recommended_action: None,
        }
    }

    /// A reused source.
    fn reused(linked: LinkedSource) -> Self {
        Self {
            status: Some(STATUS_REUSED),
            profile_id: Some(linked.profile_id),
            document_id: linked.document_id,
            pdf_extraction_id: linked.pdf_extraction_id,
            source_link_id: Some(linked.source_id),
            original_file_name: safe_basename(&linked.original_file_name),
            page_count: linked.page_count,
            character_count: linked.character_count,
            truncated: linked.truncated,
            created_profile: false,
            existing_profile_link: true,
            candidate_profile_id: None,
            matched_on: Vec::new(),
            recommended_action: None,
        }
    }

    /// A duplicate or ambiguous candidate.
    fn candidate(
        provenance: &Provenance,
        profile_id: Uuid,
        matched_on: Vec<String>,
        ambiguous: bool,
    ) -> Self {
        let mut outcome = Self::base(provenance);
        outcome.status = Some(if ambiguous {
            STATUS_AMBIGUOUS
        } else {
            STATUS_DUPLICATE
        });
        outcome.candidate_profile_id = Some(profile_id);
        outcome.matched_on = matched_on;
        outcome.recommended_action = Some(RECOMMEND_OVERWRITE.to_owned());
        outcome
    }

    /// The result of a profile written with its source link.
    fn written(
        provenance: &Provenance,
        status: &'static str,
        profile_id: Uuid,
        source_id: Uuid,
        created_profile: bool,
    ) -> Self {
        let mut outcome = Self::base(provenance);
        outcome.status = Some(status);
        outcome.profile_id = Some(profile_id);
        outcome.source_link_id = Some(source_id);
        outcome.created_profile = created_profile;
        outcome
    }

    /// The wire dictionary of the result. Identifiers are canonical strings.
    #[must_use]
    pub fn to_value(&self) -> Value {
        json!({
            "status": self.status,
            "profileId": self.profile_id.map(|id| id.to_string()),
            "documentId": self.document_id.to_string(),
            "pdfExtractionId": self.pdf_extraction_id.to_string(),
            "sourceLinkId": self.source_link_id.map(|id| id.to_string()),
            "originalFileName": self.original_file_name,
            "pageCount": self.page_count,
            "characterCount": self.character_count,
            "truncated": self.truncated,
            "createdProfile": self.created_profile,
            "existingProfileLink": self.existing_profile_link,
            "candidateProfileId": self.candidate_profile_id.map(|id| id.to_string()),
            "matchedOn": self.matched_on,
            "recommendedAction": self.recommended_action,
        })
    }
}

impl ProfilePdfSource {
    /// The wire dictionary of a source link.
    #[must_use]
    pub fn to_value(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "profileId": self.profile_id.to_string(),
            "pdfExtractionId": self.pdf_extraction_id.to_string(),
            "sourceType": self.source_type,
            "createdAt": format_instant(self.created_at),
        })
    }
}

/// Ingests profiles from stored PDFs and reads their sources.
#[derive(Debug)]
pub struct ProfilePdfService<U, P, Q, D> {
    uow: U,
    profiles: P,
    pdf: Q,
    documents: D,
    clock: Arc<dyn Clock>,
}

impl<U, P, Q, D> ProfilePdfService<U, P, Q, D>
where
    U: UnitOfWork,
    P: ProfileRepository<Tx = U::Tx>,
    Q: ProfilePdfRepository<Tx = U::Tx>,
    D: StoredPdfDocuments,
{
    /// A service over a unit of work, the profile repository, the PDF repository, the stored
    /// document port and a clock.
    pub fn new(uow: U, profiles: P, pdf: Q, documents: D, clock: Arc<dyn Clock>) -> Self {
        Self {
            uow,
            profiles,
            pdf,
            documents,
            clock,
        }
    }

    /// Ingests a profile from a stored PDF.
    ///
    /// Reuse and already-linked outcomes return before any extraction or write. Otherwise the
    /// resume text is extracted, canonicalized and persisted in one transaction.
    ///
    /// # Errors
    ///
    /// Request validation, the document port's errors, a missing provenance (internal), the
    /// extraction validation, the canonical write validation, or a revision conflict.
    pub async fn ingest_profile_from_stored_pdf(&self, request: &Value) -> Result<Value, AppError> {
        let request = IngestRequest::parse(request)?;
        // The stored name is checked before the extraction is read or persisted, so a
        // refused name leaves no extraction row. The metadata read writes nothing.
        let metadata = self
            .documents
            .stored_document_metadata(&request.document_id.to_string())
            .await?;
        let original_file_name = checked_stored_name(&metadata.original_file_name)?;
        let stored = self
            .documents
            .extract_stored_pdf_text(&json!({
                "documentId": request.document_id.to_string(),
                "maxCharacters": request.max_characters,
                "includePages": false,
                "persistExtraction": true,
            }))
            .await?;
        let extraction_id = stored.extraction_id.ok_or_else(|| {
            AppError::from(ApplicationError::with_message(
                ErrorCode::InternalError,
                PROVENANCE_MISSING_MESSAGE,
            ))
        })?;
        let provenance = Provenance {
            document_id: stored.document.id,
            document_sha256: stored.document.sha256.clone(),
            original_file_name,
            extraction_id,
            page_count: i64::from(stored.extraction.page_count),
            character_count: stored.extraction.character_count,
            truncated: stored.extraction.truncated,
        };
        let reusable = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.reusable(tx, &request, &provenance, true).await
            })
            .await?;
        if let Some(outcome) = reusable {
            return Ok(outcome.to_value());
        }
        let draft_value =
            DeterministicProfileTextExtractor.extract_profile(&stored.extraction.text)?;
        let draft = canonicalize_profile_write(&draft_value)?;
        let outcome = self
            .uow
            .transaction(READ_WRITE, async |tx| {
                self.persist(tx, &request, &provenance, &draft).await
            })
            .await?;
        Ok(outcome.to_value())
    }

    /// The one-to-one PDF source of a profile.
    ///
    /// # Errors
    ///
    /// A validation error for a malformed `profileId`, or `not_found` when no source exists.
    pub async fn get_profile_pdf_source(&self, profile_id: &str) -> Result<Value, AppError> {
        let parsed = crate::ids::admit_uuid(profile_id)
            .map_err(|refusal| refusal.validation(INGEST_VALIDATION_MESSAGE, "profileId"))?;
        let source = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.pdf.source_of_profile(tx, parsed).await
            })
            .await?;
        let source = source.ok_or_else(|| {
            AppError::from(not_found(
                SOURCE_NOT_FOUND_MESSAGE,
                [("profileId", parsed.to_string())],
            ))
        })?;
        Ok(source.to_value())
    }

    /// An existing link for the extraction or the document, then, when
    /// `check_profile` is set, a profile that already has a source.
    async fn reusable(
        &self,
        tx: &mut U::Tx,
        request: &IngestRequest,
        provenance: &Provenance,
        check_profile: bool,
    ) -> Result<Option<IngestOutcome>, AppError> {
        if let Some(linked) = self
            .pdf
            .linked_by_extraction(tx, provenance.extraction_id)
            .await?
        {
            return Ok(Some(IngestOutcome::reused(linked)));
        }
        if let Some(linked) = self
            .pdf
            .linked_by_document_hash(tx, &provenance.document_sha256)
            .await?
        {
            return Ok(Some(IngestOutcome::reused(linked)));
        }
        if check_profile
            && let Some(profile_id) = request.existing_profile_id
            && self.pdf.profile_has_source(tx, profile_id).await?
        {
            let mut outcome = IngestOutcome::candidate(
                provenance,
                profile_id,
                vec!["profileId".to_owned()],
                false,
            );
            outcome.recommended_action = Some(RECOMMEND_NO_MUTATION.to_owned());
            return Ok(Some(outcome));
        }
        Ok(None)
    }

    /// The locked checks, the identity decision, the canonical write and the
    /// source link, in a fixed order.
    async fn persist(
        &self,
        tx: &mut U::Tx,
        request: &IngestRequest,
        provenance: &Provenance,
        draft: &ProfileDraft,
    ) -> Result<IngestOutcome, AppError> {
        self.pdf
            .lock(
                tx,
                &lock_key(&format!("document:{}", provenance.document_sha256)),
            )
            .await?;
        if let Some(outcome) = self.reusable(tx, request, provenance, false).await? {
            return Ok(outcome);
        }
        let mut keys = vec![lock_key(&format!("email:{}", draft.email))];
        keys.extend(
            draft
                .links
                .iter()
                .map(|link| lock_key(&format!("link:{}:{}", link.link_type, link.url))),
        );
        keys.sort();
        keys.dedup();
        for key in &keys {
            self.pdf.lock(tx, key).await?;
        }

        let existing = request.existing_profile_id;
        match existing {
            Some(profile_id) => {
                self.pdf
                    .lock(tx, &lock_key(&format!("profile:{profile_id}")))
                    .await?;
                self.pdf.lock_profile_root(tx, profile_id).await?;
                if let Some(outcome) = self.reusable(tx, request, provenance, true).await? {
                    return Ok(outcome);
                }
            }
            None => {
                if let Some(candidate) = self.identity_candidate(tx, draft, provenance).await? {
                    return Ok(candidate);
                }
            }
        }

        let matches = self
            .pdf
            .extraction_matches(
                tx,
                provenance.extraction_id,
                provenance.document_id,
                &provenance.document_sha256,
            )
            .await?;
        if !matches {
            return Err(AppError::from(ApplicationError::database_unavailable()));
        }

        self.pdf.savepoint(tx).await?;
        let now = self.clock.now();
        let (profile_id, status) = match existing {
            None => (self.create_graph(tx, draft, now).await?, STATUS_CREATED),
            Some(profile_id) if request.overwrite_existing_profile == Some(true) => (
                self.replace_graph(tx, profile_id, request.expected_revision, draft, now)
                    .await?,
                STATUS_UPDATED,
            ),
            Some(profile_id) => {
                return Err(AppError::from(validation_error_with_details(
                    OVERWRITE_REQUIRED_MESSAGE,
                    profile_id,
                )));
            }
        };

        let source_id = Uuid::new_v4();
        let linked = self
            .pdf
            .insert_source_link(
                tx,
                &NewSourceLink {
                    id: source_id,
                    profile_id,
                    extraction_id: provenance.extraction_id,
                    created_at: now,
                },
            )
            .await?;
        if linked {
            return Ok(IngestOutcome::written(
                provenance,
                status,
                profile_id,
                source_id,
                existing.is_none(),
            ));
        }
        // Roll back every graph and posting mutation, including a newly created root, and let a
        // fresh read see the winner that another transaction committed.
        self.pdf.rollback_to_savepoint(tx).await?;
        if let Some(winner) = self.reusable(tx, request, provenance, false).await? {
            if existing.is_some() && winner.profile_id != existing {
                return Err(AppError::from(validation_error_with_details(
                    ALREADY_LINKED_MESSAGE,
                    profile_id,
                )));
            }
            return Ok(winner);
        }
        Err(AppError::from(validation_error_with_details(
            ALREADY_LINKED_MESSAGE,
            profile_id,
        )))
    }

    /// The reasons per profile in first-seen order, the strongest
    /// (first seen) profile, the complete reasons of that profile, and ambiguity.
    async fn identity_candidate(
        &self,
        tx: &mut U::Tx,
        draft: &ProfileDraft,
        provenance: &Provenance,
    ) -> Result<Option<IngestOutcome>, AppError> {
        let mut reasons: Vec<(Uuid, Vec<String>)> = Vec::new();
        for profile_id in self.pdf.email_candidates(tx, &draft.email).await? {
            add_reason(&mut reasons, profile_id, "email".to_owned());
        }
        for link in &draft.links {
            let reason = format!("link:{}", link.link_type);
            for profile_id in self
                .pdf
                .link_candidates(tx, &link.link_type, &link.url)
                .await?
            {
                add_reason(&mut reasons, profile_id, reason.clone());
            }
        }
        let Some((strongest, strongest_reasons)) = reasons.first().cloned() else {
            return Ok(None);
        };
        let mut matched_on: Vec<String> =
            if strongest_reasons.iter().any(|reason| reason == "email") {
                vec!["email".to_owned()]
            } else {
                Vec::new()
            };
        for link in &draft.links {
            let reason = format!("link:{}", link.link_type);
            if matched_on.contains(&reason) {
                continue;
            }
            if self
                .pdf
                .profile_has_link(tx, strongest, &link.link_type, &link.url)
                .await?
            {
                matched_on.push(reason);
            }
        }
        Ok(Some(IngestOutcome::candidate(
            provenance,
            strongest,
            matched_on,
            reasons.len() > 1,
        )))
    }

    /// The graph built from the canonical draft, inserted on the same transaction.
    async fn create_graph(
        &self,
        tx: &mut U::Tx,
        draft: &ProfileDraft,
        now: OffsetDateTime,
    ) -> Result<Uuid, AppError> {
        let profile_id = Uuid::new_v4();
        let mut fresh = Uuid::new_v4;
        let graph = build_profile_graph(draft, profile_id, now, now, 0, &mut fresh);
        let persisted = self.profiles.create_profile(tx, &graph).await?;
        Ok(persisted.profile.id)
    }

    /// Replaces the graph on the same transaction: the revision compare-and-set
    /// check, then the replacement of every profile-owned child.
    async fn replace_graph(
        &self,
        tx: &mut U::Tx,
        profile_id: Uuid,
        expected_request: Option<i64>,
        draft: &ProfileDraft,
        now: OffsetDateTime,
    ) -> Result<Uuid, AppError> {
        let expected_value = expected_request.map(Value::from);
        let expected = expected_revision(expected_value.as_ref())?;
        let current = self
            .profiles
            .get_profile(tx, profile_id)
            .await?
            .ok_or_else(|| profile_not_found(profile_id))?;
        if persisted_revision(current.profile.revision)? != expected {
            return Err(profile_conflict(profile_id, expected));
        }
        let new_revision = next_revision(profile_id, expected)?;
        let mut fresh = Uuid::new_v4;
        let graph = build_profile_graph(
            draft,
            profile_id,
            current.profile.created_at,
            now,
            new_revision,
            &mut fresh,
        );
        let persisted = self
            .profiles
            .replace_profile(tx, &graph, expected)
            .await?
            .ok_or_else(|| profile_conflict(profile_id, expected))?;
        Ok(persisted.profile.id)
    }
}

/// Adds one reason for a profile, keeping the first-seen order of the profiles and of the reasons.
fn add_reason(reasons: &mut Vec<(Uuid, Vec<String>)>, profile_id: Uuid, reason: String) {
    match reasons.iter_mut().find(|(id, _)| *id == profile_id) {
        Some((_, existing)) => {
            if !existing.contains(&reason) {
                existing.push(reason);
            }
        }
        None => reasons.push((profile_id, vec![reason])),
    }
}

/// The validation error of a profile id.
fn validation_error_with_details(message: &str, profile_id: Uuid) -> ApplicationError {
    ApplicationError::new(
        ErrorCode::ValidationError,
        Some(message),
        [("profileId", profile_id.to_string())],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> &'static str {
        "00000000-0000-4000-8000-000000000001"
    }

    #[test]
    fn document_id_is_required_and_canonical() {
        let missing = IngestRequest::parse(&json!({}));
        assert!(missing.is_err());
        let braced = IngestRequest::parse(&json!({"documentId": format!("{{{}}}", id())}));
        assert!(braced.is_err());
        let parsed = IngestRequest::parse(&json!({"documentId": id()}));
        assert!(parsed.is_ok());
    }

    #[test]
    fn budgets_and_revisions_follow_the_bounds() {
        assert!(max_characters_value(&json!(1)).is_ok());
        assert!(max_characters_value(&json!(250_000)).is_ok());
        assert!(max_characters_value(&json!(250_001)).is_err());
        assert!(max_characters_value(&json!(0)).is_err());
        assert!(max_characters_value(&json!(5.0)).is_err());
        assert!(revision_value(&json!(-1)).is_err());
        assert!(revision_value(&json!(9_223_372_036_854_775_807_u64)).is_ok());
        assert!(revision_value(&json!(9_223_372_036_854_775_808_u64)).is_err());
    }

    #[test]
    fn reasons_keep_first_seen_order() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let mut reasons = Vec::new();
        add_reason(&mut reasons, second, "email".to_owned());
        add_reason(&mut reasons, first, "link:website".to_owned());
        add_reason(&mut reasons, second, "link:website".to_owned());
        add_reason(&mut reasons, second, "email".to_owned());
        assert_eq!(reasons[0].0, second);
        assert_eq!(reasons[0].1, vec!["email", "link:website"]);
        assert_eq!(reasons[1].0, first);
    }
}
