//! Classified generation intents against a migrated SQLite file: birth insertion in the asset's own
//! transaction, live admission, lease expiry, fencing of a stale token, renewal and abandonment with
//! same-transaction retirement, and the intent guards that PostgreSQL enforced in a trigger. All
//! data is synthetic.

mod sqlite_generated_support;
mod sqlite_support;

use jobniac_core::clock::truncate_to_micros;
use jobniac_core::errors::AppError;
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedReservation, GenerationDescriptor, GenerationIntent, GenerationPurpose,
    GenerationState, IntentReservation, sha256_hex,
};
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::sqlite::error::SqliteError;
use jobniac_infra::sqlite::generated::IntentError;
use jobniac_infra::sqlite::generated::assets::retire_asset;
use jobniac_infra::sqlite::generated::bindings::{ASSET_COLUMNS, AssetRow};
use jobniac_infra::sqlite::generated::guards::{check_birth_intents, guard_intent_insert};
use jobniac_infra::sqlite::generated::intents::{
    abandon_generation_intent, admit_generation_claim, complete_generation_intent,
    find_generation_intent, insert_generation_intent, renew_generation_intent,
};
use jobniac_infra::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};
use rusqlite::{Connection, params};
use sqlite_generated_support::stored;
use sqlite_support::{SqliteFixture, TestResult};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Synthetic PDF bytes for the owned asset.
const PDF: &[u8] = b"%PDF-1.7\n% synthetic generated resume\n";

/// The database failure of a raw fixture statement, kept as an opaque internal error.
fn fixture_failure(_: SqliteError) -> IntentError {
    IntentError::Database(AppError::Internal)
}

/// Inserts a root, a blob, its document and a RESERVED asset in the current transaction.
fn insert_asset_rows(
    conn: &Connection,
    now: OffsetDateTime,
    content: &[u8],
    classified: bool,
) -> Result<GeneratedReservation, rusqlite::Error> {
    let root = Uuid::new_v4();
    let blob = Uuid::new_v4();
    let document = Uuid::new_v4();
    let asset_id = Uuid::new_v4();
    let digest = sha256_hex(content);
    let size = i64::try_from(content.len()).unwrap_or(i64::MAX);
    let at = stored(now);
    conn.execute(
        "INSERT INTO jobniac_generated_asset_roots (id, created_at) VALUES (?1, ?2)",
        params![root.to_string(), at],
    )?;
    conn.execute(
        "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![blob.to_string(), digest, size, content, at],
    )?;
    conn.execute(
        "INSERT INTO document_documents (id, blob_id, original_file_name, media_type, \
         created_at, updated_at) VALUES (?1, ?2, 'synthetic-resume.pdf', 'application/pdf', ?3, ?3)",
        params![document.to_string(), blob.to_string(), at],
    )?;
    conn.execute(
        "INSERT INTO jobniac_generated_assets (asset_id, root_id, relative_path, sha256, \
         size_bytes, document_id, lifecycle, created_at, generation_intent_required) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'RESERVED', ?7, ?8)",
        params![
            asset_id.to_string(),
            root.to_string(),
            format!("{asset_id}.pdf"),
            digest,
            size,
            document.to_string(),
            at,
            classified
        ],
    )?;
    Ok(GeneratedReservation {
        asset: AssetIdentity::new(asset_id, root, &digest, content.len() as u64)
            .expect("a valid identity"),
        document_id: document,
    })
}

/// Inserts a classified asset and its birth intent under `descriptor`, in the current transaction.
async fn birth_described(
    tx: &mut SqliteTransaction,
    now: OffsetDateTime,
    token: Uuid,
    descriptor: &GenerationDescriptor,
) -> Result<IntentReservation, IntentError> {
    let reservation = tx
        .run(|conn| Ok(insert_asset_rows(conn, now, PDF, true)))
        .map_err(fixture_failure)?
        .map_err(|error| fixture_failure(error.into()))?;
    insert_generation_intent(tx, &reservation, descriptor, token, now).await
}

async fn birth(
    tx: &mut SqliteTransaction,
    now: OffsetDateTime,
    token: Uuid,
    operation: Uuid,
) -> Result<IntentReservation, IntentError> {
    birth_described(tx, now, token, &standalone(operation)?).await
}

