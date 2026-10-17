//! Shared generation facade: request admission, the
//! qualification gate and the exact metadata projection shared by the desktop and MCP adapters.
//!
//! Committed outputs are never compensated here. Every method returns a JSON object with the
//! documented key names; the wire envelopes are built by the adapters.
//!
//! The German operations are served once the German use cases are attached with
//! [`GenerationService::with_german`]; without them they refuse with the unavailable error.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::cleanup::GenerationRootQualification;
use crate::clock::{Clock, format_instant, parse_instant};
use crate::documents::DocumentMetadata;
use crate::documents::model::{MAX_FILE_BYTES, PDF_MEDIA_TYPE};
use crate::errors::{AppError, ApplicationError, ErrorCode, validation_error};
use crate::generated_assets::{
    GeneratedReservation, admit_profile_resume_link, validate_file_name,
};
use crate::generation::content::validate_pdf_body;
use crate::generation::generic::{
    GeneratedProfileResumePdf, GenerationFailure, GenericGenerationService, application_error,
    pdf_error, unavailable,
};
use crate::generation::standalone::{
    GeneratedStandalonePdf, StandaloneGenerationService, standalone_file_name,
};
use crate::german::generation::GermanGenerationService;
use jobniac_pdf::layout::{PageNumberLocale, build_pdf_layout};
use jobniac_text::java::MAX_PREPARATION_UNITS;

/// Read port of the stored document metadata.
#[async_trait]
pub trait DocumentMetadataPort: Send + Sync {
    /// The safe metadata of the stored document `document_id`.
    async fn get_document_metadata(&self, document_id: &str) -> Result<DocumentMetadata, AppError>;
}

/// Whether `value` is a canonical hyphenated UUID text (36 characters, any hex case).
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

/// The request as a JSON object, or the `request` refusal.
fn request_object<'a>(
    value: &'a Value,
    message: &str,
) -> Result<&'a Map<String, Value>, ApplicationError> {
    value
        .as_object()
        .ok_or_else(|| validation_error(message, Some("request"), Some("must not be null")))
}

/// An optional text field: absent or null is `None`; anything but text is refused.
fn optional_text<'a>(
    request: &'a Map<String, Value>,
    field: &str,
) -> Result<Option<&'a str>, ApplicationError> {
    match request.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.as_str())),
        Some(_) => Err(validation_error(
            "Invalid PDF generation request",
            Some(field),
            Some("must contain text or null"),
        )),
    }
}

/// A required UUID field, returned in canonical lowercase form.
fn identity(
    request: &Map<String, Value>,
    field: &str,
    message: &str,
) -> Result<Uuid, ApplicationError> {
    let invalid = |reason: &str| validation_error(message, Some(field), Some(reason));
    match request.get(field) {
        None | Some(Value::Null) => Err(invalid("must not be null")),
        Some(Value::String(text)) if is_hyphenated_uuid(text) => {
            Uuid::parse_str(text).map_err(|_| invalid("must be a valid UUID"))
        }
        Some(_) => Err(invalid("must be a valid UUID")),
    }
}

/// Maps any failure of an admitted step to a generation-internal error with the fallback message.
fn internal() -> ApplicationError {
    ApplicationError::of(ErrorCode::InternalError)
}

/// The metadata fields of a document in the fixed key order.
fn document_json(metadata: &DocumentMetadata) -> Value {
    json!({
        "id": metadata.id.to_string(),
        "originalFileName": metadata.original_file_name,
        "mediaType": metadata.media_type,
        "byteSize": metadata.byte_size,
        "sha256": metadata.sha256,
        "createdAt": metadata.created_at,
        "updatedAt": metadata.updated_at,
    })
}

/// The `generatedFile` object of a generated file.
fn file_json(
    reservation: &GeneratedReservation,
    document: &DocumentMetadata,
    pages: usize,
    generated_at: OffsetDateTime,
) -> Result<Value, ApplicationError> {
    if !(1..=MAX_PREPARATION_UNITS).contains(&pages) {
        return Err(internal());
    }
    Ok(json!({
        "fileName": document.original_file_name,
        "path": reservation.asset.relative_path(),
        "byteSize": document.byte_size,
        "pageCount": pages,
        "generatedAt": format_instant(generated_at),
    }))
}

/// The generation facade over the standalone, profile, German and document ports.
pub struct GenerationService {
    standalone: StandaloneGenerationService,
    generic: GenericGenerationService,
    german: Option<GermanGenerationService>,
    documents: Arc<dyn DocumentMetadataPort>,
    qualification: Arc<GenerationRootQualification>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for GenerationService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GenerationService")
            .finish_non_exhaustive()
    }
}

