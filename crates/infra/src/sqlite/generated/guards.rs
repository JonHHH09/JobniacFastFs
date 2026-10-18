//! The generated-asset guards that PostgreSQL enforced with PL/pgSQL triggers (migrations V0005
//! and V0006) and that SQLite cannot express in SQL: the ones that read other tables.
//!
//! A guard is a plain function over the transaction's connection. A repository calls it right
//! before the write it guards, in the same transaction, so a refusal rolls the scope back like a
//! failing trigger did. A refusal is [`SqliteError::Constraint`] with SQLSTATE 23514 and no
//! constraint name, which is what `RAISE EXCEPTION ... USING ERRCODE = '23514'` mapped to. The
//! guards that were deferred constraint triggers are pre-commit checks:
//! [`check_birth_intents`] and [`check_abandoned_retirement`] are registered with
//! [`SqliteTransaction::before_commit`](crate::sqlite::unit_of_work::SqliteTransaction::before_commit).
//!
//! The advisory locks of the PostgreSQL functions are not needed: the single writer serializes
//! every transaction that reaches these functions, and a statement of a write transaction always
//! reads the latest committed state.
//!
//! Row-local rules (identity immutability, forward-only lifecycle, no delete, terminal intent)
//! stay SQL triggers of the baseline. Writers of the tables of other modules (profile resume
//! documents, resume and cover-letter variants, personal-details photos, PDF extractions and
//! sources, documents and blobs) call the reference guards at the bottom of this module before
//! their writes.

use jobniac_core::generated_assets::{
    GENERATION_LEASE, GenerationIntent, GenerationPurpose, GenerationState,
};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::sqlite::codec::{instant_text, parse_id, uuid_text};
use crate::sqlite::error::SqliteError;

/// The refusal of a guard: a check violation without a constraint name.
#[must_use]
pub fn violation() -> SqliteError {
    SqliteError::Constraint {
        sqlstate: "23514".to_owned(),
        constraint: None,
    }
}

fn refuse_if(condition: bool) -> Result<(), SqliteError> {
    if condition { Err(violation()) } else { Ok(()) }
}

fn exists(
    conn: &Connection,
    sql: &str,
    parameters: impl rusqlite::Params,
) -> Result<bool, SqliteError> {
    Ok(conn.query_row(sql, parameters, |row| row.get::<_, i64>(0))? != 0)
}

// ---------------------------------------------------------------------------------------------
// Reference predicates
// ---------------------------------------------------------------------------------------------

/// Whether a profile resume document, a resume or cover-letter variant, a personal-details photo, a
/// PDF source or a completed stand-alone intent references the document
/// (`jobniac.generated_document_referenced`).
///
/// # Errors
///
/// Fails with the classified database error.
pub fn generated_document_referenced(
    conn: &Connection,
    document_id: Option<Uuid>,
) -> Result<bool, SqliteError> {
    let Some(document_id) = document_id else {
        return Ok(false);
    };
    exists(
        conn,
        "SELECT EXISTS (SELECT 1 FROM profile_profile_resume_documents WHERE document_id = ?1)
             OR EXISTS (SELECT 1 FROM resume_resume_variants WHERE document_id = ?1)
             OR EXISTS (SELECT 1 FROM cover_letter_cover_letter_variants WHERE document_id = ?1)
             OR EXISTS (SELECT 1 FROM profile_profile_personal_details WHERE photo_document_id = ?1)
             OR EXISTS (
                 SELECT 1 FROM profile_profile_pdf_sources source
                 JOIN document_pdf_extractions extraction ON extraction.id = source.pdf_extraction_id
                 WHERE extraction.file_id = ?1)
             OR EXISTS (
                 SELECT 1 FROM jobniac_generation_intents intent
                 JOIN jobniac_generated_assets asset ON asset.asset_id = intent.asset_id
                 WHERE intent.state = 'COMPLETED' AND intent.purpose = 'STANDALONE_PDF'
                   AND asset.document_id = ?1)",
        params![uuid_text(document_id)],
    )
}

/// Whether the document or the relative path is referenced (`jobniac.generated_asset_referenced`).
///
/// # Errors
///
/// Fails with the classified database error.
pub fn generated_asset_referenced(
    conn: &Connection,
    document_id: Option<Uuid>,
    relative_path: &str,
) -> Result<bool, SqliteError> {
    if generated_document_referenced(conn, document_id)? {
        return Ok(true);
    }
    exists(
        conn,
        "SELECT EXISTS (SELECT 1 FROM profile_profile_resume_documents WHERE file_path = ?1)
             OR EXISTS (SELECT 1 FROM resume_resume_variants WHERE file_path = ?1)
             OR EXISTS (SELECT 1 FROM cover_letter_cover_letter_variants WHERE file_path = ?1)",
        params![relative_path],
    )
}

