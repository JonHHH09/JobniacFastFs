//! PostgreSQL stores of the German resume and cover letter.
//!
//! * [`resume`]: [`PgGermanResumeRepository`], the aggregate replace of the resume tree.
//! * [`cover_letter`]: [`PgGermanCoverLetterRepository`], the replace of one exact-resume letter.
//!
//! Each port method is one transaction of the unit of work. The write transactions begin at the
//! server default, which is READ COMMITTED, and reads use the shared read-only scope. The pools are
//! named `jobniac-german-resume` and `jobniac-german-cover-letter` through [`resume::pool`] and
//! [`cover_letter::pool`].
//!
//! Both writers take the same pair advisory lock, computed in SQL (`hashtextextended`), before
//! any row lock. They then lock the profile, the job and the resume rows, verify the PDF bindings,
//! delete and insert, and retire the replaced owned assets last.

pub mod cover_letter;
pub mod resume;
pub mod sources;

use jobniac_core::errors::{AppError, ApplicationError, conflict, not_found};
use jobniac_core::generated_assets::GeneratedReservation;
use jobniac_core::german::resume_store::{PairKey, Revision};
use time::OffsetDateTime;
use uuid::Uuid;

use super::generated::IntentError;
use super::generated::assets::retire_asset;
use super::generated::bindings::{ASSET_COLUMNS, AssetRow};
use super::profiles::{col, db};
use super::unit_of_work::PgTransaction;

pub use cover_letter::PgGermanCoverLetterRepository;
pub use resume::PgGermanResumeRepository;

/// `application_name` of the German resume sessions.
pub const RESUME_APPLICATION_NAME: &str = "jobniac-german-resume";

/// `application_name` of the German cover-letter sessions.
pub const COVER_LETTER_APPLICATION_NAME: &str = "jobniac-german-cover-letter";

/// Takes the pair advisory lock of a profile and job. The key is hashed by the
/// server so that every writer of the pair excludes the others.
pub(crate) async fn lock_pair(tx: &mut PgTransaction, pair: PairKey) -> Result<(), AppError> {
    db(tx
        .client()
        .execute(
            "SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))",
            &[&pair.lock_name()],
        )
        .await)?;
    Ok(())
}

/// Locks the profile and the job rows and compares both owner revisions, numeric and instant
///. `what` is the text of the
/// fixed messages, `German resume` or `German cover-letter`.
pub(crate) async fn check_owners(
    tx: &mut PgTransaction,
    pair: PairKey,
    expected: (Revision, Revision),
    stored: (OffsetDateTime, OffsetDateTime),
    what: &str,
) -> Result<(), AppError> {
    let profile = db(tx
        .client()
        .query_opt(
            "SELECT revision, updated_at FROM profile.profiles WHERE id = $1 FOR UPDATE",
            &[&pair.profile_id],
        )
        .await)?;
    let job = db(tx
        .client()
        .query_opt(
            "SELECT revision, updated_at FROM job_schema.jobs WHERE id = $1 FOR UPDATE",
            &[&pair.job_id],
        )
        .await)?;
    let (Some(profile), Some(job)) = (profile, job) else {
        return Err(not_found(
            &format!("{what} source owner was not found"),
            std::iter::empty::<(String, String)>(),
        )
        .into());
    };
    let profile_revision: i64 = col(&profile, "revision")?;
    let job_revision: i64 = col(&job, "revision")?;
    let profile_updated: OffsetDateTime = col(&profile, "updated_at")?;
    let job_updated: OffsetDateTime = col(&job, "updated_at")?;
    if profile_revision != expected.0.get()
        || job_revision != expected.1.get()
        || profile_updated != stored.0
        || job_updated != stored.1
    {
        return Err(source_conflict(what));
    }
    Ok(())
}

/// The fixed revision conflict of a source owner.
pub(crate) fn source_conflict(what: &str) -> AppError {
    conflict(
        &format!("{what} source revision conflict"),
        std::iter::empty::<(String, String)>(),
    )
    .into()
}

/// The refusal of a database state that cannot be.
pub(crate) fn unavailable() -> AppError {
    ApplicationError::database_unavailable().into()
}

/// Maps a failure of a generated-asset helper: database failures keep their classification, every
/// other refusal is the fixed validation error of the store.
pub(crate) fn intent_error(error: IntentError, invalid: fn() -> ApplicationError) -> AppError {
    match error {
        IntentError::Database(error) => error,
        IntentError::Unavailable | IntentError::Binding | IntentError::Invalid => invalid().into(),
    }
}