fn standalone(operation: Uuid) -> Result<GenerationDescriptor, IntentError> {
    GenerationDescriptor::new(
        operation,
        GenerationPurpose::StandalonePdf,
        None,
        None,
        None,
        None,
    )
    .map_err(IntentError::from)
}

fn options() -> TransactionOptions {
    TransactionOptions {
        read_only: false,
        requires_new: false,
    }
}

fn now() -> OffsetDateTime {
    truncate_to_micros(OffsetDateTime::now_utc())
}

/// Inserts an unclassified RESERVED asset in the current transaction. The asset has no birth intent,
/// so no classification can adopt it.
async fn reserve_unclassified(
    tx: &mut SqliteTransaction,
    now: OffsetDateTime,
    content: &[u8],
) -> Result<GeneratedReservation, IntentError> {
    tx.run(|conn| Ok(insert_asset_rows(conn, now, content, false)))
        .map_err(fixture_failure)?
        .map_err(|error| fixture_failure(error.into()))
}

/// Synthetic PDF bytes unique to `label`, so that blobs of one database never share a digest.
fn pdf_bytes(label: &str) -> Vec<u8> {
    format!("%PDF-1.7\n% synthetic {label} {}\n", Uuid::new_v4()).into_bytes()
}

/// The stored intent of an asset, read in a fresh transaction.
async fn stored_intent(
    uow: &SqliteUnitOfWork,
    asset_id: Uuid,
) -> Result<Option<GenerationIntent>, IntentError> {
    uow.transaction(options(), async |tx| {
        find_generation_intent(tx, asset_id, false).await
    })
    .await
}

/// Runs one raw statement and reports whether the database refused it.
fn refused(fixture: &SqliteFixture, sql: &str, params: &[&dyn rusqlite::ToSql]) -> bool {
    fixture.connection().execute(sql, params).is_err()
}

/// Marks the asset of a claim ACTIVE, as the materialization step does.
fn activate_asset(
    tx: &SqliteTransaction,
    asset_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), IntentError> {
    tx.run(|conn| {
        conn.execute(
            "UPDATE jobniac_generated_assets SET lifecycle = 'ACTIVE', materialized_at = ?2 \
             WHERE asset_id = ?1",
            params![asset_id.to_string(), stored(now)],
        )?;
        Ok(())
    })
    .map_err(fixture_failure)
}

#[tokio::test]
async fn birth_intent_is_admitted_renewed_and_fences_the_stale_token() -> TestResult {
    let uow = SqliteFixture::start().unit_of_work();
    let now = now();
    let operation = Uuid::new_v4();
    let token = Uuid::new_v4();
    let claim = uow
        .transaction(options(), async |tx| birth(tx, now, token, operation).await)
        .await?;
    assert_eq!(claim.intent.state, GenerationState::Pending);

    let descriptor = standalone(operation)?;
    uow.transaction(options(), async |tx| {
        admit_generation_claim(tx, &claim, &descriptor, now + Duration::seconds(1)).await
    })
    .await?;

    let renewed_token = Uuid::new_v4();
    let renewed = uow
        .transaction(options(), async |tx| {
            renew_generation_intent(tx, &claim, renewed_token, now + Duration::seconds(10)).await
        })
        .await?;
    assert_eq!(renewed.intent.lease_token, Some(renewed_token));

    // The superseded fence no longer admits the claim.
    let stale = uow
        .transaction(options(), async |tx| {
            admit_generation_claim(tx, &claim, &descriptor, now + Duration::seconds(11)).await
        })
        .await;
    assert!(matches!(stale, Err(IntentError::Unavailable)));

    let live = uow
        .transaction(options(), async |tx| {
            admit_generation_claim(tx, &renewed, &descriptor, now + Duration::seconds(12)).await
        })
        .await?;
    assert_eq!(live.lease_token, Some(renewed_token));
    Ok(())
}

