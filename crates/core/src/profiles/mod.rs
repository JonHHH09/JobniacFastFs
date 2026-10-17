//! Profile aggregate use cases: canonicalization, the normalized graph, revision
//! compare-and-set, project patches, search postings and bounded search, personal details and the
//! keyset listing of identities.
//!
//! Persistence is behind the ports of [`repository`]: every repository call takes an
//! explicit transaction handle from a [`repository::UnitOfWork`], so no connection is ambient.
//! Deterministic resume text extraction lives in `extract/` and stored-PDF ingestion (including the
//! one-to-one PDF source link) in `pdf_ingest`; generated resume-link artifacts are in
//! `generated_assets::profile_links`, not here.

pub mod draft;
pub mod extract;
pub mod graph;
pub mod link_url;
pub mod list;
pub mod model;
pub mod pdf_ingest;
pub mod personal_details;
pub mod repository;
pub mod revision;
pub mod search;
pub mod search_terms;
pub mod service;
pub mod values;

pub use draft::{
    ProfileDraft, ProjectPatch, canonicalize_profile_write, canonicalize_project_patch,
};
pub use graph::build_profile_graph;
pub use link_url::canonical_profile_link_url;
pub use list::{ProfileListPage, ProfileListService};
pub use model::{ProfileGraph, ProfileIdentity};
pub use personal_details::{
    PersonalDetailsEnvelope, PersonalDetailsInput, PersonalDetailsService,
    canonical_personal_details,
};
pub use repository::UnitOfWork;
pub use search::{ProfileSearchResponse, ProfileSearchService};
pub use search_terms::profile_search_terms;
pub use service::ProfileService;
pub use values::expected_revision;
