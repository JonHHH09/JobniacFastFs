//! Generation composition: the generated root from the environment, the generation services and
//! cleanup pass over one owned root, and the explicit root setup.
//!
//! Generation is admitted only when `JOBNIAC_GENERATED_ENABLED` is exactly `1`, the root is
//! admitted and a database is configured. Otherwise the services are still composed,
//! disabled, so that every operation refuses with the same error.
//! Composition creates only a missing per-user default directory, and only when admitted; it
//! registers nothing: the explicit `setup-generated-root` command does that. The
//! only file read is the owned marker, and only when `JOBNIAC_GENERATED_ROOT_ID` is unset (an
//! unproven candidate that the qualification gate still checks).

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::cleanup::{
    CleanupExecutor, CleanupHealthRepository, CleanupHealthService, CleanupRepository,
    GenerationLifecycle, GenerationRootQualification, OwnedAssetRemover, QualifiedCleanupHealth,
};
use jobniac_core::clock::{Clock, SystemClock};
use jobniac_core::documents::{DocumentMetadata, DocumentService};
use jobniac_core::env::{EnvValue, Environment};
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::GeneratedAssetService;
use jobniac_core::generated_root::{RootPlatform, admit_root_path, resolve_default_generated_root};
use jobniac_core::generated_root_setup::GeneratedRootSetup;
use jobniac_core::generation::facade::{DocumentMetadataPort, GenerationService};
use jobniac_core::generation::generic::GenericGenerationService;
use jobniac_core::generation::standalone::StandaloneGenerationService;
use jobniac_core::generation::translation::FrenchTranslator;
use jobniac_core::german::generation::{GermanGenerationPorts, GermanGenerationService};
use jobniac_core::german::translation::GermanResumeTranslator;
use jobniac_core::health::CleanupHealthPort;
use jobniac_infra::generated::asset_files::OwnedAssetFiles;
use jobniac_infra::generated::root::ensure_private_directory;
use jobniac_infra::generated::setup::RootSetupFiles;
use jobniac_infra::sqlite::cleanup::{
    OwnedFileAdapter, SqliteCleanupRepository, SqliteSetupRegistry,
};
use jobniac_infra::sqlite::documents::SqliteDocumentRepository;
use jobniac_infra::sqlite::generated::ports::SqliteProfileGraphs;
use jobniac_infra::sqlite::generated::profile_links::SqliteProfileResumeLinks;
use jobniac_infra::sqlite::generated::repository::SqliteGeneratedAssetRepository;
use jobniac_infra::sqlite::generated::writer::OwnedFileWriter;
use jobniac_infra::sqlite::german::cover_letter::SqliteGermanCoverLetterRepository;
use jobniac_infra::sqlite::german::resume::SqliteGermanResumeRepository;
use jobniac_infra::sqlite::german::sources::SqliteGermanSources;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::profiles::SqliteProfileRepository;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use uuid::Uuid;

use crate::blocking::BlockingGermanRenderer;

/// Opt-in flag of the generation lifecycle: admitted only as exactly `1`.
pub const GENERATED_ENABLED_VARIABLE: &str = "JOBNIAC_GENERATED_ENABLED";
/// Owned generated-asset root. Set, even empty, means used exactly.
pub const GENERATED_ROOT_VARIABLE: &str = "JOBNIAC_GENERATED_ROOT";
/// Durable registration identity of the root: a canonical lowercase UUID.
pub const GENERATED_ROOT_ID_VARIABLE: &str = "JOBNIAC_GENERATED_ROOT_ID";

/// An admitted generated root: the validated path and its registration identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedRootConfig {
    /// The validated root path. It is never printed.
    pub root: String,
    /// The registration identity of the root.
    pub root_id: Uuid,
    /// Whether the path is the per-user default, which composition creates when generation is on.
    pub is_default: bool,
}

/// A generated-root location: an admitted explicit or default path. The path is never printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedRootLocation {
    /// The validated root path.
    pub root: String,
    /// Whether the path is the per-user default (only the default may be created by setup).
    pub is_default: bool,
}

/// The platform name of the running process: `win32`, `darwin` or `linux`.
#[must_use]
pub const fn platform_name() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

