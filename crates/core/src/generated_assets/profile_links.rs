//! Immutable generic profile resume links: the normalized
//! record, the incoming admission against a reserved binding, and the replacement result.

use time::OffsetDateTime;
use uuid::Uuid;

use super::identity::{AssetIdentity, GeneratedReservation, generation_instant};
use jobniac_text::java::java_strip;

/// Accepted resume types, in canonical lowercase form.
pub const RESUME_TYPES: [&str; 3] = ["master_resume", "canadian_resume", "canadian_resume_fr"];

/// Refusal of a profile resume persistence input. The message is fixed and carries no values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Invalid profile resume persistence input")]
pub struct InvalidResumeLink;

/// Largest persisted revision (a signed 64-bit integer).
pub const MAX_REVISION: i64 = i64::MAX;

/// Admits a revision: `0..=i64::MAX`.
///
/// # Errors
///
/// Fails with [`InvalidResumeLink`] for a negative value.
pub fn validate_revision(value: i64) -> Result<i64, InvalidResumeLink> {
    if value < 0 {
        Err(InvalidResumeLink)
    } else {
        Ok(value)
    }
}

/// Normalizes a resume type: at most 64 characters, trimmed, lowercased, and one of
/// [`RESUME_TYPES`].
///
/// # Errors
///
/// Fails with [`InvalidResumeLink`] for any other value.
pub fn normalize_resume_type(value: &str) -> Result<String, InvalidResumeLink> {
    if value.chars().count() > 64 {
        return Err(InvalidResumeLink);
    }
    let canonical = java_strip(value).to_lowercase();
    if RESUME_TYPES.contains(&canonical.as_str()) {
        Ok(canonical)
    } else {
        Err(InvalidResumeLink)
    }
}

/// One profile resume link. Times are UTC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileResumeDocument {
    /// Link identifier; kept for the life of the (profile, type) pair.
    pub id: Uuid,
    /// Owning profile.
    pub profile_id: Uuid,
    /// Document that holds the bytes.
    pub document_id: Uuid,
    /// Relative path; a new link must be `<uuid>.pdf`.
    pub file_path: String,
    /// Normalized resume type.
    pub resume_type: String,
    /// Creation instant.
    pub created_at: OffsetDateTime,
    /// Last replacement instant.
    pub updated_at: OffsetDateTime,
}

impl ProfileResumeDocument {
    /// Validates and normalizes a link.
    ///
    /// # Errors
    ///
    /// Fails with [`InvalidResumeLink`] for a bad path, type or instant order.
    pub fn new(
        id: Uuid,
        profile_id: Uuid,
        document_id: Uuid,
        file_path: &str,
        resume_type: &str,
        created_at: OffsetDateTime,
        updated_at: OffsetDateTime,
    ) -> Result<Self, InvalidResumeLink> {
        if file_path.is_empty()
            || file_path.chars().count() > 4096
            || file_path.contains('\0')
            || java_strip(file_path).is_empty()
            || file_path.len() > 4096
        {
            return Err(InvalidResumeLink);
        }
        let created_at = generation_instant(created_at).map_err(|_| InvalidResumeLink)?;
        let updated_at = generation_instant(updated_at).map_err(|_| InvalidResumeLink)?;
        if updated_at < created_at {
            return Err(InvalidResumeLink);
        }
        Ok(Self {
            id,
            profile_id,
            document_id,
            file_path: file_path.to_owned(),
            resume_type: normalize_resume_type(resume_type)?,
            created_at,
            updated_at,
        })
    }

    /// Re-derives every field from the constructor rules.
    /// The fields are public, so a link can be changed after construction; a changed field that
    /// is no longer canonical (a resume type with spaces or capitals, an instant out of order)
    /// is refused.
    ///
    /// # Errors
    ///
    /// Fails with [`InvalidResumeLink`] when [`ProfileResumeDocument::new`] would refuse these
    /// values or would produce a different link.
    pub fn validate_canonical(&self) -> Result<(), InvalidResumeLink> {
        let canonical = Self::new(
            self.id,
            self.profile_id,
            self.document_id,
            &self.file_path,
            &self.resume_type,
            self.created_at,
            self.updated_at,
        )?;
        if &canonical == self {
            Ok(())
        } else {
            Err(InvalidResumeLink)
        }
    }

    /// Requires the incoming shape: the link is canonical and its path is exactly the canonical
    /// `<uuid>.pdf` of a new asset.
    ///
    /// # Errors
    ///
    /// Fails with [`InvalidResumeLink`] for a non-canonical link or any other path.
    pub fn validate_incoming(&self) -> Result<(), InvalidResumeLink> {
        self.validate_canonical()?;
        let stem = self
            .file_path
            .strip_suffix(".pdf")
            .filter(|_| self.file_path.len() == 40)
            .ok_or(InvalidResumeLink)?;
        let identity = Uuid::parse_str(stem).map_err(|_| InvalidResumeLink)?;
        if self.file_path != format!("{identity}.pdf") {
            return Err(InvalidResumeLink);
        }
        Ok(())
    }
}

/// Admits an incoming link against its reserved binding.
///
/// # Errors
///
/// Fails with [`InvalidResumeLink`] when the link is not the owned path and document of `binding`.
pub fn admit_profile_resume_link(
    link: &ProfileResumeDocument,
    binding: &GeneratedReservation,
) -> Result<(), InvalidResumeLink> {
    link.validate_incoming()?;
    let asset: &AssetIdentity = &binding.asset;
    if link.document_id != binding.document_id || link.file_path != asset.relative_path() {
        return Err(InvalidResumeLink);
    }
    Ok(())
}

/// The result of a replacement: the saved link and the link it replaced, if any.
/// The saved link keeps the previous id, type, profile and creation instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceResult {
    /// The link as stored after the replacement.
    pub saved: ProfileResumeDocument,
    /// The previous link, when one existed.
    pub previous: Option<ProfileResumeDocument>,
}

impl ReplaceResult {
    /// Checks that `saved` continues `previous`.
    ///
    /// # Errors
    ///
    /// Fails when the saved link is not an incoming link or does not continue the previous one.
    pub fn new(
        saved: ProfileResumeDocument,
        previous: Option<ProfileResumeDocument>,
    ) -> Result<Self, InvalidResumeLink> {
        saved.validate_incoming()?;
        if let Some(previous) = &previous
            && (previous.validate_canonical().is_err()
                || saved.profile_id != previous.profile_id
                || saved.resume_type != previous.resume_type
                || saved.id != previous.id
                || saved.created_at != previous.created_at)
        {
            return Err(InvalidResumeLink);
        }
        Ok(Self { saved, previous })
    }
}
