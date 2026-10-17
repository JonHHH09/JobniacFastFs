//! Composition of the opt-in document artifacts: the artifact service and the document exporter.
//!
//! The service is composed only when `JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED` is exactly `1` and the
//! database settings are complete. Otherwise nothing is built and the MCP tool stays absent. The
//! exporter protects the import root and the generated root (the configured value, and the
//! per-user default when it is in use), so an export never lands inside either of them. Composition
//! performs no IO.

use std::sync::Arc;

use jobniac_core::artifacts::ArtifactService;
use jobniac_core::env::Environment;
use jobniac_core::generated_root::RootPlatform;
use jobniac_infra::files::export::AtomicExporter;
use jobniac_infra::postgres::artifacts::PgArtifactRepository;
use jobniac_infra::postgres::config::DatabaseConfig;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;

use crate::generation::{load_generated_root_location, platform_name};
use crate::services::DOCUMENT_IMPORT_ROOT_VARIABLE;

/// The feature flag of document artifacts and native export (exact value `1`).
pub const DOCUMENT_ARTIFACTS_VARIABLE: &str = "JOBNIAC_DOCUMENT_ARTIFACTS_ENABLED";

/// The artifact use case over PostgreSQL.
pub type ArtifactUseCase = ArtifactService<PgUnitOfWork, PgArtifactRepository>;

/// The composed artifact services: the read use case and the native exporter.
#[derive(Debug, Clone)]
pub struct ArtifactServices {
    /// The read-only retrieval behind `get_document_artifact`.
    pub service: Arc<ArtifactUseCase>,
    /// The native export of a verified artifact to a new file.
    pub exporter: Arc<AtomicExporter>,
}

/// Composes the artifact services, or `None` when the flag is not exactly `1` or the database is
/// not configured.
#[must_use]
pub fn compose_artifacts(
    env: &Environment,
    uow: &PgUnitOfWork,
    database: Option<&DatabaseConfig>,
) -> Option<ArtifactServices> {
    if env.get(DOCUMENT_ARTIFACTS_VARIABLE) != Some("1") || database.is_none() {
        return None;
    }
    Some(ArtifactServices {
        service: Arc::new(ArtifactService::new(
            uow.clone(),
            PgArtifactRepository,
            true,
        )),
        exporter: Arc::new(AtomicExporter::new(protected_roots(env))),
    })
}

/// The roots an export may never enter: the import root, the generated root from the environment,
/// and the per-user default generated root when it is the one in use.
fn protected_roots(env: &Environment) -> Vec<String> {
    let mut roots: Vec<String> = [DOCUMENT_IMPORT_ROOT_VARIABLE, "JOBNIAC_GENERATED_ROOT"]
        .iter()
        .filter_map(|name| env.get(name))
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .collect();
    let platform = RootPlatform::from_platform_name(platform_name());
    if let Some(location) = load_generated_root_location(env, platform)
        && location.is_default
    {
        roots.push(location.root);
    }
    roots
}
