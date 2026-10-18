//! Deterministic matching (`deterministic-v1`) and the immutable report history.
//!
//! The scorer is a pure function of two
//! graphs and a timestamp ([`scorer::score_match`]). Persistence is behind the port of
//! [`repository::MatchingRepository`]; the SQLite adapter (SQL, the snapshot retry of
//! [`repository::retries_serialization_failure`]) lives in `jobniac-infra`. The use cases in
//! [`service::MatchAnalysisService`] take the raw request object, so validation runs in a fixed
//! order.
//!
//! The graphs are flat structs ([`ProfileGraph`],
//! [`JobGraph`]) that keep identifiers and revisions as the text the repository produced, so the
//! scorer parses them and reports errors in a fixed order.

pub mod graph;
pub mod privacy;
pub mod report;
pub mod repository;
pub mod scorer;
pub mod service;
pub mod validation;

pub use graph::{JobGraph, ProfileGraph, uuid_from_text};
pub use report::{
    ALGORITHM_VERSION, Component, ComponentStatus, MatchComponent, MatchEvidence, MatchReport,
    MatchView, Outcome, is_stale, report_id, report_identity,
};
pub use repository::{
    MatchingRepository, PairScorer, SNAPSHOT_ATTEMPTS, retries_serialization_failure,
};
pub use scorer::{ProfileSkillIndex, score_match, score_match_with_index};
pub use service::{BatchAnalysis, MatchAnalysisService, PairFailure, ReportList};
pub use validation::{validate_match_component, validate_match_evidence};
