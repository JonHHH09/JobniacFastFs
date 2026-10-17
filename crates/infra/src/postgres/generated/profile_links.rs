//! PostgreSQL adapter of the current profile resume links. A replacement is one
//! transaction: the profile row lock, the sorted PDF bindings, the owner CAS, the link write,
//! retirement of the replaced asset and the generation completion.

use std::sync::Arc;

use jobniac_core::clock::Clock;
use jobniac_core::errors::{AppError, ApplicationError, conflict, not_found, validation_error};
use jobniac_core::generated_assets::{
    GeneratedReservation, GenerationDescriptor, GenerationPurpose, IntentReservation,
    ProfileResumeDocument, ReplaceResult, admit_profile_resume_link, normalize_resume_type,
    validate_binding_intents, validate_revision,
};
use jobniac_core::transactions::TransactionOptions;
use time::OffsetDateTime;
use tokio_postgres::Row;
use uuid::Uuid;

use super::IntentError;
use super::assets::retire_asset;
use super::bindings::{AssetRow, lock_pdf_bindings, removed_assets, verify_pdf_binding};
use super::intents::{admit_generation_bindings, complete_generation_bindings};
use crate::postgres::profiles::{col, db};
use crate::postgres::unit_of_work::{PgTransaction, PgUnitOfWork};

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
fn link_from_row(row: &Row) -> Result<ProfileResumeDocument, AppError> {
    ProfileResumeDocument::new(
        col(row, "id")?,
        col(row, "profile_id")?,
        col(row, "document_id")?,
        &col::<String>(row, "file_path")?,
        &col::<String>(row, "resume_type")?,
        col(row, "created_at")?,
        col(row, "updated_at")?,
    )
    .map_err(|_| AppError::Internal)
}

/// Finds the current link of a (profile, type) pair.
async fn find(
    tx: &mut PgTransaction,
    profile_id: Uuid,
    resume_type: &str,
    for_update: bool,
) -> Result<Option<ProfileResumeDocument>, AppError> {
    let sql = format!(
        "SELECT {LINK_COLUMNS} FROM profile.profile_resume_documents \
         WHERE profile_id = $1 AND resume_type = $2{}",
        if for_update { " FOR UPDATE" } else { "" }
    );
    let row = db(tx
        .client()
        .query_opt(&sql, &[&profile_id, &resume_type])
        .await)?;
    row.as_ref().map(link_from_row).transpose()
}

/// Repository of the current profile resume links over the unit of work.
#[derive(Clone)]
pub struct PgProfileResumeLinks {
    uow: PgUnitOfWork,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for PgProfileResumeLinks {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PgProfileResumeLinks")
            .finish_non_exhaustive()
    }
}

impl PgProfileResumeLinks {
    /// Creates the repository over `uow`, reading time from `clock`.
    #[must_use]
    pub fn new(uow: PgUnitOfWork, clock: Arc<dyn Clock>) -> Self {
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
                async |tx| find(tx, profile_id, &canonical, false).await,
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

/// The replacement body, run inside one transaction.
#[expect(
    clippy::too_many_arguments,
    reason = "the replace body takes every input explicitly"
)]
async fn replace_in(
    tx: &mut PgTransaction,
    link: &ProfileResumeDocument,
    binding: &GeneratedReservation,
    expected_revision: i64,
    expected_updated: OffsetDateTime,
    claims: Option<&[IntentReservation]>,
    descriptors: &[GenerationDescriptor],
    now: OffsetDateTime,
    clock: &dyn Clock,
) -> Result<ReplaceResult, AppError> {
    // Pinned: after waiting, the owner and asset rows are reread as
    // committed, whatever the role or database default is. It must be the first statement.
    db(tx
        .client()
        .batch_execute("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .await)?;
    // Profile edits, deletion and sibling current-link writers share this owner lock.
    let owner = db(tx
        .client()
        .query_opt(
            "SELECT revision, updated_at FROM profile.profiles WHERE id = $1 FOR UPDATE",
            &[&link.profile_id],
        )
        .await)?;
    let Some(owner) = owner else {
        return Err(not_found(
            "Profile resume source owner was not found",
            std::iter::empty::<(String, String)>(),
        )
        .into());
    };
    let revision: i64 = col(&owner, "revision")?;
    let updated: OffsetDateTime = col(&owner, "updated_at")?;
    if revision != expected_revision || updated != expected_updated {
        return Err(conflict(
            "Profile resume source revision conflict",
            std::iter::empty::<(String, String)>(),
        )
        .into());
    }
    let previous = find(tx, link.profile_id, &link.resume_type, true).await?;
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
    match &previous {
        None => {
            db(tx
                .client()
                .execute(
                    &format!(
                        "INSERT INTO profile.profile_resume_documents ({LINK_COLUMNS}) \
                         VALUES ($1,$2,$3,$4,$5,$6,$7)"
                    ),
                    &[
                        &saved.id,
                        &saved.profile_id,
                        &saved.document_id,
                        &saved.file_path,
                        &saved.resume_type,
                        &saved.created_at,
                        &saved.updated_at,
                    ],
                )
                .await)?;
        }
        Some(_) => {
            db(tx
                .client()
                .execute(
                    "UPDATE profile.profile_resume_documents SET document_id = $1, \
                     file_path = $2, updated_at = $3 WHERE id = $4",
                    &[
                        &saved.document_id,
                        &saved.file_path,
                        &saved.updated_at,
                        &saved.id,
                    ],
                )
                .await)?;
        }
    }
    let retire_now = clock.now();
    for candidate in &candidates {
        let row = db(tx
            .client()
            .query_opt(
                &format!(
                    "SELECT {} FROM jobniac.generated_assets WHERE asset_id = $1 FOR UPDATE",
                    super::bindings::ASSET_COLUMNS
                ),
                &[&candidate.identity.asset_id()],
            )
            .await)?;
        let Some(row) = row else {
            continue;
        };
        let row = AssetRow::from_row(&row).map_err(link_error)?;
        if matches!(row.lifecycle.as_str(), "RESERVED" | "ACTIVE") {
            let document = row.document_id;
            let referenced_row = db(tx
                .client()
                .query_one(
                    "SELECT jobniac.generated_asset_referenced($1, $2) AS referenced",
                    &[&document, &row.identity.relative_path()],
                )
                .await)?;
            let referenced: bool = col(&referenced_row, "referenced")?;
            if !referenced {
                retire_asset(tx, &row, retire_now, false, None)
                    .await
                    .map_err(link_error)?;
            }
        }
    }
    let found = find(tx, link.profile_id, &link.resume_type, false)
        .await?
        .ok_or(AppError::Application(
            ApplicationError::database_unavailable(),
        ))?;
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
