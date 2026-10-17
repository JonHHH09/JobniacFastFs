//! The artifact integrity rules, including the native revalidation cases. All data is synthetic.

use jobniac_core::artifacts::{DocumentArtifact, MAX_ARTIFACT_BYTES, validate_artifact};
use jobniac_core::clock::format_instant;
use jobniac_core::documents::{DocumentMetadata, sha256_hex};
use jobniac_core::errors::{AppError, ErrorCode, error_envelope};
use time::macros::datetime;
use uuid::Uuid;

const DOCUMENT_ID: &str = "00000000-0000-4000-8000-0000000000AA";

/// A PDF of exactly `size` bytes that starts with the magic.
fn pdf(size: usize) -> Vec<u8> {
    let mut bytes = b"%PDF-1.4\n".to_vec();
    bytes.resize(size, b'a');
    bytes
}

/// A consistent artifact over `content` with the given stored name.
fn artifact(name: &str, content: Vec<u8>) -> DocumentArtifact {
    let created = format_instant(datetime!(2026-01-02 03:04:05 UTC));
    let updated = format_instant(datetime!(2026-01-02 03:04:06 UTC));
    DocumentArtifact {
        document: DocumentMetadata {
            id: Uuid::parse_str(DOCUMENT_ID).unwrap_or_default(),
            original_file_name: name.to_owned(),
            media_type: "application/pdf".to_owned(),
            byte_size: i64::try_from(content.len()).unwrap_or(i64::MAX),
            sha256: sha256_hex(&content),
            created_at: created,
            updated_at: updated,
        },
        content,
    }
}

fn message_of(error: &AppError) -> String {
    error_envelope(error).message
}

#[test]
fn a_consistent_artifact_is_returned_in_canonical_form() -> Result<(), AppError> {
    let checked = validate_artifact(artifact("CV.pdf", pdf(64)))?;
    assert_eq!(checked.document.original_file_name, "CV.pdf");
    assert_eq!(checked.document.media_type, "application/pdf");
    assert_eq!(checked.content.len(), 64);
    Ok(())
}

#[test]
fn the_basename_drops_drive_prefixes_and_both_separators() -> Result<(), AppError> {
    for (stored, expected) in [
        (r"C:\private\folder\cv.pdf", "cv.pdf"),
        ("dir/sub/cv.pdf", "cv.pdf"),
        ("C:cv.pdf", "cv.pdf"),
        ("dir/", "stored.pdf"),
    ] {
        let checked = validate_artifact(artifact(stored, pdf(32)))?;
        assert_eq!(checked.document.original_file_name, expected, "{stored}");
    }
    Ok(())
}

#[test]
fn dot_names_and_blank_names_fail_the_integrity_check() {
    for stored in ["..", ".", "   ", "a\u{1}b.pdf"] {
        let error = validate_artifact(artifact(stored, pdf(32))).err();
        assert_eq!(
            error.as_ref().map(message_of).as_deref(),
            Some("Stored document integrity check failed"),
            "{stored:?}"
        );
    }
}

#[test]
fn the_utf8_name_boundary_is_checked_before_the_basename_projection() {
    let at_limit = format!("{}.pdf", "a".repeat(4092));
    assert_eq!(at_limit.len(), 4096);
    assert!(validate_artifact(artifact(&at_limit, pdf(32))).is_ok());
    let over = format!("{}.pdf", "a".repeat(4093));
    assert!(validate_artifact(artifact(&over, pdf(32))).is_err());
}

#[test]
fn the_media_type_is_compared_exactly() {
    let mut value = artifact("cv.pdf", pdf(32));
    value.document.media_type = "Application/PDF".to_owned();
    assert!(validate_artifact(value).is_err());
}

#[test]
fn the_stored_size_must_match_the_content_and_the_magic_must_hold() {
    let mut short = artifact("cv.pdf", pdf(32));
    short.document.byte_size = 31;
    assert!(validate_artifact(short).is_err());

    let mut magic = artifact("cv.pdf", b"NOTPDF-1.4 bytes".to_vec());
    magic.document.sha256 = sha256_hex(&magic.content);
    assert!(validate_artifact(magic).is_err());
}

#[test]
fn a_wrong_digest_or_an_uppercase_digest_is_refused() {
    let mut wrong = artifact("cv.pdf", pdf(32));
    wrong.document.sha256 = "0".repeat(64);
    assert!(validate_artifact(wrong).is_err());

    let mut upper = artifact("cv.pdf", pdf(32));
    upper.document.sha256 = upper.document.sha256.to_uppercase();
    assert!(validate_artifact(upper).is_err());
}

#[test]
fn updated_before_created_is_refused() {
    let mut reversed = artifact("cv.pdf", pdf(32));
    std::mem::swap(
        &mut reversed.document.created_at,
        &mut reversed.document.updated_at,
    );
    assert!(validate_artifact(reversed).is_err());
}

#[test]
fn the_size_boundary_accepts_one_mebibyte_and_refuses_one_byte_more() -> Result<(), AppError> {
    let exact = validate_artifact(artifact("cv.pdf", pdf(MAX_ARTIFACT_BYTES)))?;
    assert_eq!(exact.content.len(), MAX_ARTIFACT_BYTES);

    let over = artifact("cv.pdf", pdf(MAX_ARTIFACT_BYTES + 1));
    let error = validate_artifact(over).err();
    let envelope = error.as_ref().map(error_envelope);
    assert_eq!(
        envelope.as_ref().map(|e| e.code),
        Some(ErrorCode::ValidationError)
    );
    assert_eq!(
        envelope
            .as_ref()
            .and_then(|e| e.details.get("reason"))
            .map(String::as_str),
        Some("stored PDF size exceeds limit of 1048576 bytes")
    );
    Ok(())
}

#[test]
fn an_oversize_declaration_is_refused_before_any_content_use() {
    let mut declared = artifact("cv.pdf", pdf(32));
    declared.document.byte_size = i64::try_from(MAX_ARTIFACT_BYTES + 1).unwrap_or(i64::MAX);
    let envelope = validate_artifact(declared)
        .err()
        .as_ref()
        .map(error_envelope);
    assert_eq!(envelope.map(|e| e.code), Some(ErrorCode::ValidationError));
}
