//! Classified intent transactions: insert, admission
//! with leases and fencing, completion, renewal, abandonment and the bounded binding sets.
//!
//! Admission never waits: the document and asset advisory locks are try-locks, and a busy lock is
//! [`IntentError::Unavailable`]. Callers acquire the complete sorted lock set before calling.

use jobniac_core::errors::{AppError, ApplicationError};
use jobniac_core::generated_assets::identity::generation_instant;
use jobniac_core::generated_assets::{
    GENERATION_LEASE, GeneratedReservation, GenerationDescriptor, GenerationIntent,
    GenerationPurpose, GenerationState, IntentReservation, validate_binding_intents,
};
use time::OffsetDateTime;
use tokio_postgres::Row;
use uuid::Uuid;

use super::IntentError;
use super::bindings::{ASSET_COLUMNS, AssetRow, document_lock_key_input, verify_pdf_binding};
use crate::postgres::profiles::{col, db};
use crate::postgres::unit_of_work::PgTransaction;

/// Columns of a generation intent row, in the order the queries select them.
const INTENT_COLUMNS: &str = "asset_id, operation_id, purpose, profile_id, job_id, resume_id, \
    output_scope, state, lease_token, lease_started_at, lease_expires_at, created_at, finished_at";

/// Reads one intent row.
///
/// # Errors
///
/// Fails with a caller-safe database unavailable error when a stored value is not a valid intent.
pub fn intent_from_row(row: &Row) -> Result<GenerationIntent, IntentError> {
    let unavailable = || {
        IntentError::Database(AppError::Application(
            ApplicationError::database_unavailable(),
        ))
    };
    let purpose: String = col(row, "purpose")?;
    let state: String = col(row, "state")?;
    let descriptor = GenerationDescriptor::new(
        col(row, "operation_id")?,
        GenerationPurpose::parse(&purpose).ok_or_else(unavailable)?,
        col(row, "profile_id")?,
        col(row, "job_id")?,
        col(row, "resume_id")?,
        col(row, "output_scope")?,
    )
    .map_err(|_| unavailable())?;
    GenerationIntent::new(
        col(row, "asset_id")?,
        descriptor,
        GenerationState::parse(&state).ok_or_else(unavailable)?,
        col(row, "lease_token")?,
        col(row, "lease_started_at")?,
        col(row, "lease_expires_at")?,
        col(row, "created_at")?,
        col(row, "finished_at")?,
    )
    .map_err(|_| unavailable())
}

/// Takes the document and asset try-locks of one generated asset.
async fn try_locks(
    tx: &mut PgTransaction,
    asset_id: Uuid,
    document_id: Uuid,
) -> Result<(), IntentError> {
    // Stops at the first busy lock, so the asset lock is not tried
    // when the document lock is busy.
    let document = db(tx
        .client()
        .query_one(
            "SELECT pg_try_advisory_xact_lock(hashtextextended($1::text, 0)) AS locked",
            &[&document_lock_key_input(document_id)],
        )
        .await)?;
    if !col::<bool>(&document, "locked")? {
        return Err(IntentError::Unavailable);
    }
    let asset = db(tx
        .client()
        .query_one(
            "SELECT pg_try_advisory_xact_lock(jobniac.cleanup_asset_lock_key($1)) AS locked",
            &[&asset_id],
        )
        .await)?;
    if !col::<bool>(&asset, "locked")? {
        return Err(IntentError::Unavailable);
    }
    Ok(())
}