#[tokio::test]
async fn expired_lease_is_refused_for_live_admission_but_abandonable() -> TestResult {
    let uow = SqliteFixture::start().unit_of_work();
    let now = now();
    let operation = Uuid::new_v4();
    let claim = uow
        .transaction(options(), async |tx| {
            birth(tx, now, Uuid::new_v4(), operation).await
        })
        .await?;
    let descriptor = standalone(operation)?;
    let expired = uow
        .transaction(options(), async |tx| {
            admit_generation_claim(tx, &claim, &descriptor, now + Duration::minutes(6)).await
        })
        .await;
    assert!(matches!(expired, Err(IntentError::Unavailable)));

    let abandoned = uow
        .transaction(options(), async |tx| {
            let Some((intent, row)) =
                abandon_generation_intent(tx, &claim, now + Duration::minutes(6)).await?
            else {
                return Err(IntentError::Unavailable);
            };
            retire_asset(tx, &row, now + Duration::minutes(6), false, None).await?;
            Ok(intent)
        })
        .await?;
    assert_eq!(abandoned.state, GenerationState::Abandoned);

    let stored = stored_intent(&uow, claim.asset_id())
        .await?
        .ok_or("the abandoned intent must remain as history")?;
    assert_eq!(stored.state, GenerationState::Abandoned);
    Ok(())
}

#[tokio::test]
async fn an_abandonment_without_retirement_is_refused_at_commit() -> TestResult {
    let uow = SqliteFixture::start().unit_of_work();
    let now = now();
    let claim = uow
        .transaction(options(), async |tx| {
            birth(tx, now, Uuid::new_v4(), Uuid::new_v4()).await
        })
        .await?;
    let orphan = uow
        .transaction(options(), async |tx| {
            abandon_generation_intent(tx, &claim, now).await.map(|_| ())
        })
        .await;
    assert!(
        matches!(
            orphan,
            Err(IntentError::Database(
                AppError::PersistenceConstraint { .. }
            ))
        ),
        "{orphan:?}"
    );
    let stored = stored_intent(&uow, claim.asset_id())
        .await?
        .ok_or("the birth intent")?;
    assert_eq!(
        stored.state,
        GenerationState::Pending,
        "the refused commit rolls the abandonment back"
    );
    Ok(())
}

#[tokio::test]
async fn unclassified_reservation_cannot_adopt_an_intent() -> TestResult {
    let uow = SqliteFixture::start().unit_of_work();
    let now = now();
    let operation = Uuid::new_v4();
    let unclassified = uow
        .transaction(options(), async |tx| {
            reserve_unclassified(tx, now, &pdf_bytes("unclassified")).await
        })
        .await?;
    assert!(
        stored_intent(&uow, unclassified.asset.asset_id())
            .await?
            .is_none()
    );

    let descriptor = standalone(operation)?;
    let adopted = uow
        .transaction(options(), async |tx| {
            insert_generation_intent(tx, &unclassified, &descriptor, Uuid::new_v4(), now).await
        })
        .await;
    assert!(matches!(adopted, Err(IntentError::Unavailable)));
    assert!(
        stored_intent(&uow, unclassified.asset.asset_id())
            .await?
            .is_none()
    );

    // The intent guard refuses the same intent when it is applied directly.
    let pending = GenerationIntent::pending(
        unclassified.asset.asset_id(),
        descriptor,
        Uuid::new_v4(),
        now,
    )?;
    let guarded = uow
        .transaction(options(), async |tx| {
            tx.run(|conn| guard_intent_insert(conn, &pending))
        })
        .await;
    assert!(matches!(
        guarded,
        Err(SqliteError::Constraint { sqlstate, constraint: None }) if sqlstate == "23514"
    ));
    Ok(())
}

#[tokio::test]
async fn birth_classification_is_immutable_and_a_classified_birth_needs_its_intent() -> TestResult {
    let fixture = SqliteFixture::start();
    let uow = fixture.unit_of_work();
    let now = now();
    let unclassified = uow
        .transaction(options(), async |tx| {
            reserve_unclassified(tx, now, &pdf_bytes("birth")).await
        })
        .await?;
    assert!(refused(
        &fixture,
        "UPDATE jobniac_generated_assets SET generation_intent_required = 1 WHERE asset_id = ?1",
        &[&unclassified.asset.asset_id().to_string()],
    ));

    // A classified birth without its intent is refused when the transaction commits.
    let lone = uow
        .transaction(options(), async |tx| {
            let reservation = tx
                .run(|conn| Ok(insert_asset_rows(conn, now, &pdf_bytes("lone"), true)))
                .map_err(fixture_failure)?
                .map_err(|error| fixture_failure(error.into()))?;
            let asset_id = reservation.asset.asset_id();
            tx.before_commit(Box::new(move |conn| check_birth_intents(conn, &[asset_id])));
            Ok::<GeneratedReservation, IntentError>(reservation)
        })
        .await;
    assert!(matches!(
        lone,
        Err(IntentError::Database(
            AppError::PersistenceConstraint { .. }
        ))
    ));
    Ok(())
}

