//! Profile ingestion from stored PDFs against a disposable PostgreSQL 18 server.
//!
//! The tests drive the real use cases: the document service over the PostgreSQL document
//! repository, the profile service for the seeded profiles, and the ingestion service over the
//! PDF repository on the same transactions. The resume text comes from an in-memory extractor, so
//! no PDF worker runs. The server-backed tests skip with a notice when no server is configured.
//! All data is synthetic: the reserved `.example` and `.test` domains and fixed identifiers.

mod profile_support;

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use jobniac_core::clock::SystemClock;
use jobniac_core::documents::{
    DocumentFileReader, DocumentService, FileSnapshot, PageText, PdfExtractor, PdfText,
};
use jobniac_core::errors::{AppError, ApplicationError, error_envelope};
use jobniac_core::profiles::pdf_ingest::{
    LinkedSource, NewSourceLink, ProfilePdfRepository, ProfilePdfService, ProfilePdfSource,
};
use jobniac_core::profiles::{ProfileGraph, ProfileService, profile_search_terms};
use jobniac_infra::postgres::documents::PgDocumentRepository;
use jobniac_infra::postgres::profiles::PgProfileRepository;
use jobniac_infra::postgres::profiles::pdf::PgProfilePdfRepository;
use jobniac_infra::postgres::unit_of_work::{PgTransaction, PgUnitOfWork};
use profile_support::{Fixture, TestResult, postings, start};
use serde_json::{Value, json};
use uuid::Uuid;

type Documents = Arc<DocumentService<PgUnitOfWork, PgDocumentRepository>>;
type Ingest =
    ProfilePdfService<PgUnitOfWork, PgProfileRepository, PgProfilePdfRepository, Documents>;
type SeamIngest = ProfilePdfService<PgUnitOfWork, PgProfileRepository, LinkSeam, Documents>;

/// The synthetic resume text every extractor returns unless a test changes it.
const RESUME: &str = "Ada Example\n\
ada.example@example.com\n\
+1 (555) 010-0199\n\
https://github.com/ada-example\n\
Summary\n\
Backend engineer writing synthetic tests.\n\
Skills\n\
Kotlin, PostgreSQL, Docker\n\
Experience\n\
Senior Engineer | Example Corp | Berlin\n\
Jan 2020 - Present\n\
- Built synthetic services\n";

/// A reader that returns one fixed synthetic file, so each test controls the bytes and the name.
#[derive(Debug)]
struct FixedReader {
    content: Vec<u8>,
    file_name: String,
}

#[async_trait]
impl DocumentFileReader for FixedReader {
    async fn read(&self, _path: &str) -> Result<FileSnapshot, AppError> {
        Ok(FileSnapshot {
            file_name: self.file_name.clone(),
            content: self.content.clone(),
        })
    }
}

/// An extractor that counts its calls and returns a fixed synthetic resume text.
#[derive(Debug)]
struct ResumeExtractor {
    text: String,
    calls: AtomicUsize,
}

