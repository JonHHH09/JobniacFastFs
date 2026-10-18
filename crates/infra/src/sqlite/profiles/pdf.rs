//! SQLite adapter of the stored-PDF profile ingestion port.
//!
//! The statements use `?n` binds. The adapter runs on the transaction handle that the core use
//! case opens, so the identity reads, the profile graph write (through
//! [`super::SqliteProfileRepository`] on the same connection) and the source link share one
//! transaction. Every write transaction is `BEGIN IMMEDIATE`, so the advisory locks and the row
//! lock of the PostgreSQL adapter have nothing left to do: [`ProfilePdfRepository::lock`] and
//! [`ProfilePdfRepository::lock_profile_root`] only keep the call order of the port.

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_core::profiles::pdf_ingest::{
    LinkedSource, NewSourceLink, ProfilePdfRepository, ProfilePdfSource,
};
use rusqlite::Row;
use uuid::Uuid;

use super::{assert_generated_document_attachable, col, db, execute, query_rows, with_connection};
use crate::sqlite::codec::{Id, Instant};
use crate::sqlite::unit_of_work::SqliteTransaction;

/// The transaction-scoped savepoint of a PDF ingestion write.
const WRITE_SAVEPOINT: &str = "profile_pdf_write";

/// The linked-source select, with the predicate appended.
const LINKED_SELECT: &str = "SELECT source.id, source.profile_id, source.pdf_extraction_id, \
    file.id AS document_id, file.original_file_name, \
    extraction.page_count, extraction.character_count, extraction.truncated \
    FROM profile_profile_pdf_sources source \
    JOIN document_pdf_extractions extraction ON extraction.id = source.pdf_extraction_id \
    JOIN document_documents file ON file.id = extraction.file_id \
    JOIN document_blobs blob ON blob.id = file.blob_id";

/// The source select.
const SOURCE_SELECT: &str = "SELECT source.id, source.profile_id, source.pdf_extraction_id, \
    source.source_type, source.created_at \
    FROM profile_profile_pdf_sources source WHERE source.profile_id = ?1";

/// The repository adapter of [`ProfilePdfRepository`]. It holds no state.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteProfilePdfRepository;

/// Decodes one linked-source row.
fn linked_row(row: &Row<'_>) -> Result<LinkedSource, AppError> {
    Ok(LinkedSource {
        source_id: col::<Id>(row, "id")?.0,
        profile_id: col::<Id>(row, "profile_id")?.0,
        document_id: col::<Id>(row, "document_id")?.0,
        pdf_extraction_id: col::<Id>(row, "pdf_extraction_id")?.0,
        original_file_name: col(row, "original_file_name")?,
        page_count: col(row, "page_count")?,
        character_count: col(row, "character_count")?,
        truncated: col(row, "truncated")?,
    })
}

/// Decodes one source row.
fn source_row(row: &Row<'_>) -> Result<ProfilePdfSource, AppError> {
    Ok(ProfilePdfSource {
        id: col::<Id>(row, "id")?.0,
        profile_id: col::<Id>(row, "profile_id")?.0,
        pdf_extraction_id: col::<Id>(row, "pdf_extraction_id")?.0,
        source_type: col(row, "source_type")?,
        created_at: col::<Instant>(row, "created_at")?.0,
    })
}

#[async_trait]
impl ProfilePdfRepository for SqliteProfilePdfRepository {
    type Tx = SqliteTransaction;

    async fn lock(&self, _tx: &mut SqliteTransaction, _hashed_key: &str) -> Result<(), AppError> {
        // The write transaction already holds the only writer.
        Ok(())
    }

    async fn lock_profile_root(
        &self,
        tx: &mut SqliteTransaction,
        profile_id: Uuid,
    ) -> Result<(), AppError> {
        with_connection(tx, |conn| {
            query_rows(
                conn,
                "SELECT id FROM profile_profiles WHERE id = ?1",
                &[&Id(profile_id)],
                |_| Ok(()),
            )
            .map(|_| ())
        })
    }

    async fn linked_by_extraction(
        &self,
        tx: &mut SqliteTransaction,
        extraction_id: Uuid,
    ) -> Result<Option<LinkedSource>, AppError> {
        let sql = format!(
            "{LINKED_SELECT} WHERE source.pdf_extraction_id = ?1 \
             ORDER BY source.created_at DESC, source.id LIMIT 1"
        );
        with_connection(tx, |conn| {
            let mut rows = query_rows(conn, &sql, &[&Id(extraction_id)], linked_row)?;
            Ok(rows.pop())
        })
    }

    async fn linked_by_document_hash(
        &self,
        tx: &mut SqliteTransaction,
        sha256: &str,
    ) -> Result<Option<LinkedSource>, AppError> {
        let sql = format!(
            "{LINKED_SELECT} WHERE blob.sha256 = ?1 \
             ORDER BY source.created_at DESC, source.id LIMIT 1"
        );
        with_connection(tx, |conn| {
            let mut rows = query_rows(conn, &sql, &[&sha256], linked_row)?;
            Ok(rows.pop())
        })
    }