/// Reads the intent of one asset, optionally with the owner locks and a row lock
///.
///
/// # Errors
///
/// Fails with [`IntentError::Unavailable`] when a try-lock is busy, or [`IntentError::Database`].
pub async fn find_generation_intent(
    tx: &mut PgTransaction,
    asset_id: Uuid,
    for_update: bool,
) -> Result<Option<GenerationIntent>, IntentError> {
    if for_update {
        let asset = db(tx
            .client()
            .query_opt(
                "SELECT document_id FROM jobniac.generated_assets WHERE asset_id = $1",
                &[&asset_id],
            )
            .await)?;
        let document_id: Option<Uuid> = match asset {
            Some(row) => col(&row, "document_id")?,
            None => None,
        };
        let Some(document_id) = document_id else {
            return Ok(None);
        };
        try_locks(tx, asset_id, document_id).await?;
    }
    let sql = format!(
        "SELECT {INTENT_COLUMNS} FROM jobniac.generation_intents WHERE asset_id = $1{}",
        if for_update { " FOR UPDATE" } else { "" }
    );
    let row = db(tx.client().query_opt(&sql, &[&asset_id]).await)?;
    row.as_ref().map(intent_from_row).transpose()
}

/// Locks and reads the asset of a reservation, refusing a row that moved.
async fn locked_asset(
    tx: &mut PgTransaction,
    reservation: &GeneratedReservation,
) -> Result<AssetRow, IntentError> {
    try_locks(tx, reservation.asset.asset_id(), reservation.document_id).await?;
    let row = db(tx
        .client()
        .query_opt(
            &format!("SELECT {ASSET_COLUMNS} FROM jobniac.generated_assets WHERE asset_id = $1 FOR UPDATE"),
            &[&reservation.asset.asset_id()],
        )
        .await)?;
    let row = row.as_ref().map(AssetRow::from_row).transpose()?;
    match row {
        Some(asset)
            if asset.document_id == Some(reservation.document_id)
                && asset.identity == reservation.asset
                && matches!(asset.lifecycle.as_str(), "RESERVED" | "ACTIVE") =>
        {
            Ok(asset)
        }
        _ => Err(IntentError::Unavailable),
    }
}

/// Inserts the birth intent of a reserved asset in the same transaction that created it
///.
///
/// # Errors
///
/// Fails with [`IntentError::Unavailable`] when the asset is not a reserved classified birth in
/// this transaction, or [`IntentError::Invalid`] for an invalid descriptor.
pub async fn insert_generation_intent(
    tx: &mut PgTransaction,
    reservation: &GeneratedReservation,
    descriptor: &GenerationDescriptor,
    lease_token: Uuid,
    now: OffsetDateTime,
) -> Result<IntentReservation, IntentError> {
    descriptor.validate()?;
    let now = generation_instant(now)?;
    let pending = GenerationIntent::pending(
        reservation.asset.asset_id(),
        descriptor.clone(),
        lease_token,
        now,
    )?;
    let asset = locked_asset(tx, reservation).await?;
    if asset.lifecycle != "RESERVED"
        || !asset.generation_intent_required
        || !asset.current_transaction
        || generation_instant(asset.created_at)? != now
    {
        return Err(IntentError::Unavailable);
    }
    let purpose = descriptor.purpose.as_str();
    let state = pending.state.as_str();
    db(tx
        .client()
        .execute(
            &format!(
                "INSERT INTO jobniac.generation_intents ({INTENT_COLUMNS}) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)"
            ),
            &[
                &pending.asset_id,
                &descriptor.operation_id,
                &purpose,
                &descriptor.profile_id,
                &descriptor.job_id,
                &descriptor.resume_id,
                &descriptor.output_scope,
                &state,
                &pending.lease_token,
                &pending.lease_started_at,
                &pending.lease_expires_at,
                &pending.created_at,
                &Option::<OffsetDateTime>::None,
            ],
        )
        .await)?;
    let found = find_generation_intent(tx, pending.asset_id, false).await?;
    if found.as_ref() != Some(&pending) {
        return Err(IntentError::Database(AppError::Application(
            ApplicationError::database_unavailable(),
        )));
    }
    Ok(IntentReservation {
        reservation: reservation.clone(),
        intent: pending,
    })
}

