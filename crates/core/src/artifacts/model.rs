//! The artifact snapshot and its integrity rules.
//!
//! Every malformed shape maps to one integrity message. The typed snapshot checks the fields that
//! can still be wrong:
//! names, media type, size, digest, instants and the bytes themselves. The check order is the
//! following: the oversize test comes first and raises the size error, then the projection,
//! then the content checks.

use crate::clock::{format_instant, parse_instant};
use crate::documents::model::{DocumentMetadata, PDF_MEDIA_TYPE};
use crate::documents::service::sha256_hex;
use crate::documents::view::safe_basename;
use crate::errors::{AppError, ApplicationError, ErrorCode, validation_error};

/// The upper bound of a retrieved artifact, in bytes (`MAX_ARTIFACT_BYTES`, 1 MiB).
pub const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;

/// The caller-visible message of a rejected request (`_VALIDATION_MESSAGE`).
pub const ARTIFACT_VALIDATION_MESSAGE: &str = "Invalid document artifact request";

/// The caller-visible message of a failed integrity check (`_INTEGRITY_MESSAGE`).
pub const ARTIFACT_INTEGRITY_MESSAGE: &str = "Stored document integrity check failed";

/// The message of a repository or transport failure.
pub const ARTIFACT_RETRIEVAL_FAILED_MESSAGE: &str = "Document artifact retrieval failed";

/// The message of a disabled service (`read_artifact`, first check).
pub const ARTIFACT_DISABLED_MESSAGE: &str = "Document artifact retrieval is not enabled";

/// The bound of the stored file name, in UTF-8 bytes (`_bounded_string(..., 4096)`).
const MAX_NAME_BYTES: usize = 4096;

/// The bound of the stored media type, in UTF-8 bytes (`_bounded_string(..., 255)`).
const MAX_MEDIA_BYTES: usize = 255;

/// Verified bytes and the caller-safe metadata of one stored PDF. Revalidate before native use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentArtifact {
    /// The seven metadata fields of the stored document.
    pub document: DocumentMetadata,
    /// The stored bytes.
    pub content: Vec<u8>,
}

/// The validation error of an artifact that is over the 1 MiB limit (`artifact_size_error`).
#[must_use]
pub fn artifact_size_error() -> AppError {
    AppError::from(validation_error(
        ARTIFACT_VALIDATION_MESSAGE,
        Some("documentId"),
        Some("stored PDF size exceeds limit of 1048576 bytes"),
    ))
}

/// The internal error of an integrity failure. It carries no detail and no cause.
#[must_use]
pub fn integrity_error() -> AppError {
    AppError::Application(ApplicationError::with_message(
        ErrorCode::InternalError,
        ARTIFACT_INTEGRITY_MESSAGE,
    ))
}

/// Recheck an artifact snapshot and return it in canonical form.
///
/// The canonical form keeps the basename of the stored name (both separators and a drive prefix
/// are removed, as `ntpath.basename` does), the exact PDF media type, and the instants in the
/// canonical format.
///
/// # Errors
///
/// [`artifact_size_error`] when the declared size or the content exceeds 1 MiB, and
/// [`integrity_error`] for every other mismatch.
pub fn validate_artifact(artifact: DocumentArtifact) -> Result<DocumentArtifact, AppError> {
    let size = artifact.document.byte_size;
    let oversized = size > i64::try_from(MAX_ARTIFACT_BYTES).unwrap_or(i64::MAX)
        || artifact.content.len() > MAX_ARTIFACT_BYTES;
    if oversized {
        return Err(artifact_size_error());
    }
    let checked = project(artifact).ok_or_else(integrity_error)?;
    Ok(checked)
}

/// The projection and content checks of [`validate_artifact`], in order.
fn project(artifact: DocumentArtifact) -> Option<DocumentArtifact> {
    let DocumentArtifact { document, content } = artifact;
    let name = bounded_string(&document.original_file_name, MAX_NAME_BYTES)?;
    let media = bounded_string(&document.media_type, MAX_MEDIA_BYTES)?;
    let basename = safe_basename(windows_basename(name));
    if basename == "." || basename == ".." || basename.trim().is_empty() || media != PDF_MEDIA_TYPE
    {
        return None;
    }
    if document.byte_size <= 0 || document.byte_size > i64::try_from(MAX_ARTIFACT_BYTES).ok()? {
        return None;
    }
    let checksum = document.sha256.as_str();
    if !is_lower_hex_digest(checksum) {
        return None;
    }
    let created = parse_instant(&document.created_at).ok()?;
    let updated = parse_instant(&document.updated_at).ok()?;
    if updated < created {
        return None;
    }
    if u64::try_from(document.byte_size).ok()? != content.len() as u64
        || !content.starts_with(b"%PDF-")
        || sha256_hex(&content) != checksum
    {
        return None;
    }
    let document = DocumentMetadata {
        id: document.id,
        original_file_name: basename,
        media_type: media.to_owned(),
        byte_size: document.byte_size,
        sha256: document.sha256.clone(),
        created_at: format_instant(created),
        updated_at: format_instant(updated),
    };
    Some(DocumentArtifact { document, content })
}

/// A non-blank string with no control characters and at most `maximum` UTF-8 bytes.
fn bounded_string(value: &str, maximum: usize) -> Option<&str> {
    let refused = value.trim().is_empty()
        || value.len() > maximum
        || value
            .chars()
            .any(|character| (character as u32) < 32 || character as u32 == 127);
    if refused { None } else { Some(value) }
}

/// Whether the text is a 64-digit lower-case hexadecimal SHA-256 digest.
fn is_lower_hex_digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The last component of a Windows path: a leading drive letter and
/// colon are dropped, then everything up to the last `\` or `/` is removed.
#[must_use]
pub fn windows_basename(name: &str) -> &str {
    let bytes = name.as_bytes();
    let without_drive = if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        &name[2..]
    } else {
        name
    };
    match without_drive.rfind(['\\', '/']) {
        Some(index) => &without_drive[index + 1..],
        None => without_drive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename_drops_drive_and_both_separators() {
        assert_eq!(windows_basename(r"C:\a\b.pdf"), "b.pdf");
        assert_eq!(windows_basename("C:name.pdf"), "name.pdf");
        assert_eq!(windows_basename("dir/sub/x.pdf"), "x.pdf");
        assert_eq!(windows_basename("trailing/"), "");
    }
}