    async fn profile_has_source(
        &self,
        tx: &mut SqliteTransaction,
        profile_id: Uuid,
    ) -> Result<bool, AppError> {
        with_connection(tx, |conn| {
            let rows = query_rows(conn, SOURCE_SELECT, &[&Id(profile_id)], |_| Ok(()))?;
            Ok(!rows.is_empty())
        })
    }

    async fn email_candidates(
        &self,
        tx: &mut SqliteTransaction,
        email: &str,
    ) -> Result<Vec<Uuid>, AppError> {
        // Stored emails are canonical (Rust lowercases before every write), so the ASCII-only
        // SQLite `lower` agrees with the Unicode lowercase of the caller's key.
        with_connection(tx, |conn| {
            query_rows(
                conn,
                "SELECT id AS profile_id FROM profile_profiles \
                 WHERE lower(trim(email)) = ?1 ORDER BY updated_at DESC, id LIMIT 2",
                &[&email],
                |row| Ok(col::<Id>(row, "profile_id")?.0),
            )
        })
    }

    async fn link_candidates(
        &self,
        tx: &mut SqliteTransaction,
        link_type: &str,
        url: &str,
    ) -> Result<Vec<Uuid>, AppError> {
        with_connection(tx, |conn| {
            query_rows(
                conn,
                "SELECT DISTINCT profile_id, updated_at FROM profile_profile_links \
                 WHERE lower(trim(link_type)) = ?1 AND url = ?2 \
                 ORDER BY updated_at DESC, profile_id LIMIT 2",
                &[&link_type, &url],
                |row| Ok(col::<Id>(row, "profile_id")?.0),
            )
        })
    }

    async fn profile_has_link(
        &self,
        tx: &mut SqliteTransaction,
        profile_id: Uuid,
        link_type: &str,
        url: &str,
    ) -> Result<bool, AppError> {
        with_connection(tx, |conn| {
            let rows = query_rows(
                conn,
                "SELECT 1 FROM profile_profile_links \
                 WHERE profile_id = ?1 AND lower(trim(link_type)) = ?2 AND url = ?3 LIMIT 1",
                &[&Id(profile_id), &link_type, &url],
                |_| Ok(()),
            )?;
            Ok(!rows.is_empty())
        })
    }

    async fn extraction_matches(
        &self,
        tx: &mut SqliteTransaction,
        extraction_id: Uuid,
        document_id: Uuid,
        sha256: &str,
    ) -> Result<bool, AppError> {
        with_connection(tx, |conn| {
            let rows = query_rows(
                conn,
                "SELECT extraction.id \
                 FROM document_pdf_extractions extraction \
                 JOIN document_documents file ON file.id = extraction.file_id \
                 JOIN document_blobs blob ON blob.id = file.blob_id \
                 WHERE extraction.id = ?1 AND file.id = ?2 AND blob.sha256 = ?3",
                &[&Id(extraction_id), &Id(document_id), &sha256],
                |_| Ok(()),
            )?;
            Ok(!rows.is_empty())
        })
    }

    async fn savepoint(&self, tx: &mut SqliteTransaction) -> Result<(), AppError> {
        with_connection(tx, |conn| {
            db(conn.execute_batch(&format!("SAVEPOINT {WRITE_SAVEPOINT}")))
        })
    }

    async fn rollback_to_savepoint(&self, tx: &mut SqliteTransaction) -> Result<(), AppError> {
        with_connection(tx, |conn| {
            db(conn.execute_batch(&format!("ROLLBACK TO SAVEPOINT {WRITE_SAVEPOINT}")))
        })
    }

    async fn insert_source_link(
        &self,
        tx: &mut SqliteTransaction,
        link: &NewSourceLink,
    ) -> Result<bool, AppError> {
        with_connection(tx, |conn| {
            // The PostgreSQL guard trigger of the source table: the extraction's document must
            // be attachable. It runs before the conflict check.
            let documents = query_rows(
                conn,
                "SELECT file_id FROM document_pdf_extractions WHERE id = ?1",
                &[&Id(link.extraction_id)],
                |row| Ok(col::<Id>(row, "file_id")?.0),
            )?;
            if let Some(document_id) = documents.first() {
                assert_generated_document_attachable(
                    conn,
                    *document_id,
                    "owned_pdf_source_reference_guard",
                )?;
            }
            let inserted = execute(
                conn,
                "INSERT INTO profile_profile_pdf_sources \
                     (id, profile_id, pdf_extraction_id, source_type, created_at) \
                 VALUES (?1, ?2, ?3, 'resume_pdf', ?4) \
                 ON CONFLICT DO NOTHING",
                &[
                    &Id(link.id),
                    &Id(link.profile_id),
                    &Id(link.extraction_id),
                    &Instant(link.created_at),
                ],
            )?;
            Ok(inserted > 0)
        })
    }

    async fn source_of_profile(
        &self,
        tx: &mut SqliteTransaction,
        profile_id: Uuid,
    ) -> Result<Option<ProfilePdfSource>, AppError> {
        with_connection(tx, |conn| {
            // A profile has at most one source (unique index), so the first row is the only one.
            let rows = query_rows(conn, SOURCE_SELECT, &[&Id(profile_id)], source_row)?;
            Ok(rows.into_iter().next())
        })
    }
}
