//! Owned generated-asset files: initialization, qualification, publication and removal of the
//! immutable `<asset-id>.pdf` files of one owned root.
//!
//! Every materialization and removal runs under the namespace lock and after the root controls
//! (`.jobniac-generated-root`, `.jobniac-generated-capability`) are proven byte for byte. A
//! permanent retirement fence (`.jobniac-retired-<asset-id>`) blocks late writers. Nothing here
//! chooses a root, creates a directory or reads the database: the caller supplies the path and the
//! identity. Failures carry only the fixed category names, never paths or OS text.

use crate::generated::platform::{self, Access, Publish, PublishError, Root};
use hex::encode as hex_encode;
use jobniac_core::generated_root::{
    CAPABILITY_NAME, LOCK_NAME, MARKER_NAME, MAX_GENERATED_BYTES, asset_file_name, fence_bytes,
    fence_name, is_owned_temp_name, marker_bytes,
};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use uuid::Uuid;

/// Category of a removal failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalFailureCategory {
    /// The file could not be removed, or the namespace is busy (retryable).
    FileDeleteFailed,
    /// The file does not match the recorded identity; it is preserved.
    IdentityMismatch,
    /// The root or the file could not be proven owned; nothing is touched.
    OwnershipUnproven,
    /// The platform cannot provide the namespace protocol; nothing is touched.
    UnsupportedPlatform,
}

impl RemovalFailureCategory {
    /// The fixed category name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FileDeleteFailed => "FILE_DELETE_FAILED",
            Self::IdentityMismatch => "IDENTITY_MISMATCH",
            Self::OwnershipUnproven => "OWNERSHIP_UNPROVEN",
            Self::UnsupportedPlatform => "UNSUPPORTED_PLATFORM",
        }
    }
}

/// Category of a publication failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationFailureCategory {
    /// The bytes could not be written or flushed (retryable).
    FileWriteFailed,
    /// A different file already holds the name; it is never replaced.
    IdentityMismatch,
    /// The root or an existing file could not be proven owned.
    OwnershipUnproven,
    /// The platform cannot publish without a named partial file.
    UnsupportedPlatform,
    /// The asset was retired; a late writer may not publish it again.
    RetiredAsset,
}

impl PublicationFailureCategory {
    /// The fixed category name.
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

/// Result of a removal that did not fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalOutcome {
    /// The owned file was removed.
    Removed,
    /// The file was already absent (a retired identity).
    AlreadyAbsent,
}

/// Result of a publication that did not fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationOutcome {
    /// The file was published by this call.
    Created,
    /// The same bytes were already present under the name.
    AlreadyPresent,
}

/// A removal failure carrying only its fixed category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemovalFailure {
    category: RemovalFailureCategory,
}

impl RemovalFailure {
    /// Creates the failure for `category`.
    #[must_use]
    pub const fn new(category: RemovalFailureCategory) -> Self {
        Self { category }
    }

    /// The fixed category.
    #[must_use]
    pub const fn category(&self) -> RemovalFailureCategory {
        self.category
    }
}

impl fmt::Display for RemovalFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.category.as_str())
    }
}

impl std::error::Error for RemovalFailure {}

/// A publication failure carrying only its fixed category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicationFailure {
    category: PublicationFailureCategory,
}

impl PublicationFailure {
    /// Creates the failure for `category`.
    #[must_use]
    pub const fn new(category: PublicationFailureCategory) -> Self {
        Self { category }
    }

    /// The fixed category.
    #[must_use]
    pub const fn category(&self) -> PublicationFailureCategory {
        self.category
    }
}

impl fmt::Display for PublicationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.category.as_str())
    }
}

impl std::error::Error for PublicationFailure {}

/// An immutable generated identity that the owned root must match exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetIdentity {
    asset_id: Uuid,
    root_id: Uuid,
    sha256: String,
    size_bytes: u64,
}

/// Refusal of an asset identity that cannot describe a generated PDF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidAssetIdentity;

impl fmt::Display for InvalidAssetIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("asset identity is invalid")
    }
}

impl std::error::Error for InvalidAssetIdentity {}