// ---------------------------------------------------------------------------------------------
// Assets and cleanup tasks
// ---------------------------------------------------------------------------------------------

/// The columns of a new asset row that the insert guard reads.
#[derive(Debug, Clone, Copy)]
pub struct NewAsset<'a> {
    /// The asset identity.
    pub asset_id: Uuid,
    /// The relative path of the owned file.
    pub relative_path: &'a str,
    /// The owning document, when attached.
    pub document_id: Option<Uuid>,
    /// The content digest.
    pub sha256: &'a str,
    /// The content size.
    pub size_bytes: i64,
}

/// Guards an asset insert: existing references cannot be adopted as owned, and the document's blob
/// must match the asset's digest and size.
///
/// # Errors
///
/// Fails with a check violation when a guard refuses.
pub fn guard_asset_insert(conn: &Connection, asset: &NewAsset<'_>) -> Result<(), SqliteError> {
    refuse_if(generated_asset_referenced(
        conn,
        asset.document_id,
        asset.relative_path,
    )?)?;
    if let Some(document_id) = asset.document_id {
        let consistent = exists(
            conn,
            "SELECT EXISTS (
                 SELECT 1 FROM document_documents file
                 JOIN document_blobs blob ON blob.id = file.blob_id
                 WHERE file.id = ?1 AND blob.sha256 = ?2 AND blob.byte_size = ?3)",
            params![uuid_text(document_id), asset.sha256, asset.size_bytes],
        )?;
        refuse_if(!consistent)?;
    }
    Ok(())
}

/// Guards a move of the asset to `RETIRING` or `DELETED`: a referenced asset cannot retire, and a
/// pending generation must be fenced first.
///
/// # Errors
///
/// Fails with a check violation when a guard refuses, or with the classified database error.
pub fn guard_retirement(conn: &Connection, asset_id: Uuid) -> Result<(), SqliteError> {
    let row: Option<(Option<String>, String)> = conn
        .query_row(
            "SELECT document_id, relative_path FROM jobniac_generated_assets WHERE asset_id = ?1",
            params![uuid_text(asset_id)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((document_id, relative_path)) = row else {
        return Ok(());
    };
    let document_id = document_id.map(|text| parse_id(&text)).transpose()?;
    refuse_if(generated_asset_referenced(
        conn,
        document_id,
        &relative_path,
    )?)?;
    refuse_if(exists(
        conn,
        "SELECT EXISTS (SELECT 1 FROM jobniac_generation_intents
                        WHERE asset_id = ?1 AND state = 'PENDING')",
        params![uuid_text(asset_id)],
    )?)
}

/// Guards the move of a `RETIRING` asset to `DELETED`: the same refusals as retirement, and a
/// completed cleanup task of the same retirement must exist.
///
/// # Errors
///
/// Fails with a check violation when a guard refuses, or with the classified database error.
pub fn guard_deletion(
    conn: &Connection,
    asset_id: Uuid,
    retirement_id: Uuid,
) -> Result<(), SqliteError> {
    guard_retirement(conn, asset_id)?;
    refuse_if(!exists(
        conn,
        "SELECT EXISTS (SELECT 1 FROM jobniac_generated_asset_cleanups
                        WHERE asset_id = ?1 AND retirement_id = ?2 AND status = 'COMPLETED')",
        params![uuid_text(asset_id), uuid_text(retirement_id)],
    )?)
}

/// Guards an insert or update of a cleanup task: its asset must carry the same permanent
/// retirement identity and be `RETIRING` or `DELETED`.
///
/// # Errors
///
/// Fails with a check violation when the asset is not retired under that identity.
pub fn guard_cleanup_write(
    conn: &Connection,
    asset_id: Uuid,
    retirement_id: Uuid,
) -> Result<(), SqliteError> {
    refuse_if(!exists(
        conn,
        "SELECT EXISTS (SELECT 1 FROM jobniac_generated_assets
                        WHERE asset_id = ?1 AND retirement_id = ?2
                          AND lifecycle IN ('RETIRING', 'DELETED'))",
        params![uuid_text(asset_id), uuid_text(retirement_id)],
    )?)
}

