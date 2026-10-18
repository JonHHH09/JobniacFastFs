//! SQLite adapter of the current profile resume links. A replacement is one transaction: the owner
//! check, the PDF bindings, the owner CAS, the link write, retirement of the replaced asset and the
//! generation completion.
//!
//! The profile row is read under the writer so that profile edits, deletion and sibling
//! link writers waited, pinned `READ COMMITTED` and took the sorted binding locks. `BEGIN
//! IMMEDIATE` gives the same exclusion: no other transaction writes between the owner read and the
//! commit, and the owner row read here is the committed one.

use std::sync::Arc;

use jobniac_core::clock::Clock;
use jobniac_core::errors::{AppError, ApplicationError, conflict, not_found, validation_error};
use jobniac_core::generated_assets::{
    GeneratedReservation, GenerationDescriptor, GenerationPurpose, IntentReservation,
    ProfileResumeDocument, ReplaceResult, admit_profile_resume_link, normalize_resume_type,
    validate_binding_intents, validate_revision,
};
use jobniac_core::transactions::TransactionOptions;
use rusqlite::{Connection, OptionalExtension, Row, params};
use time::OffsetDateTime;
use uuid::Uuid;

use super::IntentError;
use super::assets::retire_asset;
use super::bindings::{lock_pdf_bindings, removed_assets, verify_pdf_binding};
use super::guards::{generated_asset_referenced, guard_document_path_reference};
use super::intents::{admit_generation_bindings, complete_generation_bindings};
use super::repository::find_asset_in;
use super::run_with;
use crate::sqlite::codec::{get_id, get_ts, instant_text, parse_ts, uuid_text};
use crate::sqlite::error::SqliteError;
use crate::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};

const LINK_COLUMNS: &str =
    "id, profile_id, document_id, file_path, resume_type, created_at, updated_at";

/// Refusal of an unusable link input, with a fixed message.
fn invalid_input() -> AppError {
    AppError::Application(validation_error(
        "Invalid profile resume persistence input",
        None,
        None,
    ))
}

/// Maps a generation failure of the link path: every binding or continuation refusal is the fixed
/// validation error; database failures keep their classification.
fn link_error(error: IntentError) -> AppError {
    match error {
        IntentError::Database(error) => error,
        IntentError::Unavailable | IntentError::Binding | IntentError::Invalid => invalid_input(),
    }
}

/// Reads one link row.
fn link_from_row(row: &Row<'_>) -> Result<ProfileResumeDocument, AppError> {
    ProfileResumeDocument::new(
        get_id(row, "id")?,
        get_id(row, "profile_id")?,
        get_id(row, "document_id")?,
        &row.get::<_, String>("file_path")
            .map_err(SqliteError::from)?,
        &row.get::<_, String>("resume_type")
            .map_err(SqliteError::from)?,
        get_ts(row, "created_at")?,
        get_ts(row, "updated_at")?,
    )
    .map_err(|_| AppError::Internal)
}

/// Finds the current link of a (profile, type) pair.
fn find(
    conn: &Connection,
    profile_id: Uuid,
    resume_type: &str,
) -> Result<Option<ProfileResumeDocument>, AppError> {
    let sql = format!(
        "SELECT {LINK_COLUMNS} FROM profile_profile_resume_documents \
         WHERE profile_id = ?1 AND resume_type = ?2"
    );
    conn.query_row(&sql, params![uuid_text(profile_id), resume_type], |row| {
        Ok(link_from_row(row))
    })
    .optional()
    .map_err(SqliteError::from)?
    .transpose()
}

/// Repository of the current profile resume links over the unit of work.
#[derive(Clone)]
pub struct SqliteProfileResumeLinks {
    uow: SqliteUnitOfWork,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for SqliteProfileResumeLinks {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteProfileResumeLinks")
            .finish_non_exhaustive()
    }
}

impl SqliteProfileResumeLinks {
    /// Creates the repository over `uow`, reading time from `clock`.
    #[must_use]
    pub fn new(uow: SqliteUnitOfWork, clock: Arc<dyn Clock>) -> Self {
        Self { uow, clock }
    }