impl AssetIdentity {
    /// Validates an identity: a canonical lowercase SHA-256 digest and a size within the PDF bound.
    ///
    /// # Errors
    ///
    /// Fails when the digest is not 64 lowercase hex characters or the size is zero or too large.
    pub fn new(
        asset_id: Uuid,
        root_id: Uuid,
        sha256: &str,
        size_bytes: u64,
    ) -> Result<Self, InvalidAssetIdentity> {
        let canonical_digest = sha256.len() == 64
            && sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !canonical_digest || size_bytes == 0 || size_bytes > MAX_GENERATED_BYTES {
            return Err(InvalidAssetIdentity);
        }
        Ok(Self {
            asset_id,
            root_id,
            sha256: sha256.to_owned(),
            size_bytes,
        })
    }

    /// The asset identifier.
    #[must_use]
    pub const fn asset_id(&self) -> Uuid {
        self.asset_id
    }

    /// The root the asset belongs to.
    #[must_use]
    pub const fn root_id(&self) -> Uuid {
        self.root_id
    }

    /// The canonical lowercase SHA-256 digest.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// The expected size in bytes.
    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// The immutable file name under the root: `<asset-id>.pdf`.
    #[must_use]
    pub fn relative_path(&self) -> String {
        asset_file_name(self.asset_id)
    }
}

/// Internal flow failure of one exact-content operation, mapped by each caller to its category.
#[derive(Debug)]
enum Flow {
    /// The name holds different content or a different object.
    Mismatch,
    /// The object is not a private file owned by this user.
    Ownership,
    /// A sharing or lock violation; retryable.
    Busy,
    /// The platform cannot perform the operation.
    Unsupported,
    /// Any other I/O failure; the OS text is not kept.
    Io,
}

fn flow_from_io(error: io::Error) -> Flow {
    match error.kind() {
        io::ErrorKind::WouldBlock => Flow::Busy,
        io::ErrorKind::PermissionDenied => Flow::Ownership,
        _ => Flow::Io,
    }
}

fn ownership() -> RemovalFailure {
    RemovalFailure::new(RemovalFailureCategory::OwnershipUnproven)
}

fn delete_failed() -> RemovalFailure {
    RemovalFailure::new(RemovalFailureCategory::FileDeleteFailed)
}

/// Removal category of a flow failure in the removal and initialization paths.
fn removal_from_flow(flow: Flow) -> RemovalFailure {
    match flow {
        Flow::Mismatch => RemovalFailure::new(RemovalFailureCategory::IdentityMismatch),
        Flow::Ownership => ownership(),
        Flow::Unsupported => RemovalFailure::new(RemovalFailureCategory::UnsupportedPlatform),
        Flow::Busy | Flow::Io => delete_failed(),
    }
}

/// Maps a removal failure to the publication category of the same name; a delete failure in the
/// publication path is a write failure.
fn publication_from_removal(failure: RemovalFailure) -> PublicationFailure {
    let category = match failure.category() {
        RemovalFailureCategory::FileDeleteFailed => PublicationFailureCategory::FileWriteFailed,
        RemovalFailureCategory::IdentityMismatch => PublicationFailureCategory::IdentityMismatch,
        RemovalFailureCategory::OwnershipUnproven => PublicationFailureCategory::OwnershipUnproven,
        RemovalFailureCategory::UnsupportedPlatform => {
            PublicationFailureCategory::UnsupportedPlatform
        }
    };
    PublicationFailure::new(category)
}

fn require_platform() -> Result<(), RemovalFailure> {
    if platform::supported() {
        Ok(())
    } else {
        Err(RemovalFailure::new(
            RemovalFailureCategory::UnsupportedPlatform,
        ))
    }
}

/// Category of a failed root pin. A sharing violation on a pinned directory is transient and
/// retryable (`FILE_DELETE_FAILED`); any other failure is unproven ownership.
fn root_pin_failure(error: io::Error) -> RemovalFailure {
    if error.kind() == io::ErrorKind::WouldBlock {
        delete_failed()
    } else {
        ownership()
    }
}

