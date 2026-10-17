//! The six document and profile PDF MCP tools, driven through their handlers over one disposable,
//! migrated PostgreSQL database and the PDF worker of the built `jobniac-cli` binary.
//!
//! The PDF tools need PDFium (`JOBNIAC_PDFIUM_DIR`) and the database needs a `pg-test` cluster
//! (`JOBNIAC_TEST_PG_PORT`). Without PDFium the test prints a skip note and passes locally; on CI
//! (`CI` set) a missing `JOBNIAC_PDFIUM_DIR` fails the test. A missing database alone skips. The PDF
//! is a committed synthetic fixture of `crates/infra/tests/fixtures/pdf`, copied into a scratch
//! import root that is removed afterwards. The composition root's adapters are compiled into this
//! test from `src/document_ports.rs`, because the console crate is a binary without a library target.

#[path = "../src/document_ports.rs"]
#[allow(dead_code)]
mod document_ports;

#[path = "support/pdfium.rs"]
mod pdfium;

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jobniac_core::clock::SystemClock;
use jobniac_core::documents::{DocumentService, sha256_hex};
use jobniac_core::profiles::pdf_ingest::ProfilePdfService;
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::documents::PgDocumentRepository;
use jobniac_infra::postgres::migrations::migrate;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::profiles::PgProfileRepository;
use jobniac_infra::postgres::profiles::pdf::PgProfilePdfRepository;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use jobniac_infra::workers::WorkerSupervisor;
use jobniac_mcp::catalog::Catalog;
use jobniac_mcp::registry::{Arguments, ToolRegistry};
use jobniac_mcp::result::ToolResult;
use jobniac_mcp::tools::{documents, profile_pdf};
use jobniac_runtime::document_ports::{ImportRootReader, WorkerPdfExtractor};
use jobniac_testkit::pg::TestServer;
use serde_json::{Value, json};

use document_ports::from_use_cases;

type TestResult = Result<(), Box<dyn Error>>;

const FIXTURE: &str = "corpus/resume-simple.pdf";
const FIXTURE_NAME: &str = "resume-simple.pdf";
const RESUME_NAME: &str = "resume-synthetic.pdf";
const UNKNOWN_ID: &str = "00000000-0000-4000-8000-000000000999";

/// Call one registered tool with its wire arguments and return the handler's result.
async fn call(
    registry: &ToolRegistry,
    name: &str,
    arguments: Value,
) -> Result<ToolResult, Box<dyn Error>> {
    let arguments: Arguments = arguments
        .as_object()
        .cloned()
        .ok_or("arguments must be an object")?;
    registry
        .call(name, &arguments)
        .await
        .ok_or_else(|| format!("tool {name} is not registered").into())
}

/// The success value of a call, or an error that names the failure.
fn success(outcome: ToolResult) -> Result<Value, Box<dyn Error>> {
    match outcome {
        ToolResult::Success(value) => Ok(value),
        other => Err(format!("expected a success, got {other:?}").into()),
    }
}

/// The text of a failed call, which must be an application failure with the given code.
fn failure_text(outcome: ToolResult, code: &str) -> Result<String, Box<dyn Error>> {
    let result = outcome.into_call_result();
    if result.is_error != Some(true) {
        return Err("expected a failed call".into());
    }
    let text = result
        .content
        .first()
        .and_then(|block| block.as_text())
        .map(|text| text.text.clone())
        .ok_or("the failure has no text")?;
    let envelope: Value = serde_json::from_str(&text)?;
    if envelope["code"] != code {
        return Err(format!("expected code {code}, got {text}").into());
    }
    Ok(text)
}

#[tokio::test]
async fn the_six_tools_run_through_their_handlers() -> TestResult {
    // PDFium first: on CI a missing library must fail before the database check can skip.
    let Some(pdfium_dir) = pdfium::pdfium_dir_or_skip() else {
        return Ok(());
    };
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };

    let database = server.fresh_database().await;
    let config = load_database_config(&database.environment()).ok_or("invalid test database")?;
    migrate(&config).await.map_err(|error| error.0)?;

    // A fresh directory that the guard removes when the test returns, whatever the outcome.
    let scratch = tempfile::Builder::new()
        .prefix("jobniac-document-mcp-")
        .tempdir()?;
    let root = scratch.path().to_path_buf();
    let fixture = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../infra/tests/fixtures/pdf")
            .join(FIXTURE),
    )?;
    fs::write(root.join(FIXTURE_NAME), &fixture)?;
    fs::write(
        root.join(RESUME_NAME),
        synthetic_resume(&[
            "Jane Doe",
            "Senior Engineer",
            "jane.doe@example.test",
            "Skills: Rust, SQL",
        ]),
    )?;

    exercise(config, &root, &fixture, pdfium_dir).await
}

