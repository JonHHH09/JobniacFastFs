//! Profile ingestion of a real Europass PDF: the text extractor and the inspector run in the PDF
//! worker of the built `jobniac-cli` binary over a disposable, migrated SQLite database, and the
//! stored profile must carry the data of the attached Europass CV.
//!
//! Needs PDFium (`JOBNIAC_PDFIUM_DIR`); without it the test prints a skip note and passes locally,
//! and on CI (`CI` set) a missing directory fails the test. The PDFs are committed synthetic
//! fixtures of `crates/infra/tests/fixtures/pdf/europass`.

#[path = "support/pdfium.rs"]
mod pdfium;

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jobniac_core::clock::SystemClock;
use jobniac_core::documents::DocumentService;
use jobniac_core::profiles::pdf_ingest::ProfilePdfService;
use jobniac_core::profiles::{ProfileService, profile_search_terms};
use jobniac_infra::sqlite::documents::SqliteDocumentRepository;
use jobniac_infra::sqlite::migrations::migrate;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::profiles::SqliteProfileRepository;
use jobniac_infra::sqlite::profiles::pdf::SqliteProfilePdfRepository;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use jobniac_infra::workers::WorkerSupervisor;
use jobniac_runtime::document_ports::{
    ImportRootReader, WorkerCancellation, WorkerPdfExtractor, WorkerPdfInspector,
};
use jobniac_testkit::scratch::scratch_dir;
use jobniac_testkit::sqlite::TempDatabase;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

const CV_PDF: &str = "europass/europass-cv-alex-sample.pdf";
const LETTER_PDF: &str = "europass/europass-cover-letter-alex-sample.pdf";

#[tokio::test]
async fn a_europass_cv_pdf_builds_the_profile_from_its_attached_data() -> TestResult {
    // PDFium first: on CI a missing library must fail before the database check can skip.
    let Some(pdfium_dir) = pdfium::pdfium_dir_or_skip() else {
        return Ok(());
    };
    let database = TempDatabase::new()?;
    migrate(database.path()).map_err(|error| error.0)?;
    let pool = SqlitePool::open(database.path())?;
    let scratch = scratch_dir("jobniac-europass-ingest-")?;
    let root = scratch.path().to_path_buf();
    for (source, name) in [(CV_PDF, "cv.pdf"), (LETTER_PDF, "letter.pdf")] {
        let bytes = fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../infra/tests/fixtures/pdf")
                .join(source),
        )?;
        fs::write(root.join(name), bytes)?;
    }

    let uow = SqliteUnitOfWork::new(pool);
    let supervisor = Arc::new(WorkerSupervisor::new(
        PathBuf::from(env!("CARGO_BIN_EXE_jobniac-cli")),
        std::env::vars_os().collect(),
    ));
    let cancellation = WorkerCancellation::new();
    let documents = Arc::new(
        DocumentService::new(
            uow.clone(),
            SqliteDocumentRepository,
            Arc::new(ImportRootReader::new(Some(
                root.to_string_lossy().into_owned(),
            ))),
            Arc::new(WorkerPdfExtractor::shared(
                Some(Arc::clone(&supervisor)),
                Some(pdfium_dir.clone()),
                cancellation.clone(),
            )),
            Arc::new(SystemClock),
        )
        .with_inspector(Arc::new(WorkerPdfInspector::new(
            Some(supervisor),
            Some(pdfium_dir),
            cancellation,
        ))),
    );
    let ingestion = ProfilePdfService::new(
        uow.clone(),
        SqliteProfileRepository,
        SqliteProfilePdfRepository,
        Arc::clone(&documents),
        Arc::new(SystemClock),
    );
    let profiles = ProfileService::new(uow, SqliteProfileRepository, Arc::new(SystemClock));

    let stored = documents
        .store_document_file(&json!({"path": root.join("cv.pdf").to_string_lossy()}))
        .await?;
    let outcome: Value = ingestion
        .ingest_profile_from_stored_pdf(&json!({"documentId": stored.id.to_string()}))
        .await?;
    assert_eq!(outcome["status"], "CREATED_PROFILE");
    assert_eq!(outcome["profileSource"], "EUROPASS_CV");

    let profile_id = outcome["profileId"].as_str().ok_or("profileId")?;
    let graph = profiles.get_profile(profile_id).await?;
    assert_eq!(graph.profile.full_name, "Alex Sample");
    assert_eq!(graph.profile.email, "alex.sample@example.com");
    assert_eq!(graph.experiences.len(), 2);
    assert_eq!(graph.education.len(), 1);
    assert_eq!(graph.skills.len(), 5);
    let spanish = graph
        .languages
        .iter()
        .find(|language| language.normalized_language == "spanish")
        .ok_or("no Spanish")?;
    assert_eq!(spanish.proficiency.as_deref(), Some("B1"));
    assert!(!profile_search_terms(&graph).is_empty());

    // A cover-letter PDF has no CV: the text reader runs, as before.
    let letter = documents
        .store_document_file(&json!({"path": root.join("letter.pdf").to_string_lossy()}))
        .await?;
    let outcome = ingestion
        .ingest_profile_from_stored_pdf(&json!({"documentId": letter.id.to_string()}))
        .await?;
    assert_eq!(outcome["status"], "DUPLICATE_PROFILE_CANDIDATE");
    assert_eq!(outcome["profileSource"], "PDF_TEXT");
    Ok(())
}