/// Admits the generated-root location from one environment snapshot. An explicit root is used
/// exactly; otherwise the per-user default is resolved from the same snapshot. Nothing is created
/// or read.
#[must_use]
pub fn load_generated_root_location(
    env: &Environment,
    platform: RootPlatform,
) -> Option<GeneratedRootLocation> {
    match env.lookup(GENERATED_ROOT_VARIABLE) {
        EnvValue::Valid(value) => {
            admit_root_path(value, platform).map(|root| GeneratedRootLocation {
                root,
                is_default: false,
            })
        }
        EnvValue::Invalid => None,
        EnvValue::Missing => {
            resolve_default_generated_root(env, platform).map(|root| GeneratedRootLocation {
                root,
                is_default: true,
            })
        }
    }
}

/// A set `JOBNIAC_GENERATED_ROOT_ID` that is not the canonical lowercase hyphenated UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidRootIdentity;

/// Reads the identity of the registered root from `JOBNIAC_GENERATED_ROOT_ID`: unset
/// gives `Ok(None)`; a set value must be the canonical lowercase hyphenated form.
///
/// # Errors
///
/// [`InvalidRootIdentity`] when the variable is set but is not a canonical UUID (or not valid
/// Unicode).
pub fn configured_root_identity(env: &Environment) -> Result<Option<Uuid>, InvalidRootIdentity> {
    match env.lookup(GENERATED_ROOT_ID_VARIABLE) {
        EnvValue::Missing => Ok(None),
        EnvValue::Invalid => Err(InvalidRootIdentity),
        EnvValue::Valid(text) => {
            let parsed = Uuid::parse_str(text).map_err(|_| InvalidRootIdentity)?;
            // Only the canonical lowercase hyphenated form is admitted.
            if parsed.hyphenated().to_string() == text {
                Ok(Some(parsed))
            } else {
                Err(InvalidRootIdentity)
            }
        }
    }
}

/// Admits the generated root from one environment snapshot.
///
/// Returns `None` unless the flag is exactly `1` and the root and its identity are admitted. A
/// set but malformed identity disables generation, and a missing one is read from the owned
/// marker through `marker`.
pub fn load_generated_root_config(
    env: &Environment,
    platform: RootPlatform,
    marker: impl FnOnce(&str) -> Option<Uuid>,
) -> Option<GeneratedRootConfig> {
    if !env.flag_enabled(GENERATED_ENABLED_VARIABLE) {
        return None;
    }
    let location = load_generated_root_location(env, platform)?;
    let root_id = match configured_root_identity(env) {
        Ok(Some(root_id)) => root_id,
        Ok(None) => marker(&location.root)?,
        Err(InvalidRootIdentity) => return None,
    };
    Some(GeneratedRootConfig {
        root: location.root,
        root_id,
        is_default: location.is_default,
    })
}

/// Adapts the document use cases to the metadata port of the generation facade.
pub struct DocumentMetadataAdapter(
    pub Arc<DocumentService<SqliteUnitOfWork, SqliteDocumentRepository>>,
);

impl std::fmt::Debug for DocumentMetadataAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DocumentMetadataAdapter")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl DocumentMetadataPort for DocumentMetadataAdapter {
    async fn get_document_metadata(&self, document_id: &str) -> Result<DocumentMetadata, AppError> {
        self.0.get_document_metadata(document_id).await
    }
}

/// The composed generation use cases below the document port: the asset lifecycle, the facade
/// inputs, the German use cases, the cleanup executor and the cleanup health port.
pub struct GenerationCore {
    standalone: StandaloneGenerationService,
    generic: GenericGenerationService,
    german: GermanGenerationService,
    qualification: Arc<GenerationRootQualification>,
    clock: Arc<dyn Clock>,
    /// The cleanup pass of the `cleanup` command and of the lifecycle.
    pub lifecycle: Arc<GenerationLifecycle>,
    /// The cleanup executor. It refuses unless generation is enabled and the root is configured.
    pub cleanup: Arc<CleanupExecutor>,
    /// The cleanup health port. It reports `UNKNOWN` unless the root is qualified.
    pub cleanup_health: Arc<dyn CleanupHealthPort>,
}

impl std::fmt::Debug for GenerationCore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GenerationCore")
            .finish_non_exhaustive()
    }
}