#[tokio::test]
async fn an_operation_has_one_standalone_output_and_the_descriptor_is_immutable() -> TestResult {
    let fixture = SqliteFixture::start();
    let uow = fixture.unit_of_work();
    let now = now();
    let operation = Uuid::new_v4();
    let claim = uow
        .transaction(options(), async |tx| {
            birth(tx, now, Uuid::new_v4(), operation).await
        })
        .await?;
    let second = uow
        .transaction(options(), async |tx| {
            birth(tx, now, Uuid::new_v4(), operation).await
        })
        .await;
    assert!(second.is_err());
    let asset = claim.asset_id();
    let stored = stored_intent(&uow, asset)
        .await?
        .ok_or("the birth intent")?;
    assert_eq!(stored.descriptor, standalone(operation)?);

    assert!(refused(
        &fixture,
        "UPDATE jobniac_generation_intents SET operation_id = ?1 WHERE asset_id = ?2",
        &[&Uuid::new_v4().to_string(), &asset.to_string()],
    ));
    assert!(refused(
        &fixture,
        "UPDATE jobniac_generation_intents SET created_at = '2000-01-01T00:00:00.000000Z' \
         WHERE asset_id = ?1",
        &[&asset.to_string()],
    ));
    assert_eq!(stored_intent(&uow, asset).await?, Some(stored));
    Ok(())
}

#[tokio::test]
async fn standalone_completion_waits_for_activation_and_completed_history_is_immutable()
-> TestResult {
    let fixture = SqliteFixture::start();
    let uow = fixture.unit_of_work();
    let now = now();
    let operation = Uuid::new_v4();
    let claim = uow
        .transaction(options(), async |tx| {
            birth(tx, now, Uuid::new_v4(), operation).await
        })
        .await?;
    let descriptor = standalone(operation)?;
    let asset = claim.asset_id();

    // The reserved output is not published, so the binding check refuses completion and nothing
    // changes. The caller maps the binding refusal to its own unavailable or validation error.
    let early = uow
        .transaction(options(), async |tx| {
            complete_generation_intent(tx, &claim, &descriptor, now).await
        })
        .await;
    assert!(matches!(early, Err(IntentError::Binding)));
    let pending = stored_intent(&uow, asset)
        .await?
        .ok_or("the birth intent")?;
    assert_eq!(pending.state, GenerationState::Pending);

    let completed = uow
        .transaction(options(), async |tx| {
            activate_asset(tx, asset, now)?;
            complete_generation_intent(tx, &claim, &descriptor, now).await
        })
        .await?;
    assert_eq!(completed.state, GenerationState::Completed);
    assert_eq!(completed.lease_token, None);
    assert_eq!(completed.finished_at, Some(now));

    // Completed history cannot be revived, moved, or deleted.
    let key = asset.to_string();
    assert!(refused(
        &fixture,
        "UPDATE jobniac_generation_intents SET state = 'PENDING' WHERE asset_id = ?1",
        &[&key],
    ));
    assert!(refused(
        &fixture,
        "UPDATE jobniac_generation_intents SET finished_at = '2999-01-01T00:00:00.000000Z' \
         WHERE asset_id = ?1",
        &[&key],
    ));
    assert!(refused(
        &fixture,
        "DELETE FROM jobniac_generation_intents WHERE asset_id = ?1",
        &[&key],
    ));
    assert_eq!(stored_intent(&uow, asset).await?, Some(completed));
    Ok(())
}

