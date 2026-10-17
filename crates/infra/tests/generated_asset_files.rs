//! Owned generated-asset namespace on the host platform: controls, create-only publication, fenced
//! retirement and removal, busy locks and fail-closed roots, on synthetic data.

mod files_support;

use files_support::{SAMPLE_PDF, Scratch, skip_or_fail_without};
use jobniac_infra::generated::asset_files::{
    AssetIdentity, OwnedAssetFiles, PublicationFailureCategory, PublicationOutcome,
    RemovalFailureCategory, RemovalOutcome,
};
use jobniac_infra::generated::root::ensure_private_directory;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const CONTENT: &[u8] = b"%PDF-synthetic complete publication";
const CONTROLS: [&str; 3] = [
    ".jobniac-generated-capability",
    ".jobniac-generated-lock",
    ".jobniac-generated-root",
];

/// An initialized, empty owned root with one identity ready to publish.
struct Owned {
    scratch: Scratch,
    root: PathBuf,
    root_id: Uuid,
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
        let asset_id = Uuid::new_v4();
        let identity = identity_for(asset_id, root_id, CONTENT);
        let files = OwnedAssetFiles::new(Some(root_text), Some(root_id));
        Self {
            scratch,
            root,
            root_id,
            identity,
            files,
        }
    }

    fn asset_path(&self) -> PathBuf {
        self.root.join(self.identity.relative_path())
    }

    fn names(&self) -> Vec<String> {
        sorted_names(&self.root)
    }
}

/// Plants a file the way a foreign publisher with the owner's private settings would. The mode is
/// explicit, so the test does not depend on the umask of the host (a 0022 or 0002 umask would make
/// a plain `fs::write` group or world readable, which is rightly refused as unproven ownership).
fn plant_private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("plant a different file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("private mode");
    }
}

fn identity_for(asset_id: Uuid, root_id: Uuid, content: &[u8]) -> AssetIdentity {
    let digest = hex::encode(Sha256::digest(content));
    AssetIdentity::new(asset_id, root_id, &digest, content.len() as u64).expect("valid identity")
}

fn sorted_names(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root)
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

fn expected_controls(extra: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = CONTROLS.iter().map(|name| (*name).to_owned()).collect();
    names.extend(extra.iter().map(|name| (*name).to_owned()));
    names.sort();
    names
}

/// Holds the namespace lock the way a concurrent owner would, so that the operation under test
/// sees a busy namespace.
fn hold_namespace_lock(root: &Path) -> File {
    let path = root.join(".jobniac-generated-lock");
    #[cfg(windows)]
    {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(path)
            .expect("hold lock")
    }
    #[cfg(not(windows))]
    {
        let file = File::open(path).expect("open lock");
        file.try_lock().expect("hold lock");
        file
    }
}

#[test]
fn initialization_creates_only_the_controls_and_they_qualify_read_only() {
    let owned = Owned::new();
    assert_eq!(owned.names(), expected_controls(&[]));
    let before: Vec<Vec<u8>> = CONTROLS
        .iter()
        .map(|name| fs::read(owned.root.join(name)).expect("read control"))
        .collect();
    owned.files.qualify_owned_root().expect("qualifies");
    let after: Vec<Vec<u8>> = CONTROLS
        .iter()
        .map(|name| fs::read(owned.root.join(name)).expect("read control"))
        .collect();
    assert_eq!(before, after, "qualification never changes the controls");
    assert!(
        fs::read_to_string(owned.root.join(".jobniac-generated-root"))
            .expect("marker")
            .starts_with("jobniac-generated-root-v1\n")
    );
}

#[test]
fn publication_is_complete_idempotent_and_leaves_no_temporary_file() {
    let owned = Owned::new();
    assert_eq!(
        owned.files.materialize_owned(&owned.identity, CONTENT),
        Ok(PublicationOutcome::Created)
    );
    assert_eq!(fs::read(owned.asset_path()).expect("asset"), CONTENT);
    assert_eq!(
        owned.files.materialize_owned(&owned.identity, CONTENT),
        Ok(PublicationOutcome::AlreadyPresent)
    );
    assert_eq!(
        owned.names(),
        expected_controls(&[&owned.identity.relative_path()])
    );
}