/// Guards the deletion of a cleanup task: only a completed task whose asset is `DELETED` under the
/// same retirement identity may go.
///
/// # Errors
///
/// Fails with a check violation when the task is unresolved.
pub fn guard_cleanup_delete(conn: &Connection, task_id: Uuid) -> Result<(), SqliteError> {
    refuse_if(!exists(
        conn,
        "SELECT EXISTS (SELECT 1 FROM jobniac_generated_asset_cleanups task
                        JOIN jobniac_generated_assets asset ON asset.asset_id = task.asset_id
                        WHERE task.task_id = ?1 AND task.status = 'COMPLETED'
                          AND asset.lifecycle = 'DELETED'
                          AND asset.retirement_id = task.retirement_id)",
        params![uuid_text(task_id)],
    )?)
}

// ---------------------------------------------------------------------------------------------
// Generation intents
// ---------------------------------------------------------------------------------------------

/// The `generation_intents_state_shape` bound that SQL cannot express: a pending lease lasts at most
/// five minutes.
fn guard_lease_bound(intent: &GenerationIntent) -> Result<(), SqliteError> {
    if let (GenerationState::Pending, Some(started), Some(expires)) = (
        intent.state,
        intent.lease_started_at,
        intent.lease_expires_at,
    ) && expires > started + GENERATION_LEASE
    {
        return Err(SqliteError::constraint("generation_intents_state_shape"));
    }
    Ok(())
}

/// The lifecycle, creation instant and path of the intent's asset, with whether the asset was born
/// classified in the current transaction.
struct IntentAsset {
    document_id: Option<Uuid>,
    lifecycle: String,
    created_at: String,
    relative_path: String,
    born_here: bool,
}

/// Reads the asset of an intent. `born_here` is true for a classified asset that has no intent yet
/// and was never updated: the replacement of the `xmin = current transaction` read. Every writer
/// registers [`check_birth_intents`], so a classified asset without an intent cannot be committed,
/// and a committed one always has its intent. The only classified asset without an intent is
/// therefore one inserted by the open transaction.
fn intent_asset(conn: &Connection, asset_id: Uuid) -> Result<Option<IntentAsset>, SqliteError> {
    let row = conn
        .query_row(
            "SELECT document_id, lifecycle, created_at, relative_path,
                    generation_intent_required = 1 AND row_version = 0 AND NOT EXISTS (
                        SELECT 1 FROM jobniac_generation_intents intent
                        WHERE intent.asset_id = jobniac_generated_assets.asset_id) AS born_here
             FROM jobniac_generated_assets WHERE asset_id = ?1",
            params![uuid_text(asset_id)],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(document_id, lifecycle, created_at, relative_path, born_here)| {
            Ok(IntentAsset {
                document_id: document_id.map(|text| parse_id(&text)).transpose()?,
                lifecycle,
                created_at,
                relative_path,
                born_here,
            })
        },
    )
    .transpose()
}

/// Guards the insert of a generation intent: the asset has a document, is a reserved classified
/// birth of this transaction, and the intent is pending from the asset's creation instant.
///
/// # Errors
///
/// Fails with a check violation when a guard refuses.
pub fn guard_intent_insert(
    conn: &Connection,
    intent: &GenerationIntent,
) -> Result<(), SqliteError> {
    let asset = intent_asset(conn, intent.asset_id)?.ok_or_else(violation)?;
    refuse_if(asset.document_id.is_none())?;
    refuse_if(
        intent.state != GenerationState::Pending
            || asset.lifecycle != "RESERVED"
            || !asset.born_here
            || instant_text(intent.created_at) != asset.created_at
            || intent.lease_started_at != Some(intent.created_at),
    )?;
    guard_lease_bound(intent)
}

/// Guards the update of a pending intent to `new`: a renewal rotates the fence and never shrinks the
/// lease, a finish lies inside the old lease window, a pending intent needs an available asset, and
/// a completion needs the published asset and its exact scoped link.
///
/// # Errors
///
/// Fails with a check violation when a guard refuses.
pub fn guard_intent_update(
    conn: &Connection,
    old: &GenerationIntent,
    new: &GenerationIntent,
) -> Result<(), SqliteError> {
    let asset = intent_asset(conn, new.asset_id)?.ok_or_else(violation)?;
    let document_id = asset.document_id.ok_or_else(violation)?;
    if old.state == GenerationState::Pending && new != old {
        let (Some(old_started), Some(old_expires)) = (old.lease_started_at, old.lease_expires_at)
        else {
            return Err(violation());
        };
        if new.state == GenerationState::Pending {
            let rotated = new.lease_token != old.lease_token;
            let started_ok = new
                .lease_started_at
                .is_some_and(|started| started >= old_started && started < old_expires);
            let expiry_ok = new
                .lease_expires_at
                .is_some_and(|expires| expires >= old_expires);
            refuse_if(!(rotated && started_ok && expiry_ok))?;
        } else {
            let finished = new.finished_at.ok_or_else(violation)?;
            refuse_if(finished < old_started)?;
            refuse_if(new.state == GenerationState::Completed && finished >= old_expires)?;
        }
    }
    refuse_if(
        new.state == GenerationState::Pending
            && !matches!(asset.lifecycle.as_str(), "RESERVED" | "ACTIVE"),
    )?;
    if new.state == GenerationState::Completed && old.state == GenerationState::Pending {
        refuse_if(asset.lifecycle != "ACTIVE")?;
        refuse_if(!completion_linked(
            conn,
            new,
            document_id,
            &asset.relative_path,
        )?)?;
    }
    guard_lease_bound(new)
}

