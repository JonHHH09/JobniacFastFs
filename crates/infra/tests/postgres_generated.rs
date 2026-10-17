//! Classified generation intents against a disposable PostgreSQL 18 server: birth insertion in the
//! asset's own transaction, live admission, lease expiry, fencing of a stale token, renewal and
//! abandonment with same-transaction retirement. The server-backed tests skip with a notice when
//! `JOBNIAC_TEST_PG_PORT` is unset. All data is synthetic.

use std::error::Error;

use jobniac_core::clock::truncate_to_micros;
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedReservation, GenerationDescriptor, GenerationIntent, GenerationPurpose,
    GenerationState, IntentReservation, sha256_hex,
};
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::postgres::config::load_database_config;
use jobniac_infra::postgres::generated::IntentError;
use jobniac_infra::postgres::generated::assets::retire_asset;
use jobniac_infra::postgres::generated::bindings::{ASSET_COLUMNS, AssetRow};
use jobniac_infra::postgres::generated::intents::{
    abandon_generation_intent, admit_generation_claim, complete_generation_intent,
    find_generation_intent, insert_generation_intent, renew_generation_intent,
};
use jobniac_infra::postgres::migrations::migrate;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::{PgTransaction, PgUnitOfWork};
use jobniac_testkit::pg::{TestDatabase, TestServer};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

/// Synthetic PDF bytes for the owned asset.
const PDF: &[u8] = b"%PDF-1.7\n% synthetic generated resume\n";

/// A migrated disposable database and the unit of work over it.
struct Fixture {
    _database: TestDatabase,
    uow: PgUnitOfWork,
}

/// A fresh migrated database, or `None` without a configured server.
async fn start() -> Result<Option<Fixture>, Box<dyn Error>> {
    let Some(server) = TestServer::from_env() else {
        assert!(
            std::env::var_os("CI").is_none(),
            "JOBNIAC_TEST_PG_PORT must be set in CI so that the generated PostgreSQL tests run"
        );
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(None);
    };
    let database = server.fresh_database().await;
    let config = load_database_config(&database.environment())
        .ok_or("the disposable database configuration is invalid")?;
    migrate(&config).await.map_err(|error| error.0)?;
    let uow = PgUnitOfWork::new(PgPool::new(Some(config)));
    Ok(Some(Fixture {
        _database: database,
        uow,
    }))
}

/// The database failure of a raw fixture statement, kept as an opaque internal error.
fn fixture_failure(_: tokio_postgres::Error) -> IntentError {
    IntentError::Database(jobniac_core::errors::AppError::Internal)
}

/// Inserts a root, a blob, its document and a RESERVED classified asset in the current
/// transaction, then the birth intent. Returns the reservation and the pending claim.
async fn birth(
    tx: &mut jobniac_infra::postgres::unit_of_work::PgTransaction,
    now: OffsetDateTime,
    token: Uuid,
    operation: Uuid,
) -> Result<IntentReservation, IntentError> {
    birth_described(tx, now, token, &standalone(operation)?).await
}

/// Inserts a root, a blob, its document and a RESERVED classified asset with an intent under
/// `descriptor`, in the current transaction.
async fn birth_described(
    tx: &mut jobniac_infra::postgres::unit_of_work::PgTransaction,
    now: OffsetDateTime,
    token: Uuid,
    descriptor: &GenerationDescriptor,
) -> Result<IntentReservation, IntentError> {
    let root = Uuid::new_v4();
    let blob = Uuid::new_v4();
    let document = Uuid::new_v4();
    let asset_id = Uuid::new_v4();
    let digest = sha256_hex(PDF);
    let size = i64::try_from(PDF.len()).map_err(|_| IntentError::Invalid)?;
    let client = tx.client();
    client
        .execute(
            "INSERT INTO jobniac.generated_asset_roots (id, created_at) VALUES ($1, $2)",
            &[&root, &now],
        )
        .await
        .map_err(fixture_failure)?;
    client
        .execute(
            "INSERT INTO document.blobs (id, sha256, byte_size, content, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
            &[&blob, &digest, &size, &PDF, &now],
        )
        .await
        .map_err(fixture_failure)?;
    client
        .execute(
            "INSERT INTO document.documents (id, blob_id, original_file_name, media_type, \
             created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $5)",
            &[
                &document,
                &blob,
                &"synthetic-resume.pdf",
                &"application/pdf",
                &now,
            ],
        )
        .await
        .map_err(fixture_failure)?;
    client
        .execute(
            "INSERT INTO jobniac.generated_assets (asset_id, root_id, relative_path, sha256, \
             size_bytes, document_id, lifecycle, created_at, generation_intent_required) \
             VALUES ($1, $2, $3, $4, $5, $6, 'RESERVED', $7, true)",
            &[
                &asset_id,
                &root,
                &format!("{asset_id}.pdf"),
                &digest,
                &size,
                &document,
                &now,
            ],
        )
        .await
        .map_err(fixture_failure)?;
    let identity = AssetIdentity::new(asset_id, root, &digest, PDF.len() as u64)
        .map_err(|_| IntentError::Invalid)?;
    let reservation = GeneratedReservation {
        asset: identity,
        document_id: document,
    };
    insert_generation_intent(tx, &reservation, descriptor, token, now).await
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

#[tokio::test]
async fn birth_intent_is_admitted_renewed_and_fences_the_stale_token() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
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
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
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

    let stored = uow
        .transaction(options(), async |tx| {
            find_generation_intent(tx, claim.asset_id(), false).await
        })
        .await?
        .ok_or("the abandoned intent must remain as history")?;
    assert_eq!(stored.state, GenerationState::Abandoned);
    Ok(())
}