impl ResumeExtractor {
    fn new(text: &str) -> Arc<Self> {
        Arc::new(Self {
            text: text.to_owned(),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl PdfExtractor for ResumeExtractor {
    fn version(&self) -> &str {
        "resume-test-v1"
    }

    async fn extract(&self, _content: &[u8], _file_name: &str) -> Result<PdfText, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = self.text.clone();
        Ok(PdfText {
            file_name: "synthetic-cv.pdf".to_owned(),
            page_count: 1,
            character_count: i64::try_from(text.encode_utf16().count()).unwrap_or(i64::MAX),
            truncated: false,
            pages: vec![PageText {
                page_number: 1,
                text: text.clone(),
            }],
            text,
        })
    }
}

/// The synthetic PDF bytes of one import; `marker` keeps the SHA-256 of each document distinct.
fn pdf_bytes(marker: &str) -> Vec<u8> {
    format!("%PDF-1.4\n% synthetic {marker}\n").into_bytes()
}

/// The document use cases over the fixture, for one set of bytes and one resume text.
fn documents(fixture: &Fixture, content: Vec<u8>, extractor: &Arc<ResumeExtractor>) -> Documents {
    documents_named(fixture, content, "synthetic-cv.pdf", extractor)
}

/// The document use cases over the fixture, for bytes that the reader reports under `file_name`.
fn documents_named(
    fixture: &Fixture,
    content: Vec<u8>,
    file_name: &str,
    extractor: &Arc<ResumeExtractor>,
) -> Documents {
    Arc::new(DocumentService::new(
        fixture.uow.clone(),
        PgDocumentRepository,
        Arc::new(FixedReader {
            content,
            file_name: file_name.to_owned(),
        }),
        Arc::clone(extractor) as Arc<dyn PdfExtractor>,
        Arc::new(SystemClock),
    ))
}

/// The profile use cases over the fixture.
fn profiles(fixture: &Fixture) -> ProfileService<PgUnitOfWork, PgProfileRepository> {
    ProfileService::new(
        fixture.uow.clone(),
        PgProfileRepository,
        Arc::new(SystemClock),
    )
}

/// The ingestion use case over the fixture and a document service.
fn ingest(fixture: &Fixture, documents: Documents) -> Ingest {
    ProfilePdfService::new(
        fixture.uow.clone(),
        PgProfileRepository,
        PgProfilePdfRepository,
        documents,
        Arc::new(SystemClock),
    )
}

/// Stores one document from `content` and returns its identifier.
async fn store(
    fixture: &Fixture,
    content: Vec<u8>,
    extractor: &Arc<ResumeExtractor>,
) -> Result<String, Box<dyn Error>> {
    let metadata = documents(fixture, content, extractor)
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    Ok(metadata.id.to_string())
}

/// Stores one document whose reader reports `file_name`, and returns its identifier.
async fn store_named(
    fixture: &Fixture,
    content: Vec<u8>,
    file_name: &str,
    extractor: &Arc<ResumeExtractor>,
) -> Result<String, Box<dyn Error>> {
    let metadata = documents_named(fixture, content, file_name, extractor)
        .store_document_file(&json!({"path": "synthetic-cv.pdf"}))
        .await?;
    Ok(metadata.id.to_string())
}

/// A count of rows through the superuser connection.
async fn count(fixture: &Fixture, sql: &str) -> Result<i64, Box<dyn Error>> {
    let client = fixture.client().await;
    let row = client.query_one(sql, &[]).await?;
    Ok(row.try_get::<_, i64>(0)?)
}

/// The number of PDF extraction rows of one stored document.
async fn extraction_rows(fixture: &Fixture, document_id: Uuid) -> Result<i64, Box<dyn Error>> {
    let client = fixture.client().await;
    let row = client
        .query_one(
            "SELECT count(*)::bigint FROM document.pdf_extractions WHERE file_id = $1",
            &[&document_id],
        )
        .await?;
    Ok(row.try_get::<_, i64>(0)?)
}

/// Asserts that the stored postings of a profile are exactly the postings of its graph (the
/// shared `profile_search_terms` rule), compared as sorted multisets.
async fn assert_postings_match(fixture: &Fixture, graph: &ProfileGraph) -> TestResult {
    let client = fixture.client().await;
    let mut stored = postings(&client, graph.profile.id).await?;
    let mut expected = Vec::new();
    for term in profile_search_terms(graph) {
        expected.push((term.field_key, term.term, i16::try_from(term.weight)?));
    }
    stored.sort();
    expected.sort();
    assert!(!expected.is_empty(), "the profile has searchable text");
    assert_eq!(stored, expected);
    Ok(())
}

/// A profile PDF repository that wraps the PostgreSQL one and fails its link insert on purpose.
///
/// `Fail` returns an error from the link insert, after the graph write, so the whole transaction
/// must roll back. `Lose` makes a competing source for the same extraction appear on a separate
/// superuser connection (committed, so the in-transaction re-read sees it) and reports the lost
/// `ON CONFLICT DO NOTHING` race as `false`.
struct LinkSeam {
    client: Arc<tokio_postgres::Client>,
    outcome: SeamOutcome,
}

/// The behaviour of a [`LinkSeam`] link insert.
#[derive(Clone, Copy)]
enum SeamOutcome {
    /// The link insert fails with a database error.
    Fail,
    /// A competing source for `winner` is committed first, and the insert reports a lost race.
    Lose { winner: Uuid },
}

#[async_trait]
impl ProfilePdfRepository for LinkSeam {
    type Tx = PgTransaction;

    async fn lock(&self, tx: &mut PgTransaction, hashed_key: &str) -> Result<(), AppError> {
        PgProfilePdfRepository.lock(tx, hashed_key).await
    }

    async fn lock_profile_root(
        &self,
        tx: &mut PgTransaction,
        profile_id: Uuid,
    ) -> Result<(), AppError> {
        PgProfilePdfRepository
            .lock_profile_root(tx, profile_id)
            .await
    }

    async fn linked_by_extraction(
        &self,
        tx: &mut PgTransaction,
        extraction_id: Uuid,
    ) -> Result<Option<LinkedSource>, AppError> {
        PgProfilePdfRepository
            .linked_by_extraction(tx, extraction_id)
            .await
    }

    async fn linked_by_document_hash(
        &self,
        tx: &mut PgTransaction,
        sha256: &str,
    ) -> Result<Option<LinkedSource>, AppError> {
        PgProfilePdfRepository
            .linked_by_document_hash(tx, sha256)
            .await
    }

    async fn profile_has_source(
        &self,
        tx: &mut PgTransaction,
        profile_id: Uuid,
    ) -> Result<bool, AppError> {
        PgProfilePdfRepository
            .profile_has_source(tx, profile_id)
            .await
    }

    async fn email_candidates(
        &self,
        tx: &mut PgTransaction,
        email: &str,
    ) -> Result<Vec<Uuid>, AppError> {
        PgProfilePdfRepository.email_candidates(tx, email).await
    }

    async fn link_candidates(
        &self,
        tx: &mut PgTransaction,
        link_type: &str,
        url: &str,
    ) -> Result<Vec<Uuid>, AppError> {
        PgProfilePdfRepository
            .link_candidates(tx, link_type, url)
            .await
    }

    async fn profile_has_link(
        &self,
        tx: &mut PgTransaction,
        profile_id: Uuid,
        link_type: &str,
        url: &str,
    ) -> Result<bool, AppError> {
        PgProfilePdfRepository
            .profile_has_link(tx, profile_id, link_type, url)
            .await
    }

    async fn extraction_matches(
        &self,
        tx: &mut PgTransaction,
        extraction_id: Uuid,
        document_id: Uuid,
        sha256: &str,
    ) -> Result<bool, AppError> {
        PgProfilePdfRepository
            .extraction_matches(tx, extraction_id, document_id, sha256)
            .await
    }

    async fn savepoint(&self, tx: &mut PgTransaction) -> Result<(), AppError> {
        PgProfilePdfRepository.savepoint(tx).await
    }

    async fn rollback_to_savepoint(&self, tx: &mut PgTransaction) -> Result<(), AppError> {
        PgProfilePdfRepository.rollback_to_savepoint(tx).await
    }

    async fn insert_source_link(
        &self,
        _tx: &mut PgTransaction,
        link: &NewSourceLink,
    ) -> Result<bool, AppError> {
        match self.outcome {
            SeamOutcome::Fail => Err(AppError::from(ApplicationError::database_unavailable())),
            SeamOutcome::Lose { winner } => {
                self.client
                    .execute(
                        "INSERT INTO profile.profile_pdf_sources \
                             (id, profile_id, pdf_extraction_id, source_type, created_at) \
                         VALUES ($1, $2, $3, 'resume_pdf', now())",
                        &[&Uuid::new_v4(), &winner, &link.extraction_id],
                    )
                    .await
                    .map_err(|_| AppError::from(ApplicationError::database_unavailable()))?;
                Ok(false)
            }
        }
    }

    async fn source_of_profile(
        &self,
        tx: &mut PgTransaction,
        profile_id: Uuid,
    ) -> Result<Option<ProfilePdfSource>, AppError> {
        PgProfilePdfRepository
            .source_of_profile(tx, profile_id)
            .await
    }
}

/// The ingestion use case with the [`LinkSeam`] in place of the PostgreSQL PDF repository.
fn ingest_with_seam(fixture: &Fixture, documents: Documents, seam: LinkSeam) -> SeamIngest {
    ProfilePdfService::new(
        fixture.uow.clone(),
        PgProfileRepository,
        seam,
        documents,
        Arc::new(SystemClock),
    )
}

/// A complete profile write request with a GitHub link and no children.
fn profile_request(name: &str, email: &str) -> Value {
    json!({
        "fullName": name,
        "email": email,
        "summary": null,
        "contacts": [],
        "links": [{"linkType": "github", "url": "https://github.com/ada-example", "label": "GitHub"}],
        "skills": [],
        "languages": [],
        "education": [],
        "experiences": [],
        "projects": []
    })
}

/// A profile write request without links, so that only the email can match.
fn profile_request_without_links(name: &str, email: &str) -> Value {
    let mut request = profile_request(name, email);
    request["links"] = json!([]);
    request
}

/// The error message of a failed call, for assertions.
fn message_of(error: &AppError) -> String {
    error_envelope(error).message
}

/// The ingest request of one stored document.
fn request(document_id: &str) -> Value {
    json!({"documentId": document_id})
}

#[tokio::test]
async fn first_ingest_creates_the_profile_and_its_source_and_a_repeat_reuses_them() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("first"), &extractor).await?;
    let service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("first"), &extractor),
    );

    let created = service
        .ingest_profile_from_stored_pdf(&request(&document))
        .await?;
    assert_eq!(created["status"], "CREATED_PROFILE");
    assert_eq!(created["createdProfile"], true);
    assert_eq!(created["existingProfileLink"], false);
    assert_eq!(created["documentId"], document);
    assert_eq!(created["originalFileName"], "synthetic-cv.pdf");
    let profile_id = created["profileId"].as_str().ok_or("profileId")?.to_owned();
    let graph = profiles(&fixture).get_profile(&profile_id).await?;
    assert_eq!(graph.profile.full_name, "Ada Example");
    assert_eq!(graph.profile.email, "ada.example@example.com");
    assert_eq!(graph.profile.revision, 0);
    assert_postings_match(&fixture, &graph).await?;

    let repeated = service
        .ingest_profile_from_stored_pdf(&request(&document))
        .await?;
    assert_eq!(repeated["status"], "REUSED_EXISTING_SOURCE");
    assert_eq!(repeated["profileId"], profile_id);
    assert_eq!(repeated["sourceLinkId"], created["sourceLinkId"]);
    assert_eq!(repeated["createdProfile"], false);
    assert_eq!(repeated["existingProfileLink"], true);
    assert_eq!(
        extractor.calls(),
        1,
        "the cached extraction serves the repeat"
    );
    assert_eq!(
        count(&fixture, "SELECT count(*)::bigint FROM profile.profiles").await?,
        1
    );
    assert_eq!(
        count(
            &fixture,
            "SELECT count(*)::bigint FROM profile.profile_pdf_sources"
        )
        .await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn the_source_of_a_profile_is_read_back_and_a_missing_one_is_not_found() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("source"), &extractor).await?;
    let service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("source"), &extractor),
    );
    let created = service
        .ingest_profile_from_stored_pdf(&request(&document))
        .await?;
    let profile_id = created["profileId"].as_str().ok_or("profileId")?.to_owned();

    let source = service.get_profile_pdf_source(&profile_id).await?;
    assert_eq!(source["profileId"], profile_id);
    assert_eq!(source["pdfExtractionId"], created["pdfExtractionId"]);
    assert_eq!(source["sourceType"], "resume_pdf");
    assert_eq!(source["id"], created["sourceLinkId"]);

    let missing = Uuid::from_u128(77).to_string();
    let error = service.get_profile_pdf_source(&missing).await.err();
    assert_eq!(
        error.as_ref().map(message_of).as_deref(),
        Some("Profile PDF source was not found")
    );
    let malformed = service.get_profile_pdf_source("not-a-uuid").await.err();
    assert_eq!(
        malformed.as_ref().map(message_of).as_deref(),
        Some("Invalid profile ingestion request")
    );
    Ok(())
}