/// The six tools, composed as the console composes them, over one database.
async fn exercise(
    config: jobniac_infra::postgres::config::DatabaseConfig,
    root: &Path,
    fixture: &[u8],
    pdfium_dir: PathBuf,
) -> TestResult {
    let file_path = root.join(FIXTURE_NAME).to_string_lossy().into_owned();
    let resume_path = root.join(RESUME_NAME).to_string_lossy().into_owned();
    let escape_path = root
        .join("..")
        .join("escape-synthetic.pdf")
        .to_string_lossy()
        .into_owned();
    let uow = PgUnitOfWork::new(PgPool::new(Some(config)));
    let reader = ImportRootReader::new(Some(root.to_string_lossy().into_owned()));
    let supervisor = WorkerSupervisor::new(
        PathBuf::from(env!("CARGO_BIN_EXE_jobniac-cli")),
        std::env::vars_os().collect(),
    );
    let extractor = WorkerPdfExtractor::new(Some(supervisor), Some(pdfium_dir));
    let documents_service = Arc::new(DocumentService::new(
        uow.clone(),
        PgDocumentRepository,
        Arc::new(reader),
        Arc::new(extractor),
        Arc::new(SystemClock),
    ));
    let profile_pdf_service = ProfilePdfService::new(
        uow,
        PgProfileRepository,
        PgProfilePdfRepository,
        Arc::clone(&documents_service),
        Arc::new(SystemClock),
    );
    let ports = from_use_cases(documents_service, Arc::new(profile_pdf_service));

    let mut registry = ToolRegistry::new(Catalog::load()?);
    documents::register(&mut registry, ports.documents)?;
    profile_pdf::register(&mut registry, ports.profile_pdf)?;

    // store_document_file: metadata only, with the digest of the bytes.
    let stored = success(
        call(
            &registry,
            "store_document_file",
            json!({"request": {"path": file_path, "mediaType": null}}),
        )
        .await?,
    )?;
    let document_id = stored["id"].as_str().ok_or("no document id")?.to_owned();
    assert_eq!(stored["originalFileName"], FIXTURE_NAME);
    assert_eq!(stored["mediaType"], "application/pdf");
    assert_eq!(stored["byteSize"], fixture.len());
    assert_eq!(stored["sha256"], sha256_hex(fixture));
    assert!(stored.get("bytes").is_none(), "bytes never cross the tool");

    // get_document_metadata: the same metadata, and not_found for an unknown identifier.
    let metadata = success(
        call(
            &registry,
            "get_document_metadata",
            json!({"documentId": document_id}),
        )
        .await?,
    )?;
    assert_eq!(metadata, stored);
    failure_text(
        call(
            &registry,
            "get_document_metadata",
            json!({"documentId": UNKNOWN_ID}),
        )
        .await?,
        "not_found",
    )?;

    // extract_pdf_text: a bounded preview of an import-root file, nothing stored.
    let preview = success(
        call(
            &registry,
            "extract_pdf_text",
            json!({"request": {"path": file_path, "maxCharacters": 500, "includePages": true}}),
        )
        .await?,
    )?;
    assert_eq!(preview["fileName"], FIXTURE_NAME);
    assert!(
        preview["pageCount"]
            .as_u64()
            .is_some_and(|count| count >= 1)
    );
    // The synthetic résumé is read from the import root without storing it: its text carries the
    // name and the address, and the page list starts at page one.
    let resume_preview = success(
        call(
            &registry,
            "extract_pdf_text",
            json!({"request": {"path": resume_path, "maxCharacters": 500, "includePages": true}}),
        )
        .await?,
    )?;
    let resume_text = resume_preview["text"].as_str().ok_or("no preview text")?;
    assert!(resume_text.contains("Jane Doe"), "{resume_text}");
    assert!(
        resume_text.contains("jane.doe@example.test"),
        "{resume_text}"
    );
    assert_eq!(resume_preview["pages"][0]["pageNumber"], 1);
    assert!(
        resume_preview["pages"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("Jane Doe"))
    );
    // A five-character budget cuts the text, keeps at most five characters and omits the pages.
    let cut = success(
        call(
            &registry,
            "extract_pdf_text",
            json!({"request": {"path": resume_path, "maxCharacters": 5, "includePages": false}}),
        )
        .await?,
    )?;
    assert_eq!(cut["truncated"], true);
    assert!(
        cut["text"]
            .as_str()
            .is_some_and(|text| !text.is_empty() && text.chars().count() <= 5)
    );
    assert_eq!(cut["pages"], json!([]));
    let escaped = failure_text(
        call(
            &registry,
            "extract_pdf_text",
            json!({"request": {"path": escape_path, "maxCharacters": null, "includePages": null}}),
        )
        .await?,
        "validation_error",
    )?;
    assert!(
        !escaped.contains("escape-synthetic"),
        "a path is never echoed: {escaped}"
    );

    // extract_stored_pdf_text: persisted once, then served from the cache under the same identifier.
    let first = success(
        call(
            &registry,
            "extract_stored_pdf_text",
            json!({"request": {"documentId": document_id, "maxCharacters": null, "includePages": null, "persistExtraction": true}}),
        )
        .await?,
    )?;
    let extraction_id = first["extractionId"]
        .as_str()
        .ok_or("no extraction id")?
        .to_owned();
    assert_eq!(first["document"]["id"], document_id);
    assert_eq!(first["extraction"]["fileName"], FIXTURE_NAME);
    assert!(
        first["extraction"]["text"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "the stored extraction keeps its text: {first}"
    );
    let second = success(
        call(
            &registry,
            "extract_stored_pdf_text",
            json!({"request": {"documentId": document_id, "maxCharacters": null, "includePages": null, "persistExtraction": true}}),
        )
        .await?,
    )?;
    assert_eq!(second["extractionId"], extraction_id);
    assert_eq!(second["extraction"], first["extraction"]);

    // ingest_profile_from_stored_pdf and get_profile_pdf_source: one profile linked to the PDF.
    // The ingestion needs an email address in the text, which the fixture above does not have, so a
    // second synthetic résumé is stored and ingested.
    let resume = success(
        call(
            &registry,
            "store_document_file",
            json!({"request": {"path": resume_path, "mediaType": null}}),
        )
        .await?,
    )?;
    let resume_id = resume["id"]
        .as_str()
        .ok_or("no resume document id")?
        .to_owned();
    let ingested = success(
        call(
            &registry,
            "ingest_profile_from_stored_pdf",
            json!({"request": {"documentId": resume_id}}),
        )
        .await?,
    )?;
    assert_eq!(ingested["documentId"], resume_id);
    assert!(ingested["pdfExtractionId"].is_string());
    let profile_id = ingested["profileId"]
        .as_str()
        .ok_or("no profile id")?
        .to_owned();
    let source = success(
        call(
            &registry,
            "get_profile_pdf_source",
            json!({"profileId": profile_id}),
        )
        .await?,
    )?;
    assert_eq!(source["profileId"], profile_id);
    assert_eq!(source["pdfExtractionId"], ingested["pdfExtractionId"]);

    // Argument rejections never reach the services.
    for (tool, arguments) in [
        ("store_document_file", json!({})),
        (
            "extract_pdf_text",
            json!({"request": {"path": 5, "maxCharacters": null, "includePages": null}}),
        ),
        ("get_document_metadata", json!({"documentId": 5})),
        ("ingest_profile_from_stored_pdf", json!({"request": {}})),
        ("get_profile_pdf_source", json!({"profileId": true})),
    ] {
        let outcome = call(&registry, tool, arguments).await?;
        assert!(
            matches!(outcome, ToolResult::InvalidArguments(_)),
            "{tool}: {outcome:?}"
        );
    }
    Ok(())
}

/// A one-page synthetic résumé: each line is shown as text, so the extraction finds the email.
fn synthetic_resume(lines: &[&str]) -> Vec<u8> {
    let mut content = String::from("BT /F1 12 Tf 14 TL 72 720 Td\n");
    for line in lines {
        content.push_str(&format!("({line}) Tj T*\n"));
    }
    content.push_str("ET\n");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];
    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
    }
    let xref = pdf.len();
    pdf.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    pdf.into_bytes()
}