/// Inserts a root, a blob, its document and an unclassified RESERVED asset in the current
/// transaction. The asset has no birth intent, so no classification can adopt it.
async fn reserve_unclassified(
    tx: &mut jobniac_infra::postgres::unit_of_work::PgTransaction,
    now: OffsetDateTime,
    content: &[u8],
) -> Result<GeneratedReservation, IntentError> {
    let root = Uuid::new_v4();
    let blob = Uuid::new_v4();
    let document = Uuid::new_v4();
    let asset_id = Uuid::new_v4();
    let digest = sha256_hex(content);
    let size = i64::try_from(content.len()).map_err(|_| IntentError::Invalid)?;
    let client = tx.client();
    client
        .execute(
            "INSERT INTO jobniac.generated_asset_roots (id, created_at) VALUES ($1, $2)",
            &[&root, &now],
        )
        .await
        .map_err(fixture_failure)?;
    client
        .execute(
            "INSERT INTO document.blobs (id, sha256, byte_size, content, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
            &[&blob, &digest, &size, &content, &now],
        )
        .await
        .map_err(fixture_failure)?;
    client
        .execute(
            "INSERT INTO document.documents (id, blob_id, original_file_name, media_type, \
             created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $5)",
            &[
                &document,
                &blob,
                &"synthetic-resume.pdf",
                &"application/pdf",
                &now,
            ],
        )
        .await
        .map_err(fixture_failure)?;
    client
        .execute(
            "INSERT INTO jobniac.generated_assets (asset_id, root_id, relative_path, sha256, \
             size_bytes, document_id, lifecycle, created_at, generation_intent_required) \
             VALUES ($1, $2, $3, $4, $5, $6, 'RESERVED', $7, false)",
            &[
                &asset_id,
                &root,
                &format!("{asset_id}.pdf"),
                &digest,
                &size,
                &document,
                &now,
            ],
        )
        .await
        .map_err(fixture_failure)?;
    let identity = AssetIdentity::new(asset_id, root, &digest, content.len() as u64)
        .map_err(|_| IntentError::Invalid)?;
    Ok(GeneratedReservation {
        asset: identity,
        document_id: document,
    })
}

/// Synthetic PDF bytes unique to `label`, so that blobs of one database never share a digest.
fn pdf_bytes(label: &str) -> Vec<u8> {
    format!("%PDF-1.7\n% synthetic {label} {}\n", Uuid::new_v4()).into_bytes()
}

/// The stored intent of an asset, read in a fresh transaction.
async fn stored_intent(
    uow: &PgUnitOfWork,
    asset_id: Uuid,
) -> Result<Option<GenerationIntent>, IntentError> {
    uow.transaction(options(), async |tx| {
        find_generation_intent(tx, asset_id, false).await
    })
    .await
}

/// Runs one raw statement in its own transaction and reports whether PostgreSQL refused it.
async fn refused(uow: &PgUnitOfWork, sql: &str, params: &[Uuid]) -> bool {
    let outcome = uow
        .transaction(options(), async |tx| {
            let values: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params
                .iter()
                .map(|value| value as &(dyn tokio_postgres::types::ToSql + Sync))
                .collect();
            tx.client()
                .execute(sql, &values)
                .await
                .map(|_| ())
                .map_err(fixture_failure)
        })
        .await;
    outcome.is_err()
}

