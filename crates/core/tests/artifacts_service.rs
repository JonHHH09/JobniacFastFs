//! The artifact use cases over an in-memory unit of work and repository, at the use-case level.
//! All data is synthetic.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jobniac_core::artifacts::{
    ArtifactExporter, ArtifactLookupError, ArtifactRepository, ArtifactService, DocumentArtifact,
    ExportFailure,
};
use jobniac_core::clock::format_instant;
use jobniac_core::documents::{DocumentMetadata, sha256_hex};
use jobniac_core::errors::{AppError, ErrorCode, error_envelope};
use jobniac_core::profiles::repository::UnitOfWork;
use jobniac_core::transactions::TransactionOptions;
use serde_json::Value;
use time::macros::datetime;
use uuid::Uuid;

const DOCUMENT_ID: &str = "00000000-0000-4000-8000-0000000000aa";

/// A unit of work with no database behind it.
#[derive(Debug, Default)]
struct FakeUow {
    unavailable: bool,
}

impl UnitOfWork for FakeUow {
    type Tx = ();

    async fn transaction<T, F>(&self, _options: TransactionOptions, body: F) -> Result<T, AppError>
    where
        F: AsyncFnOnce(&mut ()) -> Result<T, AppError>,
    {
        if self.unavailable {
            return Err(AppError::from(
                jobniac_core::errors::ApplicationError::database_unavailable(),
            ));
        }
        let mut tx = ();
        body(&mut tx).await
    }
}

/// What the fake repository answers.
#[derive(Debug, Clone)]
enum Answer {
    Found(DocumentMetadata, Vec<u8>),
    Missing,
    Fail(ArtifactLookupError),
}

/// A repository that counts its calls and returns one configured answer.
#[derive(Debug)]
struct FakeRepo {
    answer: Mutex<Answer>,
    calls: Arc<AtomicUsize>,
}

