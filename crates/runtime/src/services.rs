//! Composition root for the health, profile and job services, plus the document, matching,
//! generation and opt-in services. The opt-in services (the cleanup health port, the Arbeitnow scan
//! and import services, profile and job deletion, guarded link fetch and document artifacts) stay
//! off unless their flag is exactly `1`, and each is composed only after the settings it needs
//! have been validated.
//!
//! The profile and job use cases are composed without a database too: their shared
//! pool then reports `Local database is not configured` when a use case first reads or writes.

use std::sync::Arc;

use std::path::PathBuf;

use jobniac_core::clock::{Clock, SystemClock};
use jobniac_core::documents::{DocumentListService, DocumentService};
use jobniac_core::env::Environment;
use jobniac_core::health::HealthService;
use jobniac_core::jobs::{JobSearchService, JobService};
use jobniac_core::matching::MatchAnalysisService;
use jobniac_core::pagination::{CursorCodec, CursorKey, CursorKeyError};
use jobniac_core::profiles::pdf_ingest::ProfilePdfService;
use jobniac_core::profiles::{
    PersonalDetailsService, ProfileListService, ProfileSearchService, ProfileService,
};
use jobniac_core::reviews::MatchReviewService;
use jobniac_infra::postgres::config::{DatabaseConfig, load_database_config};
use jobniac_infra::postgres::documents::PgDocumentRepository;
use jobniac_infra::postgres::health::PostgresHealthProbe;
use jobniac_infra::postgres::jobs::PgJobRepository;
use jobniac_infra::postgres::match_reviews::PgMatchReviewRepository;
use jobniac_infra::postgres::matching::{MATCHING_APPLICATION_NAME, PgMatchingRepository};
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::profiles::PgProfileRepository;
use jobniac_infra::postgres::profiles::pdf::PgProfilePdfRepository;
use jobniac_infra::postgres::session::JOB_SEARCH_APPLICATION_NAME;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;

use crate::arbeitnow::load_arbeitnow_config;
use crate::artifacts::{ArtifactServices, compose_artifacts};
use crate::blocking::BlockingHasher;
use crate::deletion::{DeletionServices, compose_deletion};
use crate::document_ports::{
    ImportRootReader, WorkerCancellation, WorkerPdfExtractor, WorkerPdfRenderer,
};
use crate::generation::{GenerationServices, compose_generation};

/// Environment variable of the document import root (`JOBNIAC_DOCUMENT_IMPORT_ROOT`).
pub const DOCUMENT_IMPORT_ROOT_VARIABLE: &str = "JOBNIAC_DOCUMENT_IMPORT_ROOT";
/// Environment variable of the PDFium library directory used by the PDF worker.
pub const PDFIUM_DIR_VARIABLE: &str = "JOBNIAC_PDFIUM_DIR";

/// The services composed at startup.
#[derive(Debug, Clone)]
pub struct Services {
    /// The readiness use case shared by every inbound adapter.
    pub health: Arc<HealthService>,
    /// The validated local database settings; `None` leaves health reporting DOWN.
    pub database: Option<DatabaseConfig>,
    /// The profile use cases, all over one PostgreSQL unit of work.
    pub profiles: ProfileServices,
    /// The job use cases, over the same unit of work as the profile use cases.
    pub jobs: JobServices,
    /// The matching and advisory review use cases, over one pool named `jobniac-matching`.
    pub matching: MatchingServices,
    /// The document use cases: import, metadata and the extraction cache.
    pub documents: DocumentServices,
    /// Profile ingestion from a stored PDF and the PDF source lookup, over the profile unit of work
    /// and the document use cases.
    pub profile_pdf: Arc<ProfilePdfServiceType>,
    /// The generation facade, the cleanup executor and the cleanup health port. Generation is
    /// enabled only when `JOBNIAC_GENERATED_ENABLED` is exactly `1` and the root is admitted.
    pub generation: Arc<GenerationServices>,
    /// The profile and job hard-delete use cases: composed only when `JOBNIAC_DELETION_ENABLED` is
    /// exactly `1` and a database is configured (see [`crate::deletion`]).
    pub deletion: DeletionServices,
    /// The opt-in document artifacts (read and native export); `None` unless the flag is exactly `1`
    /// and the database is configured.
    pub artifacts: Option<ArtifactServices>,
}

