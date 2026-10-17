//! Per-claim re-checks of `validate_binding_intents`, run for every claim. The proof fields are
//! public, so each test builds a valid claim and then breaks one invariant by hand. All data is
//! synthetic.

use std::error::Error;

use jobniac_core::generated_assets::{
    AssetIdentity, GENERATION_LEASE, GeneratedReservation, GenerationDescriptor, GenerationIntent,
    GenerationPurpose, IntentReservation, InvalidGeneratedIdentity, sha256_hex,
    validate_binding_intents,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const PDF: &[u8] = b"%PDF-1.7\n% synthetic generated resume\n";

fn instant() -> Result<OffsetDateTime, Box<dyn Error>> {
    Ok(OffsetDateTime::from_unix_timestamp(1_700_000_000)?)
}

fn binding(content: &[u8]) -> Result<GeneratedReservation, Box<dyn Error>> {
    let asset = AssetIdentity::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        &sha256_hex(content),
        content.len() as u64,
    )?;
    Ok(GeneratedReservation {
        asset,
        document_id: Uuid::new_v4(),
    })
}

fn standalone(operation: Uuid) -> Result<GenerationDescriptor, Box<dyn Error>> {
    Ok(GenerationDescriptor::new(
        operation,
        GenerationPurpose::StandalonePdf,
        None,
        None,
        None,
        None,
    )?)
}

/// A valid pending claim on `binding`, leased at `now` for the full generation lease.
fn claim_for(
    binding: &GeneratedReservation,
    operation: Uuid,
    now: OffsetDateTime,
) -> Result<IntentReservation, Box<dyn Error>> {
    let intent = GenerationIntent::pending(
        binding.asset.asset_id(),
        standalone(operation)?,
        Uuid::new_v4(),
        now,
    )?;
    Ok(IntentReservation::new(binding.clone(), intent)?)
}

fn check(
    bindings: &[GeneratedReservation],
    claims: &[IntentReservation],
) -> Result<(), InvalidGeneratedIdentity> {
    validate_binding_intents(bindings, Some(claims))
}

#[test]
fn valid_claim_on_its_own_asset_is_accepted() -> TestResult {
    let binding = binding(PDF)?;
    let claim = claim_for(&binding, Uuid::new_v4(), instant()?)?;
    assert_eq!(
        check(std::slice::from_ref(&binding), std::slice::from_ref(&claim)),
        Ok(())
    );
    Ok(())
}

#[test]
fn claim_intent_for_another_asset_is_refused() -> TestResult {
    let binding = binding(PDF)?;
    let mut claim = claim_for(&binding, Uuid::new_v4(), instant()?)?;
    claim.intent.asset_id = Uuid::new_v4();
    assert_eq!(
        check(std::slice::from_ref(&binding), std::slice::from_ref(&claim)),
        Err(InvalidGeneratedIdentity),
        "Invariant: claim.asset.asset_id != claim.intent.asset_id -> 'Generated intent is invalid'"
    );
    Ok(())
}

#[test]
fn pending_claim_without_lease_token_is_refused() -> TestResult {
    let binding = binding(PDF)?;
    let mut claim = claim_for(&binding, Uuid::new_v4(), instant()?)?;
    claim.intent.lease_token = None;
    assert_eq!(
        check(std::slice::from_ref(&binding), std::slice::from_ref(&claim)),
        Err(InvalidGeneratedIdentity),
        "Invariant: a pending intent needs a fence token -> 'Generated intent is invalid'"
    );
    Ok(())
}

#[test]
fn pending_lease_longer_than_five_minutes_is_refused() -> TestResult {
    let now = instant()?;
    let binding = binding(PDF)?;
    let mut claim = claim_for(&binding, Uuid::new_v4(), now)?;
    claim.intent.lease_expires_at = Some(now + GENERATION_LEASE + Duration::seconds(1));
    assert_eq!(
        check(std::slice::from_ref(&binding), std::slice::from_ref(&claim)),
        Err(InvalidGeneratedIdentity),
        "Invariant: not start < expires <= start + GENERATION_LEASE -> 'Generated intent is invalid'"
    );
    Ok(())
}

#[test]
fn pending_claim_with_finish_instant_is_refused() -> TestResult {
    let now = instant()?;
    let binding = binding(PDF)?;
    let mut claim = claim_for(&binding, Uuid::new_v4(), now)?;
    claim.intent.finished_at = Some(now);
    assert_eq!(
        check(std::slice::from_ref(&binding), std::slice::from_ref(&claim)),
        Err(InvalidGeneratedIdentity),
        "Invariant: a pending intent carries no finish instant -> 'Generated intent is invalid'"
    );
    Ok(())
}

#[test]
fn pending_lease_started_before_creation_is_refused() -> TestResult {
    let now = instant()?;
    let binding = binding(PDF)?;
    let mut claim = claim_for(&binding, Uuid::new_v4(), now)?;
    // Keep the lease within five minutes, so only the start-before-creation rule fails.
    claim.intent.lease_started_at = Some(now - Duration::seconds(1));
    claim.intent.lease_expires_at = Some(now + Duration::minutes(4));
    assert_eq!(
        check(std::slice::from_ref(&binding), std::slice::from_ref(&claim)),
        Err(InvalidGeneratedIdentity),
        "Invariant: start < created -> 'Generated intent is invalid'"
    );
    Ok(())
}

#[test]
fn claim_descriptor_with_foreign_scope_is_refused() -> TestResult {
    let binding = binding(PDF)?;
    let mut claim = claim_for(&binding, Uuid::new_v4(), instant()?)?;
    claim.intent.descriptor.output_scope = Some("de".to_owned());
    assert_eq!(
        check(std::slice::from_ref(&binding), std::slice::from_ref(&claim)),
        Err(InvalidGeneratedIdentity),
        "Invariant: a standalone descriptor takes no scope -> 'Generated intent is invalid'"
    );
    Ok(())
}

#[test]
fn every_claim_of_a_pair_is_rechecked() -> TestResult {
    let now = instant()?;
    let operation = Uuid::new_v4();
    let first = binding(PDF)?;
    let second = binding(b"%PDF-1.7\n% second synthetic page\n")?;
    let good = claim_for(&first, operation, now)?;
    let other = claim_for(&second, operation, now)?;
    assert_eq!(
        check(
            &[first.clone(), second.clone()],
            &[good.clone(), other.clone()]
        ),
        Ok(()),
        "two valid claims of one operation are accepted"
    );
    let mut bad = other;
    bad.intent.asset_id = Uuid::new_v4();
    assert_eq!(
        check(&[first, second], &[good, bad]),
        Err(InvalidGeneratedIdentity),
        "an invalid second claim is refused even though its reservation matches its binding"
    );
    Ok(())
}
