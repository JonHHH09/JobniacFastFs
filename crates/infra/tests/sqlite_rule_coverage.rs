//! Direct tests of the generation-intent and reference rules that the SQLite adapters enforce in
//! Rust in place of the PostgreSQL triggers (rules 6, 19, 22 to 25 and 27 of the port list), on top
//! of the repository-level tests of the other rules. All data is synthetic.

mod sqlite_german_support;
mod sqlite_support;

use jobniac_core::generated_assets::{
    GENERATION_LEASE, GenerationDescriptor, GenerationIntent, GenerationPurpose, GenerationState,
    IntentReservation,
};
use jobniac_infra::sqlite::error::SqliteError;
use jobniac_infra::sqlite::generated::guards::{
    generated_asset_referenced, generated_document_referenced, guard_intent_insert,
    guard_intent_update, guard_retirement,
};
use rusqlite::{Connection, params};
use sqlite_german_support::{
    Fixture, TestResult, later, now, resume_write, save_resume, start, stored,
};
use time::Duration;
use uuid::Uuid;

fn is_violation<T: std::fmt::Debug>(result: Result<T, SqliteError>) -> bool {
    matches!(
        result,
        Err(SqliteError::Constraint { ref sqlstate, .. }) if sqlstate == "23514"
    )
}

async fn claim(
    fixture: &Fixture,
    purpose: GenerationPurpose,
    resume: Option<Uuid>,
    scope: &str,
) -> Result<IntentReservation, Box<dyn std::error::Error + Send + Sync>> {
    let descriptor = GenerationDescriptor::new(
        Uuid::new_v4(),
        purpose,
        (purpose != GenerationPurpose::StandalonePdf).then_some(fixture.profile),
        matches!(
            purpose,
            GenerationPurpose::GermanResume | GenerationPurpose::GermanCoverLetter
        )
        .then_some(fixture.job),
        resume,
        (purpose != GenerationPurpose::StandalonePdf).then(|| scope.to_owned()),
    )?;
    Ok(fixture.classified_claim(&descriptor).await?)
}

/// The completed form of a pending intent, finished at `finished`.
fn completed(old: &GenerationIntent, finished: time::OffsetDateTime) -> GenerationIntent {
    let mut next = old.clone();
    next.state = GenerationState::Completed;
    next.lease_token = None;
    next.lease_started_at = None;
    next.lease_expires_at = None;
    next.finished_at = Some(finished);
    next
}

fn complete_raw(conn: &Connection, asset: Uuid) -> TestResult {
    conn.execute(
        "UPDATE jobniac_generation_intents SET state = 'COMPLETED', lease_token = NULL, \
         lease_started_at = NULL, lease_expires_at = NULL, finished_at = ?2 WHERE asset_id = ?1",
        params![asset.to_string(), stored(later())],
    )?;
    Ok(())
}

#[tokio::test]
async fn rule_6_a_pending_lease_lasts_at_most_five_minutes() -> TestResult {
    let fixture = start()?;
    let claim = claim(&fixture, GenerationPurpose::StandalonePdf, None, "").await?;
    let conn = fixture.admin();
    let old = claim.intent.clone();
    let mut renewed = old.clone();
    renewed.lease_token = Some(Uuid::new_v4());
    renewed.lease_expires_at = old
        .lease_expires_at
        .map(|expires| expires + Duration::seconds(1));
    // The renewal is otherwise valid, so the lease bound is what refuses it.
    assert!(is_violation(guard_intent_update(&conn, &old, &renewed)));
    renewed.lease_expires_at = Some(old.lease_started_at.ok_or("lease")? + GENERATION_LEASE);
    assert!(guard_intent_update(&conn, &old, &renewed).is_ok());
    Ok(())
}

#[tokio::test]
async fn rule_19_an_intent_needs_an_asset_with_a_document() -> TestResult {
    let fixture = start()?;
    let conn = fixture.admin();
    let asset = Uuid::new_v4();
    conn.execute(
        "INSERT INTO jobniac_generated_assets (asset_id, root_id, relative_path, sha256, \
         size_bytes, document_id, lifecycle, created_at, generation_intent_required) \
         VALUES (?1, ?2, ?3, ?4, 10, NULL, 'RESERVED', ?5, 1)",
        params![
            asset.to_string(),
            fixture.root.to_string(),
            format!("{asset}.pdf"),
            "a".repeat(64),
            stored(now())
        ],
    )?;
    let descriptor = GenerationDescriptor::new(
        Uuid::new_v4(),
        GenerationPurpose::StandalonePdf,
        None,
        None,
        None,
        None,
    )?;
    let pending = GenerationIntent::pending(asset, descriptor, Uuid::new_v4(), now())?;
    assert!(is_violation(guard_intent_insert(&conn, &pending)));
    Ok(())
}

#[tokio::test]
async fn rule_22_a_completion_must_finish_inside_the_lease() -> TestResult {
    let fixture = start()?;
    let claim = claim(&fixture, GenerationPurpose::StandalonePdf, None, "").await?;
    let conn = fixture.admin();
    let old = claim.intent.clone();
    let expires = old.lease_expires_at.ok_or("lease")?;
    assert!(is_violation(guard_intent_update(
        &conn,
        &old,
        &completed(&old, expires)
    )));
    assert!(
        guard_intent_update(
            &conn,
            &old,
            &completed(&old, expires - Duration::seconds(1))
        )
        .is_ok()
    );
    Ok(())
}