/// Admits a live claim against the stored intent and asset.
async fn admit(
    tx: &mut PgTransaction,
    claim: &IntentReservation,
    expected: &GenerationDescriptor,
    now: OffsetDateTime,
    live: bool,
) -> Result<(GenerationIntent, AssetRow), IntentError> {
    claim.intent.validate()?;
    expected.validate()?;
    let now = generation_instant(now)?;
    if &claim.intent.descriptor != expected || claim.intent.state != GenerationState::Pending {
        return Err(IntentError::Unavailable);
    }
    let asset = locked_asset(tx, &claim.reservation).await?;
    let found = find_generation_intent(tx, claim.asset_id(), true).await?;
    let Some(found) = found else {
        return Err(IntentError::Unavailable);
    };
    let expired = live && found.lease_expires_at.is_none_or(|expires| now >= expires);
    let before_start = found.lease_started_at.is_none_or(|start| now < start);
    if found != claim.intent || found.state != GenerationState::Pending || before_start || expired {
        return Err(IntentError::Unavailable);
    }
    Ok((found, asset))
}

/// Admits a live claim for the caller's descriptor and instant.
///
/// # Errors
///
/// Fails with [`IntentError::Unavailable`] for a stale, expired or foreign claim.
pub async fn admit_generation_claim(
    tx: &mut PgTransaction,
    claim: &IntentReservation,
    expected: &GenerationDescriptor,
    now: OffsetDateTime,
) -> Result<GenerationIntent, IntentError> {
    Ok(admit(tx, claim, expected, now, true).await?.0)
}

/// Completes a live claim after its PDF binding is verified.
/// Completion and the publication or link writes must share the caller's transaction.
///
/// # Errors
///
/// Fails with [`IntentError::Unavailable`] for a stale claim or [`IntentError::Binding`] when the
/// asset's bytes do not match.
pub async fn complete_generation_intent(
    tx: &mut PgTransaction,
    claim: &IntentReservation,
    expected: &GenerationDescriptor,
    now: OffsetDateTime,
) -> Result<GenerationIntent, IntentError> {
    let now = generation_instant(now)?;
    let found = admit_generation_claim(tx, claim, expected, now).await?;
    verify_pdf_binding(tx, &claim.reservation).await?;
    db(tx
        .client()
        .execute(
            "UPDATE jobniac.generation_intents SET state = 'COMPLETED', lease_token = NULL, \
             lease_started_at = NULL, lease_expires_at = NULL, finished_at = $1 WHERE asset_id = $2",
            &[&now, &found.asset_id],
        )
        .await)?;
    let completed = find_generation_intent(tx, found.asset_id, false).await?;
    match completed {
        Some(intent) if intent.state == GenerationState::Completed => Ok(intent),
        _ => Err(IntentError::Database(AppError::Application(
            ApplicationError::database_unavailable(),
        ))),
    }
}

/// Moves the fence of a live claim to `new_token` for a fresh lease. The new expiry may not move
/// backwards.
///
/// # Errors
///
/// Fails with [`IntentError::Invalid`] for a reused token and [`IntentError::Unavailable`] for a
/// stale claim or an expiry that would shrink.
pub async fn renew_generation_intent(
    tx: &mut PgTransaction,
    claim: &IntentReservation,
    new_token: Uuid,
    now: OffsetDateTime,
) -> Result<IntentReservation, IntentError> {
    claim.intent.validate()?;
    if claim.intent.lease_token == Some(new_token) {
        return Err(IntentError::Invalid);
    }
    let now = generation_instant(now)?;
    let (found, _asset) = admit(tx, claim, &claim.intent.descriptor, now, true).await?;
    let expiry = now + GENERATION_LEASE;
    if found
        .lease_expires_at
        .is_some_and(|previous| expiry < previous)
    {
        return Err(IntentError::Unavailable);
    }
    db(tx
        .client()
        .execute(
            "UPDATE jobniac.generation_intents SET lease_token = $1, lease_started_at = $2, \
             lease_expires_at = $3 WHERE asset_id = $4",
            &[&new_token, &now, &expiry, &found.asset_id],
        )
        .await)?;
    let mut renewed = found.clone();
    renewed.lease_token = Some(new_token);
    renewed.lease_started_at = Some(now);
    renewed.lease_expires_at = Some(expiry);
    if find_generation_intent(tx, found.asset_id, false)
        .await?
        .as_ref()
        != Some(&renewed)
    {
        return Err(IntentError::Database(AppError::Application(
            ApplicationError::database_unavailable(),
        )));
    }
    Ok(IntentReservation {
        reservation: claim.reservation.clone(),
        intent: renewed,
    })
}

