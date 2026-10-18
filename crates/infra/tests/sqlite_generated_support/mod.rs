//! Shared helpers of the SQLite generated-asset and cleanup tests: the recording file fake, root
//! registration, the seed rows that other modules own (profiles, jobs, resumes, documents), and raw
//! reads. The including test declares `mod sqlite_support;` first. All data is synthetic.

#![allow(dead_code)]

use std::error::Error;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use jobniac_core::clock::FixedClock;
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedAssetWriter, GenerationDescriptor, GenerationPurpose,
    PublicationFailure, PublicationFailureCategory, PublicationOutcome, sha256_hex,
};
use jobniac_infra::sqlite::cleanup::SqliteCleanupRepository;
use jobniac_infra::sqlite::generated::repository::SqliteGeneratedAssetRepository;
use rusqlite::{Connection, params};
use time::OffsetDateTime;
use uuid::Uuid;

use super::sqlite_support::SqliteFixture;

/// A synthetic PDF with a valid header.
pub const PDF: &[u8] = b"%PDF-1.7\n% synthetic generated output\n";

pub type BoxResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// The instant `seconds` after a fixed synthetic epoch.
pub fn instant(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000 + seconds).expect("valid instant")
}

/// The stored text of an instant, as the adapters write it.
pub fn stored(at: OffsetDateTime) -> String {
    jobniac_infra::sqlite::codec::instant_text(at)
}

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// File fake that records every publication and fails with a configured category.
#[derive(Debug, Default)]
pub struct RecordingWriter {
    pub failure: Option<PublicationFailureCategory>,
    pub calls: Mutex<Vec<Uuid>>,
}

impl GeneratedAssetWriter for RecordingWriter {
    fn materialize_owned(
        &self,
        identity: &AssetIdentity,
        _content: &[u8],
    ) -> Result<PublicationOutcome, PublicationFailure> {
        if let Some(category) = self.failure {
            return Err(PublicationFailure { category });
        }
        lock(&self.calls).push(identity.asset_id());
        Ok(PublicationOutcome::Created)
    }
}

pub fn standalone_descriptor() -> GenerationDescriptor {
    GenerationDescriptor::new(
        Uuid::new_v4(),
        GenerationPurpose::StandalonePdf,
        None,
        None,
        None,
        None,
    )
    .expect("the standalone descriptor is valid")
}

/// The generated-asset repository with a clock fixed `seconds` after the synthetic epoch.
pub fn repository(fixture: &SqliteFixture, seconds: i64) -> SqliteGeneratedAssetRepository {
    SqliteGeneratedAssetRepository::new(
        fixture.unit_of_work(),
        Arc::new(FixedClock::new(instant(seconds))),
    )
}

/// The cleanup repository with a clock fixed `seconds` after the synthetic epoch.
pub fn cleanup_repository(fixture: &SqliteFixture, seconds: i64) -> SqliteCleanupRepository {
    SqliteCleanupRepository::new(
        fixture.unit_of_work(),
        Arc::new(FixedClock::new(instant(seconds))),
    )
}

/// Registers a synthetic root identity, as the setup command does before any reservation.
pub async fn register_root(
    fixture: &SqliteFixture,
    root: Uuid,
    at: OffsetDateTime,
) -> BoxResult<()> {
    cleanup_repository(fixture, 0)
        .register_generated_root(root, at)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// A synthetic PDF whose bytes differ per label.
pub fn pdf(label: &str) -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n% synthetic ".to_vec();
    bytes.extend_from_slice(label.as_bytes());
    bytes.push(b'\n');
    bytes
}

/// The identity of a new asset for `content`.
pub fn identity(root: Uuid, content: &[u8]) -> AssetIdentity {
    AssetIdentity::new(
        Uuid::new_v4(),
        root,
        &sha256_hex(content),
        content.len() as u64,
    )
    .expect("a valid identity")
}

/// One text value of one asset, read on a raw connection.
pub fn text(conn: &Connection, sql: &str, key: Uuid) -> BoxResult<String> {
    Ok(conn.query_row(sql, [key.to_string()], |row| row.get::<_, String>(0))?)
}