#[tokio::test]
async fn the_same_bytes_in_a_second_document_reuse_the_first_source() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let extractor = ResumeExtractor::new(RESUME);
    let first = store(&fixture, pdf_bytes("twin"), &extractor).await?;
    let second = store(&fixture, pdf_bytes("twin"), &extractor).await?;
    let service = ingest(&fixture, documents(&fixture, pdf_bytes("twin"), &extractor));
    let created = service
        .ingest_profile_from_stored_pdf(&request(&first))
        .await?;

    let reused = service
        .ingest_profile_from_stored_pdf(&request(&second))
        .await?;
    assert_eq!(reused["status"], "REUSED_EXISTING_SOURCE");
    assert_eq!(
        reused["documentId"], first,
        "the original document is reported"
    );
    assert_eq!(reused["profileId"], created["profileId"]);
    assert_eq!(
        count(
            &fixture,
            "SELECT count(*)::bigint FROM profile.profile_pdf_sources"
        )
        .await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn an_existing_email_is_a_duplicate_candidate_and_nothing_changes() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let seeded = profiles(&fixture)
        .create_profile(&profile_request_without_links(
            "Seeded Example",
            "ada.example@example.com",
        ))
        .await?;
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("email"), &extractor).await?;
    let service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("email"), &extractor),
    );

    let outcome = service
        .ingest_profile_from_stored_pdf(&request(&document))
        .await?;
    assert_eq!(outcome["status"], "DUPLICATE_PROFILE_CANDIDATE");
    assert_eq!(outcome["candidateProfileId"], seeded.profile.id.to_string());
    assert_eq!(outcome["matchedOn"], json!(["email"]));
    assert_eq!(outcome["profileId"], Value::Null);
    assert_eq!(
        outcome["recommendedAction"],
        "rerun with existingProfileId and overwriteExistingProfile=true to replace"
    );
    let unchanged = profiles(&fixture)
        .get_profile(&seeded.profile.id.to_string())
        .await?;
    assert_eq!(unchanged.profile.full_name, "Seeded Example");
    assert_eq!(
        count(
            &fixture,
            "SELECT count(*)::bigint FROM profile.profile_pdf_sources"
        )
        .await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn a_link_match_alone_is_a_duplicate_candidate_with_its_reason() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let seeded = profiles(&fixture)
        .create_profile(&profile_request(
            "Other Example",
            "other.example@example.com",
        ))
        .await?;
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("link"), &extractor).await?;
    let service = ingest(&fixture, documents(&fixture, pdf_bytes("link"), &extractor));

    let outcome = service
        .ingest_profile_from_stored_pdf(&request(&document))
        .await?;
    assert_eq!(outcome["status"], "DUPLICATE_PROFILE_CANDIDATE");
    assert_eq!(outcome["candidateProfileId"], seeded.profile.id.to_string());
    assert_eq!(outcome["matchedOn"], json!(["link:github"]));
    Ok(())
}

