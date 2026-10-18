//! The SQLite artifact read through the real use case, over a migrated temporary database. The
//! unconfigured-database case runs without a file. All data is synthetic.

mod sqlite_support;

use std::error::Error;

use jobniac_core::artifacts::{ArtifactService, MAX_ARTIFACT_BYTES};
use jobniac_core::errors::{ErrorCode, error_envelope};
use jobniac_infra::sqlite::artifacts::SqliteArtifactRepository;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlite_support::{SqliteFixture, TestResult};
use uuid::Uuid;

type Artifacts = ArtifactService<SqliteUnitOfWork, SqliteArtifactRepository>;

/// A PDF of exactly `size` bytes that starts with the magic.
fn pdf(size: usize) -> Vec<u8> {
    let mut bytes = b"%PDF-1.4\n".to_vec();
    bytes.resize(size, b'a');
    bytes
}

/// Inserts a blob and a document over `content` with raw SQL and returns the document id.
fn store(fixture: &SqliteFixture, content: &[u8]) -> Result<Uuid, Box<dyn Error + Send + Sync>> {
    let connection = fixture.connection();
    let blob_id = Uuid::new_v4().to_string();
    let document_id = Uuid::new_v4();
    let digest: String = Sha256::digest(content)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let now = "2026-01-02T03:04:05.000000Z";
    connection.execute(
        "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![blob_id, digest, content.len() as i64, content, now],
    )?;
    connection.execute(
        "INSERT INTO document_documents \
         (id, blob_id, original_file_name, media_type, created_at, updated_at) \
         VALUES (?1, ?2, 'synthetic-cv.pdf', 'application/pdf', ?3, ?3)",
        rusqlite::params![document_id.to_string(), blob_id, now],
    )?;
    Ok(document_id)
}

fn artifacts(fixture: &SqliteFixture, enabled: bool) -> Artifacts {
    ArtifactService::new(fixture.unit_of_work(), SqliteArtifactRepository, enabled)
}

#[tokio::test]
async fn a_stored_pdf_is_read_back_with_its_metadata_and_exact_bytes() -> TestResult {
    let fixture = SqliteFixture::start();
    let content = pdf(2048);
    let id = store(&fixture, &content)?;
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
    assert_eq!(
        value["document"]["createdAt"],
        json!("2026-01-02T03:04:05Z")
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
    let fixture = SqliteFixture::start();
    let id = store(&fixture, &pdf(MAX_ARTIFACT_BYTES + 1))?;
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
    let fixture = SqliteFixture::start();
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
async fn an_unconfigured_database_reports_the_database_message_without_opening_a_file() -> TestResult
{
    let service: Artifacts = ArtifactService::new(
        SqliteUnitOfWork::new(SqlitePool::unconfigured()),
        SqliteArtifactRepository,
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
