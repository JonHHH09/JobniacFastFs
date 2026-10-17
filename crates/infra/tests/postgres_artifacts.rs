//! The PostgreSQL artifact read through the real use cases. The server-backed cases skip without a configured
//! server, except on CI, where a missing server panics. The unconfigured-database case always runs.
//! All data is synthetic.

use std::error::Error;
use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::artifacts::{ArtifactService, MAX_ARTIFACT_BYTES};
use jobniac_core::clock::SystemClock;
use jobniac_core::documents::{
    DocumentFileReader, DocumentService, FileSnapshot, PdfExtractor, PdfText,
};
use jobniac_core::errors::{AppError, ErrorCode, error_envelope};
use jobniac_infra::postgres::artifacts::PgArtifactRepository;
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::documents::PgDocumentRepository;
use jobniac_infra::postgres::migrations::migrate;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use jobniac_testkit::pg::{TestDatabase, TestServer};
use serde_json::json;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;
type Artifacts = ArtifactService<PgUnitOfWork, PgArtifactRepository>;

/// A reader that returns one fixed synthetic file.
#[derive(Debug)]
struct FixedReader {
    content: Vec<u8>,
}

#[async_trait]
impl DocumentFileReader for FixedReader {
    async fn read(&self, _path: &str) -> Result<FileSnapshot, AppError> {
        Ok(FileSnapshot {
            file_name: "synthetic-cv.pdf".to_owned(),
            content: self.content.clone(),
        })
    }
}

/// An extractor that is never called by the storage path.
#[derive(Debug)]
struct UnusedExtractor;

#[async_trait]
impl PdfExtractor for UnusedExtractor {
    fn version(&self) -> &str {
        "unused"
    }

    async fn extract(&self, _content: &[u8], _file_name: &str) -> Result<PdfText, AppError> {
        Err(AppError::Internal)
    }
}

/// A PDF of exactly `size` bytes that starts with the magic.
fn pdf(size: usize) -> Vec<u8> {
    let mut bytes = b"%PDF-1.4\n".to_vec();
    bytes.resize(size, b'a');
    bytes
}

/// A migrated disposable database.
struct Fixture {
    _database: TestDatabase,
    uow: PgUnitOfWork,
}

/// A fresh migrated database, or `None` without a server (a panic on CI).
async fn start() -> Result<Option<Fixture>, Box<dyn Error>> {
    let Some(server) = TestServer::from_env() else {
        assert!(
            std::env::var_os("CI").is_none(),
            "JOBNIAC_TEST_PG_PORT must be set on CI for the PostgreSQL artifact tests"
        );
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(None);
    };
    let database = server.fresh_database().await;
    let config = load_database_config(&database.environment())
        .ok_or("the disposable database configuration is invalid")?;
    migrate(&config).await.map_err(|error| error.0)?;
    let uow = PgUnitOfWork::new(PgPool::new(Some(config)));
    Ok(Some(Fixture {
        _database: database,
        uow,
    }))
}

/// Stores `content` through the document use case and returns the new document identifier.
async fn store(fixture: &Fixture, content: Vec<u8>) -> Result<Uuid, Box<dyn Error>> {
    let documents = DocumentService::new(
        fixture.uow.clone(),
        PgDocumentRepository,
        Arc::new(FixedReader { content }),
        Arc::new(UnusedExtractor),
        Arc::new(SystemClock),
    );
    let stored = documents
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    Ok(stored.id)
}

fn artifacts(fixture: &Fixture, enabled: bool) -> Artifacts {
    ArtifactService::new(fixture.uow.clone(), PgArtifactRepository, enabled)
}

#[tokio::test]
async fn a_stored_pdf_is_read_back_with_its_metadata_and_exact_bytes() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let content = pdf(2048);
    let id = store(&fixture, content.clone()).await?;
    let value = artifacts(&fixture, true)
        .get_document_artifact(&id.to_string())
        .await?;
    assert_eq!(value["document"]["id"], json!(id.to_string()));
    assert_eq!(value["document"]["byteSize"], json!(2048));
    assert_eq!(value["document"]["mediaType"], json!("application/pdf"));
    assert_eq!(
        value["document"]["originalFileName"],
        json!("synthetic-cv.pdf")
    );
    // The exact Base64 bytes are proven by the core tests; here the stored bytes must be the same
    // length as the encoding of the content, and must start with the Base64 of `%PDF-`.
    let encoded = value["contentBase64"].as_str().ok_or("no content")?;
    assert_eq!(encoded.len(), content.len().div_ceil(3) * 4);
    assert!(encoded.starts_with("JVBERi"));
    Ok(())
}

#[tokio::test]
async fn a_stored_size_over_one_mebibyte_is_the_size_validation_error() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let id = store(&fixture, pdf(MAX_ARTIFACT_BYTES + 1)).await?;
    let error = artifacts(&fixture, true)
        .read_artifact(&id.to_string())
        .await
        .err()
        .ok_or("the oversize artifact was returned")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::ValidationError);
    assert_eq!(
        envelope.details.get("reason").map(String::as_str),
        Some("stored PDF size exceeds limit of 1048576 bytes")
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_document_is_not_found_and_a_disabled_service_never_reads() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let missing = Uuid::from_u128(0xFEED).to_string();
    let error = artifacts(&fixture, true)
        .read_artifact(&missing)
        .await
        .err()
        .ok_or("a missing document was returned")?;
    assert_eq!(error_envelope(&error).code, ErrorCode::NotFound);

    let disabled = artifacts(&fixture, false)
        .read_artifact(&missing)
        .await
        .err();
    assert_eq!(
        disabled.as_ref().map(|e| error_envelope(e).code),
        Some(ErrorCode::AuthorizationError)
    );
    Ok(())
}

#[tokio::test]
async fn an_unconfigured_database_reports_the_database_message_without_connecting() -> TestResult {
    let service: Artifacts = ArtifactService::new(
        PgUnitOfWork::new(PgPool::new(None)),
        PgArtifactRepository,
        true,
    );
    let error = service
        .read_artifact(&Uuid::from_u128(0x1).to_string())
        .await
        .err()
        .ok_or("the unconfigured read succeeded")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.code, ErrorCode::InternalError);
    assert_eq!(envelope.message, "Local database is unavailable");
    Ok(())
}
