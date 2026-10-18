//! SQLite, filesystem, worker and network adapters for Jobniac.
//!
//! The local database is an embedded SQLite file: see [`sqlite`] for the pool, the unit of work, the
//! migration runner, the readiness probe and the repositories.

pub mod arbeitnow;
pub mod blocking;
pub mod files;
pub mod generated;
pub mod sqlite;
pub mod workers;