/// Marks the asset of a claim ACTIVE, as the materialization step does.
async fn activate_asset(
    tx: &mut PgTransaction,
    asset_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), IntentError> {
    tx.client()
        .execute(
            "UPDATE jobniac.generated_assets SET lifecycle = 'ACTIVE', materialized_at = $2 \
             WHERE asset_id = $1",
            &[&asset_id, &now],
        )
        .await
        .map_err(fixture_failure)?;
    Ok(())
}

#[tokio::test]
async fn unclassified_reservation_cannot_adopt_an_intent() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
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

    // A raw intent row for the same asset is refused by the database as well.
    let asset = unclassified.asset.asset_id();
    assert!(
        refused(
            &uow,
            "INSERT INTO jobniac.generation_intents (asset_id, operation_id, purpose, state, \
             lease_token, lease_started_at, lease_expires_at, created_at) \
             VALUES ($1, $2, 'STANDALONE_PDF', 'PENDING', $2, $3, $3, $3)",
            &[asset, operation],
        )
        .await
    );
    Ok(())
}

#[tokio::test]
async fn birth_classification_is_immutable_and_a_classified_birth_needs_its_intent() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
    let unclassified = uow
        .transaction(options(), async |tx| {
            reserve_unclassified(tx, now, &pdf_bytes("birth")).await
        })
        .await?;
    assert!(
        refused(
            &uow,
            "UPDATE jobniac.generated_assets SET generation_intent_required = true \
             WHERE asset_id = $1",
            &[unclassified.asset.asset_id()],
        )
        .await
    );

    // A classified birth without its intent is refused when the transaction commits.
    let lone = uow
        .transaction(options(), async |tx| {
            let reservation = reserve_unclassified(tx, now, &pdf_bytes("lone")).await?;
            tx.client()
                .execute(
                    "UPDATE jobniac.generated_assets SET generation_intent_required = true \
                     WHERE asset_id = $1",
                    &[&reservation.asset.asset_id()],
                )
                .await
                .map_err(fixture_failure)?;
            Ok::<GeneratedReservation, IntentError>(reservation)
        })
        .await;
    assert!(lone.is_err());
    Ok(())
}

#[tokio::test]
async fn an_operation_has_one_standalone_output_and_the_descriptor_is_immutable() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
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

    assert!(
        refused(
            &uow,
            "UPDATE jobniac.generation_intents SET operation_id = $1 WHERE asset_id = $2",
            &[Uuid::new_v4(), asset],
        )
        .await
    );
    assert!(
        refused(
            &uow,
            "UPDATE jobniac.generation_intents SET created_at = created_at - interval '1 second' \
             WHERE asset_id = $1",
            &[asset],
        )
        .await
    );
    assert_eq!(stored_intent(&uow, asset).await?, Some(stored));
    Ok(())
}

#[tokio::test]
async fn standalone_completion_waits_for_activation_and_completed_history_is_immutable()
-> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
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
            activate_asset(tx, asset, now).await?;
            complete_generation_intent(tx, &claim, &descriptor, now).await
        })
        .await?;
    assert_eq!(completed.state, GenerationState::Completed);
    assert_eq!(completed.lease_token, None);
    assert_eq!(completed.finished_at, Some(now));

    // Completed history cannot be revived, moved, or deleted.
    assert!(
        refused(
            &uow,
            "UPDATE jobniac.generation_intents SET state = 'PENDING' WHERE asset_id = $1",
            &[asset],
        )
        .await
    );
    assert!(
        refused(
            &uow,
            "UPDATE jobniac.generation_intents SET finished_at = finished_at + interval '1 second' \
             WHERE asset_id = $1",
            &[asset],
        )
        .await
    );
    assert!(
        refused(
            &uow,
            "DELETE FROM jobniac.generation_intents WHERE asset_id = $1",
            &[asset],
        )
        .await
    );
    assert_eq!(stored_intent(&uow, asset).await?, Some(completed));
    Ok(())
}

#[tokio::test]
async fn expired_claim_cannot_renew_or_complete_and_a_pending_output_cannot_retire() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
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
            activate_asset(tx, asset, now).await?;
            complete_generation_intent(tx, &claim, &descriptor, expired_at).await
        })
        .await;
    assert!(matches!(completed, Err(IntentError::Unavailable)));
    assert_eq!(stored_intent(&uow, asset).await?, before);

    // A pending output cannot be retired without the abandonment that fences it.
    let retired = uow
        .transaction(options(), async |tx| {
            let row = tx
                .client()
                .query_one(
                    &format!(
                        "SELECT {ASSET_COLUMNS} FROM jobniac.generated_assets \
                         WHERE asset_id = $1 FOR UPDATE"
                    ),
                    &[&asset],
                )
                .await
                .map_err(fixture_failure)?;
            let row = AssetRow::from_row(&row)?;
            retire_asset(tx, &row, now, false, None).await
        })
        .await;
    assert!(retired.is_err());
    Ok(())
}

