//! PostgreSQL adapter of the stored-PDF profile ingestion port.
//!
//! The statements use `$n` binds. The adapter runs on the transaction handle that the core use
//! case opens, so the advisory locks, the identity reads, the profile graph write (through
//! [`super::PgProfileRepository`] on the same client) and the source link share one transaction.
//! Advisory lock keys arrive already hashed (SHA-256 hex), and the adapter only passes them to
//! `hashtextextended`.

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_core::profiles::pdf_ingest::{
    LinkedSource, NewSourceLink, ProfilePdfRepository, ProfilePdfSource,
};
use time::OffsetDateTime;
use tokio_postgres::Row;
use uuid::Uuid;

use super::{col, db};
use crate::postgres::unit_of_work::PgTransaction;

/// The transaction-scoped savepoint of a PDF ingestion write.
const WRITE_SAVEPOINT: &str = "profile_pdf_write";

/// The linked-source select, with the predicate appended.
const LINKED_SELECT: &str = "SELECT source.id, source.profile_id, source.pdf_extraction_id, \
    file.id AS document_id, file.original_file_name, \
    extraction.page_count, extraction.character_count, extraction.truncated \
    FROM profile.profile_pdf_sources source \
    JOIN document.pdf_extractions extraction ON extraction.id = source.pdf_extraction_id \
    JOIN document.documents file ON file.id = extraction.file_id \
    JOIN document.blobs blob ON blob.id = file.blob_id";

/// The source select.
const SOURCE_SELECT: &str = "SELECT source.id, source.profile_id, source.pdf_extraction_id, \
    source.source_type, source.created_at \
    FROM profile.profile_pdf_sources source WHERE source.profile_id = $1";

/// The repository adapter of [`ProfilePdfRepository`]. It holds no state.
#[derive(Debug, Clone, Copy, Default)]
pub struct PgProfilePdfRepository;

/// Decodes one linked-source row.
fn linked_row(row: &Row) -> Result<LinkedSource, AppError> {
    let page_count: i32 = col(row, "page_count")?;
    let character_count: i32 = col(row, "character_count")?;
    Ok(LinkedSource {
        source_id: col(row, "id")?,
        profile_id: col(row, "profile_id")?,
        document_id: col(row, "document_id")?,
        pdf_extraction_id: col(row, "pdf_extraction_id")?,
        original_file_name: col(row, "original_file_name")?,
        page_count: i64::from(page_count),
        character_count: i64::from(character_count),
        truncated: col(row, "truncated")?,
    })
}

/// Decodes one source row.
fn source_row(row: &Row) -> Result<ProfilePdfSource, AppError> {
    Ok(ProfilePdfSource {
        id: col(row, "id")?,
        profile_id: col(row, "profile_id")?,
        pdf_extraction_id: col(row, "pdf_extraction_id")?,
        source_type: col(row, "source_type")?,
        created_at: col::<OffsetDateTime>(row, "created_at")?,
    })
}

#[async_trait]
impl ProfilePdfRepository for PgProfilePdfRepository {
    type Tx = PgTransaction;

    async fn lock(&self, tx: &mut PgTransaction, hashed_key: &str) -> Result<(), AppError> {
        db(tx
            .client()
            .query(
                "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
                &[&hashed_key],
            )
            .await)
        .map(|_| ())
    }

    async fn lock_profile_root(
        &self,
        tx: &mut PgTransaction,
        profile_id: Uuid,
    ) -> Result<(), AppError> {
        db(tx
            .client()
            .query(
                "SELECT id FROM profile.profiles WHERE id = $1 FOR UPDATE",
                &[&profile_id],
            )
            .await)
        .map(|_| ())
    }

    async fn linked_by_extraction(
        &self,
        tx: &mut PgTransaction,
        extraction_id: Uuid,
    ) -> Result<Option<LinkedSource>, AppError> {
        let sql = format!(
            "{LINKED_SELECT} WHERE source.pdf_extraction_id = $1 \
             ORDER BY source.created_at DESC, source.id LIMIT 1"
        );
        let rows = db(tx.client().query(sql.as_str(), &[&extraction_id]).await)?;
        rows.first().map(linked_row).transpose()
    }

    async fn linked_by_document_hash(
        &self,
        tx: &mut PgTransaction,
        sha256: &str,
    ) -> Result<Option<LinkedSource>, AppError> {
        let sql = format!(
            "{LINKED_SELECT} WHERE blob.sha256 = $1 \
             ORDER BY source.created_at DESC, source.id LIMIT 1"
        );
        let rows = db(tx.client().query(sql.as_str(), &[&sha256]).await)?;
        rows.first().map(linked_row).transpose()
    }