/// The composed generation services: the facade for the desktop and MCP adapters, the cleanup
/// executor, the lifecycle pass and the cleanup health port of the `generatedResumeCleanup` block.
pub struct GenerationServices {
    /// The generation facade. Every operation refuses while generation is not qualified.
    pub facade: Arc<GenerationService>,
    /// The cleanup executor. It refuses unless generation is enabled and the root is configured.
    pub cleanup: Arc<CleanupExecutor>,
    /// The bounded cleanup pass of the `cleanup` command.
    pub lifecycle: Arc<GenerationLifecycle>,
    /// The cleanup health port. It reports `UNKNOWN` unless the root is qualified.
    pub cleanup_health: Arc<dyn CleanupHealthPort>,
}

impl std::fmt::Debug for GenerationServices {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GenerationServices")
            .finish_non_exhaustive()
    }
}

impl GenerationCore {
    /// Adds the document use cases and builds the facade.
    #[must_use]
    pub fn with_documents(
        self,
        documents: Arc<DocumentService<SqliteUnitOfWork, SqliteDocumentRepository>>,
    ) -> GenerationServices {
        let facade = GenerationService::new(
            self.standalone,
            self.generic,
            Arc::new(DocumentMetadataAdapter(documents)),
            self.qualification,
            self.clock,
        )
        .with_german(self.german);
        GenerationServices {
            facade: Arc::new(facade),
            cleanup: self.cleanup,
            lifecycle: self.lifecycle,
            cleanup_health: self.cleanup_health,
        }
    }
}

/// Composes the generation use cases, without the document port, over `uow`.
///
/// Generation runs only when the root configuration is admitted and `uow` has a database;
/// otherwise every service is built disabled over the placeholder root, and refuses before any root
/// or database IO.
pub fn compose_core(env: &Environment, uow: &SqliteUnitOfWork) -> GenerationCore {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let config = load_generated_root_config(
        env,
        RootPlatform::from_platform_name(platform_name()),
        |root| RootSetupFiles::new(root, false).read_marker_identity(),
    )
    // Only the enabled lifecycle creates the per-user default directory, privately.
    .filter(|config| !config.is_default || ensure_private_directory(&config.root));
    // Generation needs both an admitted root and a database.
    let enabled = config.is_some() && uow.pool().configured();
    let (files, root_id) = match config {
        Some(config) => (
            OwnedAssetFiles::new(Some(config.root), Some(config.root_id)),
            config.root_id,
        ),
        None => (OwnedAssetFiles::new(None, None), Uuid::nil()),
    };
    let repository = Arc::new(SqliteGeneratedAssetRepository::new(
        uow.clone(),
        Arc::clone(&clock),
    ));
    let writer = Arc::new(OwnedFileWriter::new(files.clone()));
    let assets = Arc::new(GeneratedAssetService::new(
        repository,
        writer,
        root_id,
        Arc::clone(&clock),
        enabled,
    ));
    let standalone =
        StandaloneGenerationService::new(Arc::clone(&assets), root_id, Arc::clone(&clock), enabled);
    let generic = GenericGenerationService::new(
        Arc::new(SqliteProfileGraphs::new(
            uow.clone(),
            SqliteProfileRepository,
        )),
        Arc::new(SqliteProfileResumeLinks::new(
            uow.clone(),
            Arc::clone(&clock),
        )),
        Arc::clone(&assets),
        Arc::new(FrenchTranslator::builtin()),
        root_id,
        Arc::clone(&clock),
        enabled,
    );
    let german = GermanGenerationService::new(
        GermanGenerationPorts {
            profiles: Arc::new(SqliteProfileGraphs::new(
                uow.clone(),
                SqliteProfileRepository,
            )),
            jobs: Arc::new(SqliteGermanSources::new(uow.clone())),
            personal_details: Arc::new(SqliteGermanSources::new(uow.clone())),
            resumes: Arc::new(SqliteGermanResumeRepository::new(
                uow.clone(),
                Arc::clone(&clock),
            )),
            cover_letters: Arc::new(SqliteGermanCoverLetterRepository::new(
                uow.clone(),
                Arc::clone(&clock),
            )),
            assets: Arc::clone(&assets),
            renderer: Arc::new(BlockingGermanRenderer::default()),
        },
        Arc::new(GermanResumeTranslator::builtin()),
        root_id,
        Arc::clone(&clock),
        enabled,
    );
    let cleanup_repository = Arc::new(SqliteCleanupRepository::new(
        uow.clone(),
        Arc::clone(&clock),
    ));
    let remover: Arc<dyn OwnedAssetRemover> = Arc::new(OwnedFileAdapter::new(files.clone()));
    let qualification = Arc::new(GenerationRootQualification::new(
        Some(Arc::clone(&cleanup_repository)
            as Arc<dyn jobniac_core::cleanup::GenerationRootRepository>),
        Some(Arc::new(OwnedFileAdapter::new(files))
            as Arc<dyn jobniac_core::cleanup::GenerationRootFiles>),
        enabled.then_some(root_id),
        enabled,
    ));
    let cleanup = Arc::new(CleanupExecutor::new(
        Some(Arc::clone(&cleanup_repository) as Arc<dyn CleanupRepository>),
        Some(remover),
        enabled.then_some(root_id),
        enabled,
    ));
    let lifecycle = Arc::new(GenerationLifecycle::new(
        Arc::clone(&qualification),
        Arc::clone(&assets),
        Arc::clone(&cleanup),
        Arc::clone(&clock),
    ));
    let cleanup_health: Arc<dyn CleanupHealthPort> = if enabled {
        Arc::new(QualifiedCleanupHealth::new(
            Arc::clone(&qualification),
            cleanup_repository as Arc<dyn CleanupHealthRepository>,
            Arc::clone(&clock),
        ))
    } else {
        Arc::new(CleanupHealthService::new(None, Arc::clone(&clock), false))
    };
    GenerationCore {
        standalone,
        generic,
        german,
        qualification,
        clock,
        lifecycle,
        cleanup,
        cleanup_health,
    }
}

