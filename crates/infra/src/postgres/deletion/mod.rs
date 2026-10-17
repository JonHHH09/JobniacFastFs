//! PostgreSQL adapter of profile and job hard delete.
//!
//! One explicit transaction per operation. It locks the root row, fences a pending generation
//! (`conflict`, never a wait), captures every generated-document reference that the cascade will
//! remove, deletes the root, and then retires only owned assets that are no longer referenced.
//! Photo and imported-PDF documents are candidates only: they are retired solely when they are
//! owned generated assets that become unreferenced. Documents, blobs, metadata and unowned or
//! legacy paths are never touched.
//!
//! Lock order mirrors the generation writers: the root row, the parent rows by id, the document and
//! asset advisory locks (sorted), then the asset rows (sorted). The asset retirement is
//! `generated::assets::retire_asset`, and the binding locks come from the same module.
//!
//! Every database failure reaches the caller as a fixed envelope: an integrity
//! error keeps the constraint mapping, and every other failure is `Local database is unavailable`.
//! No SQLSTATE or message text leaves this module.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::clock::Clock;
use jobniac_core::deletion::{EntityDeletion, JobDeletionRepository, ProfileDeletionRepository};
use jobniac_core::errors::{AppError, ApplicationError, conflict};
use jobniac_core::transactions::TransactionOptions;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::postgres::generated::assets::retire_asset;
use crate::postgres::generated::bindings::{AssetRow, lock_pdf_bindings, removed_assets};
use crate::postgres::generated::repository::{app, find_asset, referenced};
use crate::postgres::profiles::{col, db};
use crate::postgres::unit_of_work::{NOT_CONFIGURED_MESSAGE, PgTransaction, PgUnitOfWork};

/// The `application_name` of the deletion sessions. The pool that
/// composes this repository must be built with it.
pub const APPLICATION_NAME: &str = "jobniac-entity-deletion";

/// The profile and job hard-delete adapter over the shared unit of work.
#[derive(Debug, Clone)]
pub struct PostgresEntityDeletionRepository {
    uow: PgUnitOfWork,
    clock: Arc<dyn Clock>,
}

impl PostgresEntityDeletionRepository {
    /// A repository over `uow` that stamps retirements with `clock`.
    #[must_use]
    pub fn new(uow: PgUnitOfWork, clock: Arc<dyn Clock>) -> Self {
        Self { uow, clock }
    }

    /// Runs one deletion body in its own transaction and applies the deletion error policy.
    async fn run<F>(&self, body: F) -> Result<EntityDeletion, AppError>
    where
        F: AsyncFnOnce(&mut PgTransaction) -> Result<EntityDeletion, AppError>,
    {
        self.uow
            .transaction(TransactionOptions::default(), body)
            .await
            .map_err(deletion_error)
    }
}

/// The deletion error policy. Integrity errors keep the constraint mapping.
/// Every other database failure, including a statement or lock timeout and a client-side bug, is
/// the unavailable error, which carries no detail. A missing database configuration is the same
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
impl ProfileDeletionRepository for PostgresEntityDeletionRepository {
    async fn delete_profile(&self, profile_id: Uuid) -> Result<EntityDeletion, AppError> {
        let clock = Arc::clone(&self.clock);
        self.run(async |tx| delete_profile_in(tx, profile_id, clock.now()).await)
            .await
    }
}

#[async_trait]
impl JobDeletionRepository for PostgresEntityDeletionRepository {
    async fn delete_job(&self, job_id: Uuid) -> Result<EntityDeletion, AppError> {
        let clock = Arc::clone(&self.clock);
        self.run(async |tx| delete_job_in(tx, job_id, clock.now()).await)
            .await
    }
}

/// The profile cascade body, run inside the deletion transaction.
async fn delete_profile_in(
    tx: &mut PgTransaction,
    profile_id: Uuid,
    now: OffsetDateTime,
) -> Result<EntityDeletion, AppError> {
    if !lock_root(tx, "profile.profiles", profile_id).await? {
        return Ok(EntityDeletion::default());
    }
    fence_pending(tx, "profile_id", profile_id, "Profile").await?;
    let resumes = resume_ids(tx, "profile_id", profile_id).await?;
    let letters = letter_ids(tx, "profile_id", profile_id, &resumes).await?;
    // Photo and imported-PDF documents are candidates too: only one that has a generated_assets
    // row and is no longer referenced is ever retired.
    let mut removed = pairs(
        tx,
        "SELECT document_id, file_path FROM profile.profile_resume_documents \
         WHERE profile_id = $1 ORDER BY id FOR UPDATE",
        profile_id,
    )
    .await?;
    removed.extend(
        pairs(
            tx,
            "SELECT photo_document_id AS document_id, '' AS file_path \
             FROM profile.profile_personal_details \
             WHERE profile_id = $1 AND photo_document_id IS NOT NULL FOR UPDATE",
            profile_id,
        )
        .await?,
    );
    removed.extend(
        pairs(
            tx,
            "SELECT extraction.file_id AS document_id, '' AS file_path \
             FROM profile.profile_pdf_sources source \
             JOIN document.pdf_extractions extraction \
               ON extraction.id = source.pdf_extraction_id \
             WHERE source.profile_id = $1 ORDER BY source.id FOR UPDATE OF source",
            profile_id,
        )
        .await?,
    );
    removed.extend(variant_pairs(tx, &resumes, &letters).await?);
    finish(
        tx,
        &removed,
        "DELETE FROM profile.profiles WHERE id = $1",
        profile_id,
        now,
    )
    .await
}

