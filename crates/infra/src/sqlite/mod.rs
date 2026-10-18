//! SQLite adapters: the connection pool, the unit of work, error classification, the readiness
//! probe and the migration runner. This module replaces [`crate::postgres`].
//!
//! Table names carry the old schema as a prefix (`profile.profiles` is `profile_profiles`); see
//! `sql/sqlite/V0001__baseline.sql`. `RULES_TO_PORT.md` lists the PostgreSQL rules that the
//! repositories enforce in Rust instead of SQL.

pub mod artifacts;
pub mod cleanup;
pub mod codec;
pub mod documents;
pub mod error;
pub mod generated;
pub mod health;
pub mod jobs;
pub mod location;
pub mod match_reviews;
pub mod matching;
pub mod migrations;
pub mod pool;
pub mod profiles;
pub mod unit_of_work;

#[cfg(test)]
mod schema_tests;
