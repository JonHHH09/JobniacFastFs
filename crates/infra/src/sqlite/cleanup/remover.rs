//! Adapter of the core cleanup and qualification ports to the owned-root file layer. The file layer
//! enforces ownership, identity and the namespace lock; this adapter only maps identities and the
//! fixed failure categories, variant for variant.

use jobniac_core::cleanup::{
    GenerationRootFiles, OwnedAssetRemover, RemovalFailure, RemovalFailureCategory, RemovalOutcome,
};
use jobniac_core::generated_assets::AssetIdentity as CoreIdentity;

use crate::generated::asset_files::{
    AssetIdentity, OwnedAssetFiles, RemovalFailureCategory as FileCategory,
    RemovalOutcome as FileOutcome,
};

/// Binds the core ports to one qualified owned namespace.
#[derive(Debug, Clone)]
pub struct OwnedFileAdapter {
    files: OwnedAssetFiles,
}

impl OwnedFileAdapter {
    /// Binds the adapter to `files`.
    #[must_use]
    pub const fn new(files: OwnedAssetFiles) -> Self {
        Self { files }
    }
}

impl OwnedAssetRemover for OwnedFileAdapter {
    fn remove_owned(&self, identity: &CoreIdentity) -> Result<RemovalOutcome, RemovalFailure> {
        let file_identity = AssetIdentity::new(
            identity.asset_id(),
            identity.root_id(),
            identity.sha256(),
            identity.size_bytes(),
        )
        .map_err(|_| RemovalFailure::new(RemovalFailureCategory::IdentityMismatch))?;
        match self.files.remove_owned(&file_identity) {
            Ok(FileOutcome::Removed) => Ok(RemovalOutcome::Removed),
            Ok(FileOutcome::AlreadyAbsent) => Ok(RemovalOutcome::AlreadyAbsent),
            Err(failure) => Err(RemovalFailure::new(map_category(failure.category()))),
        }
    }
}

impl GenerationRootFiles for OwnedFileAdapter {
    fn qualify_owned_root(&self) -> Result<(), RemovalFailure> {
        self.files
            .qualify_owned_root()
            .map_err(|failure| RemovalFailure::new(map_category(failure.category())))
    }
}

/// Maps the file-layer category to the core category, variant for variant.
const fn map_category(category: FileCategory) -> RemovalFailureCategory {
    match category {
        FileCategory::FileDeleteFailed => RemovalFailureCategory::FileDeleteFailed,
        FileCategory::IdentityMismatch => RemovalFailureCategory::IdentityMismatch,
        FileCategory::OwnershipUnproven => RemovalFailureCategory::OwnershipUnproven,
        FileCategory::UnsupportedPlatform => RemovalFailureCategory::UnsupportedPlatform,
    }
}