impl FakeRepo {
    fn new(answer: Answer) -> Self {
        Self {
            answer: Mutex::new(answer),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl ArtifactRepository for FakeRepo {
    type Tx = ();

    async fn find_artifact_content(
        &self,
        _tx: &mut (),
        _document_id: Uuid,
    ) -> Result<Option<(DocumentMetadata, Vec<u8>)>, ArtifactLookupError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let answer = self.answer.lock().map(|guard| guard.clone());
        match answer {
            Ok(Answer::Found(metadata, content)) => Ok(Some((metadata, content))),
            Ok(Answer::Missing) => Ok(None),
            Ok(Answer::Fail(error)) => Err(error),
            Err(_) => Err(ArtifactLookupError::Failed),
        }
    }
}

fn pdf(size: usize) -> Vec<u8> {
    let mut bytes = b"%PDF-1.4\n".to_vec();
    bytes.resize(size, b'a');
    bytes
}

fn metadata(content: &[u8]) -> DocumentMetadata {
    DocumentMetadata {
        id: Uuid::parse_str(DOCUMENT_ID).unwrap_or_default(),
        original_file_name: "cv.pdf".to_owned(),
        media_type: "application/pdf".to_owned(),
        byte_size: i64::try_from(content.len()).unwrap_or(i64::MAX),
        sha256: sha256_hex(content),
        created_at: format_instant(datetime!(2026-01-02 03:04:05 UTC)),
        updated_at: format_instant(datetime!(2026-01-02 03:04:06 UTC)),
    }
}

fn make(answer: Answer, enabled: bool) -> ArtifactService<FakeUow, FakeRepo> {
    ArtifactService::new(FakeUow::default(), FakeRepo::new(answer), enabled)
}

/// A service and the shared count of its repository calls.
fn counted_service(
    answer: Answer,
    enabled: bool,
) -> (ArtifactService<FakeUow, FakeRepo>, Arc<AtomicUsize>) {
    let repo = FakeRepo::new(answer);
    let calls = Arc::clone(&repo.calls);
    (
        ArtifactService::new(FakeUow::default(), repo, enabled),
        calls,
    )
}

fn envelope_of(error: &AppError) -> jobniac_core::errors::ErrorEnvelope {
    error_envelope(error)
}

#[tokio::test]
async fn a_disabled_service_refuses_without_repository_access() {
    let content = pdf(64);
    let (service, calls) = counted_service(Answer::Found(metadata(&content), content), false);
    let error = service.read_artifact(DOCUMENT_ID).await.err();
    let envelope = error.as_ref().map(envelope_of);
    assert_eq!(
        envelope.as_ref().map(|e| e.code),
        Some(ErrorCode::AuthorizationError)
    );
    assert_eq!(
        envelope.as_ref().map(|e| e.message.as_str()),
        Some("Document artifact retrieval is not enabled")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_identifier_must_be_a_uuid_before_repository_access() {
    let content = pdf(64);
    let (service, calls) = counted_service(Answer::Found(metadata(&content), content), true);
    for text in [
        "",
        "not-a-uuid",
        "{00000000-0000-4000-8000-0000000000AA}x",
        "{00000000-0000-4000-8000-0000000000aa}",
        "urn:uuid:00000000-0000-4000-8000-0000000000aa",
        "00000000000040008000-0000000000aa",
        "000000000000400080000000000000aa",
    ] {
        let error = service.read_artifact(text).await.err();
        let envelope = error.as_ref().map(envelope_of);
        assert_eq!(
            envelope.as_ref().map(|e| e.code),
            Some(ErrorCode::ValidationError)
        );
        assert_eq!(
            envelope
                .as_ref()
                .and_then(|e| e.details.get("reason"))
                .map(String::as_str),
            Some("must be a valid UUID string")
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_envelope_is_exact_base64_of_the_bytes() -> Result<(), AppError> {
    let content = pdf(70);
    let service = make(Answer::Found(metadata(&content), content.clone()), true);
    let value: Value = service.get_document_artifact(DOCUMENT_ID).await?;
    assert_eq!(
        value["contentBase64"].as_str(),
        Some(base64_of(&content).as_str())
    );
    let keys: Vec<&str> = value["document"]
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default();
    assert_eq!(
        keys,
        [
            "id",
            "originalFileName",
            "mediaType",
            "byteSize",
            "sha256",
            "createdAt",
            "updatedAt"
        ]
    );
    assert_eq!(value["document"]["id"], Value::from(DOCUMENT_ID));
    Ok(())
}

#[tokio::test]
async fn a_missing_document_has_the_existing_not_found_semantics() {
    let service = make(Answer::Missing, true);
    let error = service.read_artifact(DOCUMENT_ID).await.err();
    let envelope = error.as_ref().map(envelope_of);
    assert_eq!(envelope.as_ref().map(|e| e.code), Some(ErrorCode::NotFound));
    assert_eq!(
        envelope.as_ref().map(|e| e.message.as_str()),
        Some("Stored document was not found")
    );
    assert_eq!(
        envelope
            .as_ref()
            .and_then(|e| e.details.get("documentId"))
            .map(String::as_str),
        Some(DOCUMENT_ID)
    );
}

#[tokio::test]
async fn corrupt_content_fails_with_the_sanitised_integrity_message() {
    let content = pdf(64);
    let mut bad_digest = metadata(&content);
    bad_digest.sha256 = "f".repeat(64);
    let service = make(Answer::Found(bad_digest, content.clone()), true);
    assert_integrity(service.read_artifact(DOCUMENT_ID).await.err());

    let mut bad_size = metadata(&content);
    bad_size.byte_size = 63;
    let service = make(Answer::Found(bad_size, content), true);
    assert_integrity(service.read_artifact(DOCUMENT_ID).await.err());
}

#[tokio::test]
async fn a_document_with_another_identifier_is_an_integrity_failure() {
    let content = pdf(64);
    let mut other = metadata(&content);
    other.id = Uuid::parse_str("00000000-0000-4000-8000-0000000000BB").unwrap_or_default();
    let service = make(Answer::Found(other, content), true);
    assert_integrity(service.read_artifact(DOCUMENT_ID).await.err());
}

#[tokio::test]
async fn repository_failures_map_to_the_fixed_messages() {
    let cases = [
        (
            ArtifactLookupError::Unavailable,
            "Local database is unavailable",
            ErrorCode::InternalError,
        ),
        (
            ArtifactLookupError::Failed,
            "Document artifact retrieval failed",
            ErrorCode::InternalError,
        ),
        (
            ArtifactLookupError::Oversized,
            "Invalid document artifact request",
            ErrorCode::ValidationError,
        ),
    ];
    for (lookup, message, code) in cases {
        let service = make(Answer::Fail(lookup), true);
        let error = service.read_artifact(DOCUMENT_ID).await.err();
        let envelope = error.as_ref().map(envelope_of);
        assert_eq!(envelope.as_ref().map(|e| e.code), Some(code));
        assert_eq!(envelope.as_ref().map(|e| e.message.as_str()), Some(message));
    }
}

#[tokio::test]
async fn an_unavailable_unit_of_work_reports_the_database_message() {
    let content = pdf(64);
    let service = ArtifactService::new(
        FakeUow { unavailable: true },
        FakeRepo::new(Answer::Found(metadata(&content), content)),
        true,
    );
    let error = service.read_artifact(DOCUMENT_ID).await.err();
    assert_eq!(
        error.as_ref().map(|e| envelope_of(e).message).as_deref(),
        Some("Local database is unavailable")
    );
}

#[tokio::test]
async fn a_stored_size_over_the_limit_is_the_size_validation_error() {
    let content = pdf(64);
    let mut declared = metadata(&content);
    declared.byte_size = 1024 * 1024 + 1;
    let service = make(Answer::Found(declared, content), true);
    let error = service.read_artifact(DOCUMENT_ID).await.err();
    let envelope = error.as_ref().map(envelope_of);
    assert_eq!(
        envelope.as_ref().map(|e| e.code),
        Some(ErrorCode::ValidationError)
    );
}

#[tokio::test]
async fn the_export_publishes_through_the_exporter_and_returns_its_basename() -> Result<(), AppError>
{
    let content = pdf(64);
    let service = make(Answer::Found(metadata(&content), content), true);
    let exporter = FakeExporter::answering(Ok("saved.pdf"));
    let name = service
        .export_document(DOCUMENT_ID, "/private/saved.pdf", &exporter)
        .await?;
    assert_eq!(name, "saved.pdf");
    assert_eq!(exporter.seen_destination(), "/private/saved.pdf");
    Ok(())
}

#[tokio::test]
async fn export_failures_keep_the_two_fixed_messages() {
    let content = pdf(64);
    let service = make(Answer::Found(metadata(&content), content.clone()), true);
    let refused = FakeExporter::answering(Err(ExportFailure::Refused));
    let error = service
        .export_document(DOCUMENT_ID, "/x/a.pdf", &refused)
        .await
        .err();
    assert_eq!(
        error.as_ref().map(|e| envelope_of(e).message).as_deref(),
        Some("Unable to export PDF safely. Choose a new file in a private local folder.")
    );

    let service = make(Answer::Found(metadata(&content), content), true);
    let uncertain = FakeExporter::answering(Err(ExportFailure::Uncertain));
    let error = service
        .export_document(DOCUMENT_ID, "/x/a.pdf", &uncertain)
        .await
        .err();
    assert_eq!(
        error.as_ref().map(|e| envelope_of(e).message).as_deref(),
        Some("Export may have been saved, but completion could not be verified.")
    );
}

#[tokio::test]
async fn an_invalid_artifact_never_reaches_the_exporter() {
    let content = pdf(64);
    let mut bad = metadata(&content);
    bad.sha256 = "0".repeat(64);
    let service = make(Answer::Found(bad, content), true);
    let exporter = FakeExporter::answering(Ok("a.pdf"));
    assert!(
        service
            .export_document(DOCUMENT_ID, "/x/a.pdf", &exporter)
            .await
            .is_err()
    );
    assert_eq!(exporter.calls(), 0);
}

fn assert_integrity(error: Option<AppError>) {
    let envelope = error.as_ref().map(envelope_of);
    assert_eq!(
        envelope.as_ref().map(|e| e.code),
        Some(ErrorCode::InternalError)
    );
    assert_eq!(
        envelope.as_ref().map(|e| e.message.as_str()),
        Some("Stored document integrity check failed")
    );
}

/// An exporter that records the destination it was given and returns a configured answer.
#[derive(Debug)]
struct FakeExporter {
    answer: Result<&'static str, ExportFailure>,
    destinations: Mutex<Vec<String>>,
}

impl FakeExporter {
    fn answering(answer: Result<&'static str, ExportFailure>) -> Self {
        Self {
            answer,
            destinations: Mutex::new(Vec::new()),
        }
    }

    fn seen_destination(&self) -> String {
        self.destinations
            .lock()
            .ok()
            .and_then(|list| list.first().cloned())
            .unwrap_or_default()
    }

    fn calls(&self) -> usize {
        self.destinations.lock().map(|list| list.len()).unwrap_or(0)
    }
}

impl ArtifactExporter for FakeExporter {
    fn export(
        &self,
        _artifact: &DocumentArtifact,
        destination: &str,
    ) -> Result<String, ExportFailure> {
        if let Ok(mut list) = self.destinations.lock() {
            list.push(destination.to_owned());
        }
        self.answer.map(str::to_owned)
    }
}

/// Standard padded Base64 of `bytes`, for the expected value.
fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