#[test]
fn wrong_input_never_writes_anything() {
    let owned = Owned::new();
    let not_pdf = b"plain text, not a PDF";
    let wrong = identity_for(owned.identity.asset_id(), owned.root_id, not_pdf);
    let failure = owned
        .files
        .materialize_owned(&wrong, not_pdf)
        .expect_err("non-PDF content is refused");
    assert_eq!(
        failure.category(),
        PublicationFailureCategory::IdentityMismatch
    );
    let mismatched = owned
        .files
        .materialize_owned(&owned.identity, b"%PDF-other")
        .expect_err("digest mismatch is refused");
    assert_eq!(
        mismatched.category(),
        PublicationFailureCategory::IdentityMismatch
    );
    assert_eq!(owned.names(), expected_controls(&[]));
}

#[test]
fn an_existing_mismatched_file_is_preserved_without_replacement() {
    let owned = Owned::new();
    // Same size, different bytes: only the content comparison can tell them apart.
    let mut other = CONTENT.to_vec();
    other[10] ^= 1;
    plant_private(&owned.asset_path(), &other);
    let failure = owned
        .files
        .materialize_owned(&owned.identity, CONTENT)
        .expect_err("a different file is never replaced");
    assert_eq!(
        failure.category(),
        PublicationFailureCategory::IdentityMismatch
    );
    assert_eq!(fs::read(owned.asset_path()).expect("asset"), other);
}

#[test]
fn retirement_fence_blocks_a_late_writer_after_removal() {
    let owned = Owned::new();
    owned
        .files
        .materialize_owned(&owned.identity, CONTENT)
        .expect("publish");
    assert_eq!(
        owned.files.remove_owned(&owned.identity),
        Ok(RemovalOutcome::Removed)
    );
    assert!(!owned.asset_path().exists());
    let late = owned
        .files
        .materialize_owned(&owned.identity, CONTENT)
        .expect_err("a retired identity is never resurrected");
    assert_eq!(late.category(), PublicationFailureCategory::RetiredAsset);
    assert!(!owned.asset_path().exists());
}

#[test]
fn removal_of_a_retired_identity_is_idempotent() {
    let owned = Owned::new();
    owned
        .files
        .materialize_owned(&owned.identity, CONTENT)
        .expect("publish");
    assert_eq!(
        owned.files.remove_owned(&owned.identity),
        Ok(RemovalOutcome::Removed)
    );
    assert_eq!(
        owned.files.remove_owned(&owned.identity),
        Ok(RemovalOutcome::AlreadyAbsent)
    );
    // The permanent retirement fence stays: it is what blocks late writers.
    let fence = format!(".jobniac-retired-{}", owned.identity.asset_id());
    assert_eq!(owned.names(), expected_controls(&[&fence]));
}

#[test]
fn a_replaced_asset_is_preserved_and_reported_as_a_mismatch() {
    let owned = Owned::new();
    owned
        .files
        .materialize_owned(&owned.identity, CONTENT)
        .expect("publish");
    let other = b"%PDF-changed bytes!!";
    fs::write(owned.asset_path(), other).expect("replace content");
    let failure = owned
        .files
        .remove_owned(&owned.identity)
        .expect_err("mismatched content is never deleted");
    assert_eq!(failure.category(), RemovalFailureCategory::IdentityMismatch);
    assert_eq!(fs::read(owned.asset_path()).expect("asset"), other);
}

#[test]
fn a_hard_linked_asset_is_preserved() {
    let owned = Owned::new();
    owned
        .files
        .materialize_owned(&owned.identity, CONTENT)
        .expect("publish");
    let alias = owned.scratch.join("outside-alias.pdf");
    fs::hard_link(owned.asset_path(), &alias).expect("hard link");
    let failure = owned
        .files
        .remove_owned(&owned.identity)
        .expect_err("a multiply linked asset is not owned");
    assert_eq!(
        failure.category(),
        RemovalFailureCategory::OwnershipUnproven
    );
    assert!(owned.asset_path().exists() && alias.exists());
}