#[tokio::test]
async fn rule_23_a_pending_intent_needs_a_reserved_or_active_asset() -> TestResult {
    let fixture = start()?;
    let claim = claim(&fixture, GenerationPurpose::StandalonePdf, None, "").await?;
    let conn = fixture.admin();
    let old = claim.intent.clone();
    let mut renewed = old.clone();
    renewed.lease_token = Some(Uuid::new_v4());
    renewed.lease_started_at = Some(old.lease_started_at.ok_or("lease")? + Duration::seconds(1));
    assert!(guard_intent_update(&conn, &old, &renewed).is_ok());
    // The asset leaves ACTIVE behind the guard's back (a raw statement), so a renewal is refused.
    conn.execute(
        "UPDATE jobniac_generated_assets SET lifecycle = 'RETIRING', retirement_id = ?2, \
         retired_at = ?3 WHERE asset_id = ?1",
        params![
            claim.asset_id().to_string(),
            Uuid::new_v4().to_string(),
            stored(later())
        ],
    )?;
    assert!(is_violation(guard_intent_update(&conn, &old, &renewed)));
    Ok(())
}

#[tokio::test]
async fn rule_24_a_completion_needs_the_exact_link_of_its_purpose() -> TestResult {
    let fixture = start()?;
    let admin = fixture.admin();
    // The pair's germany resume.
    let write = resume_write(fixture.profile, fixture.job, &[])?;
    save_resume(&fixture.resumes(later()), &write, &[]).await?;
    let resume = write.resume().id();

    // A standalone output needs no link.
    let standalone = claim(&fixture, GenerationPurpose::StandalonePdf, None, "").await?;
    assert!(
        guard_intent_update(
            &admin,
            &standalone.intent,
            &completed(&standalone.intent, later())
        )
        .is_ok()
    );

    // A profile resume needs the profile resume document row.
    let profile = claim(
        &fixture,
        GenerationPurpose::ProfileResume,
        None,
        "master_resume",
    )
    .await?;
    let done = completed(&profile.intent, later());
    assert!(is_violation(guard_intent_update(
        &admin,
        &profile.intent,
        &done
    )));
    admin.execute(
        "INSERT INTO profile_profile_resume_documents (id, profile_id, document_id, file_path, \
         resume_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'master_resume', ?5, ?5)",
        params![
            Uuid::new_v4().to_string(),
            fixture.profile.to_string(),
            profile.reservation.document_id.to_string(),
            profile.reservation.asset.relative_path(),
            stored(now())
        ],
    )?;
    assert!(guard_intent_update(&admin, &profile.intent, &done).is_ok());

    // A German resume needs the variant of its language under the germany resume.
    let german = claim(
        &fixture,
        GenerationPurpose::GermanResume,
        Some(resume),
        "en",
    )
    .await?;
    let done = completed(&german.intent, later());
    assert!(is_violation(guard_intent_update(
        &admin,
        &german.intent,
        &done
    )));
    admin.execute(
        "INSERT INTO resume_resume_variants (id, resume_id, language, document_id, file_path, \
         created_at, updated_at) VALUES (?1, ?2, 'en', ?3, ?4, ?5, ?5)",
        params![
            Uuid::new_v4().to_string(),
            resume.to_string(),
            german.reservation.document_id.to_string(),
            german.reservation.asset.relative_path(),
            stored(now())
        ],
    )?;
    assert!(guard_intent_update(&admin, &german.intent, &done).is_ok());
    // The same link under another language is not the proof.
    admin.execute(
        "UPDATE resume_resume_variants SET language = 'de' WHERE document_id = ?1",
        [german.reservation.document_id.to_string()],
    )?;
    assert!(is_violation(guard_intent_update(
        &admin,
        &german.intent,
        &done
    )));

    // A German cover letter needs the variant under a letter of the same resume.
    let letter = claim(
        &fixture,
        GenerationPurpose::GermanCoverLetter,
        Some(resume),
        "de",
    )
    .await?;
    let done = completed(&letter.intent, later());
    assert!(is_violation(guard_intent_update(
        &admin,
        &letter.intent,
        &done
    )));
    sqlite_german_support::insert_letter(&admin, write.resume(), &letter.reservation)?;
    assert!(guard_intent_update(&admin, &letter.intent, &done).is_ok());
    Ok(())
}

#[tokio::test]
async fn rule_25_an_asset_with_a_pending_intent_cannot_retire() -> TestResult {
    let fixture = start()?;
    let claim = claim(&fixture, GenerationPurpose::StandalonePdf, None, "").await?;
    let conn = fixture.admin();
    assert!(is_violation(guard_retirement(&conn, claim.asset_id())));
    // An abandoned intent no longer fences the retirement.
    conn.execute(
        "UPDATE jobniac_generation_intents SET state = 'ABANDONED', lease_token = NULL, \
         lease_started_at = NULL, lease_expires_at = NULL, finished_at = ?2 WHERE asset_id = ?1",
        params![claim.asset_id().to_string(), stored(later())],
    )?;
    assert!(guard_retirement(&conn, claim.asset_id()).is_ok());
    Ok(())
}

#[tokio::test]
async fn rule_27_a_completed_standalone_output_counts_as_a_reference() -> TestResult {
    let fixture = start()?;
    let claim = claim(&fixture, GenerationPurpose::StandalonePdf, None, "").await?;
    let conn = fixture.admin();
    let document = claim.reservation.document_id;
    let path = claim.reservation.asset.relative_path();
    // A pending output does not reference its document yet.
    assert!(!generated_document_referenced(&conn, Some(document))?);
    assert!(!generated_asset_referenced(&conn, Some(document), path)?);
    complete_raw(&conn, claim.asset_id())?;
    assert!(generated_document_referenced(&conn, Some(document))?);
    assert!(generated_asset_referenced(&conn, Some(document), path)?);
    // The reference blocks the retirement of rule 9 as well.
    assert!(is_violation(guard_retirement(&conn, claim.asset_id())));
    Ok(())
}
