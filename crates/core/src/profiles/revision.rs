//! Revision compare-and-set rules and the profile errors shared by the profile use cases.
//!
//! Covers the next-revision rule, the profile conflict and the `Profile not found` error. A profile
//! at the maximum revision cannot be replaced: the next write reports a conflict rather than
//! wrapping.

use uuid::Uuid;

use crate::errors::{AppError, conflict, not_found};

use super::values::MAX_REVISION;

/// `Profile not found: <id>` with the `resource` and `profileId` details.
pub(crate) fn profile_not_found(profile_id: Uuid) -> AppError {
    AppError::from(not_found(
        &format!("Profile not found: {profile_id}"),
        [
            ("resource", "profile".to_owned()),
            ("profileId", profile_id.to_string()),
        ],
    ))
}

/// `Profile revision conflict` with `resource`, `profileId` and `expectedRevision` details.
pub(crate) fn profile_conflict(profile_id: Uuid, expected_revision: i64) -> AppError {
    AppError::from(conflict(
        "Profile revision conflict",
        [
            ("resource", "profile".to_owned()),
            ("profileId", profile_id.to_string()),
            ("expectedRevision", expected_revision.to_string()),
        ],
    ))
}

/// The revision after `expected`, or a conflict at the maximum.
///
/// # Errors
///
/// Returns the profile conflict when `expected` is the largest supported revision.
pub(crate) fn next_revision(profile_id: Uuid, expected: i64) -> Result<i64, AppError> {
    if expected == MAX_REVISION {
        return Err(profile_conflict(profile_id, expected));
    }
    Ok(expected + 1)
}

/// Checks a persisted revision: it must be a non-negative integer.
///
/// # Errors
///
/// A persisted revision that is negative is an internal error.
pub(crate) fn persisted_revision(revision: i64) -> Result<i64, AppError> {
    if revision < 0 {
        return Err(AppError::Internal);
    }
    Ok(revision)
}