pub fn lifecycle(fixture: &SqliteFixture, asset: Uuid) -> BoxResult<String> {
    text(
        &fixture.connection(),
        "SELECT lifecycle FROM jobniac_generated_assets WHERE asset_id = ?1",
        asset,
    )
}

pub fn count(conn: &Connection, sql: &str) -> BoxResult<i64> {
    Ok(conn.query_row(sql, [], |row| row.get(0))?)
}

// ---------------------------------------------------------------------------------------------
// Seed rows of other modules
// ---------------------------------------------------------------------------------------------

/// Inserts a profile with the given revision and update instant.
pub fn seed_profile(
    conn: &Connection,
    profile: Uuid,
    revision: i64,
    updated: OffsetDateTime,
) -> BoxResult<()> {
    conn.execute(
        "INSERT INTO profile_profiles (id, full_name, email, created_at, updated_at, revision)
         VALUES (?1, 'Synthetic Person', ?2, ?3, ?3, ?4)",
        params![
            profile.to_string(),
            format!("{profile}@example.invalid"),
            stored(updated),
            revision
        ],
    )?;
    Ok(())
}

/// Inserts a job with a text source (its provenance rows belong to the job adapter).
pub fn seed_job(conn: &Connection, job: Uuid, at: OffsetDateTime) -> BoxResult<()> {
    conn.execute(
        "INSERT INTO job_jobs (id, source_method, title, description, canonical_fingerprint,
                               created_at, updated_at)
         VALUES (?1, 'text', 'Synthetic Job', 'Synthetic description', ?2, ?3, ?3)",
        params![job.to_string(), format!("fingerprint-{job}"), stored(at)],
    )?;
    Ok(())
}

/// Inserts a germany resume of the profile and job.
pub fn seed_resume(
    conn: &Connection,
    resume: Uuid,
    profile: Uuid,
    job: Uuid,
    at: OffsetDateTime,
) -> BoxResult<()> {
    conn.execute(
        "INSERT INTO resume_resumes (id, profile_id, job_id, format, profile_revision,
                                     job_revision, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'germany', '0', '0', ?4, ?4)",
        params![
            resume.to_string(),
            profile.to_string(),
            job.to_string(),
            stored(at)
        ],
    )?;
    Ok(())
}

/// Stores `content` as a blob and a document, as an unowned upload would, and returns the
/// document identifier.
pub fn seed_document(conn: &Connection, content: &[u8], at: OffsetDateTime) -> BoxResult<Uuid> {
    let blob = Uuid::new_v4();
    let document = Uuid::new_v4();
    conn.execute(
        "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (sha256) DO NOTHING",
        params![
            blob.to_string(),
            sha256_hex(content),
            i64::try_from(content.len())?,
            content,
            stored(at)
        ],
    )?;
    let blob_id: String = conn.query_row(
        "SELECT id FROM document_blobs WHERE sha256 = ?1",
        [sha256_hex(content)],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT INTO document_documents (id, blob_id, original_file_name, media_type,
                                         created_at, updated_at)
         VALUES (?1, ?2, 'synthetic.pdf', 'application/pdf', ?3, ?3)",
        params![document.to_string(), blob_id, stored(at)],
    )?;
    Ok(document)
}

/// Installs a trigger that fails every `event` (`INSERT`, `UPDATE`, `DELETE`) on `table` for which
/// the SQL `condition` holds, to prove that a failed write rolls the whole scope back.
pub fn fail_at(conn: &Connection, table: &str, event: &str, condition: &str) -> BoxResult<()> {
    let condition = if condition == "TRUE" { "1" } else { condition };
    conn.execute_batch(&format!(
        "CREATE TRIGGER synthetic_failure BEFORE {event} ON {table} FOR EACH ROW \
         WHEN {condition} BEGIN SELECT RAISE(ABORT, 'private synthetic failure'); END"
    ))?;
    Ok(())
}
