//! SQLite adapter of profile and job hard delete.
//!
//! One explicit `BEGIN IMMEDIATE` transaction per operation. It checks the root row, fences a
//! pending generation (`conflict`, never a wait), captures every generated-document reference that
//! the cascade will remove, deletes the root, and then retires only owned assets that are no longer
//! referenced. Photo and imported-PDF documents are candidates only: they are retired solely when
//! they are owned generated assets that become unreferenced. Documents, blobs, metadata and unowned
//! or legacy paths are never touched.
//!
//! No row or advisory locks are needed: the writer is exclusive for
//! the whole transaction. The asset retirement is `generated::assets::retire_asset`.
//!
//! Every database failure reaches the caller as a fixed envelope: an integrity
//! error keeps the constraint mapping, and every other failure is `Local database is unavailable`.
//! No engine message text leaves this module.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::clock::Clock;
use jobniac_core::deletion::{EntityDeletion, JobDeletionRepository, ProfileDeletionRepository};
use jobniac_core::errors::{AppError, ApplicationError, conflict};
use jobniac_core::transactions::TransactionOptions;
use rusqlite::Connection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::sqlite::codec::{Id, uuid_array_text};
use crate::sqlite::error::NOT_CONFIGURED_MESSAGE;
use crate::sqlite::generated::assets::retire_asset;
use crate::sqlite::generated::bindings::{AssetRow, lock_pdf_bindings, removed_assets};
use crate::sqlite::generated::repository::{app, find_asset, referenced};
use crate::sqlite::profiles::{col, query_rows, with_connection};
use crate::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};

/// The profile and job hard-delete adapter over the shared unit of work.
#[derive(Debug, Clone)]
pub struct SqliteEntityDeletionRepository {
    uow: SqliteUnitOfWork,
    clock: Arc<dyn Clock>,
}

impl SqliteEntityDeletionRepository {
    /// A repository over `uow` that stamps retirements with `clock`.
    #[must_use]
    pub fn new(uow: SqliteUnitOfWork, clock: Arc<dyn Clock>) -> Self {
        Self { uow, clock }
    }

    /// Runs one deletion body in its own transaction and applies the deletion error policy.
    async fn run<F>(&self, body: F) -> Result<EntityDeletion, AppError>
    where
        F: AsyncFnOnce(&mut SqliteTransaction) -> Result<EntityDeletion, AppError>,
    {
        self.uow
            .transaction(TransactionOptions::default(), body)
            .await
            .map_err(deletion_error)
    }
}

/// The deletion error policy. Integrity errors keep the constraint mapping.
/// Every other database failure, including a busy timeout and a client-side bug, is the
/// unavailable error, which carries no detail. A missing database configuration is the same
/// unavailable error too.
fn deletion_error(error: AppError) -> AppError {
    match error {
        AppError::Internal => ApplicationError::database_unavailable().into(),
        AppError::Application(ref application)
            if application.message() == NOT_CONFIGURED_MESSAGE =>
        {
            ApplicationError::database_unavailable().into()
        }
        other => other,
    }
}

#[async_trait]
impl ProfileDeletionRepository for SqliteEntityDeletionRepository {
    async fn delete_profile(&self, profile_id: Uuid) -> Result<EntityDeletion, AppError> {
        let clock = Arc::clone(&self.clock);
        self.run(async |tx| delete_profile_in(tx, profile_id, clock.now()).await)
            .await
    }
}

#[async_trait]
impl JobDeletionRepository for SqliteEntityDeletionRepository {
    async fn delete_job(&self, job_id: Uuid) -> Result<EntityDeletion, AppError> {
        let clock = Arc::clone(&self.clock);
        self.run(async |tx| delete_job_in(tx, job_id, clock.now()).await)
            .await
    }
}

