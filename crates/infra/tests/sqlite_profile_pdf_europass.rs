//! Profile ingestion from stored PDFs that carry a Europass attachment, against a disposable SQLite
//! database.
//!
//! The use cases are the real ones (document service, profile service, ingestion over the SQLite
//! repositories). The resume text comes from an in-memory extractor and the attachments from an
//! in-memory inspector, so no PDF worker runs. The text names `Ada Example` and the attachment
//! `Alex Sample`, so the stored profile shows which reader built it. All data is synthetic.

mod sqlite_profile_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use jobniac_core::clock::SystemClock;
use jobniac_core::documents::{
    DocumentFileReader, DocumentService, FileSnapshot, PageText, PdfAttachmentInfo, PdfExtractor,
    PdfInspection, PdfInspector, PdfText,
};
use jobniac_core::errors::{AppError, ApplicationError, ErrorCode, error_envelope};
use jobniac_core::profiles::ProfileService;
use jobniac_core::profiles::pdf_ingest::ProfilePdfService;
use jobniac_infra::sqlite::documents::SqliteDocumentRepository;
use jobniac_infra::sqlite::profiles::SqliteProfileRepository;
use jobniac_infra::sqlite::profiles::pdf::SqliteProfilePdfRepository;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use serde_json::{Value, json};
use sqlite_profile_support::{Fixture, TestResult, start};

const CV: &[u8] = include_bytes!("../../core/tests/fixtures/europass/cv-alex-sample.xml");
const LETTER: &[u8] =
    include_bytes!("../../core/tests/fixtures/europass/cover-letter-alex-sample.xml");

