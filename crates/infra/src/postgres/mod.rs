//! PostgreSQL adapters: connection settings, the readiness probe, SQLSTATE mapping, migrations.

pub mod artifacts;
pub mod cleanup;
pub mod config;
pub mod deletion;
pub mod documents;
pub mod generated;
pub mod german;
pub mod health;
pub mod jobs;
pub mod match_reviews;
pub mod matching;
pub mod migrations;
pub mod pool;
pub mod profiles;
pub mod session;
pub mod sqlstate;
pub mod unit_of_work;