/// The profile cascade body, run inside the deletion transaction.
async fn delete_profile_in(
    tx: &mut SqliteTransaction,
    profile_id: Uuid,
    now: OffsetDateTime,
) -> Result<EntityDeletion, AppError> {
    let removed = with_connection(tx, |conn| {
        if !root_exists(conn, "profile_profiles", profile_id)? {
            return Ok(None);
        }
        fence_pending(conn, "profile_id", profile_id, "Profile")?;
        let resumes = resume_ids(conn, "profile_id", profile_id)?;
        let letters = letter_ids(conn, "profile_id", profile_id, &resumes)?;
        // Photo and imported-PDF documents are candidates too: only one that has a
        // generated_assets row and is no longer referenced is ever retired.
        let mut removed = pairs(
            conn,
            "SELECT document_id, file_path FROM profile_profile_resume_documents \
             WHERE profile_id = ?1 ORDER BY id",
            profile_id,
        )?;
        removed.extend(pairs(
            conn,
            "SELECT photo_document_id AS document_id, '' AS file_path \
             FROM profile_profile_personal_details \
             WHERE profile_id = ?1 AND photo_document_id IS NOT NULL",
            profile_id,
        )?);
        removed.extend(pairs(
            conn,
            "SELECT extraction.file_id AS document_id, '' AS file_path \
             FROM profile_profile_pdf_sources source \
             JOIN document_pdf_extractions extraction \
               ON extraction.id = source.pdf_extraction_id \
             WHERE source.profile_id = ?1 ORDER BY source.id",
            profile_id,
        )?);
        removed.extend(variant_pairs(conn, &resumes, &letters)?);
        Ok(Some(removed))
    })?;
    let Some(removed) = removed else {
        return Ok(EntityDeletion::default());
    };
    finish(
        tx,
        &removed,
        "DELETE FROM profile_profiles WHERE id = ?1",
        profile_id,
        now,
    )
    .await
}

/// The job cascade body, run inside the deletion transaction.
async fn delete_job_in(
    tx: &mut SqliteTransaction,
    job_id: Uuid,
    now: OffsetDateTime,
) -> Result<EntityDeletion, AppError> {
    let removed = with_connection(tx, |conn| {
        if !root_exists(conn, "job_jobs", job_id)? {
            return Ok(None);
        }
        fence_pending(conn, "job_id", job_id, "Job")?;
        let resumes = resume_ids(conn, "job_id", job_id)?;
        let letters = letter_ids(conn, "job_id", job_id, &resumes)?;
        Ok(Some(variant_pairs(conn, &resumes, &letters)?))
    })?;
    let Some(removed) = removed else {
        return Ok(EntityDeletion::default());
    };
    finish(
        tx,
        &removed,
        "DELETE FROM job_jobs WHERE id = ?1",
        job_id,
        now,
    )
    .await
}

/// Removes the root, retires the owned unreferenced assets and returns the retired ids. The
/// caller's transaction commits or rolls back all of it.
async fn finish(
    tx: &mut SqliteTransaction,
    removed: &[(Uuid, String)],
    delete_sql: &str,
    root_id: Uuid,
    now: OffsetDateTime,
) -> Result<EntityDeletion, AppError> {
    let document_ids: Vec<Uuid> = removed.iter().map(|(document, _)| *document).collect();
    let paths: Vec<String> = removed.iter().map(|(_, path)| path.clone()).collect();
    let candidates = removed_assets(tx, &document_ids, &paths)
        .await
        .map_err(app)?;
    let removed_documents: Vec<Option<Uuid>> = document_ids.iter().copied().map(Some).collect();
    lock_pdf_bindings(tx, &candidates, &[], &removed_documents)
        .await
        .map_err(app)?;
    // Re-read lifecycle and ownership, in sorted order.
    let mut asset_ids: Vec<Uuid> = candidates
        .iter()
        .map(|row| row.identity.asset_id())
        .collect();
    asset_ids.sort_unstable();
    let mut locked: Vec<AssetRow> = Vec::new();
    for asset_id in asset_ids {
        let row = find_asset(tx, asset_id, true).await?;
        if let Some(row) = row.filter(|row| matches!(row.lifecycle.as_str(), "RESERVED" | "ACTIVE"))
        {
            locked.push(row);
        }
    }
    with_connection(tx, |conn| {
        crate::sqlite::profiles::execute(conn, delete_sql, &[&Id(root_id)]).map(|_| ())
    })?;
    let mut retired = Vec::new();
    for row in &locked {
        if pending_intent(tx, row.identity.asset_id())?
            || referenced(tx, row.document_id, row.identity.relative_path()).await?
        {
            continue;
        }
        retire_asset(tx, row, now, false, None).await.map_err(app)?;
        retired.push(row.identity.asset_id());
    }
    Ok(EntityDeletion {
        deleted: true,
        retired_asset_ids: retired,
    })
}