    /// Reads the current link of a profile and resume type (read-only transaction).
    ///
    /// # Errors
    ///
    /// Fails with a validation error for an unknown type, or the database classification.
    pub async fn find_by_profile_id_and_resume_type(
        &self,
        profile_id: Uuid,
        resume_type: &str,
    ) -> Result<Option<ProfileResumeDocument>, AppError> {
        let canonical = normalize_resume_type(resume_type).map_err(|_| invalid_input())?;
        self.uow
            .transaction(
                TransactionOptions {
                    read_only: true,
                    requires_new: false,
                },
                async |tx| run_with(tx, |conn| find(conn, profile_id, &canonical)),
            )
            .await
    }

    /// Replaces the current link of the link's (profile, type) pair.
    ///
    /// # Errors
    ///
    /// Fails with a validation error for invalid input or a refused binding, `not_found` for a
    /// missing owner, `conflict` for a revision or timestamp conflict, or the database
    /// classification.
    pub async fn replace(
        &self,
        link: &ProfileResumeDocument,
        binding: &GeneratedReservation,
        expected_profile_revision: i64,
        expected_profile_updated_at: OffsetDateTime,
        generation_claim: Option<&IntentReservation>,
    ) -> Result<ReplaceResult, AppError> {
        admit_profile_resume_link(link, binding).map_err(|_| invalid_input())?;
        validate_revision(expected_profile_revision).map_err(|_| invalid_input())?;
        if let Some(claim) = generation_claim {
            // The proof fields are public: re-check the intent shape and its asset before any
            // connection.
            claim.intent.validate().map_err(|_| invalid_input())?;
            if claim.intent.asset_id != binding.asset.asset_id() {
                return Err(invalid_input());
            }
        }
        let expected_updated = jobniac_core::generated_assets::identity::generation_instant(
            expected_profile_updated_at,
        )
        .map_err(|_| invalid_input())?;
        let claims = generation_claim.map(std::slice::from_ref);
        validate_binding_intents(std::slice::from_ref(binding), claims)
            .map_err(|_| invalid_input())?;
        let descriptors: Vec<GenerationDescriptor> = match generation_claim {
            None => Vec::new(),
            Some(claim) => {
                let expected = GenerationDescriptor::new(
                    claim.intent.descriptor.operation_id,
                    GenerationPurpose::ProfileResume,
                    Some(link.profile_id),
                    None,
                    None,
                    Some(link.resume_type.clone()),
                )
                .map_err(|_| invalid_input())?;
                if claim.intent.descriptor != expected {
                    return Err(invalid_input());
                }
                vec![expected]
            }
        };
        let clock = Arc::clone(&self.clock);
        let link = link.clone();
        let binding = binding.clone();
        let claim_owned = generation_claim.cloned();
        self.uow
            .transaction(
                TransactionOptions {
                    read_only: false,
                    requires_new: false,
                },
                async |tx| {
                    let claims_owned = claim_owned.as_ref().map(std::slice::from_ref);
                    replace_in(
                        tx,
                        &link,
                        &binding,
                        expected_profile_revision,
                        expected_updated,
                        claims_owned,
                        &descriptors,
                        clock.now(),
                        clock.as_ref(),
                    )
                    .await
                },
            )
            .await
    }
}