#[tokio::test]
async fn two_candidates_on_different_profiles_are_ambiguous() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let by_email = profiles(&fixture)
        .create_profile(&profile_request_without_links(
            "Email Example",
            "ada.example@example.com",
        ))
        .await?;
    let by_link = profiles(&fixture)
        .create_profile(&profile_request("Link Example", "link.example@example.com"))
        .await?;
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("ambiguous"), &extractor).await?;
    let service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("ambiguous"), &extractor),
    );

    let outcome = service
        .ingest_profile_from_stored_pdf(&request(&document))
        .await?;
    assert_eq!(outcome["status"], "AMBIGUOUS_PROFILE_CANDIDATES");
    assert_eq!(
        outcome["candidateProfileId"],
        by_email.profile.id.to_string()
    );
    assert_eq!(outcome["matchedOn"], json!(["email"]));
    assert_ne!(by_link.profile.id, by_email.profile.id);
    Ok(())
}

#[tokio::test]
async fn overwrite_replaces_the_profile_at_its_revision_and_links_the_source() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let seeded = profiles(&fixture)
        .create_profile(&profile_request("Old Example", "old.example@example.com"))
        .await?;
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("overwrite"), &extractor).await?;
    let service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("overwrite"), &extractor),
    );
    let profile_id = seeded.profile.id.to_string();

    let outcome = service
        .ingest_profile_from_stored_pdf(&json!({
            "documentId": document,
            "existingProfileId": profile_id,
            "overwriteExistingProfile": true,
            "expectedRevision": 0,
        }))
        .await?;
    assert_eq!(outcome["status"], "UPDATED_PROFILE");
    assert_eq!(outcome["createdProfile"], false);
    assert_eq!(outcome["profileId"], profile_id);
    let graph = profiles(&fixture).get_profile(&profile_id).await?;
    assert_eq!(graph.profile.full_name, "Ada Example");
    assert_eq!(graph.profile.email, "ada.example@example.com");
    assert_eq!(graph.profile.revision, 1);
    assert_eq!(graph.links.len(), 1, "the children are replaced");
    assert_postings_match(&fixture, &graph).await?;
    Ok(())
}

