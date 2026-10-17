//! Advisory reviews and divergence disagreements.
//!
//! A review is a reviewer's scored assessment of a deterministic report. It is stored separately and
//! never changes the report. Under policy `divergence-v1` a review that diverges from its report
//! creates a disagreement, which a person can acknowledge or link to an issue reference.
//!
//! The identifiers are content addresses (see [`review_fingerprint`] and [`disagreement_fingerprint`]),
//! so the same content always has the same identifier. The persistence is behind
//! [`MatchReviewRepository`]; the PostgreSQL adapter lives in `jobniac-infra`.

pub mod divergence;
pub mod fingerprint;
pub mod model;
pub mod repository;
mod request;
pub mod service;
pub mod validation;

pub use divergence::{create_match_disagreement, divergence_reasons};
pub use fingerprint::{disagreement_fingerprint, review_fingerprint};
pub use model::{
    DisagreementList, DisagreementStatus, EvidenceDefectCode, MatchDisagreement, MatchReview,
    POLICY_VERSION, Reason, ReviewList, ReviewSubmission, Summary,
};
pub use repository::{DisagreementFactory, MatchReviewRepository, ReviewFactory};
pub use service::MatchReviewService;
pub use validation::{create_match_review, validate_match_disagreement, validate_match_review};