/// Pins the configured root after the platform check.
fn open_root(root: Option<&str>) -> Result<Root, RemovalFailure> {
    require_platform()?;
    let path = root.ok_or_else(ownership)?;
    Root::open(path).map_err(root_pin_failure)
}

/// Reads at most `limit` bytes from the start of the file.
fn read_limited(file: &mut File, limit: u64) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    file.take(limit).read_to_end(&mut data)?;
    Ok(data)
}

/// Byte-for-byte check of an existing child against `expected`, with before-and-after identity.
/// `Ok(false)` when the name is absent; a different object or content is `Flow::Mismatch`.
fn existing_exact(root: &Root, name: &str, expected: &[u8]) -> Result<bool, Flow> {
    let mut file = match root.open_child(name, Access::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(flow_from_io(error)),
    };
    let before = platform::stamp_of(&file).map_err(io_flow)?;
    let data = read_limited(&mut file, expected.len() as u64 + 1).map_err(io_flow)?;
    let after = platform::stamp_of(&file).map_err(io_flow)?;
    let still_named = root.name_refers_to(name, &file).map_err(io_flow)?;
    if data != expected || before != after || !still_named {
        return Err(Flow::Mismatch);
    }
    platform::sync_existing(&file).map_err(io_flow)?;
    Ok(true)
}

/// Creates `name` with exactly `content`, or confirms that the same bytes already hold it.
/// `Ok(true)` when created by this call, `Ok(false)` when already present, and never a replace.
fn publish_exact(root: &Root, name: &str, content: &[u8]) -> Result<bool, Flow> {
    if existing_exact(root, name, content)? {
        root.sync().map_err(io_flow)?;
        return Ok(false);
    }
    match root.publish_anonymous(name, content) {
        Ok(Publish::Created) => Ok(true),
        Ok(Publish::NameExists) => {
            if existing_exact(root, name, content)? {
                root.sync().map_err(io_flow)?;
                Ok(false)
            } else {
                Err(Flow::Mismatch)
            }
        }
        Err(PublishError::Mismatch) => Err(Flow::Mismatch),
        Err(PublishError::Unsupported) => Err(Flow::Unsupported),
        // Any OS failure while creating or writing the anonymous file is a write failure, even a
        // permission error (not an ownership proof).
        Err(PublishError::Io(_error)) => Err(Flow::Io),
    }
}

/// Maps a plain I/O failure of an exact-content step.
fn io_flow(_error: io::Error) -> Flow {
    Flow::Io
}

/// Requires the named control to hold exactly `expected` and to be the object it was read from.
fn require_control(root: &Root, name: &str, expected: &[u8]) -> Result<(), RemovalFailure> {
    let mut file = root.open_child(name, Access::Read).map_err(|error| {
        // A sharing violation is transient, so it is a retryable delete failure.
        if error.kind() == io::ErrorKind::WouldBlock {
            delete_failed()
        } else {
            ownership()
        }
    })?;
    let before = platform::stamp_of(&file).map_err(|_| ownership())?;
    let data = read_limited(&mut file, expected.len() as u64 + 1).map_err(|_| ownership())?;
    let after = platform::stamp_of(&file).map_err(|_| ownership())?;
    let still_named = root.name_refers_to(name, &file).map_err(|_| ownership())?;
    if data != expected || before != after || !still_named {
        return Err(ownership());
    }
    Ok(())
}

/// Holds the namespace lock and the proven controls for the duration of one operation.
struct Namespace {
    root: Root,
    _lock: File,
}

/// Takes the exclusive namespace lock and proves the marker and capability controls. A busy lock is
/// retryable; any other failure is unproven ownership.
fn enter_namespace(root: Root, root_id: Uuid) -> Result<Namespace, RemovalFailure> {
    let lock = root
        .open_child(LOCK_NAME, Access::Exclusive)
        .map_err(|error| match error.kind() {
            io::ErrorKind::WouldBlock => delete_failed(),
            _ => ownership(),
        })?;
    root.lock_exclusive(&lock).map_err(|_| delete_failed())?;
    root.verify().map_err(|_| ownership())?;
    require_control(&root, MARKER_NAME, &marker_bytes(root_id))?;
    let capability = platform::capability_bytes(&root, root_id).map_err(|_| ownership())?;
    require_control(&root, CAPABILITY_NAME, &capability)?;
    root.require_capability_link()
        .map_err(|_| RemovalFailure::new(RemovalFailureCategory::UnsupportedPlatform))?;
    Ok(Namespace { root, _lock: lock })
}

