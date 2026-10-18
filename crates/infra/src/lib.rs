//! PostgreSQL, filesystem, worker and network adapters for Jobniac.
//!
//! Only the PostgreSQL foundation exists so far: validated connection settings, the readiness
//! probe, SQLSTATE mapping and the migration runner. See [`postgres`].

pub mod arbeitnow;
pub mod blocking;
pub mod files;
pub mod generated;
pub mod postgres;
pub mod sqlite;
pub mod workers;