/// The stored-PDF profile ingestion over the profile unit of work, the profile repository, the
/// PDF repository and the document use cases.
pub type ProfilePdfServiceType = ProfilePdfService<
    PgUnitOfWork,
    PgProfileRepository,
    PgProfilePdfRepository,
    Arc<DocumentService<PgUnitOfWork, PgDocumentRepository>>,
>;

/// The document use cases, over the profile unit of work.
#[derive(Debug, Clone)]
pub struct DocumentServices {
    /// Store from the import root, metadata reads, stored or import-root text extraction, and the
    /// stored page render (`render_stored_page`).
    pub service: Arc<DocumentService<PgUnitOfWork, PgDocumentRepository>>,
    /// The cancellation of the PDF worker calls made by `service`. Reset and shutdown cancel every
    /// call in flight through it.
    pub cancellation: WorkerCancellation,
    /// The PDF renderer behind `service`, shared with the artifact viewer, which renders the verified
    /// bytes it holds through the same worker path.
    pub renderer: Arc<dyn jobniac_core::documents::repository::PdfRenderer>,
    /// Desktop-only keyset listing of stored documents, signed with the cursor key of
    /// `JOBNIAC_CURSOR_HMAC_SECRET`, validated at startup like the other listings.
    pub list: Arc<DocumentListService<PgUnitOfWork, PgDocumentRepository>>,
}

/// The matching and review use cases.
///
/// Both share one session pool named `jobniac-matching`. Without a
/// database configuration every call fails with `Local database is not configured` before any
/// connection is attempted.
#[derive(Debug, Clone)]
pub struct MatchingServices {
    /// Deterministic pair analysis, the bounded batch over candidate jobs, the report read and the
    /// keyset report history. The history is signed with the cursor key of
    /// `JOBNIAC_CURSOR_HMAC_SECRET`, validated at startup like the other listings.
    pub analysis: Arc<MatchAnalysisService<PgMatchingRepository>>,
    /// Advisory review submission, the review and disagreement histories, and the status changes of
    /// disagreements. Cursors are signed with the same key.
    pub reviews: Arc<MatchReviewService<PgMatchReviewRepository>>,
}

/// The job use cases.
///
/// The capture, read and update use case shares the session pool of [`ProfileServices`]. The
/// search use case has a pool of its own. Without a database configuration
/// every call fails with `Local database is not configured` before any connection is attempted.
#[derive(Debug, Clone)]
pub struct JobServices {
    /// Paste-first capture, get, keyset list and the revision-checked update. The list is signed
    /// with the cursor key of `JOBNIAC_CURSOR_HMAC_SECRET`, validated at startup like the profile
    /// listing.
    pub service: Arc<JobService<PgUnitOfWork, PgJobRepository>>,
    /// Bounded job search over the job posting index. It runs over its own pool, whose sessions are
    /// named `jobniac-job-search`, so they stand apart from the
    /// other job and profile sessions in `pg_stat_activity`.
    pub search: Arc<JobSearchService<PgUnitOfWork, PgJobRepository>>,
}

/// The profile use cases.
///
/// All of them share one session pool, which the job use cases use too. Without a database
/// configuration every call fails with the `Local database is not configured` error before
/// any connection is attempted.
#[derive(Debug, Clone)]
pub struct ProfileServices {
    /// Profile graph reads and writes: get, create, replace and project patch.
    pub details: Arc<ProfileService<PgUnitOfWork, PgProfileRepository>>,
    /// Personal details: the optional date of birth and nationality under the revision check.
    pub personal_details: Arc<PersonalDetailsService<PgUnitOfWork, PgProfileRepository>>,
    /// Bounded search over the profile posting index.
    pub search: Arc<ProfileSearchService<PgUnitOfWork, PgProfileRepository>>,
    /// Keyset listing of identities, signed with the cursor key of `JOBNIAC_CURSOR_HMAC_SECRET`.
    /// [`compose`] never returns the `Err` variant: a too-short secret stops startup instead, so a
    /// composed value is always `Ok`. The `Result` is kept so the consumers in other crates do not
    /// change.
    pub list: Result<Arc<ProfileListService<PgUnitOfWork, PgProfileRepository>>, CursorKeyError>,
}

