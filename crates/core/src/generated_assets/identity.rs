//! Immutable identities of generated PDFs and their publication outcomes.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::clock::truncate_to_micros;
use crate::generated_root::{MAX_GENERATED_BYTES, asset_file_name};

/// Largest generated PDF, in bytes (the document bound, 10 MiB).
pub const MAX_PDF_BYTES: u64 = MAX_GENERATED_BYTES;

/// Longest accepted file name, in UTF-8 bytes.
pub const MAX_FILE_NAME_BYTES: usize = 4096;

/// Refusal of an identity or input that cannot describe a generated PDF. Carries no values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("generated identity is invalid")]
pub struct InvalidGeneratedIdentity;

/// An immutable generated identity: asset id, owning root, the `<asset-id>.pdf` relative path,
/// the lowercase SHA-256 digest and the byte size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetIdentity {
    asset_id: Uuid,
    root_id: Uuid,
    relative_path: String,
    sha256: String,
    size_bytes: u64,
}

impl AssetIdentity {
    /// Builds the identity of a new asset; the relative path is derived from the asset id.
    ///
    /// # Errors
    ///
    /// Fails when the digest is not 64 lowercase hex characters or the size is outside 1..=10 MiB.
    pub fn new(
        asset_id: Uuid,
        root_id: Uuid,
        sha256: &str,
        size_bytes: u64,
    ) -> Result<Self, InvalidGeneratedIdentity> {
        let canonical_digest = sha256.len() == 64
            && sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !canonical_digest || size_bytes == 0 || size_bytes > MAX_PDF_BYTES {
            return Err(InvalidGeneratedIdentity);
        }
        Ok(Self {
            asset_id,
            root_id,
            relative_path: asset_file_name(asset_id),
            sha256: sha256.to_owned(),
            size_bytes,
        })
    }

    /// Rebuilds an identity read from storage, refusing a path that differs from the asset id.
    ///
    /// # Errors
    ///
    /// Fails for every condition [`AssetIdentity::new`] refuses, or when the path is not
    /// `<asset-id>.pdf`.
    pub fn from_parts(
        asset_id: Uuid,
        root_id: Uuid,
        relative_path: &str,
        sha256: &str,
        size_bytes: u64,
    ) -> Result<Self, InvalidGeneratedIdentity> {
        let identity = Self::new(asset_id, root_id, sha256, size_bytes)?;
        if relative_path != identity.relative_path {
            return Err(InvalidGeneratedIdentity);
        }
        Ok(identity)
    }

    /// The asset identifier.
    #[must_use]
    pub const fn asset_id(&self) -> Uuid {
        self.asset_id
    }

    /// The owning generated root.
    #[must_use]
    pub const fn root_id(&self) -> Uuid {
        self.root_id
    }

    /// The relative path, always `<asset-id>.pdf`.
    #[must_use]
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    /// The lowercase hexadecimal SHA-256 digest.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// The exact byte size.
    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

/// A generated asset reserved for one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedReservation {
    /// The immutable file identity.
    pub asset: AssetIdentity,
    /// The document that owns the verified bytes.
    pub document_id: Uuid,
}

/// Result of publishing a verified asset file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationOutcome {
    /// The file was written by this call.
    Created,
    /// The identical file was already present.
    AlreadyPresent,
}

/// Fixed categories of a failed publication; no path, content or database text is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationFailureCategory {
    /// The write did not complete.
    FileWriteFailed,
    /// The bytes or identity do not match the reserved identity.
    IdentityMismatch,
    /// The root or file cannot be proven as owned.
    OwnershipUnproven,
    /// The host platform cannot provide the owned-file guarantees.
    UnsupportedPlatform,
    /// The asset was retired and must not be written again.
    RetiredAsset,
}

impl PublicationFailureCategory {
    /// The stored enum value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FileWriteFailed => "FILE_WRITE_FAILED",
            Self::IdentityMismatch => "IDENTITY_MISMATCH",
            Self::OwnershipUnproven => "OWNERSHIP_UNPROVEN",
            Self::UnsupportedPlatform => "UNSUPPORTED_PLATFORM",
            Self::RetiredAsset => "RETIRED_ASSET",
        }
    }
}

/// A publication that failed with a fixed category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{}", .category.as_str())]
pub struct PublicationFailure {
    /// The fixed category.
    pub category: PublicationFailureCategory,
}

/// Outcome of materializing a reserved asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializationOutcome {
    /// The asset is active and its file is verified.
    Active,
    /// The asset was retired and a cleanup task is pending.
    CleanupPending,
    /// The asset cannot proceed until an operator resolves it.
    Blocked,
    /// The asset is not eligible for materialization.
    Ineligible,
}

/// Admits PDF bytes: non-empty, at most 10 MiB, starting with `%PDF-`.
///
/// # Errors
///
/// Fails with [`InvalidGeneratedIdentity`] for every other input.
pub fn validate_generated_pdf(content: &[u8]) -> Result<(), InvalidGeneratedIdentity> {
    if content.is_empty() || content.len() as u64 > MAX_PDF_BYTES || !content.starts_with(b"%PDF-")
    {
        return Err(InvalidGeneratedIdentity);
    }
    Ok(())
}

/// Admits a generated PDF file name: trimmed, ends in `.pdf`
/// case-insensitively, no path separators, printable, at most 4096 UTF-8 bytes.
///
/// # Errors
///
/// Fails with [`InvalidGeneratedIdentity`] for every other name.
pub fn validate_file_name(name: &str) -> Result<(), InvalidGeneratedIdentity> {
    if name.is_empty()
        || jobniac_text::str_rules::strip_text(name) != name
        || !name.to_lowercase().ends_with(".pdf")
        || name.contains('/')
        || name.contains('\\')
        || !jobniac_text::str_rules::str_is_printable(name)
        || name.len() > MAX_FILE_NAME_BYTES
    {
        return Err(InvalidGeneratedIdentity);
    }
    Ok(())
}

/// Converts an instant to UTC, refusing values without an offset.
///
/// # Errors
///
/// Never fails for a valid `OffsetDateTime`; the result type keeps the boundary explicit.
pub fn generation_instant(
    value: OffsetDateTime,
) -> Result<OffsetDateTime, InvalidGeneratedIdentity> {
    Ok(truncate_to_micros(value))
}