/// Fenced removal of one owned asset (`_remove_identity`).
fn remove_identity(
    root: &Root,
    identity: &AssetIdentity,
) -> Result<RemovalOutcome, RemovalFailure> {
    let name = identity.relative_path();
    let fence = fence_name(identity.asset_id());
    let fence_content = fence_bytes(
        identity.root_id(),
        identity.asset_id(),
        identity.sha256(),
        identity.size_bytes(),
    );
    // A lost database session can leave a live writer, so the fence is published first.
    publish_exact(root, &fence, &fence_content).map_err(|flow| match flow {
        Flow::Mismatch | Flow::Ownership => ownership(),
        other => removal_from_flow(other),
    })?;
    let mut file = match root.open_child(&name, Access::ReadDelete) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            root.sync().map_err(|_| delete_failed())?;
            return Ok(RemovalOutcome::AlreadyAbsent);
        }
        Err(error) => return Err(removal_from_flow(flow_from_io(error))),
    };
    let before = platform::stamp_of(&file).map_err(|_| delete_failed())?;
    let size = file.metadata().map_err(|_| delete_failed())?.len();
    if size != identity.size_bytes() {
        return Err(RemovalFailure::new(
            RemovalFailureCategory::IdentityMismatch,
        ));
    }
    let data = read_limited(&mut file, identity.size_bytes() + 1).map_err(|_| delete_failed())?;
    let after = platform::stamp_of(&file).map_err(|_| delete_failed())?;
    let still_named = root
        .name_refers_to(&name, &file)
        .map_err(|_| delete_failed())?;
    let digest = hex_encode(Sha256::digest(&data));
    if data.len() as u64 != identity.size_bytes()
        || digest != identity.sha256()
        || before != after
        || !still_named
    {
        return Err(RemovalFailure::new(
            RemovalFailureCategory::IdentityMismatch,
        ));
    }
    root.unlink(&name, &file).map_err(|_| delete_failed())?;
    drop(file);
    if !root.absent(&name).map_err(|_| delete_failed())? {
        return Err(delete_failed());
    }
    root.sync().map_err(|_| delete_failed())?;
    Ok(RemovalOutcome::Removed)
}

/// The owned asset namespace of one generated root.
///
/// `root` and `root_id` come from configuration (or are unset). Without both, every operation
/// refuses with unproven ownership and touches nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedAssetFiles {
    root: Option<String>,
    root_id: Option<Uuid>,
}

impl OwnedAssetFiles {
    /// Binds the namespace to a root path and its registered identity.
    #[must_use]
    pub const fn new(root: Option<String>, root_id: Option<Uuid>) -> Self {
        Self { root, root_id }
    }

    /// Initializes an existing, empty private root with its controls and never completes a
    /// partial initialization (`initialize_owned_root`).
    ///
    /// Only the controls are created; the marker is written last, after the capability is proven
    /// published. A non-empty root is refused and left untouched, except for stale owned temporary
    /// files of an interrupted publication. On any failure, the controls this call created on
    /// Windows are removed so that a retry sees an empty root.
    ///
    /// # Errors
    ///
    /// Fails with `OWNERSHIP_UNPROVEN` for a root that is missing, non-empty or unsafe,
    /// `FILE_DELETE_FAILED` for a transient sharing violation on the pinned root (retryable), and
    /// `UNSUPPORTED_PLATFORM` where the protocol is unavailable.
    pub fn initialize_owned_root(root: &str, root_id: Uuid) -> Result<(), RemovalFailure> {
        require_platform()?;
        let pinned = Root::open(root).map_err(root_pin_failure)?;
        pinned.verify().map_err(|_| ownership())?;
        let entries = pinned.list_names().map_err(|_| ownership())?;
        for name in entries {
            let tolerated = cfg!(windows) && name.to_str().is_some_and(is_owned_temp_name);
            if !tolerated {
                return Err(ownership());
            }
        }
        let mut created: Vec<&'static str> = Vec::new();
        match initialize_controls(&pinned, root_id, &mut created) {
            Ok(()) => {
                pinned.sync().map_err(|_| ownership())?;
                Ok(())
            }
            Err(failure) => {
                for name in created.iter().rev() {
                    let _ = pinned.discard_child(name);
                }
                Err(failure)
            }
        }
    }

