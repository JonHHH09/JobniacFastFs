//! SQLite stores of the German resume and cover letter.
//!
//! * [`resume`]: [`SqliteGermanResumeRepository`], the aggregate replace of the resume tree.
//! * [`cover_letter`]: [`SqliteGermanCoverLetterRepository`], the replace of one exact-resume
//!   letter.
//!
//! Each port method is one transaction of the unit of work: the write transactions are
//! `BEGIN IMMEDIATE` on the single writer and the reads use the shared read-only scope.
//!
//! The pair advisory lock and the `FOR UPDATE` row locks of the PostgreSQL stores are not needed:
//! the writer is exclusive for the whole transaction, so the owner revisions that
//! [`check_owners`] reads cannot change before the delete and insert. The writers verify the PDF
//! bindings, delete and insert (calling the owned-document path guard before each variant
//! insert, rule 14), and retire the replaced owned assets last.

pub mod cover_letter;
pub mod resume;
pub mod sources;

use jobniac_core::errors::{AppError, ApplicationError, conflict, not_found};
use jobniac_core::generated_assets::GeneratedReservation;
use jobniac_core::german::resume_store::{PairKey, Revision};
use time::OffsetDateTime;
use uuid::Uuid;

use super::codec::{Id, Instant};
use super::generated::IntentError;
use super::generated::assets::retire_asset;
use super::generated::bindings::{ASSET_COLUMNS, AssetRow};
use super::generated::guards::generated_asset_referenced;
use super::profiles::{col, query_rows, with_connection};
use super::unit_of_work::SqliteTransaction;

pub use cover_letter::SqliteGermanCoverLetterRepository;
pub use resume::SqliteGermanResumeRepository;
pub use sources::SqliteGermanSources;

/// Compares both owner revisions, numeric and instant. `what` is the text of the fixed messages,
/// `German resume` or `German cover-letter`.
pub(crate) fn check_owners(
    tx: &SqliteTransaction,
    pair: PairKey,
    expected: (Revision, Revision),
    stored: (OffsetDateTime, OffsetDateTime),
    what: &str,
) -> Result<(), AppError> {
    let owners = with_connection(tx, |conn| {
        let read = |sql: &str, id: Uuid| {
            query_rows(conn, sql, &[&Id(id)], |row| {
                Ok((
                    col::<i64>(row, "revision")?,
                    col::<Instant>(row, "updated_at")?.0,
                ))
            })
        };
        Ok((
            read(
                "SELECT revision, updated_at FROM profile_profiles WHERE id = ?1",
                pair.profile_id,
            )?,
            read(
                "SELECT revision, updated_at FROM job_jobs WHERE id = ?1",
                pair.job_id,
            )?,
        ))
    })?;
    let (Some(profile), Some(job)) = (owners.0.first(), owners.1.first()) else {
        return Err(not_found(
            &format!("{what} source owner was not found"),
            std::iter::empty::<(String, String)>(),
        )
        .into());
    };
    if profile.0 != expected.0.get()
        || job.0 != expected.1.get()
        || profile.1 != stored.0
        || job.1 != stored.1
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
/// tail of both replace transactions). Each candidate's lifecycle is re-read; a `RESERVED` or
/// `ACTIVE` asset that [`generated_asset_referenced`] reports unreferenced is moved to `RETIRING`
/// with its cleanup task.
pub(crate) async fn retire_unreferenced(
    tx: &mut SqliteTransaction,
    candidates: &[AssetRow],
    now: OffsetDateTime,
) -> Result<(), AppError> {
    let mut asset_ids: Vec<Uuid> = candidates
        .iter()
        .map(|row| row.identity.asset_id())
        .collect();
    asset_ids.sort_unstable();
    for asset_id in asset_ids {
        let rows = with_connection(tx, |conn| {
            query_rows(
                conn,
                &format!(
                    "SELECT {ASSET_COLUMNS} FROM jobniac_generated_assets WHERE asset_id = ?1"
                ),
                &[&Id(asset_id)],
                // A stored identity the schema would not allow is corrupted data, not caller input.
                |row| AssetRow::from_row(row).map_err(|_| AppError::Internal),
            )
        })?;
        let Some(row) = rows.into_iter().next() else {
            continue;
        };
        if !matches!(row.lifecycle.as_str(), "RESERVED" | "ACTIVE") {
            continue;
        }
        // Check both link identities after the new references have been inserted.
        let referenced = with_connection(tx, |conn| {
            Ok(generated_asset_referenced(
                conn,
                row.document_id,
                row.identity.relative_path(),
            )?)
        })?;
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
