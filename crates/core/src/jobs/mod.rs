//! Paste-first job capture, revision-checked update, read and bounded job search.
//!
//! Persistence is behind the ports of [`repository`]; the PostgreSQL adapter (verbatim SQL, advisory
//! locks, the 500-candidate sample and the recursive search CTE) lives in `jobniac-infra`. The
//! link-create path (`add_job_from_link`) is not here: [`model::LinkIngestionRecord`] carries only
//! the identity that the fingerprint rule needs.

pub mod capture;
pub mod model;
pub mod repository;
pub mod request;
pub mod search;
pub mod service;
pub mod text;
pub mod update;

pub use capture::{CaptureHash, TextJobFields, assemble_text_job, capture_hash, prepare_text_job};
pub use model::{
    CaptureStatus, IdSource, JobAggregate, JobCaptureResult, JobListPage, JobRecord,
    JobSkillRecord, LinkIngestionRecord, RandomIds, TextIngestionRecord,
};
pub use repository::{JobRepository, JobSearchRepository};
pub use request::{JOB_REQUEST_MESSAGE, optional_instant};
pub use search::{JobSearchMatch, JobSearchResponse, JobSearchService};
pub use service::JobService;
pub use text::{
    TITLE_MAX_UTF16_UNITS, UNTITLED_JOB, canonical_whitespace, derive_title, extract_experience,
    extract_skills, fingerprint, input_text_hash, merge_skills, normalized_key, split_skill_list,
};
pub use update::{TextField, UpdateRequest, build_update, parse_update, planned_fingerprint};