#[test]
fn a_symbolic_link_asset_is_preserved() {
    let owned = Owned::new();
    let target = owned.scratch.write("elsewhere.pdf", CONTENT);
    if let Err(error) = symlink_file(&target, &owned.asset_path()) {
        skip_or_fail_without("symbolic-link privilege", &error);
        return;
    }
    let failure = owned
        .files
        .remove_owned(&owned.identity)
        .expect_err("a symbolic link is not an owned asset");
    assert_eq!(
        failure.category(),
        RemovalFailureCategory::OwnershipUnproven
    );
    assert!(target.exists());
}

#[cfg(unix)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[test]
fn a_busy_namespace_lock_is_retried_without_deletion() {
    let owned = Owned::new();
    owned
        .files
        .materialize_owned(&owned.identity, CONTENT)
        .expect("publish");
    let holder = hold_namespace_lock(&owned.root);
    let failure = owned
        .files
        .remove_owned(&owned.identity)
        .expect_err("a busy namespace is not a permanent failure");
    assert_eq!(failure.category(), RemovalFailureCategory::FileDeleteFailed);
    assert!(owned.asset_path().exists());
    drop(holder);
    assert_eq!(
        owned.files.remove_owned(&owned.identity),
        Ok(RemovalOutcome::Removed)
    );
    assert!(!owned.asset_path().exists());
}

#[test]
fn a_missing_control_leaves_ownership_unproven() {
    for control in CONTROLS {
        let owned = Owned::new();
        owned
            .files
            .materialize_owned(&owned.identity, CONTENT)
            .expect("publish");
        fs::remove_file(owned.root.join(control)).expect("remove control");
        assert_eq!(
            owned.files.qualify_owned_root().map_err(|e| e.category()),
            Err(RemovalFailureCategory::OwnershipUnproven),
            "control {control}"
        );
        assert_eq!(
            owned
                .files
                .remove_owned(&owned.identity)
                .map_err(|e| e.category()),
            Err(RemovalFailureCategory::OwnershipUnproven),
            "control {control}"
        );
        assert!(owned.asset_path().exists());
    }
}

#[test]
fn a_tampered_marker_or_capability_never_authorizes_removal() {
    for control in [".jobniac-generated-root", ".jobniac-generated-capability"] {
        let owned = Owned::new();
        owned
            .files
            .materialize_owned(&owned.identity, CONTENT)
            .expect("publish");
        fs::write(
            owned.root.join(control),
            b"jobniac-generated-root-v1\nnot-the-root\n",
        )
        .expect("tamper");
        assert_eq!(
            owned.files.qualify_owned_root().map_err(|e| e.category()),
            Err(RemovalFailureCategory::OwnershipUnproven),
            "control {control}"
        );
        assert_eq!(
            owned
                .files
                .remove_owned(&owned.identity)
                .map_err(|e| e.category()),
            Err(RemovalFailureCategory::OwnershipUnproven)
        );
        assert!(owned.asset_path().exists());
    }
}

#[test]
fn controls_copied_to_another_root_do_not_transfer_the_capability() {
    let owned = Owned::new();
    let other = owned.scratch.join("copied");
    assert!(ensure_private_directory(&other.to_string_lossy()));
    for control in CONTROLS {
        fs::copy(owned.root.join(control), other.join(control)).expect("copy control");
    }
    let copied = OwnedAssetFiles::new(
        Some(other.to_string_lossy().into_owned()),
        Some(owned.root_id),
    );
    assert_eq!(
        copied.qualify_owned_root().map_err(|e| e.category()),
        Err(RemovalFailureCategory::OwnershipUnproven)
    );
}

#[test]
fn initialization_refuses_a_non_empty_root_and_touches_nothing() {
    let scratch = Scratch::new();
    let root = scratch.join("generated");
    let root_text = root.to_string_lossy().into_owned();
    assert!(ensure_private_directory(&root_text));
    fs::write(root.join("keep.txt"), b"user content").expect("user file");
    let failure = OwnedAssetFiles::initialize_owned_root(&root_text, Uuid::new_v4())
        .expect_err("a non-empty root is never adopted");
    assert_eq!(
        failure.category(),
        RemovalFailureCategory::OwnershipUnproven
    );
    assert_eq!(sorted_names(&root), vec!["keep.txt".to_owned()]);
}

