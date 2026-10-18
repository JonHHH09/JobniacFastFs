//! Adapter of the core file port to the owned-root file layer. The file layer enforces
//! ownership, identity and the retirement fence.

use jobniac_core::generated_assets::{
    AssetIdentity as CoreIdentity, GeneratedAssetWriter, PublicationFailure as CoreFailure,
    PublicationFailureCategory as CoreCategory, PublicationOutcome as CoreOutcome,
};

use crate::generated::asset_files::{
    AssetIdentity, OwnedAssetFiles, PublicationFailureCategory, PublicationOutcome,
};

/// A blocking file phase (hashing, write, fsync) runs without parking an async worker; see
/// [`crate::blocking::run_blocking`].
pub(crate) use crate::blocking::run_blocking as run_file_phase;

/// Publishes verified generated PDFs under one owned root.
#[derive(Debug, Clone)]
pub struct OwnedFileWriter {
    files: OwnedAssetFiles,
}

impl OwnedFileWriter {
    /// Binds the writer to the qualified owned namespace.
    #[must_use]
    pub const fn new(files: OwnedAssetFiles) -> Self {
        Self { files }
    }
}

impl GeneratedAssetWriter for OwnedFileWriter {
    fn materialize_owned(
        &self,
        identity: &CoreIdentity,
        content: &[u8],
    ) -> Result<CoreOutcome, CoreFailure> {
        let file_identity = AssetIdentity::new(
            identity.asset_id(),
            identity.root_id(),
            identity.sha256(),
            identity.size_bytes(),
        )
        .map_err(|_| CoreFailure {
            category: CoreCategory::IdentityMismatch,
        })?;
        match run_file_phase(|| self.files.materialize_owned(&file_identity, content)) {
            Ok(PublicationOutcome::Created) => Ok(CoreOutcome::Created),
            Ok(PublicationOutcome::AlreadyPresent) => Ok(CoreOutcome::AlreadyPresent),
            Err(failure) => Err(CoreFailure {
                category: map_category(failure.category()),
            }),
        }
    }
}

/// Maps the file-layer category to the core category, variant for variant.
const fn map_category(category: PublicationFailureCategory) -> CoreCategory {
    match category {
        PublicationFailureCategory::FileWriteFailed => CoreCategory::FileWriteFailed,
        PublicationFailureCategory::IdentityMismatch => CoreCategory::IdentityMismatch,
        PublicationFailureCategory::OwnershipUnproven => CoreCategory::OwnershipUnproven,
        PublicationFailureCategory::UnsupportedPlatform => CoreCategory::UnsupportedPlatform,
        PublicationFailureCategory::RetiredAsset => CoreCategory::RetiredAsset,
    }
}
