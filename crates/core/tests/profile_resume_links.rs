//! Admission contracts of the generic profile resume links and the core part of the generated
//! intents. The checks run without a database: the admission refuses a bad link before any adapter
//! is asked to connect. All data is synthetic.
//!
//! Cases that the Rust types make impossible are not written: a subclass of the link or of
//! the binding (Rust has no inheritance), a naive datetime (`OffsetDateTime` always carries an
//! offset), an identifier that is not a UUID (the fields are `Uuid`), and a mutated asset identity
//! (`AssetIdentity` keeps its fields private and is built only through its constructors).

use std::error::Error;

use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedReservation, InvalidResumeLink, ProfileResumeDocument,
    admit_profile_resume_link, normalize_resume_type, sha256_hex,
};
use time::macros::datetime;
use time::{Duration, OffsetDateTime, UtcOffset};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

/// The fixed instant of the synthetic links, in UTC.
const NOW: OffsetDateTime = datetime!(2026-10-03 0:00 UTC);

/// The refusal message, which every refused input shares.
const REFUSAL: &str = "Invalid profile resume persistence input";

/// A synthetic reserved binding: a new asset whose relative path is `<asset-id>.pdf`.
fn binding() -> Result<GeneratedReservation, Box<dyn Error>> {
    let content = b"%PDF-test";
    Ok(GeneratedReservation {
        asset: AssetIdentity::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            &sha256_hex(content),
            content.len() as u64,
        )?,
        document_id: Uuid::new_v4(),
    })
}

/// A synthetic link to `reservation`, with both instants at [`NOW`] and the master type.
fn link(reservation: &GeneratedReservation) -> Result<ProfileResumeDocument, Box<dyn Error>> {
    Ok(ProfileResumeDocument::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        reservation.document_id,
        reservation.asset.relative_path(),
        "master_resume",
        NOW,
        NOW,
    )?)
}

/// Asserts that a constructor refused its input with the fixed message.
fn assert_refused<T: std::fmt::Debug>(result: Result<T, InvalidResumeLink>) {
    match result {
        Err(error) => assert_eq!(error.to_string(), REFUSAL),
        Ok(value) => panic!("the input was accepted: {value:?}"),
    }
}

#[test]
fn resume_types_are_trimmed_lowercased_and_closed() -> TestResult {
    assert_eq!(
        normalize_resume_type("  CANADIAN_RESUME_FR  ")?,
        "canadian_resume_fr"
    );
    assert_eq!(normalize_resume_type("Master_Resume")?, "master_resume");
    assert_refused(normalize_resume_type("germany"));
    assert_refused(normalize_resume_type(&"x".repeat(100_000)));
    Ok(())
}

#[test]
fn canonical_links_are_utc_and_keep_the_offset_free_instant() -> TestResult {
    let east = NOW.to_offset(UtcOffset::from_hms(2, 0, 0)?);
    let value = ProfileResumeDocument::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        "archive/person/resume.pdf",
        "  CANADIAN_RESUME_FR  ",
        east,
        east + Duration::seconds(1),
    )?;
    assert_eq!(value.resume_type, "canadian_resume_fr");
    assert_eq!(value.created_at, NOW);
    assert_eq!(value.created_at.offset(), UtcOffset::UTC);
    assert_eq!(value.updated_at, NOW + Duration::seconds(1));
    value.validate_canonical()?;
    Ok(())
}

#[test]
fn invalid_constructor_fields_are_refused_with_the_fixed_message() {
    let id = Uuid::new_v4();
    let profile = Uuid::new_v4();
    let document = Uuid::new_v4();
    let path = "archive/person/resume.pdf";
    // Each case changes one field of an otherwise valid link (every field is checked and refused safely).
    assert_refused(ProfileResumeDocument::new(
        id, profile, document, path, "germany", NOW, NOW,
    ));
    assert_refused(ProfileResumeDocument::new(
        id,
        profile,
        document,
        path,
        &"x".repeat(100_000),
        NOW,
        NOW,
    ));
    assert_refused(ProfileResumeDocument::new(
        id,
        profile,
        document,
        "",
        "master_resume",
        NOW,
        NOW,
    ));
    assert_refused(ProfileResumeDocument::new(
        id,
        profile,
        document,
        "private\0path",
        "master_resume",
        NOW,
        NOW,
    ));
    assert_refused(ProfileResumeDocument::new(
        id,
        profile,
        document,
        &"x".repeat(100_000),
        "master_resume",
        NOW,
        NOW,
    ));
    // 2049 two-byte characters: 2049 characters but 4098 bytes, over the byte bound.
    assert_refused(ProfileResumeDocument::new(
        id,
        profile,
        document,
        &"\u{e9}".repeat(2049),
        "master_resume",
        NOW,
        NOW,
    ));
    assert_refused(ProfileResumeDocument::new(
        id,
        profile,
        document,
        path,
        "master_resume",
        NOW,
        NOW - Duration::seconds(1),
    ));
}

#[test]
fn mutated_frozen_link_is_rejected_by_admission() -> TestResult {
    // The Rust fields are public,
    // so each case assigns the field directly. Every mutated link must be refused.
    let reservation = binding()?;
    let valid = link(&reservation)?;
    admit_profile_resume_link(&valid, &reservation)?;

    let mut path = valid.clone();
    path.file_path = "archive/path.pdf".to_owned();
    assert!(admit_profile_resume_link(&path, &reservation).is_err());

    let mut resume_type = valid.clone();
    resume_type.resume_type = " MASTER_RESUME ".to_owned();
    assert!(admit_profile_resume_link(&resume_type, &reservation).is_err());

    let mut updated = valid.clone();
    updated.updated_at = NOW - Duration::seconds(1);
    assert!(admit_profile_resume_link(&updated, &reservation).is_err());

    // The same instant in another offset is still the same link: both aware datetimes and
    // `OffsetDateTime` equality compare instants, so it is admitted.
    let mut offset = valid.clone();
    offset.updated_at = NOW.to_offset(UtcOffset::from_hms(-5, 0, 0)?);
    admit_profile_resume_link(&offset, &reservation)?;

    // A mutated creation instant after the update instant is refused as well.
    let mut created = valid.clone();
    created.created_at = NOW + Duration::seconds(1);
    assert!(admit_profile_resume_link(&created, &reservation).is_err());
    Ok(())
}

#[test]
fn wrong_document_and_wrong_path_are_rejected() -> TestResult {
    let reservation = binding()?;
    let mut wrong_document = link(&reservation)?;
    wrong_document.document_id = Uuid::new_v4();
    assert!(admit_profile_resume_link(&wrong_document, &reservation).is_err());

    // A link to another asset of the same document shape carries another path.
    let other = binding()?;
    let wrong_path = link(&other)?;
    assert!(admit_profile_resume_link(&wrong_path, &reservation).is_err());

    // A binding whose document was swapped out after the link was built is refused too.
    let mut swapped = reservation.clone();
    swapped.document_id = Uuid::new_v4();
    assert!(admit_profile_resume_link(&link(&reservation)?, &swapped).is_err());
    Ok(())
}

#[test]
fn a_new_link_must_use_the_owned_asset_path() -> TestResult {
    let reservation = binding()?;
    let unlinked = ProfileResumeDocument::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        reservation.document_id,
        "archive/path.pdf",
        "master_resume",
        NOW,
        NOW,
    )?;
    // Historical paths are readable metadata; an incoming replacement must be `<uuid>.pdf`.
    assert!(admit_profile_resume_link(&unlinked, &reservation).is_err());
    Ok(())
}