#[tokio::test]
async fn an_existing_profile_without_the_opt_in_is_refused_and_nothing_changes() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let seeded = profiles(&fixture)
        .create_profile(&profile_request("Old Example", "old.example@example.com"))
        .await?;
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("optin"), &extractor).await?;
    let service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("optin"), &extractor),
    );
    let profile_id = seeded.profile.id.to_string();

    let error = service
        .ingest_profile_from_stored_pdf(&json!({
            "documentId": document,
            "existingProfileId": profile_id,
            "expectedRevision": 0,
        }))
        .await
        .err()
        .ok_or("the call must fail")?;
    assert_eq!(
        message_of(&error),
        "Existing profile ingestion requires overwriteExistingProfile=true"
    );
    assert_eq!(
        error_envelope(&error).details.get("profileId"),
        Some(&profile_id)
    );
    let graph = profiles(&fixture).get_profile(&profile_id).await?;
    assert_eq!(graph.profile.full_name, "Old Example");
    assert_eq!(graph.profile.revision, 0);
    assert_eq!(
        count(
            &fixture,
            "SELECT count(*)::bigint FROM profile.profile_pdf_sources"
        )
        .await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn a_stale_revision_is_a_conflict_and_the_transaction_rolls_back() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let seeded = profiles(&fixture)
        .create_profile(&profile_request("Old Example", "old.example@example.com"))
        .await?;
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("stale"), &extractor).await?;
    let service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("stale"), &extractor),
    );
    let profile_id = seeded.profile.id.to_string();

    let error = service
        .ingest_profile_from_stored_pdf(&json!({
            "documentId": document,
            "existingProfileId": profile_id,
            "overwriteExistingProfile": true,
            "expectedRevision": 7,
        }))
        .await
        .err()
        .ok_or("the call must fail")?;
    assert_eq!(message_of(&error), "Profile revision conflict");
    let graph = profiles(&fixture).get_profile(&profile_id).await?;
    assert_eq!(graph.profile.full_name, "Old Example");
    assert_eq!(graph.profile.revision, 0);
    assert_eq!(
        count(
            &fixture,
            "SELECT count(*)::bigint FROM profile.profile_pdf_sources"
        )
        .await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn an_overwrite_without_an_expected_revision_is_refused_before_any_write() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let seeded = profiles(&fixture)
        .create_profile(&profile_request("Old Example", "old.example@example.com"))
        .await?;
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("norev"), &extractor).await?;
    let service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("norev"), &extractor),
    );

    let error = service
        .ingest_profile_from_stored_pdf(&json!({
            "documentId": document,
            "existingProfileId": seeded.profile.id.to_string(),
            "overwriteExistingProfile": true,
        }))
        .await
        .err()
        .ok_or("the call must fail")?;
    assert_eq!(error_envelope(&error).code.code(), "validation_error");
    let graph = profiles(&fixture)
        .get_profile(&seeded.profile.id.to_string())
        .await?;
    assert_eq!(graph.profile.revision, 0);
    Ok(())
}

