//! The generated-asset guards, applied directly on a
//! raw connection of a migrated SQLite file: adoption, document identity, retirement, deletion,
//! cleanup retention, attachment of owned documents and the lease rules of an intent renewal. All
//! data is synthetic.

mod sqlite_generated_support;
mod sqlite_support;

use jobniac_core::generated_assets::service::GeneratedAssetRepository;
use jobniac_core::generated_assets::standalone::StandaloneAssetRepository;
use jobniac_core::generated_assets::{GenerationIntent, GenerationState};
use jobniac_infra::sqlite::error::SqliteError;
use jobniac_infra::sqlite::generated::guards::{
    NewAsset, assert_generated_document_attachable, guard_asset_insert, guard_cleanup_delete,
    guard_cleanup_write, guard_deletion, guard_document_path_reference, guard_intent_update,
    guard_pdf_source, guard_retirement,
};
use rusqlite::params;
use sqlite_generated_support::{
    identity, instant, pdf, register_root, repository, seed_document, seed_profile,
    standalone_descriptor, stored,
};
use sqlite_support::{SqliteFixture, TestResult};
use time::Duration;
use uuid::Uuid;

fn is_violation<T: std::fmt::Debug>(result: Result<T, SqliteError>) -> bool {
    matches!(
        result,
        Err(SqliteError::Constraint { ref sqlstate, constraint: None }) if sqlstate == "23514"
    )
}

/// Reserves one owned asset through the repository and returns (asset, document).
async fn owned(fixture: &SqliteFixture, root: Uuid, label: &str) -> Result<(Uuid, Uuid), String> {
    let content = pdf(label);
    let identity = identity(root, &content);
    let document = Uuid::new_v4();
    repository(fixture, 0)
        .reserve_generated_pdf(&identity, document, "owned.pdf", &content)
        .await
        .map_err(|error| error.to_string())?;
    Ok((identity.asset_id(), document))
}

fn set_lifecycle(fixture: &SqliteFixture, asset: Uuid, lifecycle: &str) {
    let retired = lifecycle == "RETIRING";
    fixture
        .connection()
        .execute(
            "UPDATE jobniac_generated_assets SET lifecycle = ?2, materialized_at = ?3, \
             retirement_id = ?4, retired_at = ?5 WHERE asset_id = ?1",
            params![
                asset.to_string(),
                lifecycle,
                stored(instant(1)),
                retired.then(|| Uuid::new_v4().to_string()),
                retired.then(|| stored(instant(1)))
            ],
        )
        .expect("lifecycle updates");
}

#[tokio::test]
async fn an_existing_reference_cannot_be_adopted_and_the_document_identity_must_match() -> TestResult
{
    let fixture = SqliteFixture::start();
    let conn = fixture.connection();
    let content = pdf("adopt");
    let document = seed_document(&conn, &content, instant(0))?;
    let asset_id = Uuid::new_v4();
    let path = format!("{asset_id}.pdf");
    let digest = jobniac_core::generated_assets::sha256_hex(&content);
    let new = NewAsset {
        asset_id,
        relative_path: &path,
        document_id: Some(document),
        sha256: &digest,
        size_bytes: content.len() as i64,
    };
    assert!(guard_asset_insert(&conn, &new).is_ok());
    // A wrong digest or size is an inconsistent identity.
    let wrong = NewAsset {
        sha256: &"0".repeat(64),
        ..new
    };
    assert!(is_violation(guard_asset_insert(&conn, &wrong)));
    let wrong = NewAsset {
        size_bytes: 1,
        ..new
    };
    assert!(is_violation(guard_asset_insert(&conn, &wrong)));
    // A photo that already references the document blocks adoption.
    let profile = Uuid::new_v4();
    seed_profile(&conn, profile, 0, instant(0))?;
    conn.execute(
        "INSERT INTO profile_profile_personal_details \
         (profile_id, photo_document_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![
            profile.to_string(),
            document.to_string(),
            stored(instant(0))
        ],
    )?;
    assert!(is_violation(guard_asset_insert(&conn, &new)));
    Ok(())
}

#[tokio::test]
async fn retirement_and_deletion_follow_references_and_completed_cleanup() -> TestResult {
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, instant(0)).await?;
    let (asset, document) = owned(&fixture, root, "retire").await?;
    let conn = fixture.connection();
    assert!(guard_retirement(&conn, asset).is_ok());

    let profile = Uuid::new_v4();
    seed_profile(&conn, profile, 0, instant(0))?;
    conn.execute(
        "INSERT INTO profile_profile_personal_details \
         (profile_id, photo_document_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![
            profile.to_string(),
            document.to_string(),
            stored(instant(0))
        ],
    )?;
    assert!(is_violation(guard_retirement(&conn, asset)));
    conn.execute("DELETE FROM profile_profile_personal_details", [])?;

    // A retired asset is deleted only after its cleanup task completed.
    let retirement = Uuid::new_v4();
    conn.execute(
        "UPDATE jobniac_generated_assets SET lifecycle = 'RETIRING', retirement_id = ?2, \
         retired_at = ?3 WHERE asset_id = ?1",
        params![
            asset.to_string(),
            retirement.to_string(),
            stored(instant(1))
        ],
    )?;
    assert!(is_violation(guard_deletion(&conn, asset, retirement)));
    assert!(guard_cleanup_write(&conn, asset, retirement).is_ok());
    assert!(is_violation(guard_cleanup_write(
        &conn,
        asset,
        Uuid::new_v4()
    )));
    let task = Uuid::new_v4();
    conn.execute(
        "INSERT INTO jobniac_generated_asset_cleanups \
         (task_id, asset_id, retirement_id, status, next_attempt_at, created_at, updated_at) \
         VALUES (?1, ?2, ?3, 'PENDING', ?4, ?4, ?4)",
        params![
            task.to_string(),
            asset.to_string(),
            retirement.to_string(),
            stored(instant(1))
        ],
    )?;
    assert!(is_violation(guard_cleanup_delete(&conn, task)));
    conn.execute(
        "UPDATE jobniac_generated_asset_cleanups SET status = 'COMPLETED', completed_at = ?2 \
         WHERE task_id = ?1",
        params![task.to_string(), stored(instant(2))],
    )?;
    assert!(guard_deletion(&conn, asset, retirement).is_ok());
    // The task of an asset that is not yet DELETED is still retained.
    assert!(is_violation(guard_cleanup_delete(&conn, task)));
    conn.execute(
        "UPDATE jobniac_generated_assets SET lifecycle = 'DELETED', deleted_at = ?2 \
         WHERE asset_id = ?1",
        params![asset.to_string(), stored(instant(3))],
    )?;
    assert!(guard_cleanup_delete(&conn, task).is_ok());
    Ok(())
}