#[tokio::test]
async fn expired_claim_cannot_renew_or_complete_and_a_pending_output_cannot_retire() -> TestResult {
    let uow = SqliteFixture::start().unit_of_work();
    let now = now();
    let operation = Uuid::new_v4();
    let claim = uow
        .transaction(options(), async |tx| {
            birth(tx, now, Uuid::new_v4(), operation).await
        })
        .await?;
    let descriptor = standalone(operation)?;
    let asset = claim.asset_id();
    let expired_at = now + Duration::minutes(6);
    let before = stored_intent(&uow, asset).await?;

    let renewed = uow
        .transaction(options(), async |tx| {
            renew_generation_intent(tx, &claim, Uuid::new_v4(), expired_at).await
        })
        .await;
    assert!(matches!(renewed, Err(IntentError::Unavailable)));
    let completed = uow
        .transaction(options(), async |tx| {
            activate_asset(tx, asset, now)?;
            complete_generation_intent(tx, &claim, &descriptor, expired_at).await
        })
        .await;
    assert!(matches!(completed, Err(IntentError::Unavailable)));
    assert_eq!(stored_intent(&uow, asset).await?, before);

    // A pending output cannot be retired without the abandonment that fences it.
    let retired = uow
        .transaction(options(), async |tx| {
            let row = tx
                .run(|conn| {
                    Ok(conn.query_row(
                        &format!(
                            "SELECT {ASSET_COLUMNS} FROM jobniac_generated_assets \
                             WHERE asset_id = ?1"
                        ),
                        [asset.to_string()],
                        |row| Ok(AssetRow::from_row(row)),
                    ))
                })
                .map_err(fixture_failure)?
                .map_err(|error| fixture_failure(error.into()))??;
            retire_asset(tx, &row, now, false, None).await
        })
        .await;
    assert!(matches!(
        retired,
        Err(IntentError::Database(
            AppError::PersistenceConstraint { .. }
        ))
    ));
    Ok(())
}

#[tokio::test]
async fn rotation_fences_the_old_claim_and_expired_abandonment_cannot_override_it() -> TestResult {
    let uow = SqliteFixture::start().unit_of_work();
    let now = now();
    let operation = Uuid::new_v4();
    let claim = uow
        .transaction(options(), async |tx| {
            birth(tx, now, Uuid::new_v4(), operation).await
        })
        .await?;
    let descriptor = standalone(operation)?;
    let renewed = uow
        .transaction(options(), async |tx| {
            renew_generation_intent(tx, &claim, Uuid::new_v4(), now + Duration::minutes(1)).await
        })
        .await?;
    assert_eq!(
        renewed.intent.lease_expires_at,
        Some(now + Duration::minutes(6))
    );
    assert_ne!(renewed.intent.lease_token, claim.intent.lease_token);
    let before = stored_intent(&uow, claim.asset_id()).await?;

    let stale_admit = uow
        .transaction(options(), async |tx| {
            admit_generation_claim(tx, &claim, &descriptor, now).await
        })
        .await;
    assert!(matches!(stale_admit, Err(IntentError::Unavailable)));
    let old_expiry = claim.intent.lease_expires_at.ok_or("the birth lease")?;
    let stale_abandon = uow
        .transaction(options(), async |tx| {
            abandon_generation_intent(tx, &claim, old_expiry).await
        })
        .await;
    assert!(matches!(stale_abandon, Err(IntentError::Unavailable)));
    assert_eq!(stored_intent(&uow, claim.asset_id()).await?, before);
    Ok(())
}

#[tokio::test]
async fn continuation_requires_the_exact_persisted_binding_and_fence() -> TestResult {
    let uow = SqliteFixture::start().unit_of_work();
    let now = now();
    let operation = Uuid::new_v4();
    let claim = uow
        .transaction(options(), async |tx| {
            birth(tx, now, Uuid::new_v4(), operation).await
        })
        .await?;
    let descriptor = standalone(operation)?;
    let before = stored_intent(&uow, claim.asset_id()).await?;

    let mut operation_changed = claim.clone();
    operation_changed.intent.descriptor = standalone(Uuid::new_v4())?;
    let mut token_changed = claim.clone();
    token_changed.intent.lease_token = Some(Uuid::new_v4());
    let mut root_changed = claim.clone();
    root_changed.reservation.asset = AssetIdentity::new(
        claim.asset_id(),
        Uuid::new_v4(),
        claim.reservation.asset.sha256(),
        claim.reservation.asset.size_bytes(),
    )?;
    let mut document_changed = claim.clone();
    document_changed.reservation.document_id = Uuid::new_v4();
    for mismatch in [
        operation_changed,
        token_changed,
        root_changed,
        document_changed,
    ] {
        let admitted = uow
            .transaction(options(), async |tx| {
                admit_generation_claim(tx, &mismatch, &descriptor, now).await
            })
            .await;
        assert!(matches!(admitted, Err(IntentError::Unavailable)));
    }
    assert_eq!(stored_intent(&uow, claim.asset_id()).await?, before);
    Ok(())
}

