//! Document content is permanent (row-local triggers), and the birth of a classified asset is
//! tracked by an explicit marker instead of `row_version`, so an update of the asset row before
//! its intent insert in the same transaction still works. All data is synthetic.

mod sqlite_generated_support;
mod sqlite_support;

use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedReservation, GenerationDescriptor, GenerationPurpose, sha256_hex,
};
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::sqlite::generated::IntentError;
use jobniac_infra::sqlite::generated::intents::insert_generation_intent;
use rusqlite::params;
use sqlite_generated_support::{count, instant, pdf, register_root, seed_document, stored};
use sqlite_support::{SqliteFixture, TestResult};
use uuid::Uuid;

#[test]
fn document_blob_binding_and_blob_content_cannot_be_updated() -> TestResult {
    let fixture = SqliteFixture::start();
    let conn = fixture.connection();
    let first = seed_document(&conn, &pdf("first"), instant(0))?;
    let second = seed_document(&conn, &pdf("second"), instant(0))?;
    let blob_of = |document: Uuid| -> rusqlite::Result<String> {
        conn.query_row(
            "SELECT blob_id FROM document_documents WHERE id = ?1",
            [document.to_string()],
            |row| row.get(0),
        )
    };
    let (first_blob, second_blob) = (blob_of(first)?, blob_of(second)?);

    // Even a no-op assignment is refused: the rule is "never updated".
    for sql in [
        "UPDATE document_documents SET blob_id = ?1",
        "UPDATE document_documents SET blob_id = ?2 WHERE id = ?3",
        "UPDATE document_blobs SET sha256 = sha256",
        "UPDATE document_blobs SET byte_size = byte_size",
        "UPDATE document_blobs SET content = content",
    ] {
        let refused = conn.execute(sql, params![first_blob, second_blob, first.to_string()]);
        assert!(refused.is_err(), "{sql} must be refused");
    }
    assert_eq!(blob_of(first)?, first_blob);

    // Other columns of the document stay writable.
    conn.execute(
        "UPDATE document_documents SET original_file_name = 'renamed.pdf' WHERE id = ?1",
        [first.to_string()],
    )?;
    Ok(())
}

#[tokio::test]
async fn an_asset_update_before_the_intent_insert_keeps_the_birth() -> TestResult {
    let fixture = SqliteFixture::start();
    let uow = fixture.unit_of_work();
    let now = instant(1_700_000_000);
    let root = Uuid::new_v4();
    register_root(&fixture, root, now).await?;
    let content = pdf("born");
    let blob = Uuid::new_v4();
    let document = Uuid::new_v4();
    let asset_id = Uuid::new_v4();
    let digest = sha256_hex(&content);
    let size = i64::try_from(content.len())?;
    let identity = AssetIdentity::new(asset_id, root, &digest, content.len() as u64)?;
    let reservation = GeneratedReservation {
        asset: identity,
        document_id: document,
    };
    let descriptor = GenerationDescriptor::new(
        Uuid::new_v4(),
        GenerationPurpose::StandalonePdf,
        None,
        None,
        None,
        None,
    )?;
    uow.transaction::<(), IntentError>(TransactionOptions::default(), async |tx| {
        tx.run(|conn| {
            conn.execute(
                "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![blob.to_string(), digest, size, content, stored(now)],
            )?;
            conn.execute(
                "INSERT INTO document_documents (id, blob_id, original_file_name, media_type, \
                 created_at, updated_at) VALUES (?1, ?2, 'born.pdf', 'application/pdf', ?3, ?3)",
                params![document.to_string(), blob.to_string(), stored(now)],
            )?;
            conn.execute(
                "INSERT INTO jobniac_generated_assets (asset_id, root_id, relative_path, sha256, \
                 size_bytes, document_id, lifecycle, created_at, generation_intent_required) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'RESERVED', ?7, 1)",
                params![
                    asset_id.to_string(),
                    root.to_string(),
                    format!("{asset_id}.pdf"),
                    digest,
                    size,
                    document.to_string(),
                    stored(now)
                ],
            )?;
            // Any update bumps `row_version`; the birth must not depend on it.
            conn.execute(
                "UPDATE jobniac_generated_assets SET materialized_at = materialized_at \
                 WHERE asset_id = ?1",
                [asset_id.to_string()],
            )?;
            Ok(())
        })
        .map_err(|error| IntentError::Database(error.into()))?;
        insert_generation_intent(tx, &reservation, &descriptor, Uuid::new_v4(), now).await?;
        Ok(())
    })
    .await?;

    // The marker is gone once the intent exists, and a later transaction cannot be a birth.
    let conn = fixture.connection();
    assert_eq!(
        count(&conn, "SELECT count(*) FROM jobniac_generation_births")?,
        0
    );
    let again = uow
        .transaction::<(), IntentError>(TransactionOptions::default(), async |tx| {
            insert_generation_intent(tx, &reservation, &descriptor, Uuid::new_v4(), now).await?;
            Ok(())
        })
        .await;
    assert!(again.is_err());
    Ok(())
}