#[tokio::test]
async fn rotation_fences_the_old_claim_and_expired_abandonment_cannot_override_it() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
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
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
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

/// Inserts a synthetic profile owner with the given identity (a profile row).
async fn insert_profile(
    admin: &tokio_postgres::Client,
    profile: Uuid,
    now: OffsetDateTime,
) -> Result<(), Box<dyn Error>> {
    admin
        .execute(
            "INSERT INTO profile.profiles (id, full_name, email, created_at, updated_at) \
             VALUES ($1, 'Synthetic intent owner', 'intent-owner@example.test', $2, $2)",
            &[&profile, &now],
        )
        .await?;
    Ok(())
}

/// Inserts one profile resume link row.
async fn insert_profile_link(
    tx: &mut PgTransaction,
    profile: Uuid,
    document: Uuid,
    file_path: &str,
    resume_type: &str,
    now: OffsetDateTime,
) -> Result<(), IntentError> {
    tx.client()
        .execute(
            "INSERT INTO profile.profile_resume_documents \
             (id, profile_id, document_id, file_path, resume_type, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $6)",
            &[
                &Uuid::new_v4(),
                &profile,
                &document,
                &file_path,
                &resume_type,
                &now,
            ],
        )
        .await
        .map_err(fixture_failure)?;
    Ok(())
}

#[tokio::test]
async fn noop_update_cannot_adopt_a_preexisting_unclassified_asset() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
    let unclassified = uow
        .transaction(options(), async |tx| {
            reserve_unclassified(tx, now, &pdf_bytes("noop-adoption")).await
        })
        .await?;
    let asset = unclassified.asset.asset_id();
    let before = stored_intent(&uow, asset).await?;
    assert!(before.is_none());
    let operation = Uuid::new_v4();
    let descriptor = standalone(operation)?;

    // A no-op lifecycle write does not make the asset adoptable.
    let helper = uow
        .transaction(options(), async |tx| {
            tx.client()
                .execute(
                    "UPDATE jobniac.generated_assets SET lifecycle = lifecycle WHERE asset_id = $1",
                    &[&asset],
                )
                .await
                .map_err(fixture_failure)?;
            insert_generation_intent(tx, &unclassified, &descriptor, Uuid::new_v4(), now)
                .await
                .map(|_| ())
        })
        .await;
    assert!(matches!(helper, Err(IntentError::Unavailable)));
    let raw = uow
        .transaction::<(), IntentError>(options(), async |tx| {
            tx.client()
                .execute(
                    "UPDATE jobniac.generated_assets SET lifecycle = lifecycle WHERE asset_id = $1",
                    &[&asset],
                )
                .await
                .map_err(fixture_failure)?;
            tx.client()
                .execute(
                    "INSERT INTO jobniac.generation_intents (asset_id, operation_id, purpose, state, \
                     lease_token, lease_started_at, lease_expires_at, created_at) \
                     VALUES ($1, $2, 'STANDALONE_PDF', 'PENDING', $2, $3, $3, $3)",
                    &[&asset, &operation, &now],
                )
                .await
                .map_err(fixture_failure)?;
            Ok(())
        })
        .await;
    assert!(raw.is_err());
    assert_eq!(stored_intent(&uow, asset).await?, before);
    Ok(())
}

#[tokio::test]
async fn intent_update_does_not_wait_for_the_asset_lock_while_holding_the_intent_row() -> TestResult
{
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow.clone();
    let admin = fixture._database.admin_client().await;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
    let claim = uow
        .transaction(options(), async |tx| {
            birth(tx, now, Uuid::new_v4(), Uuid::new_v4()).await
        })
        .await?;
    let asset = claim.asset_id();
    let before = stored_intent(&uow, asset).await?;

    // Another session holds the asset's cleanup lock. The intent update must be refused at once
    // rather than wait for that lock while it holds the intent row.
    admin
        .execute(
            "SELECT pg_advisory_lock(jobniac.cleanup_asset_lock_key($1))",
            &[&asset],
        )
        .await?;
    let attempt = uow.transaction::<(), IntentError>(options(), async |tx| {
        tx.client()
            .execute(
                "UPDATE jobniac.generation_intents SET lease_token = $1, lease_started_at = $2, \
                 lease_expires_at = $3 WHERE asset_id = $4",
                &[
                    &Uuid::new_v4(),
                    &(now + Duration::seconds(1)),
                    &(now + jobniac_core::generated_assets::GENERATION_LEASE),
                    &asset,
                ],
            )
            .await
            .map_err(fixture_failure)?;
        Ok(())
    });
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(3), attempt).await;
    admin
        .execute(
            "SELECT pg_advisory_unlock(jobniac.cleanup_asset_lock_key($1))",
            &[&asset],
        )
        .await?;
    assert!(
        matches!(outcome, Ok(Err(_))),
        "the update waited for the asset lock"
    );
    assert_eq!(stored_intent(&uow, asset).await?, before);
    Ok(())
}