/// Reads the owner's revision and update instant.
fn read_owner(
    conn: &Connection,
    profile_id: Uuid,
) -> Result<Option<(i64, OffsetDateTime)>, AppError> {
    let owner: Option<(i64, String)> = conn
        .query_row(
            "SELECT revision, updated_at FROM profile_profiles WHERE id = ?1",
            [uuid_text(profile_id)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(SqliteError::from)?;
    owner
        .map(|(revision, updated)| Ok((revision, parse_ts(&updated)?)))
        .transpose()
}

/// Writes the saved link: an insert for a first link, an update of the previous row otherwise. The
/// reference guard of the document and path runs first.
fn write_link(
    conn: &Connection,
    saved: &ProfileResumeDocument,
    previous: Option<&ProfileResumeDocument>,
) -> Result<(), AppError> {
    guard_document_path_reference(conn, saved.document_id, &saved.file_path)
        .map_err(AppError::from)?;
    match previous {
        None => conn.execute(
            &format!(
                "INSERT INTO profile_profile_resume_documents ({LINK_COLUMNS}) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7)"
            ),
            params![
                uuid_text(saved.id),
                uuid_text(saved.profile_id),
                uuid_text(saved.document_id),
                saved.file_path,
                saved.resume_type,
                instant_text(saved.created_at),
                instant_text(saved.updated_at),
            ],
        ),
        Some(_) => conn.execute(
            "UPDATE profile_profile_resume_documents SET document_id = ?1, \
             file_path = ?2, updated_at = ?3 WHERE id = ?4",
            params![
                uuid_text(saved.document_id),
                saved.file_path,
                instant_text(saved.updated_at),
                uuid_text(saved.id),
            ],
        ),
    }
    .map_err(SqliteError::from)?;
    Ok(())
}

/// The replacement body, run inside one transaction.
#[expect(
    clippy::too_many_arguments,
    reason = "the replace body takes every input explicitly"
)]
async fn replace_in(
    tx: &mut SqliteTransaction,
    link: &ProfileResumeDocument,
    binding: &GeneratedReservation,
    expected_revision: i64,
    expected_updated: OffsetDateTime,
    claims: Option<&[IntentReservation]>,
    descriptors: &[GenerationDescriptor],
    now: OffsetDateTime,
    clock: &dyn Clock,
) -> Result<ReplaceResult, AppError> {
    // Profile edits, deletion and sibling current-link writers wait for the writer this
    // transaction holds; the owner row is the committed one.
    let owner = run_with(tx, |conn| read_owner(conn, link.profile_id))?;
    let Some((revision, updated)) = owner else {
        return Err(not_found(
            "Profile resume source owner was not found",
            std::iter::empty::<(String, String)>(),
        )
        .into());
    };
    if revision != expected_revision || updated != expected_updated {
        return Err(conflict(
            "Profile resume source revision conflict",
            std::iter::empty::<(String, String)>(),
        )
        .into());
    }
    let previous = run_with(tx, |conn| find(conn, link.profile_id, &link.resume_type))?;
    if previous
        .as_ref()
        .is_some_and(|previous| link.updated_at < previous.updated_at)
    {
        return Err(conflict(
            "Profile resume replacement timestamp conflict",
            std::iter::empty::<(String, String)>(),
        )
        .into());
    }
    let saved = match &previous {
        None => link.clone(),
        Some(previous) => ProfileResumeDocument {
            id: previous.id,
            created_at: previous.created_at,
            ..link.clone()
        },
    };
    let removed_docs: Vec<Option<Uuid>> = previous
        .iter()
        .map(|previous| Some(previous.document_id))
        .collect();
    let removed_paths: Vec<String> = previous
        .iter()
        .map(|previous| previous.file_path.clone())
        .collect();
    let removed_doc_ids: Vec<Uuid> = previous
        .iter()
        .map(|previous| previous.document_id)
        .collect();
    let candidates = removed_assets(tx, &removed_doc_ids, &removed_paths)
        .await
        .map_err(link_error)?;
    lock_pdf_bindings(
        tx,
        &candidates,
        std::slice::from_ref(binding),
        &removed_docs,
    )
    .await
    .map_err(link_error)?;
    verify_pdf_binding(tx, binding).await.map_err(link_error)?;
    admit_generation_bindings(tx, std::slice::from_ref(binding), claims, descriptors, now)
        .await
        .map_err(link_error)?;
    run_with(tx, |conn| write_link(conn, &saved, previous.as_ref()))?;
    let retire_now = clock.now();
    for candidate in &candidates {
        let row = run_with(tx, |conn| {
            find_asset_in(conn, candidate.identity.asset_id())
        })
        .map_err(link_error)?;
        let Some(row) = row else {
            continue;
        };
        if matches!(row.lifecycle.as_str(), "RESERVED" | "ACTIVE") {
            let still_referenced = run_with::<_, SqliteError>(tx, |conn| {
                generated_asset_referenced(conn, row.document_id, row.identity.relative_path())
            })?;
            if !still_referenced {
                retire_asset(tx, &row, retire_now, false, None)
                    .await
                    .map_err(link_error)?;
            }
        }
    }
    let found = run_with(tx, |conn| find(conn, link.profile_id, &link.resume_type))?.ok_or(
        AppError::Application(ApplicationError::database_unavailable()),
    )?;
    if found != saved {
        return Err(AppError::Application(
            ApplicationError::database_unavailable(),
        ));
    }
    complete_generation_bindings(tx, claims, descriptors, clock.now())
        .await
        .map_err(link_error)?;
    ReplaceResult::new(found, previous).map_err(|_| AppError::Internal)
}