/// Retires the replaced assets that nothing references any more, in `asset_id` order (the shared
/// tail of both replace transactions). Each candidate row is locked and its lifecycle re-read; a
/// `RESERVED` or `ACTIVE` asset that `jobniac.generated_asset_referenced` reports unreferenced is
/// moved to `RETIRING` with its cleanup task.
pub(crate) async fn retire_unreferenced(
    tx: &mut PgTransaction,
    candidates: &[AssetRow],
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let mut asset_ids: Vec<Uuid> = candidates
        .iter()
        .map(|row| row.identity.asset_id())
        .collect();
    asset_ids.sort_unstable();
    for asset_id in asset_ids {
        let row = db(tx
            .client()
            .query_opt(
                &format!(
                    "SELECT {ASSET_COLUMNS} FROM jobniac.generated_assets \
                     WHERE asset_id = $1 FOR UPDATE"
                ),
                &[&asset_id],
            )
            .await)?;
        let Some(row) = row else {
            continue;
        };
        // A stored identity the schema would not allow is corrupted data, not caller input.
        let row = AssetRow::from_row(&row).map_err(|_| AppError::Internal)?;
        if !matches!(row.lifecycle.as_str(), "RESERVED" | "ACTIVE") {
            continue;
        }
        // Re-read lifecycle and ownership after lock acquisition, then check both link identities
        // after the new references have been inserted.
        let referenced = db(tx
            .client()
            .query_one(
                "SELECT jobniac.generated_asset_referenced($1, $2) AS referenced",
                &[&row.document_id, &row.identity.relative_path()],
            )
            .await)?;
        let referenced: bool = col(&referenced, "referenced")?;
        if !referenced {
            retire_asset(tx, &row, now, false, None)
                .await
                .map_err(|error| match error {
                    IntentError::Database(error) => error,
                    _ => AppError::Internal,
                })?;
        }
    }
    Ok(())
}

/// The document ids and relative paths of the bindings that a variant list must carry, for the
/// removed-asset selection of a replace.
pub(crate) fn removed_parts(removed: &[(Uuid, String)]) -> (Vec<Uuid>, Vec<String>) {
    removed.iter().cloned().unzip()
}

/// Whether `binding` carries the document and path of a stored variant.
pub(crate) fn binding_matches(
    binding: &GeneratedReservation,
    document_id: Uuid,
    file_path: &str,
) -> bool {
    binding.document_id == document_id && binding.asset.relative_path() == file_path
}

#[cfg(test)]
mod tests {
    use jobniac_core::generated_assets::AssetIdentity;

    use super::*;

    fn binding() -> GeneratedReservation {
        let asset = AssetIdentity::new(Uuid::from_u128(1), Uuid::from_u128(2), &"a".repeat(64), 9)
            .expect("a valid identity");
        GeneratedReservation {
            asset,
            document_id: Uuid::from_u128(3),
        }
    }

    #[test]
    fn binding_matches_requires_the_document_and_the_path() {
        let binding = binding();
        let path = binding.asset.relative_path().to_owned();
        assert!(binding_matches(&binding, Uuid::from_u128(3), &path));
        assert!(!binding_matches(&binding, Uuid::from_u128(4), &path));
        assert!(!binding_matches(&binding, Uuid::from_u128(3), "other.pdf"));
    }

    #[test]
    fn pair_lock_name_is_the_key_text() {
        let pair = PairKey {
            profile_id: Uuid::from_u128(1),
            job_id: Uuid::from_u128(2),
        };
        assert_eq!(
            pair.lock_name(),
            "jobniac-germany-resume:00000000-0000-0000-0000-000000000001:\
             00000000-0000-0000-0000-000000000002"
        );
    }

    #[test]
    fn removed_parts_splits_documents_and_paths_in_order() {
        let removed = vec![
            (Uuid::from_u128(1), "a.pdf".to_owned()),
            (Uuid::from_u128(2), "b.pdf".to_owned()),
        ];
        let (documents, paths) = removed_parts(&removed);
        assert_eq!(documents, vec![Uuid::from_u128(1), Uuid::from_u128(2)]);
        assert_eq!(paths, vec!["a.pdf".to_owned(), "b.pdf".to_owned()]);
    }

    #[test]
    fn intent_failures_become_the_store_validation_error() {
        let invalid = || jobniac_core::errors::validation_error("fixed", None, None);
        for error in [
            IntentError::Unavailable,
            IntentError::Binding,
            IntentError::Invalid,
        ] {
            assert_eq!(intent_error(error, invalid), AppError::from(invalid()));
        }
        assert_eq!(
            intent_error(IntentError::Database(AppError::Internal), invalid),
            AppError::Internal
        );
    }
}
