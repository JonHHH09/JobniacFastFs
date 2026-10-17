//! The artifact use cases: reading a verified artifact, and exporting it through an exporter port.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

use crate::documents::model::{DocumentMetadata, MISSING_DOCUMENT_MESSAGE};
use crate::errors::{AppError, ApplicationError, ErrorCode, not_found, validation_error};
use crate::ids::admit_uuid;
use crate::profiles::repository::{READ_ONLY, UnitOfWork};

use super::model::{
    ARTIFACT_DISABLED_MESSAGE, ARTIFACT_RETRIEVAL_FAILED_MESSAGE, ARTIFACT_VALIDATION_MESSAGE,
    DocumentArtifact, artifact_size_error, integrity_error, validate_artifact,
};
use super::repository::{ArtifactLookupError, ArtifactRepository};

/// The fixed message of a failed native export before publication.
pub const EXPORT_REFUSED_MESSAGE: &str =
    "Unable to export PDF safely. Choose a new file in a private local folder.";

/// The fixed message of a failure after publication, when completion could not be verified.
pub const EXPORT_UNCERTAIN_MESSAGE: &str =
    "Export may have been saved, but completion could not be verified.";

/// The outcome of a failed export, without any path or operating-system text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFailure {
    /// Refused before publication: no name was created.
    Refused,
    /// Published, but completion could not be verified; the file may exist.
    Uncertain,
}

/// The native export port: publishes a verified artifact to a new file (the atomic exporter in
/// `jobniac_infra::files::export` implements it). Implementations do blocking IO.
pub trait ArtifactExporter: Send + Sync + std::fmt::Debug {
    /// Publish `artifact` to a new `.pdf` file at `destination` and return its basename.
    ///
    /// # Errors
    ///
    /// [`ExportFailure`] with no detail of the path or the operating-system cause.
    fn export(
        &self,
        artifact: &DocumentArtifact,
        destination: &str,
    ) -> Result<String, ExportFailure>;
}

/// Read-only, opt-in access to verified stored PDFs.
#[derive(Debug)]
pub struct ArtifactService<U, R> {
    uow: U,
    repo: R,
    enabled: bool,
}

impl<U, R> ArtifactService<U, R>
where
    U: UnitOfWork,
    R: ArtifactRepository<Tx = U::Tx>,
{
    /// A service over a unit of work and a repository. Retrieval is refused unless `enabled` is
    /// set; the composition root sets it only when the exact flag `1` is present.
    pub fn new(uow: U, repo: R, enabled: bool) -> Self {
        Self { uow, repo, enabled }
    }

    /// The verified artifact of one stored document. The check order is
    /// enabled, UUID, repository, mapping, size, projection, identifier.
    ///
    /// # Errors
    ///
    /// Authorization error when disabled, validation error for a malformed identifier or an
    /// oversize document, not-found for a missing document, and sanitised internal errors for
    /// everything else.
    pub async fn read_artifact(&self, document_id: &str) -> Result<DocumentArtifact, AppError> {
        if !self.enabled {
            return Err(AppError::Application(ApplicationError::with_message(
                ErrorCode::AuthorizationError,
                ARTIFACT_DISABLED_MESSAGE,
            )));
        }
        let identifier = admit_uuid(document_id).map_err(|_| {
            AppError::from(validation_error(
                ARTIFACT_VALIDATION_MESSAGE,
                Some("documentId"),
                Some("must be a valid UUID string"),
            ))
        })?;
        let lookup = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                Ok(self.repo.find_artifact_content(tx, identifier).await)
            })
            .await
            .map_err(|_| AppError::from(ApplicationError::database_unavailable()))?;
        let found = match lookup {
            Ok(found) => found,
            Err(ArtifactLookupError::Unavailable) => {
                return Err(AppError::from(ApplicationError::database_unavailable()));
            }
            Err(ArtifactLookupError::Oversized) => return Err(artifact_size_error()),
            Err(ArtifactLookupError::Failed) => {
                return Err(AppError::Application(ApplicationError::with_message(
                    ErrorCode::InternalError,
                    ARTIFACT_RETRIEVAL_FAILED_MESSAGE,
                )));
            }
        };
        let Some((document, content)) = found else {
            return Err(AppError::from(not_found(
                MISSING_DOCUMENT_MESSAGE,
                [("documentId", identifier.to_string())],
            )));
        };
        let artifact = validate_artifact(DocumentArtifact { document, content })?;
        if artifact.document.id != identifier {
            return Err(integrity_error());
        }
        Ok(artifact)
    }

    /// The `get_document_artifact` answer: `document` (seven fields in order) and `contentBase64`
    /// (standard padded Base64 of the bytes).
    ///
    /// # Errors
    ///
    /// The errors of [`Self::read_artifact`].
    pub async fn get_document_artifact(&self, document_id: &str) -> Result<Value, AppError> {
        let artifact = self.read_artifact(document_id).await?;
        Ok(json!({
            "document": metadata_object(&artifact.document),
            "contentBase64": STANDARD.encode(&artifact.content),
        }))
    }

    /// Publish the verified artifact of one stored document to a new file through `exporter`
    /// (the artifact is checked before the exporter runs; the exporter checks it again).
    ///
    /// # Errors
    ///
    /// The errors of [`Self::read_artifact`], or a validation error with the fixed refusal message
    /// or an internal error with the fixed uncertainty message. The destination is never echoed.
    pub async fn export_document(
        &self,
        document_id: &str,
        destination: &str,
        exporter: &dyn ArtifactExporter,
    ) -> Result<String, AppError> {
        let artifact = self.read_artifact(document_id).await?;
        match exporter.export(&artifact, destination) {
            Ok(basename) => Ok(basename),
            Err(ExportFailure::Refused) => Err(AppError::from(validation_error(
                EXPORT_REFUSED_MESSAGE,
                None,
                None,
            ))),
            Err(ExportFailure::Uncertain) => Err(AppError::Application(
                ApplicationError::with_message(ErrorCode::InternalError, EXPORT_UNCERTAIN_MESSAGE),
            )),
        }
    }
}

/// The seven metadata fields in their fixed key order.
#[must_use]
pub fn metadata_object(document: &DocumentMetadata) -> Value {
    let mut object = Map::new();
    object.insert("id".to_owned(), Value::String(document.id.to_string()));
    object.insert(
        "originalFileName".to_owned(),
        Value::String(document.original_file_name.clone()),
    );
    object.insert(
        "mediaType".to_owned(),
        Value::String(document.media_type.clone()),
    );
    object.insert("byteSize".to_owned(), json!(document.byte_size));
    object.insert("sha256".to_owned(), Value::String(document.sha256.clone()));
    object.insert(
        "createdAt".to_owned(),
        Value::String(document.created_at.clone()),
    );
    object.insert(
        "updatedAt".to_owned(),
        Value::String(document.updated_at.clone()),
    );
    Value::Object(object)
}