impl GenerationService {
    /// Builds the facade. `qualification` is the gate that every operation passes first.
    #[must_use]
    pub fn new(
        standalone: StandaloneGenerationService,
        generic: GenericGenerationService,
        documents: Arc<dyn DocumentMetadataPort>,
        qualification: Arc<GenerationRootQualification>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            standalone,
            generic,
            german: None,
            documents,
            qualification,
            clock,
        }
    }

    /// Adds the German resume and cover-letter use cases.
    /// Without them the German operations refuse with the generic unavailable error.
    #[must_use]
    pub fn with_german(mut self, german: GermanGenerationService) -> Self {
        self.german = Some(german);
        self
    }

    /// The qualification gate. Any refusal is the same internal error.
    async fn ready(&self) -> Result<(), ApplicationError> {
        self.qualification
            .inspect(self.clock.now())
            .await
            .map(|_| ())
            .map_err(|_| unavailable("Generation is not available"))
    }

    /// Whether generation is enabled and its root qualifies right now: the gate that every operation
    /// passes first, offered as a probe so a screen can show itself disabled without doing work.
    pub async fn is_available(&self) -> bool {
        self.ready().await.is_ok()
    }

    /// Validates the document metadata of a committed reservation and returns it.
    async fn document(
        &self,
        reservation: &GeneratedReservation,
        byte_size: usize,
        file_name: Option<&str>,
    ) -> Result<DocumentMetadata, ApplicationError> {
        if !(1..=MAX_FILE_BYTES).contains(&byte_size) {
            return Err(internal());
        }
        let metadata = self
            .documents
            .get_document_metadata(&reservation.document_id.to_string())
            .await
            .map_err(application_error)?;
        validate_file_name(&metadata.original_file_name).map_err(|_| internal())?;
        let created = parse_instant(&metadata.created_at).map_err(|_| internal())?;
        let updated = parse_instant(&metadata.updated_at).map_err(|_| internal())?;
        if let Some(file_name) = file_name {
            validate_file_name(file_name).map_err(|_| internal())?;
            if metadata.original_file_name != file_name {
                return Err(internal());
            }
        }
        if metadata.id != reservation.document_id
            || metadata.media_type != PDF_MEDIA_TYPE
            || metadata.byte_size != byte_size as i64
            || reservation.asset.size_bytes() != byte_size as u64
            || metadata.sha256 != reservation.asset.sha256()
            || created > updated
        {
            return Err(internal());
        }
        Ok(metadata)
    }

    /// Generates a standalone PDF and returns its file metadata.
    ///
    /// # Errors
    ///
    /// Fails with the admission, qualification, generation or metadata errors.
    pub async fn generate_pdf_file(&self, request: &Value) -> Result<Value, ApplicationError> {
        self.standalone_pdf(request, false).await
    }

    /// Like [`Self::generate_pdf_file`], and also returns the verified document identifier for the
    /// native artifact workflow only.
    ///
    /// # Errors
    ///
    /// The same refusals as [`Self::generate_pdf_file`].
    pub async fn generate_pdf_file_for_desktop(
        &self,
        request: &Value,
    ) -> Result<Value, ApplicationError> {
        self.standalone_pdf(request, true).await
    }

    /// The shared standalone operation.
    async fn standalone_pdf(
        &self,
        request: &Value,
        include_document_id: bool,
    ) -> Result<Value, ApplicationError> {
        let admitted = request_object(request, "Invalid PDF generation request")?;
        let body = optional_text(admitted, "body")?;
        if body.is_some_and(|text| text.chars().count() > MAX_PREPARATION_UNITS) {
            return Err(validation_error(
                "PDF preparation limit exceeded",
                Some("body"),
                Some("exceeds preparation safety budget"),
            ));
        }
        let validated_body = validate_pdf_body(body)?;
        let name = optional_text(admitted, "fileName")?;
        standalone_file_name(name)?;
        let title = optional_text(admitted, "title")?;
        build_pdf_layout(
            title,
            Some(validated_body.as_str()),
            false,
            PageNumberLocale::English,
        )
        .map_err(pdf_error)?;
        self.ready().await?;
        let result: GeneratedStandalonePdf = self
            .standalone
            .generate_pdf_file(name, title, body)
            .await
            .map_err(|failure: GenerationFailure| failure.error)?;
        let document = self
            .document(
                &result.reservation,
                result.byte_size,
                Some(result.file_name.as_str()),
            )
            .await?;
        if result.path != result.reservation.asset.relative_path() {
            return Err(internal());
        }
        let mut metadata = file_json(
            &result.reservation,
            &document,
            result.page_count,
            result.generated_at,
        )?;
        if include_document_id && let Value::Object(map) = &mut metadata {
            map.insert(
                "documentId".to_owned(),
                Value::String(result.reservation.document_id.to_string()),
            );
        }
        Ok(metadata)
    }

    /// Generates the master resume of a profile.
    ///
    /// # Errors
    ///
    /// Fails with the admission, qualification, generation or link errors.
    pub async fn generate_pdf_resume(&self, request: &Value) -> Result<Value, ApplicationError> {
        self.profile(request, ProfileOperation::Master).await
    }

    /// Generates the Canadian English resume of a profile.
    ///
    /// # Errors
    ///
    /// The same refusals as [`Self::generate_pdf_resume`].
    pub async fn generate_canadian_pdf_resume(
        &self,
        request: &Value,
    ) -> Result<Value, ApplicationError> {
        self.profile(request, ProfileOperation::Canadian).await
    }

    /// Generates the Canadian French resume of a profile.
    ///
    /// # Errors
    ///
    /// The same refusals as [`Self::generate_pdf_resume`].
    pub async fn generate_canadian_french_pdf_resume(
        &self,
        request: &Value,
    ) -> Result<Value, ApplicationError> {
        self.profile(request, ProfileOperation::CanadianFrench)
            .await
    }

    /// The shared profile resume operation.
    async fn profile(
        &self,
        request: &Value,
        operation: ProfileOperation,
    ) -> Result<Value, ApplicationError> {
        const MESSAGE: &str = "Invalid profile resume generation request";
        let admitted = request_object(request, MESSAGE)?;
        let profile_id = identity(admitted, "profileId", MESSAGE)?;
        self.ready().await?;
        let result: GeneratedProfileResumePdf = match operation {
            ProfileOperation::Master => self.generic.generate_pdf_resume(profile_id).await,
            ProfileOperation::Canadian => {
                self.generic.generate_canadian_pdf_resume(profile_id).await
            }
            ProfileOperation::CanadianFrench => {
                self.generic
                    .generate_canadian_french_pdf_resume(profile_id)
                    .await
            }
        }
        .map_err(|failure| failure.error)?;
        if result.link.profile_id != profile_id
            || result.link.resume_type != operation.resume_type()
            || result.link.document_id != result.reservation.document_id
            || result.link.file_path != result.reservation.asset.relative_path()
        {
            return Err(internal());
        }
        admit_profile_resume_link(&result.link, &result.reservation).map_err(|_| internal())?;
        let document = self
            .document(
                &result.reservation,
                result.byte_size,
                Some(result.file_name.as_str()),
            )
            .await?;
        let generated_file = file_json(
            &result.reservation,
            &document,
            result.page_count,
            result.generated_at,
        )?;
        Ok(json!({
            "profileId": result.link.profile_id.to_string(),
            "resumeLinkId": result.link.id.to_string(),
            "documentId": result.link.document_id.to_string(),
            "filePath": result.link.file_path,
            "resumeType": result.link.resume_type,
            "document": document_json(&document),
            "generatedFile": generated_file,
            "replacedExistingLink": result.replaced_existing_link,
            "linkedAt": format_instant(result.link.updated_at),
        }))
    }
}