#[tokio::test]
async fn a_profile_that_already_has_a_source_is_reported_and_not_mutated() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let extractor = ResumeExtractor::new(RESUME);
    let first = store(&fixture, pdf_bytes("owned"), &extractor).await?;
    let service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("owned"), &extractor),
    );
    let created = service
        .ingest_profile_from_stored_pdf(&request(&first))
        .await?;
    let profile_id = created["profileId"].as_str().ok_or("profileId")?.to_owned();

    let second = store(&fixture, pdf_bytes("other-bytes"), &extractor).await?;
    let outcome = service
        .ingest_profile_from_stored_pdf(&json!({
            "documentId": second,
            "existingProfileId": profile_id,
            "overwriteExistingProfile": true,
            "expectedRevision": 0,
        }))
        .await?;
    assert_eq!(outcome["status"], "DUPLICATE_PROFILE_CANDIDATE");
    assert_eq!(outcome["candidateProfileId"], profile_id);
    assert_eq!(outcome["matchedOn"], json!(["profileId"]));
    assert_eq!(
        outcome["recommendedAction"],
        "profile already has a linked PDF source; no mutation was performed"
    );
    let graph = profiles(&fixture).get_profile(&profile_id).await?;
    assert_eq!(graph.profile.revision, 0);
    Ok(())
}

#[tokio::test]
async fn concurrent_ingests_of_one_document_create_one_profile_and_one_source() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("race"), &extractor).await?;
    let service = ingest(&fixture, documents(&fixture, pdf_bytes("race"), &extractor));
    let first_request = request(&document);
    let second_request = request(&document);
    let (left, right) = tokio::join!(
        service.ingest_profile_from_stored_pdf(&first_request),
        service.ingest_profile_from_stored_pdf(&second_request),
    );
    let statuses = [left?["status"].clone(), right?["status"].clone()];
    let created = statuses
        .iter()
        .filter(|status| status.as_str() == Some("CREATED_PROFILE"))
        .count();
    assert_eq!(created, 1, "exactly one ingest creates the profile");
    assert_eq!(
        count(&fixture, "SELECT count(*)::bigint FROM profile.profiles").await?,
        1
    );
    assert_eq!(
        count(
            &fixture,
            "SELECT count(*)::bigint FROM profile.profile_pdf_sources"
        )
        .await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn an_unparseable_resume_is_an_extraction_error_and_writes_nothing() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let extractor = ResumeExtractor::new("Ada Example\nno contact details here");
    let document = store(&fixture, pdf_bytes("no-email"), &extractor).await?;
    let service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("no-email"), &extractor),
    );

    let error = service
        .ingest_profile_from_stored_pdf(&request(&document))
        .await
        .err()
        .ok_or("the call must fail")?;
    assert_eq!(message_of(&error), "Invalid profile extraction input");
    assert_eq!(
        error_envelope(&error)
            .details
            .get("field")
            .map(String::as_str),
        Some("email")
    );
    assert_eq!(
        count(&fixture, "SELECT count(*)::bigint FROM profile.profiles").await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn the_revision_interplay_with_profile_writes_is_strict() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let seeded = profiles(&fixture)
        .create_profile(&profile_request("Old Example", "old.example@example.com"))
        .await?;
    let profile_id = seeded.profile.id.to_string();
    // A profile write moves the revision to 1; a PDF overwrite that read 0 must now conflict.
    profiles(&fixture)
        .update_profile(
            &profile_id,
            &json!(0),
            &profile_request("Edited Example", "old.example@example.com"),
        )
        .await?;
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("interplay"), &extractor).await?;
    let service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("interplay"), &extractor),
    );

    let stale = service
        .ingest_profile_from_stored_pdf(&json!({
            "documentId": document,
            "existingProfileId": profile_id,
            "overwriteExistingProfile": true,
            "expectedRevision": 0,
        }))
        .await
        .err()
        .ok_or("the stale call must fail")?;
    assert_eq!(message_of(&stale), "Profile revision conflict");

    let current = service
        .ingest_profile_from_stored_pdf(&json!({
            "documentId": document,
            "existingProfileId": profile_id,
            "overwriteExistingProfile": true,
            "expectedRevision": 1,
        }))
        .await?;
    assert_eq!(current["status"], "UPDATED_PROFILE");
    let graph = profiles(&fixture).get_profile(&profile_id).await?;
    assert_eq!(graph.profile.revision, 2);
    assert_eq!(graph.profile.full_name, "Ada Example");
    Ok(())
}

