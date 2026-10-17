//! Framework-free domain types, use cases and ports for Jobniac.
//!
//! This crate has no Tauri, database, HTTP or filesystem dependency. Infrastructure adapters
//! implement the ports defined here; desktop commands, MCP tools and the CLI call the use cases.
#![forbid(unsafe_code)]

pub mod arbeitnow;
pub mod artifacts;
pub mod cleanup;
pub mod clock;
pub mod deletion;
pub mod documents;
pub mod env;
pub mod errors;
pub mod generated_assets;
pub mod generated_root;
pub mod generated_root_setup;
pub mod generation;
pub mod german;
pub mod health;
pub mod ids;
pub mod job_links;
pub mod jobs;
pub mod matching;
pub mod outbound;
pub mod pagination;
pub mod profiles;
pub mod reviews;
pub mod transactions;
