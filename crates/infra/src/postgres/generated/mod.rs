//! PostgreSQL transactions of the generated-asset lifecycle.
//!
//! Every function takes the open client of a caller transaction, so the caller owns the BEGIN and
//! COMMIT, the advisory-lock order and the clock instant. Locks are taken in the try or
//! blocking forms with shared key derivations.

pub mod assets;
pub mod bindings;
pub mod classified;
pub mod intents;
pub mod ports;
pub mod profile_links;
pub mod repository;
pub mod writer;

use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::InvalidGeneratedIdentity;

use crate::postgres::unit_of_work::PgError;

/// Failure of a generated-asset transaction helper.
///
/// `Unavailable` means a lock is busy, a row moved, or a fence
/// is stale. `Binding` is a failed PDF binding check, which the profile link layer maps to its
/// caller-safe validation error. `Invalid` is an input that cannot be a generated identity.
/// `Database` carries the classified database failure of [`crate::postgres::profiles::db`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntentError {
    /// The generation claim is unavailable or stale.
    #[error("Generation continuation is unavailable")]
    Unavailable,
    /// The binding does not match the owned, active document content.
    #[error("Generated binding is invalid")]
    Binding,
    /// The value is not a valid generated identity or intent.
    #[error("Generated intent is invalid")]
    Invalid,
    /// A classified database failure.
    #[error("{0}")]
    Database(AppError),
}

impl From<InvalidGeneratedIdentity> for IntentError {
    fn from(_: InvalidGeneratedIdentity) -> Self {
        Self::Invalid
    }
}

impl From<AppError> for IntentError {
    fn from(error: AppError) -> Self {
        Self::Database(error)
    }
}

impl From<PgError> for IntentError {
    fn from(error: PgError) -> Self {
        Self::Database(AppError::from(error))
    }
}