#[tokio::test]
async fn stored_names_keep_only_their_base_name_on_create_overwrite_and_reuse() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let extractor = ResumeExtractor::new(RESUME);
    let created_document = store_named(
        &fixture,
        pdf_bytes("name-create"),
        r"C:\Users\synthetic\cv\synthetic-cv.pdf",
        &extractor,
    )
    .await?;
    let service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("name-create"), &extractor),
    );
    let created = service
        .ingest_profile_from_stored_pdf(&request(&created_document))
        .await?;
    assert_eq!(created["status"], "CREATED_PROFILE");
    assert_eq!(created["originalFileName"], "synthetic-cv.pdf");

    let reused = service
        .ingest_profile_from_stored_pdf(&request(&created_document))
        .await?;
    assert_eq!(reused["status"], "REUSED_EXISTING_SOURCE");
    assert_eq!(reused["originalFileName"], "synthetic-cv.pdf");

    let seeded = profiles(&fixture)
        .create_profile(&profile_request_without_links(
            "Old Example",
            "old.example@example.com",
        ))
        .await?;
    let other_text = RESUME
        .replace("ada.example@example.com", "other.example@example.com")
        .replace(
            "https://github.com/ada-example",
            "https://github.com/other-example",
        );
    let other = ResumeExtractor::new(&other_text);
    let overwrite_document = store_named(
        &fixture,
        pdf_bytes("name-overwrite"),
        "/srv/synthetic/cv/nested/synthetic-cv.pdf",
        &other,
    )
    .await?;
    let overwrite_service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("name-overwrite"), &other),
    );
    let overwritten = overwrite_service
        .ingest_profile_from_stored_pdf(&json!({
            "documentId": overwrite_document,
            "existingProfileId": seeded.profile.id.to_string(),
            "overwriteExistingProfile": true,
            "expectedRevision": 0,
        }))
        .await?;
    assert_eq!(overwritten["status"], "UPDATED_PROFILE");
    assert_eq!(overwritten["originalFileName"], "synthetic-cv.pdf");
    Ok(())
}

#[tokio::test]
async fn a_stored_name_that_reduces_to_a_relative_marker_is_refused_before_any_write() -> TestResult
{
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let extractor = ResumeExtractor::new(RESUME);
    let client = fixture.client().await;
    for (label, marker) in [("dot-dot", ".."), ("dot", ".")] {
        let document = store(&fixture, pdf_bytes(label), &extractor).await?;
        let document_id = Uuid::parse_str(&document)?;
        client
            .execute(
                "UPDATE document.documents SET original_file_name = $2 WHERE id = $1",
                &[&document_id, &marker],
            )
            .await?;
        let service = ingest(&fixture, documents(&fixture, pdf_bytes(label), &extractor));
        let error = service
            .ingest_profile_from_stored_pdf(&request(&document))
            .await
            .err()
            .ok_or("the call must fail")?;
        assert_eq!(message_of(&error), "Invalid profile ingestion request");
        assert_eq!(
            error_envelope(&error)
                .details
                .get("field")
                .map(String::as_str),
            Some("originalFileName")
        );
        assert_eq!(
            extraction_rows(&fixture, document_id).await?,
            0,
            "{label}: a refused name must not persist an extraction"
        );
    }
    assert_eq!(
        count(&fixture, "SELECT count(*)::bigint FROM profile.profiles").await?,
        0
    );
    assert_eq!(
        count(
            &fixture,
            "SELECT count(*)::bigint FROM profile.profile_pdf_sources"
        )
        .await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn a_refused_stored_name_stores_no_extraction_and_a_valid_one_stores_exactly_one()
-> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let extractor = ResumeExtractor::new(RESUME);
    let client = fixture.client().await;
    let refused = store(&fixture, pdf_bytes("refused"), &extractor).await?;
    let refused_id = Uuid::parse_str(&refused)?;
    client
        .execute(
            "UPDATE document.documents SET original_file_name = $2 WHERE id = $1",
            &[&refused_id, &".."],
        )
        .await?;
    let service = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("refused"), &extractor),
    );
    let error = service
        .ingest_profile_from_stored_pdf(&request(&refused))
        .await
        .err()
        .ok_or("the call must fail")?;
    assert_eq!(message_of(&error), "Invalid profile ingestion request");
    assert_eq!(extraction_rows(&fixture, refused_id).await?, 0);

    let valid = store(&fixture, pdf_bytes("valid"), &extractor).await?;
    let valid_id = Uuid::parse_str(&valid)?;
    let created = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("valid"), &extractor),
    )
    .ingest_profile_from_stored_pdf(&request(&valid))
    .await?;
    assert_eq!(created["originalFileName"], "synthetic-cv.pdf");
    assert_eq!(extraction_rows(&fixture, valid_id).await?, 1);
    Ok(())
}