/// Composes the generation services over `uow` and the document use cases.
pub fn compose_generation(
    env: &Environment,
    uow: &SqliteUnitOfWork,
    documents: Arc<DocumentService<SqliteUnitOfWork, SqliteDocumentRepository>>,
) -> GenerationServices {
    compose_core(env, uow).with_documents(documents)
}

/// Composes the explicit root setup over `database` for the admitted `location`. Construction
/// performs no IO.
#[must_use]
pub fn compose_root_setup(
    database: &SqlitePool,
    location: &GeneratedRootLocation,
    configured_id: Option<Uuid>,
) -> GeneratedRootSetup {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let repository =
        SqliteCleanupRepository::new(SqliteUnitOfWork::new(database.clone()), Arc::clone(&clock));
    GeneratedRootSetup::new(
        Arc::new(RootSetupFiles::new(
            location.root.clone(),
            location.is_default,
        )),
        Arc::new(SqliteSetupRegistry::new(repository, clock)),
        configured_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT_ID: &str = "6f1c2d3e-4a5b-4c6d-8e7f-900112233445";

    fn env(pairs: &[(&str, &str)]) -> Environment {
        Environment::from_pairs(pairs.iter().copied())
    }

    #[test]
    fn only_the_exact_flag_one_admits_the_root() {
        let base = [
            (GENERATED_ROOT_VARIABLE, "/srv/jobniac/generated"),
            (GENERATED_ROOT_ID_VARIABLE, ROOT_ID),
        ];
        for value in ["true", "0", "01", " 1", ""] {
            let mut pairs = base.to_vec();
            pairs.push((GENERATED_ENABLED_VARIABLE, value));
            assert!(
                load_generated_root_config(&env(&pairs), RootPlatform::Other, |_| None).is_none(),
                "flag {value:?}"
            );
        }
        let mut pairs = base.to_vec();
        pairs.push((GENERATED_ENABLED_VARIABLE, "1"));
        let config = load_generated_root_config(&env(&pairs), RootPlatform::Other, |_| None);
        assert_eq!(
            config,
            Some(GeneratedRootConfig {
                root: "/srv/jobniac/generated".to_owned(),
                root_id: Uuid::parse_str(ROOT_ID).expect("synthetic UUID"),
                is_default: false,
            })
        );
    }

    #[test]
    fn a_malformed_identity_disables_generation_and_is_never_replaced_by_the_marker() {
        for identity in [
            "6F1C2D3E-4A5B-4C6D-8E7F-900112233445",
            "{6f1c2d3e-4a5b-4c6d-8e7f-900112233445}",
            "6f1c2d3e4a5b4c6d8e7f900112233445",
            "not-a-uuid",
        ] {
            let pairs = [
                (GENERATED_ENABLED_VARIABLE, "1"),
                (GENERATED_ROOT_VARIABLE, "/srv/jobniac/generated"),
                (GENERATED_ROOT_ID_VARIABLE, identity),
            ];
            let config = load_generated_root_config(&env(&pairs), RootPlatform::Other, |_| {
                panic!("the marker must not be read when the identity is set")
            });
            assert!(config.is_none(), "identity {identity:?}");
        }
    }

    #[test]
    fn an_unset_identity_comes_from_the_marker_candidate() {
        let pairs = [
            (GENERATED_ENABLED_VARIABLE, "1"),
            (GENERATED_ROOT_VARIABLE, "/srv/jobniac/generated"),
        ];
        let marker_id = Uuid::parse_str(ROOT_ID).expect("synthetic UUID");
        let config =
            load_generated_root_config(&env(&pairs), RootPlatform::Other, |_| Some(marker_id));
        assert_eq!(config.map(|config| config.root_id), Some(marker_id));
    }

    #[test]
    fn an_empty_or_relative_explicit_root_is_refused_without_a_fallback() {
        for root in ["", "relative/generated", "/srv/../generated"] {
            let pairs = [
                (GENERATED_ENABLED_VARIABLE, "1"),
                (GENERATED_ROOT_VARIABLE, root),
                (GENERATED_ROOT_ID_VARIABLE, ROOT_ID),
            ];
            assert!(
                load_generated_root_config(&env(&pairs), RootPlatform::Other, |_| None).is_none(),
                "root {root:?}"
            );
        }
    }

    #[test]
    fn the_default_root_is_resolved_from_the_snapshot_when_unset() {
        let pairs = [
            (GENERATED_ENABLED_VARIABLE, "1"),
            (GENERATED_ROOT_ID_VARIABLE, ROOT_ID),
            ("HOME", "/home/synthetic"),
        ];
        let config = load_generated_root_config(&env(&pairs), RootPlatform::Other, |_| None);
        assert_eq!(
            config.map(|config| config.root),
            Some("/home/synthetic/.local/share/jobniac/generated".to_owned())
        );
    }

    #[test]
    fn the_setup_location_does_not_need_the_flag_and_marks_the_default() {
        let explicit = [(GENERATED_ROOT_VARIABLE, "/srv/jobniac/generated")];
        assert_eq!(
            load_generated_root_location(&env(&explicit), RootPlatform::Other),
            Some(GeneratedRootLocation {
                root: "/srv/jobniac/generated".to_owned(),
                is_default: false,
            })
        );
        let default = [("HOME", "/home/synthetic")];
        assert_eq!(
            load_generated_root_location(&env(&default), RootPlatform::Other),
            Some(GeneratedRootLocation {
                root: "/home/synthetic/.local/share/jobniac/generated".to_owned(),
                is_default: true,
            })
        );
    }

    #[test]
    fn the_configured_identity_must_be_canonical() {
        assert_eq!(configured_root_identity(&env(&[])), Ok(None));
        assert_eq!(
            configured_root_identity(&env(&[(GENERATED_ROOT_ID_VARIABLE, ROOT_ID)])),
            Ok(Some(Uuid::parse_str(ROOT_ID).expect("synthetic UUID")))
        );
        assert_eq!(
            configured_root_identity(&env(&[(GENERATED_ROOT_ID_VARIABLE, "not-a-uuid")])),
            Err(InvalidRootIdentity)
        );
    }

    #[test]
    fn an_uppercase_uuid_is_refused_as_non_canonical() {
        let uppercase = ROOT_ID.to_ascii_uppercase();
        assert_eq!(
            configured_root_identity(&env(&[(GENERATED_ROOT_ID_VARIABLE, uppercase.as_str())])),
            Err(InvalidRootIdentity)
        );
    }
}
