//! Publication and retirement-fence obligations of the owned generated-asset namespace: a failed
//! anonymous write leaves no named partial file, and every unqualified retirement fence (wrong
//! content, symbolic link, hard link) blocks both removal and publication while the payload stays
//! intact. Synthetic data only.

// The owned generated-asset namespace exists only on Linux and Windows.
#![cfg(any(target_os = "linux", windows))]

mod files_support;

use files_support::Scratch;
use jobniac_infra::generated::asset_files::{
    AssetIdentity, OwnedAssetFiles, PublicationFailureCategory, RemovalFailureCategory,
};
use jobniac_infra::generated::root::ensure_private_directory;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const CONTENT: &[u8] = b"%PDF-synthetic complete publication";
/// The controls of an initialized root, sorted.
#[cfg(unix)]
const CONTROLS: [&str; 3] = [
    ".jobniac-generated-capability",
    ".jobniac-generated-lock",
    ".jobniac-generated-root",
];

/// An initialized, empty owned root with one identity ready to publish. The scratch folder is kept
/// alive for the duration of the test and removed with it.
struct Owned {
    _scratch: Scratch,
    root: PathBuf,
    identity: AssetIdentity,
    files: OwnedAssetFiles,
}

impl Owned {
    fn new() -> Self {
        let scratch = Scratch::new();
        let root = scratch.join("generated");
        let root_text = root.to_string_lossy().into_owned();
        assert!(ensure_private_directory(&root_text), "private root created");
        let root_id = Uuid::new_v4();
        OwnedAssetFiles::initialize_owned_root(&root_text, root_id).expect("initialize root");
        let digest = hex::encode(Sha256::digest(CONTENT));
        let identity = AssetIdentity::new(Uuid::new_v4(), root_id, &digest, CONTENT.len() as u64)
            .expect("valid identity");
        let files = OwnedAssetFiles::new(Some(root_text), Some(root_id));
        Self {
            _scratch: scratch,
            root,
            identity,
            files,
        }
    }

    /// An owned root with the asset already published.
    fn published() -> Self {
        let owned = Self::new();
        owned
            .files
            .materialize_owned(&owned.identity, CONTENT)
            .expect("publish");
        owned
    }

    fn asset_path(&self) -> PathBuf {
        self.root.join(self.identity.relative_path())
    }

    fn fence_path(&self) -> PathBuf {
        self.root
            .join(format!(".jobniac-retired-{}", self.identity.asset_id()))
    }

    #[cfg(unix)]
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.root)
            .expect("list root")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }
}

/// Plants a fence with `bytes`, private on Unix so that only its content can be at fault.
fn plant_fence(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("plant fence");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("private fence");
    }
}

/// With an unqualified fence present, removal and publication both refuse with unproven
/// ownership, and the published payload is never touched.
fn assert_unqualified_fence_refuses(owned: &Owned) {
    let removal = owned
        .files
        .remove_owned(&owned.identity)
        .expect_err("an unqualified fence blocks removal");
    assert_eq!(
        removal.category(),
        RemovalFailureCategory::OwnershipUnproven
    );
    let publication = owned
        .files
        .materialize_owned(&owned.identity, CONTENT)
        .expect_err("an unqualified fence blocks publication");
    assert_eq!(
        publication.category(),
        PublicationFailureCategory::OwnershipUnproven
    );
    assert_eq!(fs::read(owned.asset_path()).expect("payload kept"), CONTENT);
}

#[test]
fn a_fence_with_wrong_content_blocks_removal_and_publication_and_keeps_the_payload() {
    let owned = Owned::published();
    plant_fence(&owned.fence_path(), b"wrong root or identity");
    assert_unqualified_fence_refuses(&owned);
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_fence_blocks_removal_and_publication_and_keeps_the_payload() {
    let owned = Owned::published();
    std::os::unix::fs::symlink(owned.asset_path(), owned.fence_path()).expect("plant symlink");
    assert_unqualified_fence_refuses(&owned);
}

#[cfg(unix)]
#[test]
fn a_hard_linked_fence_blocks_removal_and_publication_and_keeps_the_payload() {
    let owned = Owned::published();
    fs::hard_link(owned.asset_path(), owned.fence_path()).expect("plant hard link");
    assert_unqualified_fence_refuses(&owned);
}

#[cfg(unix)]
#[test]
#[allow(clippy::print_stdout)]
fn a_failed_anonymous_write_leaves_no_named_partial_file() {
    use std::os::unix::fs::PermissionsExt;
    let owned = Owned::new();
    let original = fs::metadata(&owned.root)
        .expect("root metadata")
        .permissions();
    // Searchable but not writable: the anonymous file cannot be created, so no name can appear.
    fs::set_permissions(&owned.root, fs::Permissions::from_mode(0o500)).expect("lock root");
    let probe = owned.root.join(".jobniac-probe");
    let writable_anyway = fs::File::create(&probe).is_ok();
    let _ = fs::remove_file(&probe);
    if writable_anyway {
        // Privileged processes ignore the mode bits, so the failure cannot be provoked here.
        fs::set_permissions(&owned.root, original).expect("unlock root");
        println!("skipped: the directory mode does not restrict this process");
        return;
    }
    let outcome = owned.files.materialize_owned(&owned.identity, CONTENT);
    fs::set_permissions(&owned.root, original).expect("unlock root");
    let failure = outcome.expect_err("a failed anonymous write is a publication failure");
    assert_eq!(
        failure.category(),
        PublicationFailureCategory::FileWriteFailed
    );
    assert_eq!(owned.names(), CONTROLS.map(str::to_owned).to_vec());
    assert!(!owned.asset_path().exists());
}