#[tokio::test]
async fn a_failed_link_insert_rolls_back_the_graph_the_postings_and_the_source() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("fail-link"), &extractor).await?;
    let seam = LinkSeam {
        client: Arc::new(fixture.client().await),
        outcome: SeamOutcome::Fail,
    };
    let service = ingest_with_seam(
        &fixture,
        documents(&fixture, pdf_bytes("fail-link"), &extractor),
        seam,
    );

    assert!(
        service
            .ingest_profile_from_stored_pdf(&request(&document))
            .await
            .is_err(),
        "a failed link insert fails the call"
    );
    for table in [
        "profile.profiles",
        "profile.profile_links",
        "profile.search_terms",
        "profile.profile_pdf_sources",
    ] {
        assert_eq!(
            count(&fixture, &format!("SELECT count(*)::bigint FROM {table}")).await?,
            0,
            "{table} is rolled back"
        );
    }
    // The same document ingests normally afterwards: no lock or savepoint state was left behind.
    let retried = ingest(
        &fixture,
        documents(&fixture, pdf_bytes("fail-link"), &extractor),
    )
    .ingest_profile_from_stored_pdf(&request(&document))
    .await?;
    assert_eq!(retried["status"], "CREATED_PROFILE");
    Ok(())
}

#[tokio::test]
async fn a_lost_link_race_reuses_the_winner_and_leaves_no_graph_of_its_own() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let winner = profiles(&fixture)
        .create_profile(&profile_request_without_links(
            "Winner Example",
            "winner.example@example.com",
        ))
        .await?;
    let winner_id = winner.profile.id;
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("lose-race"), &extractor).await?;
    let seam = LinkSeam {
        client: Arc::new(fixture.client().await),
        outcome: SeamOutcome::Lose { winner: winner_id },
    };
    let service = ingest_with_seam(
        &fixture,
        documents(&fixture, pdf_bytes("lose-race"), &extractor),
        seam,
    );

    let outcome = service
        .ingest_profile_from_stored_pdf(&request(&document))
        .await?;
    assert_eq!(outcome["status"], "REUSED_EXISTING_SOURCE");
    assert_eq!(outcome["profileId"], winner_id.to_string());
    assert_eq!(outcome["createdProfile"], false);
    assert_eq!(
        count(&fixture, "SELECT count(*)::bigint FROM profile.profiles").await?,
        1,
        "the graph the race lost is rolled back"
    );
    assert_eq!(
        count(
            &fixture,
            "SELECT count(*)::bigint FROM profile.profile_pdf_sources"
        )
        .await?,
        1
    );
    let client = fixture.client().await;
    assert_eq!(
        count(
            &fixture,
            "SELECT count(*)::bigint FROM profile.search_terms"
        )
        .await?,
        i64::try_from(postings(&client, winner_id).await?.len())?,
        "only the winner's postings remain"
    );
    Ok(())
}

#[tokio::test]
async fn a_lost_link_race_for_an_overwrite_of_another_profile_is_already_linked() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let target = profiles(&fixture)
        .create_profile(&profile_request_without_links(
            "Old Example",
            "old.example@example.com",
        ))
        .await?;
    let winner = profiles(&fixture)
        .create_profile(&profile_request_without_links(
            "Winner Example",
            "winner.example@example.com",
        ))
        .await?;
    let target_id = target.profile.id.to_string();
    let extractor = ResumeExtractor::new(RESUME);
    let document = store(&fixture, pdf_bytes("lose-overwrite"), &extractor).await?;
    let seam = LinkSeam {
        client: Arc::new(fixture.client().await),
        outcome: SeamOutcome::Lose {
            winner: winner.profile.id,
        },
    };
    let service = ingest_with_seam(
        &fixture,
        documents(&fixture, pdf_bytes("lose-overwrite"), &extractor),
        seam,
    );

    let error = service
        .ingest_profile_from_stored_pdf(&json!({
            "documentId": document,
            "existingProfileId": target_id,
            "overwriteExistingProfile": true,
            "expectedRevision": 0,
        }))
        .await
        .err()
        .ok_or("the call must fail")?;
    assert_eq!(
        message_of(&error),
        "Profile is already linked to a PDF extraction"
    );
    let graph = profiles(&fixture).get_profile(&target_id).await?;
    assert_eq!(graph.profile.full_name, "Old Example");
    assert_eq!(graph.profile.revision, 0);
    assert_eq!(
        count(
            &fixture,
            "SELECT count(*)::bigint FROM profile.profile_pdf_sources"
        )
        .await?,
        1,
        "only the competing winner's source exists"
    );
    Ok(())
}
