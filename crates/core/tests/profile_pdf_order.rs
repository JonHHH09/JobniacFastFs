//! The order of the stored-PDF ingestion checks: the stored file name is checked through
//! the document port's read-only metadata before any extraction is read or persisted.
//!
//! The doubles record every call, so a refused name must show one metadata read, no extraction and
//! no persistence call. A valid name must reach the extraction after the metadata read. The
//! SQLite tests in `jobniac-infra` prove the same against real rows.

mod profile_support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jobniac_core::clock::SystemClock;
use jobniac_core::documents::{DocumentMetadata, StoredExtraction};
use jobniac_core::errors::{AppError, ApplicationError, ErrorCode, error_envelope};
use jobniac_core::profiles::pdf_ingest::{
    INGEST_VALIDATION_MESSAGE, LinkedSource, NewSourceLink, ProfilePdfRepository,
    ProfilePdfService, ProfilePdfSource, StoredPdfDocuments,
};
use profile_support::{MemoryProfiles, MemoryUow};
use serde_json::{Value, json};
use uuid::Uuid;

/// The message of the extraction double, which a valid name reaches and returns.
const REACHED: &str = "extraction reached";

/// A document port double that reports one stored name and logs each call in order.
#[derive(Debug)]
struct Documents {
    name: String,
    log: Mutex<Vec<&'static str>>,
}

impl Documents {
    fn named(name: &str) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_owned(),
            log: Mutex::new(Vec::new()),
        })
    }

    fn log(&self) -> Vec<&'static str> {
        self.log
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

impl StoredPdfDocuments for Documents {
    async fn stored_document_metadata(
        &self,
        _document_id: &str,
    ) -> Result<DocumentMetadata, AppError> {
        push(&self.log, "metadata");
        Ok(DocumentMetadata {
            id: Uuid::nil(),
            original_file_name: self.name.clone(),
            media_type: "application/pdf".to_owned(),
            byte_size: 0,
            sha256: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
        })
    }

    async fn extract_stored_pdf_text(
        &self,
        _request: &Value,
    ) -> Result<StoredExtraction, AppError> {
        push(&self.log, "extract");
        Err(unexpected(REACHED))
    }
}

/// A persistence double that records every call. The ingestion must never reach it in these
/// tests, because the extraction double stops the flow for a valid name.
#[derive(Debug, Default)]
struct Pdf {
    calls: Arc<AtomicUsize>,
}

impl Pdf {
    fn refuse(&self) -> AppError {
        self.calls.fetch_add(1, Ordering::SeqCst);
        unexpected("persistence reached")
    }
}

#[async_trait]
impl ProfilePdfRepository for Pdf {
    type Tx = ();

    async fn lock(&self, _tx: &mut (), _hashed_key: &str) -> Result<(), AppError> {
        Err(self.refuse())
    }

    async fn lock_profile_root(&self, _tx: &mut (), _profile_id: Uuid) -> Result<(), AppError> {
        Err(self.refuse())
    }

    async fn linked_by_extraction(
        &self,
        _tx: &mut (),
        _extraction_id: Uuid,
    ) -> Result<Option<LinkedSource>, AppError> {
        Err(self.refuse())
    }

    async fn linked_by_document_hash(
        &self,
        _tx: &mut (),
        _sha256: &str,
    ) -> Result<Option<LinkedSource>, AppError> {
        Err(self.refuse())
    }

    async fn profile_has_source(&self, _tx: &mut (), _profile_id: Uuid) -> Result<bool, AppError> {
        Err(self.refuse())
    }

    async fn email_candidates(&self, _tx: &mut (), _email: &str) -> Result<Vec<Uuid>, AppError> {
        Err(self.refuse())
    }

    async fn link_candidates(
        &self,
        _tx: &mut (),
        _link_type: &str,
        _url: &str,
    ) -> Result<Vec<Uuid>, AppError> {
        Err(self.refuse())
    }