/// Whether the exact link that a completion of this purpose needs exists.
fn completion_linked(
    conn: &Connection,
    intent: &GenerationIntent,
    document_id: Uuid,
    path: &str,
) -> Result<bool, SqliteError> {
    let descriptor = &intent.descriptor;
    let optional = |value: Option<Uuid>| value.map(uuid_text);
    match descriptor.purpose {
        GenerationPurpose::StandalonePdf => Ok(true),
        GenerationPurpose::ProfileResume => exists(
            conn,
            "SELECT EXISTS (SELECT 1 FROM profile_profile_resume_documents
                            WHERE profile_id = ?1 AND resume_type = ?2
                              AND document_id = ?3 AND file_path = ?4)",
            params![
                optional(descriptor.profile_id),
                descriptor.output_scope,
                uuid_text(document_id),
                path
            ],
        ),
        GenerationPurpose::GermanResume => exists(
            conn,
            "SELECT EXISTS (
                 SELECT 1 FROM resume_resume_variants variant
                 JOIN resume_resumes parent ON parent.id = variant.resume_id
                 WHERE parent.id = ?1 AND parent.profile_id = ?2 AND parent.job_id = ?3
                   AND parent.format = 'germany' AND variant.language = ?4
                   AND variant.document_id = ?5 AND variant.file_path = ?6)",
            params![
                optional(descriptor.resume_id),
                optional(descriptor.profile_id),
                optional(descriptor.job_id),
                descriptor.output_scope,
                uuid_text(document_id),
                path
            ],
        ),
        GenerationPurpose::GermanCoverLetter => exists(
            conn,
            "SELECT EXISTS (
                 SELECT 1 FROM cover_letter_cover_letter_variants variant
                 JOIN cover_letter_cover_letters parent ON parent.id = variant.cover_letter_id
                 WHERE parent.resume_id = ?1 AND parent.profile_id = ?2 AND parent.job_id = ?3
                   AND variant.format = 'germany' AND variant.language = ?4
                   AND variant.document_id = ?5 AND variant.file_path = ?6)",
            params![
                optional(descriptor.resume_id),
                optional(descriptor.profile_id),
                optional(descriptor.job_id),
                descriptor.output_scope,
                uuid_text(document_id),
                path
            ],
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// Pre-commit checks (the deferred constraint triggers)
// ---------------------------------------------------------------------------------------------

/// Pre-commit check of a classified birth: every listed asset that requires a generation intent has
/// one (`generation_birth_intent_required`).
///
/// # Errors
///
/// Fails with a check violation for an asset without its birth intent.
pub fn check_birth_intents(conn: &Connection, asset_ids: &[Uuid]) -> Result<(), SqliteError> {
    for asset_id in asset_ids {
        refuse_if(exists(
            conn,
            "SELECT EXISTS (SELECT 1 FROM jobniac_generated_assets asset
                            WHERE asset.asset_id = ?1 AND asset.generation_intent_required = 1
                              AND NOT EXISTS (SELECT 1 FROM jobniac_generation_intents intent
                                              WHERE intent.asset_id = asset.asset_id))",
            params![uuid_text(*asset_id)],
        )?)?;
    }
    Ok(())
}

/// Pre-commit check of an abandonment: an abandoned intent's asset is `RETIRING` or `DELETED`
/// (`abandoned_generation_retirement_required`).
///
/// # Errors
///
/// Fails with a check violation for an abandoned orphan.
pub fn check_abandoned_retirement(conn: &Connection, asset_id: Uuid) -> Result<(), SqliteError> {
    refuse_if(exists(
        conn,
        "SELECT EXISTS (SELECT 1 FROM jobniac_generation_intents intent
                        JOIN jobniac_generated_assets asset ON asset.asset_id = intent.asset_id
                        WHERE intent.asset_id = ?1 AND intent.state = 'ABANDONED'
                          AND asset.lifecycle NOT IN ('RETIRING', 'DELETED'))",
        params![uuid_text(asset_id)],
    )?)
}