impl GenerationService {
    /// Generates the English and German tailored resume of a profile for a job.
    /// `includeProjects` is absent, `null` or a boolean.
    ///
    /// # Errors
    ///
    /// The admission, qualification, German generation or metadata error.
    pub async fn generate_german_tailored_resume(
        &self,
        request: &Value,
    ) -> Result<Value, ApplicationError> {
        const MESSAGE: &str = "Invalid German tailored resume request";
        let admitted = request_object(request, MESSAGE)?;
        let profile_id = identity(admitted, "profileId", MESSAGE)?;
        let job_id = identity(admitted, "jobId", MESSAGE)?;
        let include_projects = match admitted.get("includeProjects") {
            None | Some(Value::Null) => Value::Null,
            Some(Value::Bool(flag)) => Value::Bool(*flag),
            Some(_) => {
                return Err(validation_error(
                    MESSAGE,
                    Some("includeProjects"),
                    Some("must be a boolean"),
                ));
            }
        };
        self.ready().await?;
        let german = self.german()?;
        let result = german
            .generate_german_tailored_resume(
                &Value::String(profile_id.to_string()),
                &Value::String(job_id.to_string()),
                &include_projects,
            )
            .await
            .map_err(|failure| failure.error)?;
        if result.resume.profile_id() != profile_id
            || result.resume.job_id() != job_id
            || result.resume.format() != "germany"
            || result.variants.len() != 2
        {
            return Err(internal());
        }
        let mut variants = Vec::with_capacity(2);
        let mut seen = std::collections::HashSet::new();
        for output in &result.variants {
            let variant = &output.variant;
            if variant.resume_id() != result.resume.id()
                || variant.document_id() != output.reservation.document_id
                || variant.file_path() != output.reservation.asset.relative_path()
                || !seen.insert(variant.id())
            {
                return Err(internal());
            }
            let document = self
                .document(&output.reservation, output.byte_size, None)
                .await?;
            let generated_file = file_json(
                &output.reservation,
                &document,
                output.page_count,
                result.generated_at,
            )?;
            variants.push(json!({
                "variantId": variant.id().to_string(),
                "language": variant.language(),
                "documentId": variant.document_id().to_string(),
                "filePath": variant.file_path(),
                "document": document_json(&document),
                "generatedFile": generated_file,
            }));
        }
        let languages: Vec<&str> = result
            .variants
            .iter()
            .map(|output| output.variant.language())
            .collect();
        if languages != ["en", "de"] {
            return Err(internal());
        }
        Ok(json!({
            "resumeId": result.resume.id().to_string(),
            "profileId": profile_id.to_string(),
            "jobId": job_id.to_string(),
            "format": result.resume.format(),
            "variants": variants,
            "replacedExisting": result.replaced_existing,
            "generatedAt": format_instant(result.generated_at),
        }))
    }

