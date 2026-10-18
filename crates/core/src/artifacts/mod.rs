//! Document artifacts.
//!
//! Read-only, disabled by default. A verified stored PDF of at most 1 MiB is returned as its
//! seven-field metadata projection plus standard padded Base64 content. The bytes are checked
//! against the stored size and SHA-256, and the `%PDF-` magic is re-verified on every read.
//!
//! * [`model`]: the bounds, the fixed messages and [`model::validate_artifact`].
//! * [`repository`]: the persistence port (the SQLite adapter lives in
//!   `jobniac_infra::sqlite::artifacts`).
//! * [`service`]: the [`service::ArtifactService`] use case and the export use case over the
//!   [`service::ArtifactExporter`] port (the atomic exporter lives in `jobniac_infra::files::export`).

pub mod model;
pub mod render;
pub mod repository;
pub mod service;

pub use model::{
    ARTIFACT_INTEGRITY_MESSAGE, ARTIFACT_VALIDATION_MESSAGE, DocumentArtifact, MAX_ARTIFACT_BYTES,
    validate_artifact,
};
pub use render::ArtifactPageRenderer;
pub use repository::{ArtifactLookupError, ArtifactRepository};
pub use service::{ArtifactExporter, ArtifactService, ExportFailure};
