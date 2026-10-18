//! Rules 13 to 15 of the PostgreSQL guard triggers, through the repositories that write the guarded
//! tables: an owned generated document is attached to a PDF extraction or a PDF source only while
//! its asset is `ACTIVE`. The path rules of resume documents are covered by the profile link tests
//! and the German tests. All data is synthetic.

mod sqlite_generated_support;
mod sqlite_support;

use jobniac_core::documents::{DocumentRepository, ExtractionRecord};
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::standalone::StandaloneAssetRepository;
use jobniac_core::profiles::pdf_ingest::{NewSourceLink, ProfilePdfRepository};
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::sqlite::documents::SqliteDocumentRepository;
use jobniac_infra::sqlite::profiles::pdf::SqliteProfilePdfRepository;
use rusqlite::params;
use sqlite_generated_support::{
    identity, instant, pdf, register_root, repository, seed_profile, stored,
};
use sqlite_support::{SqliteFixture, TestResult};
use uuid::Uuid;

fn is_violation<T: std::fmt::Debug>(result: Result<T, AppError>) -> bool {
    matches!(
        result,
        Err(AppError::PersistenceConstraint { ref sqlstate, constraint: None })
            if sqlstate.as_deref() == Some("23514")
    )
}

/// Reserves an owned (RESERVED, not yet ACTIVE) document and returns its asset and document.
async fn owned(fixture: &SqliteFixture) -> Result<(Uuid, Uuid), String> {
    let root = Uuid::new_v4();
    register_root(fixture, root, instant(0))
        .await
        .map_err(|error| error.to_string())?;
    let content = pdf("guarded");
    let identity = identity(root, &content);
    let document = Uuid::new_v4();
    repository(fixture, 0)
        .reserve_generated_pdf(&identity, document, "owned.pdf", &content)
        .await
        .map_err(|error| error.to_string())?;
    Ok((identity.asset_id(), document))
}

fn activate(fixture: &SqliteFixture, asset: Uuid) {
    fixture
        .connection()
        .execute(
            "UPDATE jobniac_generated_assets SET lifecycle = 'ACTIVE', materialized_at = ?2 \
             WHERE asset_id = ?1",
            params![asset.to_string(), stored(instant(1))],
        )
        .expect("the asset activates");
}

fn extraction(document: Uuid) -> ExtractionRecord {
    ExtractionRecord {
        id: Uuid::new_v4(),
        file_id: document,
        extractor: "synthetic".to_owned(),
        character_count: 1,
        page_count: 1,
        truncated: false,
        extracted_text: "x".to_owned(),
        pages: Vec::new(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
    }
}

#[tokio::test]
async fn an_extraction_of_an_owned_document_is_stored_only_while_its_asset_is_active() -> TestResult
{
    let fixture = SqliteFixture::start();
    let (asset, document) = owned(&fixture).await?;
    let uow = fixture.unit_of_work();
    let save = |record: ExtractionRecord| {
        let uow = uow.clone();
        async move {
            uow.transaction::<_, AppError>(TransactionOptions::default(), async |tx| {
                SqliteDocumentRepository.save_extraction(tx, &record).await
            })
            .await
        }
    };
    assert!(is_violation(save(extraction(document)).await));
    activate(&fixture, asset);
    assert!(save(extraction(document)).await.is_ok());
    Ok(())
}

#[tokio::test]
async fn a_pdf_source_over_an_owned_document_is_linked_only_while_its_asset_is_active() -> TestResult
{
    let fixture = SqliteFixture::start();
    let (asset, document) = owned(&fixture).await?;
    let profile = Uuid::new_v4();
    let conn = fixture.connection();
    seed_profile(&conn, profile, 0, instant(0))?;
    let extraction_id = Uuid::new_v4();
    conn.execute(
        "INSERT INTO document_pdf_extractions (id, file_id, extractor, character_count, \
         page_count, truncated, extracted_text, created_at) \
         VALUES (?1, ?2, 'synthetic', 1, 1, 0, 'x', ?3)",
        params![
            extraction_id.to_string(),
            document.to_string(),
            stored(instant(0))
        ],
    )?;
    let link = || NewSourceLink {
        id: Uuid::new_v4(),
        profile_id: profile,
        extraction_id,
        created_at: instant(2),
    };
    let uow = fixture.unit_of_work();
    let insert = |link: NewSourceLink| {
        let uow = uow.clone();
        async move {
            uow.transaction::<_, AppError>(TransactionOptions::default(), async |tx| {
                SqliteProfilePdfRepository
                    .insert_source_link(tx, &link)
                    .await
            })
            .await
        }
    };
    assert!(is_violation(insert(link()).await));
    activate(&fixture, asset);
    assert!(insert(link()).await?);
    Ok(())
}