// ---------------------------------------------------------------------------------------------
// Reference guards for the tables of other modules
// ---------------------------------------------------------------------------------------------

/// A document that maps to an owned asset can be attached only while that asset is `ACTIVE`
/// (`jobniac.assert_generated_document_attachable`).
///
/// # Errors
///
/// Fails with a check violation when the document's asset is not `ACTIVE`.
pub fn assert_generated_document_attachable(
    conn: &Connection,
    document_id: Option<Uuid>,
) -> Result<(), SqliteError> {
    let Some(document_id) = document_id else {
        return Ok(());
    };
    let lifecycle: Option<String> = conn
        .query_row(
            "SELECT lifecycle FROM jobniac_generated_assets WHERE document_id = ?1",
            params![uuid_text(document_id)],
            |row| row.get(0),
        )
        .optional()?;
    refuse_if(lifecycle.is_some_and(|lifecycle| lifecycle != "ACTIVE"))
}

/// Guard for the insert or update of a document reference on `profile_profile_personal_details`
/// (`photo_document_id`) or `document_pdf_extractions` (`file_id`): the attachability rule.
///
/// # Errors
///
/// Fails with a check violation when the document's asset is not `ACTIVE`.
pub fn guard_document_reference(
    conn: &Connection,
    document_id: Option<Uuid>,
) -> Result<(), SqliteError> {
    assert_generated_document_attachable(conn, document_id)
}

/// Guard for the insert or update of `profile_profile_resume_documents`,
/// `resume_resume_variants` and `cover_letter_cover_letter_variants`: the attachability rule, a
/// `file_path` that maps to an owned asset must belong to the same document, and a document with an
/// owned asset keeps that asset's path.
///
/// # Errors
///
/// Fails with a check violation when a guard refuses.
pub fn guard_document_path_reference(
    conn: &Connection,
    document_id: Uuid,
    file_path: &str,
) -> Result<(), SqliteError> {
    assert_generated_document_attachable(conn, Some(document_id))?;
    let mapped: Option<Option<String>> = conn
        .query_row(
            "SELECT document_id FROM jobniac_generated_assets WHERE relative_path = ?1",
            params![file_path],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(mapped) = mapped {
        refuse_if(mapped.as_deref() != Some(uuid_text(document_id).as_str()))?;
    }
    refuse_if(exists(
        conn,
        "SELECT EXISTS (SELECT 1 FROM jobniac_generated_assets
                        WHERE document_id = ?1 AND relative_path <> ?2)",
        params![uuid_text(document_id), file_path],
    )?)
}

/// Guard for the insert or update of `profile_profile_pdf_sources.pdf_extraction_id`: the
/// extraction's document is attachable.
///
/// # Errors
///
/// Fails with a check violation when the extraction's document is an owned asset that is not
/// `ACTIVE`.
pub fn guard_pdf_source(conn: &Connection, extraction_id: Uuid) -> Result<(), SqliteError> {
    let document: Option<String> = conn
        .query_row(
            "SELECT file_id FROM document_pdf_extractions WHERE id = ?1",
            params![uuid_text(extraction_id)],
            |row| row.get(0),
        )
        .optional()?;
    let document = document.map(|text| parse_id(&text)).transpose()?;
    assert_generated_document_attachable(conn, document)
}

/// Guard for an update of `document_documents.blob_id`: a document with an owned asset cannot be
/// rebound to another blob.
///
/// # Errors
///
/// Fails with a check violation for an owned document whose blob would change.
pub fn guard_document_content(
    conn: &Connection,
    document_id: Uuid,
    new_blob_id: Uuid,
) -> Result<(), SqliteError> {
    refuse_if(exists(
        conn,
        "SELECT EXISTS (SELECT 1 FROM document_documents file
                        WHERE file.id = ?1 AND file.blob_id <> ?2
                          AND EXISTS (SELECT 1 FROM jobniac_generated_assets
                                      WHERE document_id = file.id))",
        params![uuid_text(document_id), uuid_text(new_blob_id)],
    )?)
}

/// Guard for an update of the digest, size or bytes of `document_blobs`: the blob of an owned
/// document is immutable.
///
/// # Errors
///
/// Fails with a check violation when an owned document uses the blob.
pub fn guard_blob_content(conn: &Connection, blob_id: Uuid) -> Result<(), SqliteError> {
    refuse_if(exists(
        conn,
        "SELECT EXISTS (SELECT 1 FROM jobniac_generated_assets asset
                        JOIN document_documents file ON file.id = asset.document_id
                        WHERE file.blob_id = ?1)",
        params![uuid_text(blob_id)],
    )?)
}