/// Whether the root row exists.
fn root_exists(conn: &Connection, table: &'static str, root_id: Uuid) -> Result<bool, AppError> {
    let sql = format!("SELECT id FROM {table} WHERE id = ?1");
    Ok(!query_rows(conn, &sql, &[&Id(root_id)], |row| {
        Ok(col::<Id>(row, "id")?.0)
    })?
    .is_empty())
}

/// Refuses while a generation for the root is pending. Never waits.
fn fence_pending(
    conn: &Connection,
    column: &'static str,
    root_id: Uuid,
    label: &str,
) -> Result<(), AppError> {
    let sql = format!(
        "SELECT 1 AS found FROM jobniac_generation_intents \
         WHERE {column} = ?1 AND state = 'PENDING' LIMIT 1"
    );
    let pending = query_rows(conn, &sql, &[&Id(root_id)], |row| col::<i64>(row, "found"))?;
    if !pending.is_empty() {
        return Err(conflict(
            &format!("{label} has a pending generation"),
            [("reason", "generation_pending")],
        )
        .into());
    }
    Ok(())
}

/// The resume ids of the root, in id order.
fn resume_ids(
    conn: &Connection,
    column: &'static str,
    root_id: Uuid,
) -> Result<Vec<Uuid>, AppError> {
    let sql = format!("SELECT id FROM resume_resumes WHERE {column} = ?1 ORDER BY id");
    query_rows(conn, &sql, &[&Id(root_id)], |row| {
        Ok(col::<Id>(row, "id")?.0)
    })
}

/// The cover letter ids of the root or of its resumes, in id order.
fn letter_ids(
    conn: &Connection,
    column: &'static str,
    root_id: Uuid,
    resumes: &[Uuid],
) -> Result<Vec<Uuid>, AppError> {
    let sql = format!(
        "SELECT id FROM cover_letter_cover_letters \
         WHERE {column} = ?1 OR resume_id IN (SELECT value FROM json_each(?2)) ORDER BY id"
    );
    query_rows(
        conn,
        &sql,
        &[&Id(root_id), &uuid_array_text(resumes)],
        |row| Ok(col::<Id>(row, "id")?.0),
    )
}

/// The `(document_id, file_path)` pairs of one query keyed by a profile id.
fn pairs(conn: &Connection, sql: &str, key: Uuid) -> Result<Vec<(Uuid, String)>, AppError> {
    query_rows(conn, sql, &[&Id(key)], document_pair)
}

/// The resume and cover-letter variant documents of the given resumes and letters.
fn variant_pairs(
    conn: &Connection,
    resumes: &[Uuid],
    letters: &[Uuid],
) -> Result<Vec<(Uuid, String)>, AppError> {
    let mut removed = query_rows(
        conn,
        "SELECT document_id, file_path FROM resume_resume_variants \
         WHERE resume_id IN (SELECT value FROM json_each(?1)) ORDER BY id",
        &[&uuid_array_text(resumes)],
        document_pair,
    )?;
    removed.extend(query_rows(
        conn,
        "SELECT document_id, file_path FROM cover_letter_cover_letter_variants \
         WHERE cover_letter_id IN (SELECT value FROM json_each(?1)) ORDER BY id",
        &[&uuid_array_text(letters)],
        document_pair,
    )?);
    Ok(removed)
}

/// Reads one `(document_id, file_path)` row.
fn document_pair(row: &rusqlite::Row<'_>) -> Result<(Uuid, String), AppError> {
    Ok((col::<Id>(row, "document_id")?.0, col(row, "file_path")?))
}

/// Whether a pending generation intent exists for the asset.
fn pending_intent(tx: &SqliteTransaction, asset_id: Uuid) -> Result<bool, AppError> {
    with_connection(tx, |conn| {
        Ok(!query_rows(
            conn,
            "SELECT 1 AS found FROM jobniac_generation_intents \
             WHERE asset_id = ?1 AND state = 'PENDING'",
            &[&Id(asset_id)],
            |row| col::<i64>(row, "found"),
        )?
        .is_empty())
    })
}