    /// Proves the existing controls of the configured root under the namespace lock. Never
    /// initializes anything (`qualify_owned_root`).
    ///
    /// # Errors
    ///
    /// Fails with `OWNERSHIP_UNPROVEN` when the root, its controls or its permissions do not prove
    /// ownership, `FILE_DELETE_FAILED` while the namespace lock is busy and `UNSUPPORTED_PLATFORM`
    /// where the protocol is unavailable.
    pub fn qualify_owned_root(&self) -> Result<(), RemovalFailure> {
        let root_id = self.root_id.ok_or_else(ownership)?;
        let root = open_root(self.root.as_deref())?;
        if !root.qualifies_for_controls().map_err(|_| ownership())? {
            return Err(ownership());
        }
        enter_namespace(root, root_id).map(|_namespace| ())
    }

    /// Removes one owned asset after a fenced, verified identity check
    /// (`remove_owned`). An absent file is `AlreadyAbsent`; a mismatched file is preserved.
    ///
    /// # Errors
    ///
    /// Fails with a fixed category: `OWNERSHIP_UNPROVEN` for an identity from another root or an
    /// unproven root or file, `IDENTITY_MISMATCH` for a file that is not the recorded content,
    /// `FILE_DELETE_FAILED` for a busy or failed removal, and `UNSUPPORTED_PLATFORM`.
    pub fn remove_owned(&self, identity: &AssetIdentity) -> Result<RemovalOutcome, RemovalFailure> {
        if self.root_id != Some(identity.root_id()) {
            return Err(ownership());
        }
        let root = open_root(self.root.as_deref())?;
        let namespace = enter_namespace(root, identity.root_id())?;
        remove_identity(&namespace.root, identity)
    }

    /// Publishes a complete generated PDF under its immutable name
    /// (`materialize_owned`). The same bytes are idempotent, a different file is never replaced,
    /// and a retired identity is never resurrected.
    ///
    /// # Errors
    ///
    /// Fails with a fixed category: `IDENTITY_MISMATCH` for content that is not a PDF of the
    /// recorded size and digest, or a different file under the name; `RETIRED_ASSET` after a
    /// retirement fence; `OWNERSHIP_UNPROVEN`; `FILE_WRITE_FAILED`; and `UNSUPPORTED_PLATFORM`.
    pub fn materialize_owned(
        &self,
        identity: &AssetIdentity,
        content: &[u8],
    ) -> Result<PublicationOutcome, PublicationFailure> {
        if self.root_id != Some(identity.root_id()) {
            return Err(PublicationFailure::new(
                PublicationFailureCategory::OwnershipUnproven,
            ));
        }
        let identity_mismatch =
            PublicationFailure::new(PublicationFailureCategory::IdentityMismatch);
        let is_pdf = content.starts_with(b"%PDF-")
            && (1..=MAX_GENERATED_BYTES as usize).contains(&content.len());
        if !is_pdf
            || content.len() as u64 != identity.size_bytes()
            || hex_encode(Sha256::digest(content)) != identity.sha256()
        {
            return Err(identity_mismatch);
        }
        let root = open_root(self.root.as_deref()).map_err(publication_from_removal)?;
        let namespace =
            enter_namespace(root, identity.root_id()).map_err(publication_from_removal)?;
        let fence = fence_name(identity.asset_id());
        let fence_content = fence_bytes(
            identity.root_id(),
            identity.asset_id(),
            identity.sha256(),
            identity.size_bytes(),
        );
        match existing_exact(&namespace.root, &fence, &fence_content) {
            Ok(true) => {
                namespace.root.sync().map_err(|_| {
                    PublicationFailure::new(PublicationFailureCategory::FileWriteFailed)
                })?;
                return Err(PublicationFailure::new(
                    PublicationFailureCategory::RetiredAsset,
                ));
            }
            Ok(false) => {}
            Err(flow) => return Err(publication_from_flow(flow, true)),
        }
        match publish_exact(&namespace.root, &identity.relative_path(), content) {
            Ok(true) => Ok(PublicationOutcome::Created),
            Ok(false) => Ok(PublicationOutcome::AlreadyPresent),
            Err(flow) => Err(publication_from_flow(flow, false)),
        }
    }
}