#[tokio::test]
async fn owned_documents_attach_only_while_active_and_keep_their_path_and_content() -> TestResult {
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, instant(0)).await?;
    let (asset, document) = owned(&fixture, root, "attach").await?;
    let conn = fixture.connection();
    let path = format!("{asset}.pdf");

    assert!(is_violation(assert_generated_document_attachable(
        &conn,
        Some(document)
    )));
    assert!(assert_generated_document_attachable(&conn, None).is_ok());
    set_lifecycle(&fixture, asset, "ACTIVE");
    assert!(assert_generated_document_attachable(&conn, Some(document)).is_ok());
    assert!(guard_document_path_reference(&conn, document, &path).is_ok());
    // The document keeps the owned path, and the owned path stays with its document.
    assert!(is_violation(guard_document_path_reference(
        &conn,
        document,
        "other.pdf"
    )));
    let stranger = seed_document(&conn, &pdf("stranger"), instant(0))?;
    assert!(is_violation(guard_document_path_reference(
        &conn, stranger, &path
    )));

    // Owned content cannot be rebound or rewritten.
    let other_blob: String = conn.query_row(
        "SELECT blob_id FROM document_documents WHERE id = ?1",
        [stranger.to_string()],
        |row| row.get(0),
    )?;
    let own_blob: String = conn.query_row(
        "SELECT blob_id FROM document_documents WHERE id = ?1",
        [document.to_string()],
        |row| row.get(0),
    )?;
    // Owned content is permanent: the baseline triggers refuse any rebinding or rewrite.
    let rebind = conn.execute(
        "UPDATE document_documents SET blob_id = ?2 WHERE id = ?1",
        params![document.to_string(), other_blob],
    );
    assert!(rebind.is_err());
    let rewrite = conn.execute(
        "UPDATE document_blobs SET byte_size = byte_size WHERE id = ?1",
        params![own_blob],
    );
    assert!(rewrite.is_err());

    // A PDF source over an extraction of the owned document needs the asset ACTIVE.
    let extraction = Uuid::new_v4();
    conn.execute(
        "INSERT INTO document_pdf_extractions (id, file_id, extractor, character_count, \
         page_count, truncated, extracted_text, created_at) VALUES (?1, ?2, 'synthetic', 1, 1, 0, 'x', ?3)",
        params![extraction.to_string(), document.to_string(), stored(instant(0))],
    )?;
    assert!(guard_pdf_source(&conn, extraction).is_ok());
    set_lifecycle(&fixture, asset, "RETIRING");
    assert!(is_violation(guard_pdf_source(&conn, extraction)));
    Ok(())
}

#[tokio::test]
async fn a_renewal_rotates_the_fence_and_never_shrinks_the_lease() -> TestResult {
    let fixture = SqliteFixture::start();
    let root = Uuid::new_v4();
    register_root(&fixture, root, instant(0)).await?;
    let content = pdf("renewal");
    let identity = identity(root, &content);
    let claim = repository(&fixture, 0)
        .reserve_classified_pdf(
            &identity,
            Uuid::new_v4(),
            "renewal.pdf",
            &content,
            &standalone_descriptor(),
            Uuid::new_v4(),
            instant(0),
        )
        .await?;
    let old: GenerationIntent = claim.intent.clone();
    let conn = fixture.connection();
    let mut renewed = old.clone();
    renewed.lease_token = Some(Uuid::new_v4());
    renewed.lease_started_at = Some(instant(10));
    renewed.lease_expires_at = Some(instant(10) + Duration::minutes(5));
    assert!(guard_intent_update(&conn, &old, &renewed).is_ok());

    let mut same_token = renewed.clone();
    same_token.lease_token = old.lease_token;
    assert!(is_violation(guard_intent_update(&conn, &old, &same_token)));
    let mut shrunk = renewed.clone();
    shrunk.lease_expires_at = Some(instant(0) + Duration::minutes(1));
    shrunk.lease_started_at = Some(instant(0));
    assert!(is_violation(guard_intent_update(&conn, &old, &shrunk)));
    let mut late_start = renewed.clone();
    late_start.lease_started_at = old.lease_expires_at;
    assert!(is_violation(guard_intent_update(&conn, &old, &late_start)));

    // A finish before the lease started, or a completion after expiry, is refused.
    let mut finished = old.clone();
    finished.state = GenerationState::Abandoned;
    finished.lease_token = None;
    finished.lease_started_at = None;
    finished.lease_expires_at = None;
    finished.finished_at = Some(instant(0) - Duration::seconds(1));
    assert!(is_violation(guard_intent_update(&conn, &old, &finished)));
    Ok(())
}
