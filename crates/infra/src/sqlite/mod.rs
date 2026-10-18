//! SQLite adapters: the connection pool, the unit of work, error classification, the readiness
//! probe, the migration runner, the shared column codecs and the repositories. This module replaces
//! [`crate::postgres`].
//!
//! Table names carry the old schema as a prefix (`profile.profiles` is `profile_profiles`); see
//! `sql/sqlite/V0001__baseline.sql`.
//!
//! # Rules enforced in Rust
//!
//! The baseline schema keeps every rule that SQLite can express row-locally (CHECK constraints and
//! `RAISE(ABORT)` triggers). The PostgreSQL rules that read other tables, need the whole
//! transaction state or need Unicode case folding run in Rust. A guard refusal is
//! `SqliteError::Constraint` with SQLSTATE 23514, the class of the PL/pgSQL trigger it replaces.
//! Guards run right before the write they guard; the rules that were deferred constraint triggers
//! are pre-commit checks registered with
//! [`SqliteTransaction::before_commit`](unit_of_work::SqliteTransaction::before_commit). Advisory
//! locks and `FOR UPDATE` reads are dropped: every write transaction is `BEGIN IMMEDIATE`.
//!
//! | Rule | Where it runs |
//! |---|---|
//! | Job link URL userinfo (`normalized_url`, `url`) | [`jobs::graph::check_link_graph`] |
//! | Disagreement evidence defect codes | `match_reviews` store, before the insert |
//! | Job source provenance (one link or text row per job) | [`jobs::graph::provenance_check`], pre-commit |
//! | Canonical Unicode lowercase of emails, types, skills, languages, technologies | core normalization before every write; SQL checks and indexes only cover ASCII |
//! | Five-minute pending lease | [`generated::guards::guard_intent_insert`], [`generated::guards::guard_intent_update`] |
//! | Asset adoption, document identity, retirement, deletion, cleanup identity and retention | [`generated::guards`] (`guard_asset_insert`, `guard_retirement`, `guard_deletion`, `guard_cleanup_write`, `guard_cleanup_delete`), called by `generated` and `cleanup` |
//! | Owned documents attach only while `ACTIVE`, keep their path and content | `guard_document_reference`, `guard_document_path_reference`, `guard_pdf_source`, `guard_document_content`, `guard_blob_content`, called by `documents`, `profiles`, `generated::profile_links` and `german` |
//! | Intent birth, document, lease renewal, finish window, completion proof, pending retirement | [`generated::guards::guard_intent_insert`], [`generated::guards::guard_intent_update`], [`generated::guards::guard_retirement`] |
//! | Birth intent and abandonment retirement at commit | [`generated::guards::check_birth_intents`], [`generated::guards::check_abandoned_retirement`], pre-commit |
//! | Completed stand-alone outputs count as references | [`generated::guards::generated_document_referenced`] |
//!
//! Conventions for the writers: every instant is the fixed-width form of [`codec::instant_text`],
//! JSON columns hold compact text, and uuids are lowercase hyphenated text ([`codec`]).

pub mod artifacts;
pub mod cleanup;
pub mod codec;
pub mod deletion;
pub mod documents;
pub mod error;
pub mod generated;
pub mod german;
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