/// The attachments of a synthetic PDF: a name and, when readable, its bytes.
type Files = Vec<(&'static str, Option<Vec<u8>>)>;

type Documents = Arc<DocumentService<SqliteUnitOfWork, SqliteDocumentRepository>>;
type Ingest = ProfilePdfService<
    SqliteUnitOfWork,
    SqliteProfileRepository,
    SqliteProfilePdfRepository,
    Documents,
>;

const RESUME: &str = "Ada Example\n\
ada.example@example.com\n\
Summary\n\
Backend engineer writing synthetic tests.\n\
Skills\n\
Kotlin, SQLite, Docker\n";

#[derive(Debug)]
struct FixedReader(Vec<u8>);

#[async_trait]
impl DocumentFileReader for FixedReader {
    async fn read(&self, _path: &str) -> Result<FileSnapshot, AppError> {
        Ok(FileSnapshot {
            file_name: "synthetic-cv.pdf".to_owned(),
            content: self.0.clone(),
        })
    }
}

#[derive(Debug)]
struct ResumeExtractor;

#[async_trait]
impl PdfExtractor for ResumeExtractor {
    fn version(&self) -> &str {
        "resume-test-v1"
    }

    async fn extract(&self, _content: &[u8], _file_name: &str) -> Result<PdfText, AppError> {
        Ok(PdfText {
            file_name: "synthetic-cv.pdf".to_owned(),
            page_count: 1,
            character_count: i64::try_from(RESUME.encode_utf16().count()).unwrap_or(i64::MAX),
            truncated: false,
            pages: vec![PageText {
                page_number: 1,
                text: RESUME.to_owned(),
            }],
            text: RESUME.to_owned(),
        })
    }
}

/// An inspector that reports fixed attachments, or fails.
#[derive(Debug)]
struct Attachments {
    files: Option<Files>,
    calls: AtomicUsize,
}

impl Attachments {
    fn of(files: Files) -> Arc<Self> {
        Arc::new(Self {
            files: Some(files),
            calls: AtomicUsize::new(0),
        })
    }

    fn failing() -> Arc<Self> {
        Arc::new(Self {
            files: None,
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl PdfInspector for Attachments {
    async fn inspect(&self, _content: &[u8]) -> Result<PdfInspection, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let files = self.files.as_ref().ok_or_else(|| {
            AppError::from(ApplicationError::with_message(
                ErrorCode::InternalError,
                "PDF inspection failed",
            ))
        })?;
        Ok(PdfInspection {
            page_count: 1,
            pages: Vec::new(),
            title: None,
            producer: None,
            creator: None,
            attachments: files
                .iter()
                .map(|(name, content)| PdfAttachmentInfo {
                    name: (*name).to_owned(),
                    byte_size: content.as_ref().map_or(0, |bytes| bytes.len() as u64),
                    content: content.clone(),
                })
                .collect(),
            attachments_truncated: false,
        })
    }
}

fn pdf_bytes(marker: &str) -> Vec<u8> {
    format!("%PDF-1.4\n% synthetic {marker}\n").into_bytes()
}

fn documents(fixture: &Fixture, marker: &str, inspector: &Arc<Attachments>) -> Documents {
    Arc::new(
        DocumentService::new(
            fixture.uow.clone(),
            SqliteDocumentRepository,
            Arc::new(FixedReader(pdf_bytes(marker))),
            Arc::new(ResumeExtractor),
            Arc::new(SystemClock),
        )
        .with_inspector(Arc::clone(inspector) as Arc<dyn PdfInspector>),
    )
}

fn profiles(fixture: &Fixture) -> ProfileService<SqliteUnitOfWork, SqliteProfileRepository> {
    ProfileService::new(
        fixture.uow.clone(),
        SqliteProfileRepository,
        Arc::new(SystemClock),
    )
}

/// The ingestion service and the stored document id of one synthetic PDF.
async fn setup(
    fixture: &Fixture,
    marker: &str,
    inspector: &Arc<Attachments>,
) -> Result<(Ingest, String), Box<dyn std::error::Error + Send + Sync>> {
    let documents = documents(fixture, marker, inspector);
    let stored = documents
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    let service = ProfilePdfService::new(
        fixture.uow.clone(),
        SqliteProfileRepository,
        SqliteProfilePdfRepository,
        documents,
        Arc::new(SystemClock),
    );
    Ok((service, stored.id.to_string()))
}

async fn created_name(
    fixture: &Fixture,
    outcome: &Value,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let profile_id = outcome["profileId"].as_str().ok_or("profileId")?;
    Ok(profiles(fixture)
        .get_profile(profile_id)
        .await?
        .profile
        .full_name)
}

/// Ingests with `files` as the attachments and returns the outcome and the stored name.
async fn ingest_with(
    marker: &str,
    files: Files,
) -> Result<(Value, String), Box<dyn std::error::Error + Send + Sync>> {
    let fixture = start();
    let inspector = Attachments::of(files);
    let (service, document) = setup(&fixture, marker, &inspector).await?;
    let outcome = service
        .ingest_profile_from_stored_pdf(&json!({"documentId": document}))
        .await?;
    let name = created_name(&fixture, &outcome).await?;
    Ok((outcome, name))
}

#[tokio::test]
async fn a_europass_cv_attachment_builds_the_profile_and_a_repeat_reuses_it() -> TestResult {
    let fixture = start();
    let inspector = Attachments::of(vec![
        ("notes.txt", Some(b"ignored".to_vec())),
        ("Europass-CV.XML", Some(CV.to_vec())),
    ]);
    let (service, document) = setup(&fixture, "cv", &inspector).await?;

    let created = service
        .ingest_profile_from_stored_pdf(&json!({"documentId": document}))
        .await?;
    assert_eq!(created["status"], "CREATED_PROFILE");
    assert_eq!(created["profileSource"], "EUROPASS_CV");
    let profile_id = created["profileId"].as_str().ok_or("profileId")?;
    let graph = profiles(&fixture).get_profile(profile_id).await?;
    assert_eq!(graph.profile.full_name, "Alex Sample");
    assert_eq!(graph.profile.email, "alex.sample@example.com");
    assert_eq!(graph.experiences.len(), 2);
    assert_eq!(graph.education.len(), 1);
    assert_eq!(graph.skills.len(), 5);
    let spanish = graph
        .languages
        .iter()
        .find(|language| language.normalized_language == "spanish")
        .ok_or("spanish")?;
    assert_eq!(spanish.proficiency.as_deref(), Some("B1"));
    let current = graph
        .experiences
        .iter()
        .find(|experience| experience.display_order == 0)
        .ok_or("first experience")?;
    assert!(current.end_date.is_none());

    let repeated = service
        .ingest_profile_from_stored_pdf(&json!({"documentId": document}))
        .await?;
    assert_eq!(repeated["status"], "REUSED_EXISTING_SOURCE");
    assert_eq!(repeated["profileSource"], Value::Null);
    assert_eq!(inspector.calls(), 1, "a reused source is not inspected");
    Ok(())
}

#[tokio::test]
async fn pdfs_without_a_usable_europass_cv_use_the_text_reader() -> TestResult {
    let cv_without_email = String::from_utf8(CV.to_vec())?.replace(
        "<oa:URI>alex.sample@example.com</oa:URI>",
        "<oa:URI></oa:URI>",
    );

    let cases: Vec<(&str, Files)> = vec![
        ("no-attachment", vec![]),
        ("pdf-attachment", vec![("cv.pdf", Some(CV.to_vec()))]),
        ("no-content", vec![("cv.xml", None)]),
        ("cover-letter", vec![("letter.xml", Some(LETTER.to_vec()))]),
        (
            "malformed",
            vec![("cv.xml", Some(b"<Candidate><oops".to_vec()))],
        ),
        ("not-europass", vec![("cv.xml", Some(b"<a/>".to_vec()))]),
        (
            "no-email",
            vec![("cv.xml", Some(cv_without_email.into_bytes()))],
        ),
    ];
    for (marker, files) in cases {
        let (outcome, name) = ingest_with(marker, files).await?;
        assert_eq!(outcome["status"], "CREATED_PROFILE", "{marker}");
        assert_eq!(outcome["profileSource"], "PDF_TEXT", "{marker}");
        assert_eq!(name, "Ada Example", "{marker}");
    }
    Ok(())
}

#[tokio::test]
async fn the_first_xml_attachment_that_is_a_europass_cv_is_used() -> TestResult {
    let (outcome, name) = ingest_with(
        "second-xml",
        vec![
            ("other.xml", Some(b"<a/>".to_vec())),
            ("letter.xml", Some(LETTER.to_vec())),
            ("cv.xml", Some(CV.to_vec())),
        ],
    )
    .await?;
    assert_eq!(outcome["profileSource"], "EUROPASS_CV");
    assert_eq!(name, "Alex Sample");
    Ok(())
}

#[tokio::test]
async fn a_cv_with_duplicate_link_variants_still_comes_from_europass() -> TestResult {
    let website = "<oa:URI>https://example.com/alex-sample</oa:URI>";
    let duplicated = String::from_utf8(CV.to_vec())?.replace(
        website,
        &format!(
            "{website}</Communication><Communication><ChannelCode>Web</ChannelCode>\
             <oa:URI>http://example.com/alex-sample/</oa:URI>"
        ),
    );
    let (outcome, name) = ingest_with(
        "duplicate-links",
        vec![("cv.xml", Some(duplicated.into_bytes()))],
    )
    .await?;
    assert_eq!(outcome["profileSource"], "EUROPASS_CV");
    assert_eq!(name, "Alex Sample");
    Ok(())
}

#[tokio::test]
async fn a_failed_inspection_falls_back_to_the_text_reader() -> TestResult {
    let fixture = start();
    let inspector = Attachments::failing();
    let (service, document) = setup(&fixture, "failing", &inspector).await?;
    let outcome = service
        .ingest_profile_from_stored_pdf(&json!({"documentId": document}))
        .await?;
    assert_eq!(outcome["profileSource"], "PDF_TEXT");
    assert_eq!(created_name(&fixture, &outcome).await?, "Ada Example");
    assert_eq!(inspector.calls(), 1);
    Ok(())
}

#[tokio::test]
async fn an_overwrite_with_a_europass_cv_replaces_the_profile_at_its_revision() -> TestResult {
    let fixture = start();
    let seeded = profiles(&fixture)
        .create_profile(&json!({
            "fullName": "Old Example",
            "email": "old.example@example.com",
            "summary": null,
            "contacts": [],
            "links": [],
            "skills": [],
            "languages": [],
            "education": [],
            "experiences": [],
            "projects": []
        }))
        .await?;
    let profile_id = seeded.profile.id.to_string();
    let inspector = Attachments::of(vec![("cv.xml", Some(CV.to_vec()))]);
    let (service, document) = setup(&fixture, "overwrite", &inspector).await?;

    let conflict = service
        .ingest_profile_from_stored_pdf(&json!({
            "documentId": document,
            "existingProfileId": profile_id,
            "overwriteExistingProfile": true,
            "expectedRevision": 7,
        }))
        .await
        .err()
        .ok_or("the call must fail")?;
    assert_eq!(
        error_envelope(&conflict).message,
        "Profile revision conflict"
    );
    let untouched = profiles(&fixture).get_profile(&profile_id).await?;
    assert_eq!(untouched.profile.full_name, "Old Example");
    assert_eq!(untouched.profile.revision, 0);

    let outcome = service
        .ingest_profile_from_stored_pdf(&json!({
            "documentId": document,
            "existingProfileId": profile_id,
            "overwriteExistingProfile": true,
            "expectedRevision": 0,
        }))
        .await?;
    assert_eq!(outcome["status"], "UPDATED_PROFILE");
    assert_eq!(outcome["profileSource"], "EUROPASS_CV");
    let graph = profiles(&fixture).get_profile(&profile_id).await?;
    assert_eq!(graph.profile.full_name, "Alex Sample");
    assert_eq!(graph.profile.revision, 1);
    assert_eq!(graph.experiences.len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_duplicate_candidate_from_a_europass_cv_names_its_source() -> TestResult {
    let fixture = start();
    profiles(&fixture)
        .create_profile(&json!({
            "fullName": "Alex Existing",
            "email": "alex.sample@example.com",
            "summary": null,
            "contacts": [],
            "links": [],
            "skills": [],
            "languages": [],
            "education": [],
            "experiences": [],
            "projects": []
        }))
        .await?;
    let inspector = Attachments::of(vec![("cv.xml", Some(CV.to_vec()))]);
    let (service, document) = setup(&fixture, "duplicate", &inspector).await?;
    let outcome = service
        .ingest_profile_from_stored_pdf(&json!({"documentId": document}))
        .await?;
    assert_eq!(outcome["status"], "DUPLICATE_PROFILE_CANDIDATE");
    assert_eq!(outcome["matchedOn"], json!(["email"]));
    assert_eq!(outcome["profileSource"], "EUROPASS_CV");
    Ok(())
}