/// The sanitized startup failure. It never says which setting is wrong.
#[derive(Debug, thiserror::Error)]
#[error("Jobniac could not start. Check local configuration.")]
pub struct StartupError;

/// Compose the services from one environment snapshot.
///
/// The opt-in Arbeitnow keys are validated first, before anything is built. An enabled feature
/// with bad keys stops startup. Incomplete database settings are not an error: they produce a
/// service whose health check reports DOWN without connecting.
pub fn compose(env: &Environment) -> Result<Services, StartupError> {
    let pdfium_dir = env.get(PDFIUM_DIR_VARIABLE).map(PathBuf::from);
    compose_with_pdfium(env, pdfium_dir)
}

/// Compose the services with the PDFium directory the caller resolved (see
/// [`crate::pdfium_location`]). `None` leaves the PDF operations failing when they run.
///
/// # Errors
///
/// The same startup failures as [`compose`].
pub fn compose_with_pdfium(
    env: &Environment,
    pdfium_dir: Option<PathBuf>,
) -> Result<Services, StartupError> {
    load_arbeitnow_config(env).map_err(|_| StartupError)?;
    let database = load_database_config(env);
    let probe = Arc::new(PostgresHealthProbe::new(database.clone()));
    let uow = PgUnitOfWork::new(PgPool::new(database.clone()));
    let search_uow = PgUnitOfWork::new(PgPool::with_application_name(
        database.clone(),
        JOB_SEARCH_APPLICATION_NAME,
    ));
    let matching_uow = PgUnitOfWork::new(PgPool::with_application_name(
        database.clone(),
        MATCHING_APPLICATION_NAME,
    ));
    let profiles = compose_profiles(env, &uow)?;
    let jobs = compose_jobs(env, &uow, &search_uow)?;
    let matching = compose_matching(env, &matching_uow)?;
    let documents = compose_documents(env, &uow, pdfium_dir)?;
    let profile_pdf = compose_profile_pdf(&uow, &documents);
    let generation = Arc::new(compose_generation(
        env,
        &uow,
        database.clone(),
        Arc::clone(&documents.service),
    ));
    let deletion = compose_deletion(env, database.as_ref());
    let artifacts = compose_artifacts(env, &uow, database.as_ref());
    let health = HealthService::new(
        probe,
        Some(Arc::clone(&generation.cleanup_health)),
        Arc::new(SystemClock),
    );
    Ok(Services {
        health: Arc::new(health),
        database,
        profiles,
        jobs,
        matching,
        documents,
        profile_pdf,
        generation,
        deletion,
        artifacts,
    })
}

/// Builds the stored-PDF profile ingestion over the profile unit of work, the document use cases
/// and the process clock. It shares the connection pool of the profile use cases.
fn compose_profile_pdf(
    uow: &PgUnitOfWork,
    documents: &DocumentServices,
) -> Arc<ProfilePdfServiceType> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    Arc::new(ProfilePdfService::new(
        uow.clone(),
        PgProfileRepository,
        PgProfilePdfRepository,
        Arc::clone(&documents.service),
        clock,
    ))
}

/// Builds the document use cases over the profile unit of work, with the import root from the
/// environment and the PDF worker over the resolved PDFium directory. Composition performs no IO: a
/// missing import root or PDFium directory, or a failed worker supervisor, only makes the matching
/// operation fail when it runs.
fn compose_documents(
    env: &Environment,
    uow: &PgUnitOfWork,
    pdfium_dir: Option<PathBuf>,
) -> Result<DocumentServices, StartupError> {
    let list_key = CursorKey::from_environment(env).map_err(|_| StartupError)?;
    let reader = ImportRootReader::new(env.get(DOCUMENT_IMPORT_ROOT_VARIABLE).map(str::to_owned));
    let supervisor = crate::diagnostics::current_process_supervisor();
    let cancellation = WorkerCancellation::new();
    let extractor =
        WorkerPdfExtractor::shared(supervisor.clone(), pdfium_dir.clone(), cancellation.clone());
    let renderer = Arc::new(WorkerPdfRenderer::new(
        supervisor,
        pdfium_dir,
        cancellation.clone(),
    ));
    Ok(DocumentServices {
        service: Arc::new(
            DocumentService::new(
                uow.clone(),
                PgDocumentRepository,
                Arc::new(reader),
                Arc::new(extractor),
                Arc::new(SystemClock),
            )
            .with_renderer(renderer.clone())
            .with_hasher(Arc::new(BlockingHasher)),
        ),
        cancellation,
        renderer,
        list: Arc::new(DocumentListService::new(
            uow.clone(),
            PgDocumentRepository,
            CursorCodec::new(list_key),
        )),
    })
}