/// The job cascade body, run inside the deletion transaction.
async fn delete_job_in(
    tx: &mut PgTransaction,
    job_id: Uuid,
    now: OffsetDateTime,
) -> Result<EntityDeletion, AppError> {
    if !lock_root(tx, "job_schema.jobs", job_id).await? {
        return Ok(EntityDeletion::default());
    }
    fence_pending(tx, "job_id", job_id, "Job").await?;
    let resumes = resume_ids(tx, "job_id", job_id).await?;
    let letters = letter_ids(tx, "job_id", job_id, &resumes).await?;
    let removed = variant_pairs(tx, &resumes, &letters).await?;
    finish(
        tx,
        &removed,
        "DELETE FROM job_schema.jobs WHERE id = $1",
        job_id,
        now,
    )
    .await
}

/// Removes the root, retires the owned unreferenced assets and returns the retired ids. The
/// caller's transaction commits or rolls back all of it.
async fn finish(
    tx: &mut PgTransaction,
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
    // Re-read lifecycle and ownership after the locks, in sorted order.
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
    db(tx.client().execute(delete_sql, &[&root_id]).await)?;
    let mut retired = Vec::new();
    for row in &locked {
        if pending_intent(tx, row.identity.asset_id()).await?
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

/// Locks the root row. `false` when it does not exist.
async fn lock_root(
    tx: &mut PgTransaction,
    table: &'static str,
    root_id: Uuid,
) -> Result<bool, AppError> {
    let sql = format!("SELECT id FROM {table} WHERE id = $1 FOR UPDATE");
    let row = db(tx.client().query_opt(&sql, &[&root_id]).await)?;
    Ok(row.is_some())
}

/// Refuses while a generation for the root is pending. Never waits.
async fn fence_pending(
    tx: &mut PgTransaction,
    column: &'static str,
    root_id: Uuid,
    label: &str,
) -> Result<(), AppError> {
    let sql = format!(
        "SELECT 1 FROM jobniac.generation_intents \
         WHERE {column} = $1 AND state = 'PENDING' LIMIT 1"
    );
    let pending = db(tx.client().query_opt(&sql, &[&root_id]).await)?;
    if pending.is_some() {
        return Err(conflict(
            &format!("{label} has a pending generation"),
            [("reason", "generation_pending")],
        )
        .into());
    }
    Ok(())
}

/// The resume ids of the root, locked in id order.
async fn resume_ids(
    tx: &mut PgTransaction,
    column: &'static str,
    root_id: Uuid,
) -> Result<Vec<Uuid>, AppError> {
    let sql = format!("SELECT id FROM resume.resumes WHERE {column} = $1 ORDER BY id FOR UPDATE");
    let rows = db(tx.client().query(&sql, &[&root_id]).await)?;
    rows.iter().map(|row| col(row, "id")).collect()
}

/// The cover letter ids of the root or of its resumes, locked in id order.
async fn letter_ids(
    tx: &mut PgTransaction,
    column: &'static str,
    root_id: Uuid,
    resumes: &[Uuid],
) -> Result<Vec<Uuid>, AppError> {
    let sql = format!(
        "SELECT id FROM cover_letter.cover_letters \
         WHERE {column} = $1 OR resume_id = ANY($2) ORDER BY id FOR UPDATE"
    );
    let rows = db(tx.client().query(&sql, &[&root_id, &resumes]).await)?;
    rows.iter().map(|row| col(row, "id")).collect()
}

/// The `(document_id, file_path)` pairs of one locking query keyed by a profile id.
async fn pairs(
    tx: &mut PgTransaction,
    sql: &str,
    key: Uuid,
) -> Result<Vec<(Uuid, String)>, AppError> {
    let rows = db(tx.client().query(sql, &[&key]).await)?;
    rows.iter().map(document_pair).collect()
}

/// The resume and cover-letter variant documents of the given resumes and letters.
async fn variant_pairs(
    tx: &mut PgTransaction,
    resumes: &[Uuid],
    letters: &[Uuid],
) -> Result<Vec<(Uuid, String)>, AppError> {
    let rows = db(tx
        .client()
        .query(
            "SELECT document_id, file_path FROM resume.resume_variants \
             WHERE resume_id = ANY($1) ORDER BY id FOR UPDATE",
            &[&resumes],
        )
        .await)?;
    let mut removed: Vec<(Uuid, String)> =
        rows.iter().map(document_pair).collect::<Result<_, _>>()?;
    let rows = db(tx
        .client()
        .query(
            "SELECT document_id, file_path FROM cover_letter.cover_letter_variants \
             WHERE cover_letter_id = ANY($1) ORDER BY id FOR UPDATE",
            &[&letters],
        )
        .await)?;
    removed.extend(
        rows.iter()
            .map(document_pair)
            .collect::<Result<Vec<_>, _>>()?,
    );
    Ok(removed)
}

/// Reads one `(document_id, file_path)` row.
fn document_pair(row: &tokio_postgres::Row) -> Result<(Uuid, String), AppError> {
    Ok((col(row, "document_id")?, col(row, "file_path")?))
}

/// Whether a pending generation intent exists for the asset.
async fn pending_intent(tx: &mut PgTransaction, asset_id: Uuid) -> Result<bool, AppError> {
    let row: Option<tokio_postgres::Row> = db(tx
        .client()
        .query_opt(
            "SELECT 1 FROM jobniac.generation_intents WHERE asset_id = $1 AND state = 'PENDING'",
            &[&asset_id],
        )
        .await)?;
    Ok(row.is_some())
}