    async fn profile_has_source(
        &self,
        tx: &mut PgTransaction,
        profile_id: Uuid,
    ) -> Result<bool, AppError> {
        let rows = db(tx.client().query(SOURCE_SELECT, &[&profile_id]).await)?;
        Ok(!rows.is_empty())
    }

    async fn email_candidates(
        &self,
        tx: &mut PgTransaction,
        email: &str,
    ) -> Result<Vec<Uuid>, AppError> {
        let rows = db(tx
            .client()
            .query(
                "SELECT id AS profile_id FROM profile.profiles \
                 WHERE lower(btrim(email)) = $1 ORDER BY updated_at DESC, id LIMIT 2",
                &[&email],
            )
            .await)?;
        rows.iter()
            .map(|row| col::<Uuid>(row, "profile_id"))
            .collect()
    }

    async fn link_candidates(
        &self,
        tx: &mut PgTransaction,
        link_type: &str,
        url: &str,
    ) -> Result<Vec<Uuid>, AppError> {
        let rows = db(tx
            .client()
            .query(
                "SELECT DISTINCT profile_id, updated_at FROM profile.profile_links \
                 WHERE lower(btrim(link_type)) = $1 AND url = $2 \
                 ORDER BY updated_at DESC, profile_id LIMIT 2",
                &[&link_type, &url],
            )
            .await)?;
        rows.iter()
            .map(|row| col::<Uuid>(row, "profile_id"))
            .collect()
    }

    async fn profile_has_link(
        &self,
        tx: &mut PgTransaction,
        profile_id: Uuid,
        link_type: &str,
        url: &str,
    ) -> Result<bool, AppError> {
        let rows = db(tx
            .client()
            .query(
                "SELECT 1 FROM profile.profile_links \
                 WHERE profile_id = $1 AND lower(btrim(link_type)) = $2 AND url = $3 LIMIT 1",
                &[&profile_id, &link_type, &url],
            )
            .await)?;
        Ok(!rows.is_empty())
    }

    async fn extraction_matches(
        &self,
        tx: &mut PgTransaction,
        extraction_id: Uuid,
        document_id: Uuid,
        sha256: &str,
    ) -> Result<bool, AppError> {
        let rows = db(tx
            .client()
            .query(
                "SELECT extraction.id \
                 FROM document.pdf_extractions extraction \
                 JOIN document.documents file ON file.id = extraction.file_id \
                 JOIN document.blobs blob ON blob.id = file.blob_id \
                 WHERE extraction.id = $1 AND file.id = $2 AND blob.sha256 = $3",
                &[&extraction_id, &document_id, &sha256],
            )
            .await)?;
        Ok(!rows.is_empty())
    }

    async fn savepoint(&self, tx: &mut PgTransaction) -> Result<(), AppError> {
        db(tx
            .client()
            .batch_execute(&format!("SAVEPOINT {WRITE_SAVEPOINT}"))
            .await)
    }

    async fn rollback_to_savepoint(&self, tx: &mut PgTransaction) -> Result<(), AppError> {
        db(tx
            .client()
            .batch_execute(&format!("ROLLBACK TO SAVEPOINT {WRITE_SAVEPOINT}"))
            .await)
    }

    async fn insert_source_link(
        &self,
        tx: &mut PgTransaction,
        link: &NewSourceLink,
    ) -> Result<bool, AppError> {
        let rows = db(tx
            .client()
            .query(
                "INSERT INTO profile.profile_pdf_sources \
                     (id, profile_id, pdf_extraction_id, source_type, created_at) \
                 VALUES ($1, $2, $3, 'resume_pdf', $4) \
                 ON CONFLICT DO NOTHING \
                 RETURNING id",
                &[
                    &link.id,
                    &link.profile_id,
                    &link.extraction_id,
                    &link.created_at,
                ],
            )
            .await)?;
        Ok(!rows.is_empty())
    }

    async fn source_of_profile(
        &self,
        tx: &mut PgTransaction,
        profile_id: Uuid,
    ) -> Result<Option<ProfilePdfSource>, AppError> {
        let rows = db(tx.client().query(SOURCE_SELECT, &[&profile_id]).await)?;
        rows.first().map(source_row).transpose()
    }
}