/// Builds the matching and review use cases over `uow`, with the process clock and the cursor key.
///
/// Both listings are signed with the cursor key of `JOBNIAC_CURSOR_HMAC_SECRET`, so a too-short
/// secret stops startup, as for the profile and job listings.
fn compose_matching(
    env: &Environment,
    uow: &PgUnitOfWork,
) -> Result<MatchingServices, StartupError> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let analysis_key = CursorKey::from_environment(env).map_err(|_| StartupError)?;
    let review_key = CursorKey::from_environment(env).map_err(|_| StartupError)?;
    Ok(MatchingServices {
        analysis: Arc::new(MatchAnalysisService::new(
            PgMatchingRepository::new(uow.clone()),
            CursorCodec::new(analysis_key),
            Arc::clone(&clock),
        )),
        reviews: Arc::new(MatchReviewService::new(
            PgMatchReviewRepository::new(uow.clone()),
            CursorCodec::new(review_key),
            clock,
        )),
    })
}

/// Builds the profile use cases over one unit of work and the process clock and cursor key.
///
/// A too-short `JOBNIAC_CURSOR_HMAC_SECRET` stops startup.
fn compose_profiles(
    env: &Environment,
    uow: &PgUnitOfWork,
) -> Result<ProfileServices, StartupError> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let key = CursorKey::from_environment(env).map_err(|_| StartupError)?;
    let list = Arc::new(ProfileListService::new(
        uow.clone(),
        PgProfileRepository,
        CursorCodec::new(key),
    ));
    Ok(ProfileServices {
        details: Arc::new(ProfileService::new(
            uow.clone(),
            PgProfileRepository,
            Arc::clone(&clock),
        )),
        personal_details: Arc::new(PersonalDetailsService::new(
            uow.clone(),
            PgProfileRepository,
            clock,
        )),
        search: Arc::new(ProfileSearchService::new(uow.clone(), PgProfileRepository)),
        list: Ok(list),
    })
}