/// Fences abandonment, including expiry. Returns `None` and
/// changes nothing when the asset is referenced. The caller retires the asset in the same
/// transaction; a deferred constraint rejects an abandoned orphan otherwise.
///
/// # Errors
///
/// Fails with [`IntentError::Unavailable`] for a stale claim.
pub async fn abandon_generation_intent(
    tx: &mut PgTransaction,
    claim: &IntentReservation,
    now: OffsetDateTime,
) -> Result<Option<(GenerationIntent, AssetRow)>, IntentError> {
    let now = generation_instant(now)?;
    let (found, asset) = admit(tx, claim, &claim.intent.descriptor, now, false).await?;
    let document_id = asset.document_id;
    let referenced_row = db(tx
        .client()
        .query_one(
            "SELECT jobniac.generated_asset_referenced($1, $2) AS referenced",
            &[&document_id, &asset.identity.relative_path()],
        )
        .await)?;
    let referenced: bool = col(&referenced_row, "referenced")?;
    if referenced {
        return Ok(None);
    }
    db(tx
        .client()
        .execute(
            "UPDATE jobniac.generation_intents SET state = 'ABANDONED', lease_token = NULL, \
             lease_started_at = NULL, lease_expires_at = NULL, finished_at = $1 WHERE asset_id = $2",
            &[&now, &found.asset_id],
        )
        .await)?;
    let abandoned = find_generation_intent(tx, found.asset_id, false).await?;
    match abandoned {
        Some(intent) if intent.state == GenerationState::Abandoned => Ok(Some((intent, asset))),
        _ => Err(IntentError::Database(AppError::Application(
            ApplicationError::database_unavailable(),
        ))),
    }
}

/// Admits the claims of a bounded binding set. With no claims,
/// any existing intent of a binding is refused.
///
/// # Errors
///
/// Fails with [`IntentError::Invalid`] for a malformed set, or [`IntentError::Unavailable`].
pub async fn admit_generation_bindings(
    tx: &mut PgTransaction,
    bindings: &[GeneratedReservation],
    claims: Option<&[IntentReservation]>,
    expected: &[GenerationDescriptor],
    now: OffsetDateTime,
) -> Result<(), IntentError> {
    validate_binding_intents(bindings, claims)?;
    let Some(claims) = claims else {
        for binding in bindings {
            if find_generation_intent(tx, binding.asset.asset_id(), false)
                .await?
                .is_some()
            {
                return Err(IntentError::Invalid);
            }
        }
        return Ok(());
    };
    if expected.len() != claims.len() {
        return Err(IntentError::Invalid);
    }
    let mut pairs: Vec<(&IntentReservation, &GenerationDescriptor)> =
        claims.iter().zip(expected).collect();
    pairs.sort_by_key(|(claim, _)| claim.asset_id());
    for (claim, descriptor) in pairs {
        admit_generation_claim(tx, claim, descriptor, now).await?;
    }
    Ok(())
}

/// Completes the claims of a bounded binding set in asset-id order. A `None` claim set is the
/// unclassified path and does nothing.
///
/// # Errors
///
/// Fails with the first [`IntentError`] of a claim that cannot complete.
pub async fn complete_generation_bindings(
    tx: &mut PgTransaction,
    claims: Option<&[IntentReservation]>,
    expected: &[GenerationDescriptor],
    now: OffsetDateTime,
) -> Result<(), IntentError> {
    let Some(claims) = claims else {
        return Ok(());
    };
    if expected.len() != claims.len() {
        return Err(IntentError::Invalid);
    }
    let mut pairs: Vec<(&IntentReservation, &GenerationDescriptor)> =
        claims.iter().zip(expected).collect();
    pairs.sort_by_key(|(claim, _)| claim.asset_id());
    for (claim, descriptor) in pairs {
        complete_generation_intent(tx, claim, descriptor, now).await?;
    }
    Ok(())
}