/// Publication category of a flow failure. A retired fence mismatch is an ownership failure;
/// a mismatched asset is an identity mismatch.
fn publication_from_flow(flow: Flow, fence: bool) -> PublicationFailure {
    let category = match flow {
        Flow::Mismatch if fence => PublicationFailureCategory::OwnershipUnproven,
        Flow::Mismatch => PublicationFailureCategory::IdentityMismatch,
        Flow::Ownership => PublicationFailureCategory::OwnershipUnproven,
        Flow::Unsupported => PublicationFailureCategory::UnsupportedPlatform,
        Flow::Busy | Flow::Io => PublicationFailureCategory::FileWriteFailed,
    };
    PublicationFailure::new(category)
}

/// Creates the controls in order: the lock, then the capability (published
/// anonymously) and the marker last. Any failure is initialization unproven, except an
/// unsupported platform.
fn initialize_controls(
    root: &Root,
    root_id: Uuid,
    created: &mut Vec<&'static str>,
) -> Result<(), RemovalFailure> {
    // Feasibility is checked before anything is created, so an unsupported filesystem leaves the
    // root empty and the setup can be retried or reported as unsupported.
    root.require_capability_link()
        .map_err(|_| RemovalFailure::new(RemovalFailureCategory::UnsupportedPlatform))?;
    root.probe_publication().map_err(|error| match error {
        PublishError::Unsupported => {
            RemovalFailure::new(RemovalFailureCategory::UnsupportedPlatform)
        }
        _ => ownership(),
    })?;
    // A name is recorded only once this call created it, so a failed create (the name belongs to a
    // concurrent initializer) never makes the rollback remove another process's control.
    root.create_child_with(LOCK_NAME, b"")
        .map_err(|_| ownership())?;
    created.push(LOCK_NAME);
    let capability = platform::capability_bytes(root, root_id).map_err(|_| ownership())?;
    match publish_exact(root, CAPABILITY_NAME, &capability) {
        Ok(true) => created.push(CAPABILITY_NAME),
        Ok(false) => return Err(ownership()),
        Err(Flow::Unsupported) => {
            return Err(RemovalFailure::new(
                RemovalFailureCategory::UnsupportedPlatform,
            ));
        }
        Err(_) => return Err(ownership()),
    }
    created.push(MARKER_NAME);
    root.create_child_with(MARKER_NAME, &marker_bytes(root_id))
        .map_err(|_| ownership())?;
    Ok(())
}

// The only test needs the generated-asset backend, which exists on Linux and Windows only.
#[cfg(all(test, any(target_os = "linux", windows)))]
mod tests {
    use super::*;

    /// A name is recorded for rollback only when this call created it: a lock held by a
    /// concurrent initializer must never be on the list that a failed run discards.
    #[test]
    fn a_lock_created_by_someone_else_is_not_recorded_for_rollback() {
        let path = std::env::temp_dir().join(format!("jobniac-init-{}", Uuid::new_v4()));
        let text = path.to_str().expect("utf-8 temp path");
        platform::create_private_directory(text).expect("mkdir");
        let root = Root::open(text).expect("open root");
        root.create_child_with(LOCK_NAME, b"")
            .expect("foreign lock");
        let mut created = Vec::new();
        let result = initialize_controls(&root, Uuid::new_v4(), &mut created);
        let _ = std::fs::remove_dir_all(&path);
        assert!(result.is_err(), "the existing lock refuses initialization");
        assert!(created.is_empty(), "nothing was created by this call");
    }
}