    /// Generates the German cover letter for the exact selected resume.
    ///
    /// # Errors
    ///
    /// The admission, qualification, German generation or metadata error.
    pub async fn generate_german_cover_letter(
        &self,
        request: &Value,
    ) -> Result<Value, ApplicationError> {
        const MESSAGE: &str = "Invalid German cover-letter request";
        let admitted = request_object(request, MESSAGE)?;
        let profile_id = identity(admitted, "profileId", MESSAGE)?;
        let job_id = identity(admitted, "jobId", MESSAGE)?;
        let resume_id = identity(admitted, "resumeId", MESSAGE)?;
        self.ready().await?;
        let german = self.german()?;
        let result = german
            .generate_german_cover_letter(
                &Value::String(profile_id.to_string()),
                &Value::String(job_id.to_string()),
                &Value::String(resume_id.to_string()),
            )
            .await
            .map_err(|failure| failure.error)?;
        if result.cover_letter.profile_id() != profile_id
            || result.cover_letter.job_id() != job_id
            || result.cover_letter.resume_id() != resume_id
            || result.variant.cover_letter_id() != result.cover_letter.id()
            || result.variant.document_id() != result.reservation.document_id
            || result.variant.file_path() != result.reservation.asset.relative_path()
            || result.variant.format() != "germany"
            || result.variant.language() != "de"
        {
            return Err(internal());
        }
        let document = self
            .document(&result.reservation, result.byte_size, None)
            .await?;
        let generated_file = file_json(
            &result.reservation,
            &document,
            result.page_count,
            result.generated_at,
        )?;
        Ok(json!({
            "coverLetterId": result.cover_letter.id().to_string(),
            "profileId": profile_id.to_string(),
            "jobId": job_id.to_string(),
            "resumeId": resume_id.to_string(),
            "variantId": result.variant.id().to_string(),
            "format": result.variant.format(),
            "language": result.variant.language(),
            "documentId": result.variant.document_id().to_string(),
            "filePath": result.variant.file_path(),
            "byteSize": generated_file["byteSize"],
            "pageCount": generated_file["pageCount"],
            "replacedExisting": result.replaced_existing,
            "generatedAt": generated_file["generatedAt"],
        }))
    }

    /// The German use cases, or the generic unavailable refusal when they were not composed.
    fn german(&self) -> Result<&GermanGenerationService, ApplicationError> {
        self.german
            .as_ref()
            .ok_or_else(|| unavailable("Generation is not available"))
    }
}

/// The three profile resume operations of the facade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileOperation {
    Master,
    Canadian,
    CanadianFrench,
}

impl ProfileOperation {
    /// The stored `resume_type` of the operation.
    const fn resume_type(self) -> &'static str {
        match self {
            Self::Master => "master_resume",
            Self::Canadian => "canadian_resume",
            Self::CanadianFrench => "canadian_resume_fr",
        }
    }
}