/// A profile-resume descriptor of `profile` with the master resume scope.
fn profile_resume(operation: Uuid, profile: Uuid) -> Result<GenerationDescriptor, IntentError> {
    GenerationDescriptor::new(
        operation,
        GenerationPurpose::ProfileResume,
        Some(profile),
        None,
        None,
        Some("master_resume".to_owned()),
    )
    .map_err(IntentError::from)
}

/// Inserts one profile resume link row.
fn insert_profile_link(
    tx: &SqliteTransaction,
    profile: Uuid,
    document: Uuid,
    file_path: &str,
    resume_type: &str,
    now: OffsetDateTime,
) -> Result<(), IntentError> {
    tx.run(|conn| {
        conn.execute(
            "INSERT INTO profile_profile_resume_documents \
             (id, profile_id, document_id, file_path, resume_type, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                Uuid::new_v4().to_string(),
                profile.to_string(),
                document.to_string(),
                file_path,
                resume_type,
                stored(now)
            ],
        )?;
        Ok(())
    })
    .map_err(fixture_failure)
}

#[tokio::test]
async fn noop_update_cannot_adopt_a_preexisting_unclassified_asset() -> TestResult {
    let uow = SqliteFixture::start().unit_of_work();
    let now = now();
    let unclassified = uow
        .transaction(options(), async |tx| {
            reserve_unclassified(tx, now, &pdf_bytes("noop-adoption")).await
        })
        .await?;
    let asset = unclassified.asset.asset_id();
    assert!(stored_intent(&uow, asset).await?.is_none());
    let descriptor = standalone(Uuid::new_v4())?;

    // A no-op lifecycle write does not make the asset adoptable.
    let helper = uow
        .transaction(options(), async |tx| {
            tx.run(|conn| {
                conn.execute(
                    "UPDATE jobniac_generated_assets SET lifecycle = lifecycle WHERE asset_id = ?1",
                    [asset.to_string()],
                )?;
                Ok(())
            })
            .map_err(fixture_failure)?;
            insert_generation_intent(tx, &unclassified, &descriptor, Uuid::new_v4(), now)
                .await
                .map(|_| ())
        })
        .await;
    assert!(matches!(helper, Err(IntentError::Unavailable)));
    assert!(stored_intent(&uow, asset).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn intent_descriptor_fields_are_immutable() -> TestResult {
    let fixture = SqliteFixture::start();
    let uow = fixture.unit_of_work();
    let now = now();
    let claim = uow
        .transaction(options(), async |tx| {
            birth(tx, now, Uuid::new_v4(), Uuid::new_v4()).await
        })
        .await?;
    let asset = claim.asset_id();
    let before = stored_intent(&uow, asset).await?;
    let other = Uuid::new_v4().to_string();
    for sql in [
        "UPDATE jobniac_generation_intents SET purpose = 'PROFILE_RESUME' WHERE asset_id = ?1",
        "UPDATE jobniac_generation_intents SET profile_id = ?2 WHERE asset_id = ?1",
        "UPDATE jobniac_generation_intents SET job_id = ?2 WHERE asset_id = ?1",
        "UPDATE jobniac_generation_intents SET resume_id = ?2 WHERE asset_id = ?1",
        "UPDATE jobniac_generation_intents SET output_scope = 'master_resume' WHERE asset_id = ?1",
    ] {
        let connection = fixture.connection();
        let outcome = if sql.contains("?2") {
            connection.execute(sql, params![asset.to_string(), other])
        } else {
            connection.execute(sql, params![asset.to_string()])
        };
        assert!(outcome.is_err(), "{sql}");
    }
    assert_eq!(stored_intent(&uow, asset).await?, before);
    Ok(())
}

#[tokio::test]
async fn referenced_pending_output_is_not_abandoned() -> TestResult {
    let fixture = SqliteFixture::start();
    let uow = fixture.unit_of_work();
    let now = now();
    let profile = Uuid::new_v4();
    sqlite_generated_support::seed_profile(&fixture.connection(), profile, 0, now)?;
    let descriptor = profile_resume(Uuid::new_v4(), profile)?;
    let claim = uow
        .transaction::<IntentReservation, IntentError>(options(), async |tx| {
            let claim = birth_described(tx, now, Uuid::new_v4(), &descriptor).await?;
            activate_asset(tx, claim.asset_id(), now)?;
            tx.run(|conn| {
                conn.execute(
                    "INSERT INTO profile_profile_personal_details \
                     (profile_id, photo_document_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
                    params![
                        profile.to_string(),
                        claim.reservation.document_id.to_string(),
                        stored(now)
                    ],
                )?;
                Ok(())
            })
            .map_err(fixture_failure)?;
            Ok(claim)
        })
        .await?;
    let before = stored_intent(&uow, claim.asset_id()).await?;
    let expiry = claim.intent.lease_expires_at.ok_or("the birth lease")?;
    let abandoned = uow
        .transaction(options(), async |tx| {
            abandon_generation_intent(tx, &claim, expiry)
                .await
                .map(|found| found.is_some())
        })
        .await?;
    assert!(!abandoned, "a referenced pending output is never abandoned");
    assert_eq!(stored_intent(&uow, claim.asset_id()).await?, before);
    Ok(())
}

#[tokio::test]
async fn scoped_completion_requires_the_exact_profile_link() -> TestResult {
    let fixture = SqliteFixture::start();
    let uow = fixture.unit_of_work();
    let now = now();
    let profile = Uuid::new_v4();
    sqlite_generated_support::seed_profile(&fixture.connection(), profile, 0, now)?;
    let descriptor = profile_resume(Uuid::new_v4(), profile)?;
    let claim = uow
        .transaction(options(), async |tx| {
            birth_described(tx, now, Uuid::new_v4(), &descriptor).await
        })
        .await?;
    let asset = claim.asset_id();
    let before = stored_intent(&uow, asset).await?;
    let file_path = claim.reservation.asset.relative_path().to_owned();

    // Without a link, and with a link for another scope, completion is refused: the guard requires
    // the exact link of the descriptor. Both attempts roll back.
    let no_link = uow
        .transaction::<(), IntentError>(options(), async |tx| {
            activate_asset(tx, asset, now)?;
            complete_generation_intent(tx, &claim, &descriptor, now)
                .await
                .map(|_| ())
        })
        .await;
    assert!(matches!(
        no_link,
        Err(IntentError::Database(
            AppError::PersistenceConstraint { .. }
        ))
    ));
    assert_eq!(stored_intent(&uow, asset).await?, before);
    let other_scope = uow
        .transaction::<(), IntentError>(options(), async |tx| {
            activate_asset(tx, asset, now)?;
            insert_profile_link(
                tx,
                profile,
                claim.reservation.document_id,
                &file_path,
                "canadian_resume",
                now,
            )?;
            complete_generation_intent(tx, &claim, &descriptor, now)
                .await
                .map(|_| ())
        })
        .await;
    assert!(matches!(
        other_scope,
        Err(IntentError::Database(
            AppError::PersistenceConstraint { .. }
        ))
    ));
    assert_eq!(stored_intent(&uow, asset).await?, before);

    let completed = uow
        .transaction(options(), async |tx| {
            activate_asset(tx, asset, now)?;
            insert_profile_link(
                tx,
                profile,
                claim.reservation.document_id,
                &file_path,
                "master_resume",
                now,
            )?;
            complete_generation_intent(tx, &claim, &descriptor, now).await
        })
        .await?;
    assert_eq!(completed.state, GenerationState::Completed);
    assert_eq!(stored_intent(&uow, asset).await?, Some(completed));
    Ok(())
}