/// Builds the job use cases: capture, read and update over `uow`, and search over `search_uow`,
/// with the process clock and the cursor key.
///
/// The job listing is signed with the same key as the profile listing, so the same startup rule
/// applies: a too-short secret stops startup.
fn compose_jobs(
    env: &Environment,
    uow: &PgUnitOfWork,
    search_uow: &PgUnitOfWork,
) -> Result<JobServices, StartupError> {
    let key = CursorKey::from_environment(env).map_err(|_| StartupError)?;
    Ok(JobServices {
        service: Arc::new(JobService::new(
            uow.clone(),
            PgJobRepository,
            Arc::new(SystemClock),
            CursorCodec::new(key),
        )),
        search: Arc::new(JobSearchService::new(search_uow.clone(), PgJobRepository)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobniac_core::health::{CleanupReport, ErrorCategory, HealthStatus};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const DATABASE_PAIRS: [(&str, &str); 5] = [
        ("JOBNIAC_DB_HOST", "127.0.0.1"),
        ("JOBNIAC_DB_PORT", "5432"),
        ("JOBNIAC_DB_NAME", "jobniac_test"),
        ("JOBNIAC_DB_USER", "app_user"),
        ("JOBNIAC_DB_PASSWORD", "synthetic-password"),
    ];
    const CANDIDATE_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
    const CURSOR_SECRET: &str = "cursor-secret-0123456789abcdefghijklmnop";

    #[tokio::test]
    async fn empty_environment_composes_and_reports_down() -> TestResult {
        let services = compose(&Environment::default())?;
        assert!(services.database.is_none());
        let report = services.health.check().await;
        assert_eq!(report.status, HealthStatus::Down);
        assert_eq!(report.error_category, ErrorCategory::ConnectionUnavailable);
        assert_eq!(report.generated_resume_cleanup, CleanupReport::unknown());
        assert_eq!(report.metadata.total_checks, 1);
        assert_eq!(report.metadata.failed_checks, 1);
        Ok(())
    }

    #[test]
    fn complete_database_settings_are_kept_without_connecting() -> TestResult {
        let services = compose(&Environment::from_pairs(DATABASE_PAIRS))?;
        let database = services
            .database
            .as_ref()
            .ok_or("database config missing")?;
        assert_eq!(database.host(), "127.0.0.1");
        assert_eq!(database.port(), 5432);
        Ok(())
    }

    #[test]
    fn enabled_arbeitnow_with_invalid_keys_stops_startup() {
        let env = Environment::from_pairs([
            ("JOBNIAC_ARBEITNOW_ENABLED", "1"),
            ("JOBNIAC_ARBEITNOW_CANDIDATE_KEY", "not a valid key!"),
            ("JOBNIAC_CURSOR_HMAC_SECRET", "short"),
        ]);
        assert!(matches!(compose(&env), Err(StartupError)));
    }

    #[test]
    fn invalid_arbeitnow_settings_stop_startup_even_with_a_database() {
        let mut pairs = DATABASE_PAIRS.to_vec();
        pairs.push(("JOBNIAC_ARBEITNOW_ENABLED", "1"));
        pairs.push(("JOBNIAC_ARBEITNOW_CANDIDATE_KEY", "AAAA"));
        pairs.push(("JOBNIAC_CURSOR_HMAC_SECRET", CURSOR_SECRET));
        assert!(matches!(
            compose(&Environment::from_pairs(pairs)),
            Err(StartupError)
        ));
    }

    #[test]
    fn invalid_arbeitnow_keys_are_ignored_when_the_flag_is_not_one() -> TestResult {
        let env = Environment::from_pairs([
            ("JOBNIAC_ARBEITNOW_ENABLED", "true"),
            ("JOBNIAC_ARBEITNOW_CANDIDATE_KEY", "not a valid key!"),
            ("JOBNIAC_CURSOR_HMAC_SECRET", CURSOR_SECRET),
        ]);
        let services = compose(&env)?;
        assert!(services.database.is_none());
        Ok(())
    }

    #[test]
    fn valid_arbeitnow_keys_compose_without_opt_in_services() -> TestResult {
        let env = Environment::from_pairs([
            ("JOBNIAC_ARBEITNOW_ENABLED", "1"),
            ("JOBNIAC_ARBEITNOW_CANDIDATE_KEY", CANDIDATE_KEY),
            ("JOBNIAC_CURSOR_HMAC_SECRET", CURSOR_SECRET),
        ]);
        let services = compose(&env)?;
        assert!(services.database.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn profile_reads_report_not_configured_without_a_database() -> TestResult {
        let services = compose(&Environment::default())?;
        assert!(services.database.is_none());
        let error = services
            .profiles
            .search
            .search_profiles(&serde_json::json!({"query": "ada"}))
            .await
            .err()
            .ok_or("search must fail without a database")?;
        assert_eq!(
            jobniac_core::errors::error_envelope(&error).message,
            "Local database is not configured"
        );
        Ok(())
    }

    #[tokio::test]
    async fn job_reads_report_not_configured_without_a_database() -> TestResult {
        let services = compose(&Environment::default())?;
        assert!(services.database.is_none());
        let get = services
            .jobs
            .service
            .get_job("00000000-0000-0000-0000-000000000001")
            .await
            .err()
            .ok_or("get must fail without a database")?;
        assert_eq!(
            jobniac_core::errors::error_envelope(&get).message,
            "Local database is not configured"
        );
        let list = services
            .jobs
            .service
            .list_jobs(Some(2), None)
            .await
            .err()
            .ok_or("list must fail without a database")?;
        assert_eq!(
            jobniac_core::errors::error_envelope(&list).message,
            "Local database is not configured"
        );
        let search = services
            .jobs
            .search
            .search_jobs(&serde_json::json!({"query": "java"}))
            .await
            .err()
            .ok_or("search must fail without a database")?;
        assert_eq!(
            jobniac_core::errors::error_envelope(&search).message,
            "Local database is not configured"
        );
        Ok(())
    }

    #[tokio::test]
    async fn matching_reads_and_writes_report_not_configured_without_a_database() -> TestResult {
        let services = compose(&Environment::default())?;
        assert!(services.database.is_none());
        let analysis = services
            .matching
            .analysis
            .analyze_job_match(&serde_json::json!({
                "profileId": "00000000-0000-0000-0000-000000000001",
                "jobId": "00000000-0000-0000-0000-000000000002",
            }))
            .await
            .err()
            .ok_or("analysis must fail without a database")?;
        assert_eq!(
            jobniac_core::errors::error_envelope(&analysis).message,
            "Local database is not configured"
        );
        let review = services
            .matching
            .reviews
            .submit_match_review(&serde_json::json!({
                "reportId": "00000000-0000-0000-0000-000000000003",
                "overallScore": 50,
                "blockerMismatch": false,
            }))
            .await
            .err()
            .ok_or("submission must fail without a database")?;
        assert_eq!(
            jobniac_core::errors::error_envelope(&review).message,
            "Local database is not configured"
        );
        Ok(())
    }

    #[test]
    fn a_job_search_request_is_validated_before_the_database_is_consulted() -> TestResult {
        let services = compose(&Environment::default())?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let error = runtime
            .block_on(
                services
                    .jobs
                    .search
                    .search_jobs(&serde_json::json!({"query": "  "})),
            )
            .err()
            .ok_or("a blank query must fail")?;
        assert_eq!(
            jobniac_core::errors::error_envelope(&error).message,
            "Invalid job request"
        );
        Ok(())
    }

    #[test]
    fn a_valid_cursor_secret_composes_the_listing() -> TestResult {
        let env = Environment::from_pairs([("JOBNIAC_CURSOR_HMAC_SECRET", CURSOR_SECRET)]);
        let services = compose(&env)?;
        assert!(services.profiles.list.is_ok());
        Ok(())
    }

    #[test]
    fn a_short_cursor_secret_stops_startup_in_every_agent_mode() {
        let env = Environment::from_pairs([("JOBNIAC_CURSOR_HMAC_SECRET", "short")]);
        assert!(matches!(compose(&env), Err(StartupError)));
        let mut pairs = DATABASE_PAIRS.to_vec();
        pairs.push(("JOBNIAC_CURSOR_HMAC_SECRET", "short"));
        assert!(matches!(
            compose(&Environment::from_pairs(pairs)),
            Err(StartupError)
        ));
    }

    #[test]
    fn a_blank_cursor_secret_is_unset_and_composes() -> TestResult {
        let env = Environment::from_pairs([("JOBNIAC_CURSOR_HMAC_SECRET", "   ")]);
        let services = compose(&env)?;
        assert!(services.profiles.list.is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn an_enabled_but_unqualified_root_reports_unknown_cleanup() -> TestResult {
        let env = Environment::from_pairs([
            ("JOBNIAC_GENERATED_ENABLED", "1"),
            ("JOBNIAC_GENERATED_ROOT", "/srv/jobniac-synthetic/generated"),
            (
                "JOBNIAC_GENERATED_ROOT_ID",
                "6f1c2d3e-4a5b-4c6d-8e7f-900112233445",
            ),
        ]);
        let services = compose(&env)?;
        let error = services
            .generation
            .facade
            .generate_pdf_file(&serde_json::json!({"body": "Synthetic body"}))
            .await
            .err()
            .ok_or("generation must refuse without a qualified root")?;
        assert_eq!(error.message(), "Generation is not available");
        let report = services.health.check().await;
        assert_eq!(report.generated_resume_cleanup, CleanupReport::unknown());
        Ok(())
    }

    #[test]
    fn startup_error_message_is_fixed() {
        assert_eq!(
            StartupError.to_string(),
            "Jobniac could not start. Check local configuration."
        );
    }
}
