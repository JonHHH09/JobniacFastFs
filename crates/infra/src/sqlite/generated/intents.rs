//! Classified intent transactions: insert, admission with leases and fencing, completion, renewal,
//! abandonment and the bounded binding sets.
//!
//! PostgreSQL admission never waited: the document and asset advisory locks were try-locks, and a
//! busy lock was [`IntentError::Unavailable`]. Here the single writer already excludes every
//! concurrent claim, so there is nothing to try; the stale-claim refusals (a moved row, a rotated
//! fence, an expired lease) are unchanged. The intent guards of the PostgreSQL trigger run in
//! [`super::guards`] before each write.

use jobniac_core::errors::{AppError, ApplicationError};
use jobniac_core::generated_assets::identity::generation_instant;
use jobniac_core::generated_assets::{
    GENERATION_LEASE, GeneratedReservation, GenerationDescriptor, GenerationIntent,
    GenerationPurpose, GenerationState, IntentReservation, validate_binding_intents,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use time::OffsetDateTime;
use uuid::Uuid;

use super::IntentError;
use super::bindings::{ASSET_COLUMNS, AssetRow, verify_pdf_binding};
use super::guards::{
    check_abandoned_retirement, generated_asset_referenced, guard_intent_insert,
    guard_intent_update,
};
use super::run_with;
use crate::sqlite::codec::{
    get_id, get_opt_id, get_opt_ts, get_ts, instant_text, optional_instant_text, uuid_text,
};
use crate::sqlite::unit_of_work::SqliteTransaction;

/// Columns of a generation intent row, in the order the queries select them.
const INTENT_COLUMNS: &str = "asset_id, operation_id, purpose, profile_id, job_id, resume_id, \
    output_scope, state, lease_token, lease_started_at, lease_expires_at, created_at, finished_at";

fn unavailable_database() -> IntentError {
    IntentError::Database(AppError::Application(
        ApplicationError::database_unavailable(),
    ))
}

/// Reads one intent row.
///
/// # Errors
///
/// Fails with a caller-safe database unavailable error when a stored value is not a valid intent.
pub fn intent_from_row(row: &Row<'_>) -> Result<GenerationIntent, IntentError> {
    let purpose: String = row.get("purpose")?;
    let state: String = row.get("state")?;
    let descriptor = GenerationDescriptor::new(
        get_id(row, "operation_id")?,
        GenerationPurpose::parse(&purpose).ok_or_else(unavailable_database)?,
        get_opt_id(row, "profile_id")?,
        get_opt_id(row, "job_id")?,
        get_opt_id(row, "resume_id")?,
        row.get("output_scope")?,
    )
    .map_err(|_| unavailable_database())?;
    GenerationIntent::new(
        get_id(row, "asset_id")?,
        descriptor,
        GenerationState::parse(&state).ok_or_else(unavailable_database)?,
        get_opt_id(row, "lease_token")?,
        get_opt_ts(row, "lease_started_at")?,
        get_opt_ts(row, "lease_expires_at")?,
        get_ts(row, "created_at")?,
        get_opt_ts(row, "finished_at")?,
    )
    .map_err(|_| unavailable_database())
}

/// Reads the intent of one asset on `conn`.
pub(crate) fn find_intent_in(
    conn: &Connection,
    asset_id: Uuid,
) -> Result<Option<GenerationIntent>, IntentError> {
    let sql =
        format!("SELECT {INTENT_COLUMNS} FROM jobniac_generation_intents WHERE asset_id = ?1");
    conn.query_row(&sql, [uuid_text(asset_id)], |row| Ok(intent_from_row(row)))
        .optional()?
        .transpose()
}

/// Reads one asset row on `conn`.
fn find_asset_in(conn: &Connection, asset_id: Uuid) -> Result<Option<AssetRow>, IntentError> {
    let sql = format!("SELECT {ASSET_COLUMNS} FROM jobniac_generated_assets WHERE asset_id = ?1");
    conn.query_row(&sql, [uuid_text(asset_id)], |row| {
        Ok(AssetRow::from_row(row))
    })
    .optional()?
    .transpose()
}

/// Reads the intent of one asset. With `for_update` (the PostgreSQL row and owner locks) an asset
/// that is missing or has no document has no intent to claim, so the result is `None`.
///
/// # Errors
///
/// Fails with [`IntentError::Database`].
pub async fn find_generation_intent(
    tx: &mut SqliteTransaction,
    asset_id: Uuid,
    for_update: bool,
) -> Result<Option<GenerationIntent>, IntentError> {
    run_with(tx, |conn| {
        if for_update {
            let document: Option<Option<String>> = conn
                .query_row(
                    "SELECT document_id FROM jobniac_generated_assets WHERE asset_id = ?1",
                    [uuid_text(asset_id)],
                    |row| row.get(0),
                )
                .optional()?;
            if document.flatten().is_none() {
                return Ok(None);
            }
        }
        find_intent_in(conn, asset_id)
    })
}

/// Reads the asset of a reservation, refusing a row that moved.
fn locked_asset(
    conn: &Connection,
    reservation: &GeneratedReservation,
) -> Result<AssetRow, IntentError> {
    match find_asset_in(conn, reservation.asset.asset_id())? {
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

/// Inserts the birth intent of a reserved asset in the same transaction that created it.
///
/// # Errors
///
/// Fails with [`IntentError::Unavailable`] when the asset is not a reserved classified birth in
/// this transaction, or [`IntentError::Invalid`] for an invalid descriptor.
pub async fn insert_generation_intent(
    tx: &mut SqliteTransaction,
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
    run_with(tx, |conn| {
        let asset = locked_asset(conn, reservation)?;
        if asset.lifecycle != "RESERVED"
            || !asset.generation_intent_required
            || !asset.current_transaction
            || generation_instant(asset.created_at)? != now
        {
            return Err(IntentError::Unavailable);
        }
        guard_intent_insert(conn, &pending)?;
        let purpose = descriptor.purpose.as_str();
        let state = pending.state.as_str();
        conn.execute(
            &format!(
                "INSERT INTO jobniac_generation_intents ({INTENT_COLUMNS}) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)"
            ),
            params![
                uuid_text(pending.asset_id),
                uuid_text(descriptor.operation_id),
                purpose,
                descriptor.profile_id.map(uuid_text),
                descriptor.job_id.map(uuid_text),
                descriptor.resume_id.map(uuid_text),
                descriptor.output_scope,
                state,
                pending.lease_token.map(uuid_text),
                optional_instant_text(pending.lease_started_at),
                optional_instant_text(pending.lease_expires_at),
                instant_text(pending.created_at),
                Option::<String>::None,
            ],
        )?;
        if find_intent_in(conn, pending.asset_id)?.as_ref() != Some(&pending) {
            return Err(unavailable_database());
        }
        Ok(())
    })?;
    Ok(IntentReservation {
        reservation: reservation.clone(),
        intent: pending,
    })
}

/// Admits a live claim against the stored intent and asset.
fn admit(
    conn: &Connection,
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
    let asset = locked_asset(conn, &claim.reservation)?;
    let found = find_intent_in(conn, claim.asset_id())?;
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
    tx: &mut SqliteTransaction,
    claim: &IntentReservation,
    expected: &GenerationDescriptor,
    now: OffsetDateTime,
) -> Result<GenerationIntent, IntentError> {
    run_with(tx, |conn| Ok(admit(conn, claim, expected, now, true)?.0))
}

/// Completes a live claim after its PDF binding is verified. Completion and the publication or link
/// writes must share the caller's transaction.
///
/// # Errors
///
/// Fails with [`IntentError::Unavailable`] for a stale claim or [`IntentError::Binding`] when the
/// asset's bytes do not match.
pub async fn complete_generation_intent(
    tx: &mut SqliteTransaction,
    claim: &IntentReservation,
    expected: &GenerationDescriptor,
    now: OffsetDateTime,
) -> Result<GenerationIntent, IntentError> {
    let now = generation_instant(now)?;
    let found = admit_generation_claim(tx, claim, expected, now).await?;
    verify_pdf_binding(tx, &claim.reservation).await?;
    run_with(tx, |conn| {
        let mut completed = found.clone();
        completed.state = GenerationState::Completed;
        completed.lease_token = None;
        completed.lease_started_at = None;
        completed.lease_expires_at = None;
        completed.finished_at = Some(now);
        guard_intent_update(conn, &found, &completed)?;
        conn.execute(
            "UPDATE jobniac_generation_intents SET state = 'COMPLETED', lease_token = NULL, \
             lease_started_at = NULL, lease_expires_at = NULL, finished_at = ?1 \
             WHERE asset_id = ?2",
            params![instant_text(now), uuid_text(found.asset_id)],
        )?;
        match find_intent_in(conn, found.asset_id)? {
            Some(intent) if intent.state == GenerationState::Completed => Ok(intent),
            _ => Err(unavailable_database()),
        }
    })
}

/// Moves the fence of a live claim to `new_token` for a fresh lease. The new expiry may not move
/// backwards.
///
/// # Errors
///
/// Fails with [`IntentError::Invalid`] for a reused token and [`IntentError::Unavailable`] for a
/// stale claim or an expiry that would shrink.
pub async fn renew_generation_intent(
    tx: &mut SqliteTransaction,
    claim: &IntentReservation,
    new_token: Uuid,
    now: OffsetDateTime,
) -> Result<IntentReservation, IntentError> {
    claim.intent.validate()?;
    if claim.intent.lease_token == Some(new_token) {
        return Err(IntentError::Invalid);
    }
    let now = generation_instant(now)?;
    let intent = run_with(tx, |conn| {
        let (found, _asset) = admit(conn, claim, &claim.intent.descriptor, now, true)?;
        let expiry = now + GENERATION_LEASE;
        if found
            .lease_expires_at
            .is_some_and(|previous| expiry < previous)
        {
            return Err(IntentError::Unavailable);
        }
        let mut renewed = found.clone();
        renewed.lease_token = Some(new_token);
        renewed.lease_started_at = Some(now);
        renewed.lease_expires_at = Some(expiry);
        guard_intent_update(conn, &found, &renewed)?;
        conn.execute(
            "UPDATE jobniac_generation_intents SET lease_token = ?1, lease_started_at = ?2, \
             lease_expires_at = ?3 WHERE asset_id = ?4",
            params![
                uuid_text(new_token),
                instant_text(now),
                instant_text(expiry),
                uuid_text(found.asset_id)
            ],
        )?;
        if find_intent_in(conn, found.asset_id)?.as_ref() != Some(&renewed) {
            return Err(unavailable_database());
        }
        Ok(renewed)
    })?;
    Ok(IntentReservation {
        reservation: claim.reservation.clone(),
        intent,
    })
}

/// Fences abandonment, including expiry. Returns `None` and changes nothing when the asset is
/// referenced. The caller retires the asset in the same transaction; the pre-commit check
/// registered here rejects an abandoned orphan otherwise.
///
/// # Errors
///
/// Fails with [`IntentError::Unavailable`] for a stale claim.
pub async fn abandon_generation_intent(
    tx: &mut SqliteTransaction,
    claim: &IntentReservation,
    now: OffsetDateTime,
) -> Result<Option<(GenerationIntent, AssetRow)>, IntentError> {
    let now = generation_instant(now)?;
    let abandoned = run_with(tx, |conn| {
        let (found, asset) = admit(conn, claim, &claim.intent.descriptor, now, false)?;
        if generated_asset_referenced(conn, asset.document_id, asset.identity.relative_path())? {
            return Ok(None);
        }
        let mut next = found.clone();
        next.state = GenerationState::Abandoned;
        next.lease_token = None;
        next.lease_started_at = None;
        next.lease_expires_at = None;
        next.finished_at = Some(now);
        guard_intent_update(conn, &found, &next)?;
        conn.execute(
            "UPDATE jobniac_generation_intents SET state = 'ABANDONED', lease_token = NULL, \
             lease_started_at = NULL, lease_expires_at = NULL, finished_at = ?1 \
             WHERE asset_id = ?2",
            params![instant_text(now), uuid_text(found.asset_id)],
        )?;
        match find_intent_in(conn, found.asset_id)? {
            Some(intent) if intent.state == GenerationState::Abandoned => Ok(Some((intent, asset))),
            _ => Err(unavailable_database()),
        }
    })?;
    if let Some((intent, _)) = &abandoned {
        let asset_id = intent.asset_id;
        tx.before_commit(Box::new(move |conn| {
            check_abandoned_retirement(conn, asset_id)
        }));
    }
    Ok(abandoned)
}

/// Admits the claims of a bounded binding set. With no claims, any existing intent of a binding is
/// refused.
///
/// # Errors
///
/// Fails with [`IntentError::Invalid`] for a malformed set, or [`IntentError::Unavailable`].
pub async fn admit_generation_bindings(
    tx: &mut SqliteTransaction,
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
    tx: &mut SqliteTransaction,
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
