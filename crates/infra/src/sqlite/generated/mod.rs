//! SQLite transactions of the generated-asset lifecycle.
//!
//! Every function takes the open transaction of a caller scope, so the caller owns the BEGIN and
//! COMMIT and the clock instant. There are no advisory locks: every write scope is `BEGIN
//! IMMEDIATE` on the single writer, so the lifecycle, the link writers and the cleanup exclude each
//! other by construction. The PL/pgSQL guards of the PostgreSQL schema run in Rust, in
//! [`guards`], right before the write they guard, or as a pre-commit check where the guard was a
//! deferred constraint trigger.

pub mod assets;
pub mod bindings;
pub mod classified;
pub mod guards;
pub mod intents;
pub mod ports;
pub mod profile_links;
pub mod repository;
pub mod writer;

use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::InvalidGeneratedIdentity;
use rusqlite::Connection;

use crate::sqlite::error::SqliteError;
use crate::sqlite::unit_of_work::SqliteTransaction;

/// Failure of a generated-asset transaction helper.
///
/// `Unavailable` means a row moved or a fence is stale. `Binding` is a failed PDF binding check,
/// which the profile link layer maps to its caller-safe validation error. `Invalid` is an input
/// that cannot be a generated identity. `Database` carries the classified database failure.
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

impl From<SqliteError> for IntentError {
    fn from(error: SqliteError) -> Self {
        Self::Database(AppError::from(error))
    }
}

impl From<rusqlite::Error> for IntentError {
    fn from(error: rusqlite::Error) -> Self {
        Self::from(SqliteError::from(error))
    }
}

/// Runs `work` on the transaction's connection. A failure of the bridge itself converts to `E`; the
/// body's own result (which may be an [`IntentError`] that is not a database failure) comes back as
/// is.
pub(crate) fn run_with<T, E: From<SqliteError>>(
    tx: &SqliteTransaction,
    work: impl FnOnce(&Connection) -> Result<T, E>,
) -> Result<T, E> {
    match tx.run(|conn| Ok(work(conn))) {
        Ok(result) => result,
        Err(error) => Err(E::from(error)),
    }
}