#[tokio::test]
async fn intent_descriptor_fields_are_immutable() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
    let claim = uow
        .transaction(options(), async |tx| {
            birth(tx, now, Uuid::new_v4(), Uuid::new_v4()).await
        })
        .await?;
    let asset = claim.asset_id();
    let before = stored_intent(&uow, asset).await?;
    for sql in [
        "UPDATE jobniac.generation_intents SET purpose = 'PROFILE_RESUME' WHERE asset_id = $1",
        "UPDATE jobniac.generation_intents SET profile_id = gen_random_uuid() WHERE asset_id = $1",
        "UPDATE jobniac.generation_intents SET job_id = gen_random_uuid() WHERE asset_id = $1",
        "UPDATE jobniac.generation_intents SET resume_id = gen_random_uuid() WHERE asset_id = $1",
        "UPDATE jobniac.generation_intents SET output_scope = 'master_resume' WHERE asset_id = $1",
    ] {
        assert!(refused(&uow, sql, &[asset]).await, "{sql}");
    }
    assert_eq!(stored_intent(&uow, asset).await?, before);
    Ok(())
}

#[tokio::test]
async fn referenced_pending_output_is_not_abandoned() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow.clone();
    let admin = fixture._database.admin_client().await;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
    let profile = Uuid::new_v4();
    insert_profile(&admin, profile, now).await?;
    let descriptor = profile_resume(Uuid::new_v4(), profile)?;
    let claim = uow
        .transaction::<IntentReservation, IntentError>(options(), async |tx| {
            let claim = birth_described(tx, now, Uuid::new_v4(), &descriptor).await?;
            activate_asset(tx, claim.asset_id(), now).await?;
            tx.client()
                .execute(
                    "INSERT INTO profile.profile_personal_details \
                     (profile_id, photo_document_id, created_at, updated_at) VALUES ($1, $2, $3, $3)",
                    &[&profile, &claim.reservation.document_id, &now],
                )
                .await
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
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow.clone();
    let admin = fixture._database.admin_client().await;
    let now = truncate_to_micros(OffsetDateTime::now_utc());
    let profile = Uuid::new_v4();
    insert_profile(&admin, profile, now).await?;
    let descriptor = profile_resume(Uuid::new_v4(), profile)?;
    let claim = uow
        .transaction(options(), async |tx| {
            birth_described(tx, now, Uuid::new_v4(), &descriptor).await
        })
        .await?;
    let asset = claim.asset_id();
    let before = stored_intent(&uow, asset).await?;
    let file_path = claim.reservation.asset.relative_path().to_owned();

    // Without a link, and with a link for another scope, completion is refused: the database
    // requires the exact link of the descriptor. Both attempts roll back.
    let no_link = uow
        .transaction::<(), IntentError>(options(), async |tx| {
            activate_asset(tx, asset, now).await?;
            complete_generation_intent(tx, &claim, &descriptor, now)
                .await
                .map(|_| ())
        })
        .await;
    assert!(no_link.is_err());
    assert_eq!(stored_intent(&uow, asset).await?, before);
    let other_scope = uow
        .transaction::<(), IntentError>(options(), async |tx| {
            activate_asset(tx, asset, now).await?;
            insert_profile_link(
                tx,
                profile,
                claim.reservation.document_id,
                &file_path,
                "canadian_resume",
                now,
            )
            .await?;
            complete_generation_intent(tx, &claim, &descriptor, now)
                .await
                .map(|_| ())
        })
        .await;
    assert!(other_scope.is_err());
    assert_eq!(stored_intent(&uow, asset).await?, before);

    let completed = uow
        .transaction(options(), async |tx| {
            activate_asset(tx, asset, now).await?;
            insert_profile_link(
                tx,
                profile,
                claim.reservation.document_id,
                &file_path,
                "master_resume",
                now,
            )
            .await?;
            complete_generation_intent(tx, &claim, &descriptor, now).await
        })
        .await?;
    assert_eq!(completed.state, GenerationState::Completed);
    assert_eq!(stored_intent(&uow, asset).await?, Some(completed));
    Ok(())
}