    async fn profile_has_link(
        &self,
        _tx: &mut (),
        _profile_id: Uuid,
        _link_type: &str,
        _url: &str,
    ) -> Result<bool, AppError> {
        Err(self.refuse())
    }

    async fn extraction_matches(
        &self,
        _tx: &mut (),
        _extraction_id: Uuid,
        _document_id: Uuid,
        _sha256: &str,
    ) -> Result<bool, AppError> {
        Err(self.refuse())
    }

    async fn savepoint(&self, _tx: &mut ()) -> Result<(), AppError> {
        Err(self.refuse())
    }

    async fn rollback_to_savepoint(&self, _tx: &mut ()) -> Result<(), AppError> {
        Err(self.refuse())
    }

    async fn insert_source_link(
        &self,
        _tx: &mut (),
        _link: &NewSourceLink,
    ) -> Result<bool, AppError> {
        Err(self.refuse())
    }

    async fn source_of_profile(
        &self,
        _tx: &mut (),
        _profile_id: Uuid,
    ) -> Result<Option<ProfilePdfSource>, AppError> {
        Err(self.refuse())
    }
}

fn push(log: &Mutex<Vec<&'static str>>, entry: &'static str) {
    if let Ok(mut guard) = log.lock() {
        guard.push(entry);
    }
}

fn unexpected(message: &str) -> AppError {
    AppError::from(ApplicationError::with_message(
        ErrorCode::InternalError,
        message,
    ))
}

/// Runs the ingestion over `documents` with a fixed valid document identifier. Returns the result,
/// the document log and the number of persistence calls.
async fn ingest(
    documents: Arc<Documents>,
) -> (Result<Value, AppError>, Arc<Documents>, Arc<AtomicUsize>) {
    let pdf = Pdf::default();
    let calls = Arc::clone(&pdf.calls);
    let service = ProfilePdfService::new(
        MemoryUow,
        MemoryProfiles::new(),
        pdf,
        Arc::clone(&documents),
        Arc::new(SystemClock),
    );
    let request = json!({"documentId": "00000000-0000-4000-8000-000000000001"});
    (
        service.ingest_profile_from_stored_pdf(&request).await,
        documents,
        calls,
    )
}

#[tokio::test]
async fn a_name_that_reduces_to_a_relative_marker_is_refused_before_the_extraction() {
    for name in ["..", ".", "folder/..", "folder\\.", "nested\\folder\\.."] {
        let (result, documents, pdf) = ingest(Documents::named(name)).await;
        let error = result
            .err()
            .unwrap_or_else(|| panic!("{name} must be refused"));
        let envelope = error_envelope(&error);
        assert_eq!(envelope.message, INGEST_VALIDATION_MESSAGE, "{name}");
        assert_eq!(
            envelope.details.get("field").map(String::as_str),
            Some("originalFileName"),
            "{name}"
        );
        assert_eq!(
            documents.log(),
            vec!["metadata"],
            "{name}: only the read-only metadata may run before the refusal"
        );
        assert_eq!(
            pdf.load(Ordering::SeqCst),
            0,
            "{name}: nothing may be persisted"
        );
    }
}

#[tokio::test]
async fn a_valid_name_reaches_the_extraction_after_the_metadata_read() {
    let (result, documents, pdf) = ingest(Documents::named("synthetic-cv.pdf")).await;
    let error = result
        .err()
        .unwrap_or_else(|| panic!("the extraction double stops the flow"));
    assert_eq!(error_envelope(&error).message, REACHED);
    assert_eq!(documents.log(), vec!["metadata", "extract"]);
    assert_eq!(pdf.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_blank_name_falls_back_to_the_stored_pdf_base_name_and_is_not_refused() {
    let (result, documents, _pdf) = ingest(Documents::named("")).await;
    let error = result
        .err()
        .unwrap_or_else(|| panic!("the extraction double stops the flow"));
    assert_eq!(error_envelope(&error).message, REACHED);
    assert_eq!(documents.log(), vec!["metadata", "extract"]);
}