#[test]
fn a_foreign_name_blocks_initialization() {
    let scratch = Scratch::new();
    let root = scratch.join("generated");
    let root_text = root.to_string_lossy().into_owned();
    assert!(ensure_private_directory(&root_text));
    fs::write(root.join(".jobniac-tmp-notours"), b"x").expect("foreign temp name");
    assert!(OwnedAssetFiles::initialize_owned_root(&root_text, Uuid::new_v4()).is_err());
    assert_eq!(sorted_names(&root), vec![".jobniac-tmp-notours".to_owned()]);
}

#[test]
fn unconfigured_or_invalid_roots_never_create_or_delete() {
    let scratch = Scratch::new();
    let absent = scratch.join("absent");
    let identity = identity_for(Uuid::new_v4(), Uuid::new_v4(), CONTENT);
    let roots = [
        None,
        Some(String::new()),
        Some("relative".to_owned()),
        Some(absent.to_string_lossy().into_owned()),
    ];
    for root in roots {
        let files = OwnedAssetFiles::new(root, Some(identity.root_id()));
        assert_eq!(
            files.remove_owned(&identity).map_err(|e| e.category()),
            Err(RemovalFailureCategory::OwnershipUnproven)
        );
    }
    assert!(!absent.exists());
}

#[test]
fn an_identity_from_another_root_is_refused() {
    let owned = Owned::new();
    let foreign = identity_for(Uuid::new_v4(), Uuid::new_v4(), CONTENT);
    assert_eq!(
        owned.files.remove_owned(&foreign).map_err(|e| e.category()),
        Err(RemovalFailureCategory::OwnershipUnproven)
    );
    assert_eq!(
        owned
            .files
            .materialize_owned(&foreign, CONTENT)
            .map_err(|e| e.category()),
        Err(PublicationFailureCategory::OwnershipUnproven)
    );
}

#[test]
fn qualification_without_a_registered_identity_is_refused() {
    let owned = Owned::new();
    let unbound = OwnedAssetFiles::new(Some(owned.root.to_string_lossy().into_owned()), None);
    assert_eq!(
        unbound.qualify_owned_root().map_err(|e| e.category()),
        Err(RemovalFailureCategory::OwnershipUnproven)
    );
}

#[test]
fn asset_identity_admits_only_canonical_digests_and_bounded_sizes() {
    let asset = Uuid::new_v4();
    let root = Uuid::new_v4();
    let digest = hex::encode(Sha256::digest(SAMPLE_PDF));
    assert!(AssetIdentity::new(asset, root, &digest, 10).is_ok());
    assert!(AssetIdentity::new(asset, root, &digest.to_uppercase(), 10).is_err());
    assert!(AssetIdentity::new(asset, root, "abc", 10).is_err());
    assert!(AssetIdentity::new(asset, root, &digest, 0).is_err());
    assert!(AssetIdentity::new(asset, root, &digest, 10 * 1024 * 1024 + 1).is_err());
    let identity = AssetIdentity::new(asset, root, &digest, 10).expect("identity");
    assert_eq!(identity.relative_path(), format!("{asset}.pdf"));
}

#[test]
fn a_group_readable_file_under_the_asset_name_is_an_unproven_owner_not_a_mismatch() {
    let owned = Owned::new();
    plant_private(&owned.asset_path(), CONTENT);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(owned.asset_path(), fs::Permissions::from_mode(0o644))
            .expect("widen mode");
        let failure = owned
            .files
            .materialize_owned(&owned.identity, CONTENT)
            .expect_err("a non-private file is never trusted");
        assert_eq!(
            failure.category(),
            PublicationFailureCategory::OwnershipUnproven
        );
    }
}

#[test]
fn concurrent_first_run_creation_of_a_missing_root_succeeds_for_every_caller() {
    let scratch = Scratch::new();
    let target = scratch.join("a/b/generated");
    let text = target.to_string_lossy().into_owned();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (barrier, text) = (std::sync::Arc::clone(&barrier), text.clone());
            std::thread::spawn(move || {
                barrier.wait();
                ensure_private_directory(&text)
            })
        })
        .collect();
    for handle in handles {
        assert!(handle.join().expect("thread"), "every racer sees a root");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&target).expect("leaf").permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
