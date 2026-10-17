//! Platform obligations of the owned generated root: Windows sharing violations are retryable and
//! ACLs fail closed, junctions and symbolic links are never followed, POSIX permissions and
//! control modes are private. Synthetic data only.

mod files_support;

use files_support::Scratch;
use jobniac_infra::generated::asset_files::{
    AssetIdentity, OwnedAssetFiles, RemovalFailureCategory,
};
use jobniac_infra::generated::root::ensure_private_directory;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

const CONTENT: &[u8] = b"%PDF-synthetic platform publication";

/// An owned root with one published asset, in a scratch folder.
fn owned_with_asset() -> (Scratch, PathBuf, AssetIdentity, OwnedAssetFiles) {
    let scratch = Scratch::new();
    let root = scratch.join("generated");
    let root_text = root.to_string_lossy().into_owned();
    assert!(ensure_private_directory(&root_text));
    let root_id = Uuid::new_v4();
    OwnedAssetFiles::initialize_owned_root(&root_text, root_id).expect("initialize");
    let asset = Uuid::new_v4();
    let digest = hex::encode(Sha256::digest(CONTENT));
    let identity =
        AssetIdentity::new(asset, root_id, &digest, CONTENT.len() as u64).expect("identity");
    let files = OwnedAssetFiles::new(Some(root_text), Some(root_id));
    files
        .materialize_owned(&identity, CONTENT)
        .expect("publish");
    (scratch, root, identity, files)
}

#[cfg(windows)]
mod windows {
    use super::*;
    use files_support::{make_junction, skip_or_fail_without};
    use jobniac_infra::generated::asset_files::RemovalOutcome;
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;

    #[test]
    fn a_sharing_violation_on_an_asset_is_retryable_not_permanent() {
        let (_scratch, root, identity, files) = owned_with_asset();
        let asset = root.join(identity.relative_path());
        // Share mode 0: the holder denies every other access, as a scanner or indexer may.
        let holder = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&asset)
            .expect("hold asset");
        let failure = files
            .remove_owned(&identity)
            .expect_err("a busy asset is retryable");
        assert_eq!(failure.category(), RemovalFailureCategory::FileDeleteFailed);
        assert!(asset.exists());
        drop(holder);
        assert_eq!(files.remove_owned(&identity), Ok(RemovalOutcome::Removed));
        assert!(!asset.exists());
    }

    #[test]
    fn a_sharing_violation_on_a_pinned_ancestor_is_retryable_not_permanent() {
        let (_scratch, root, _identity, files) = owned_with_asset();
        let ancestor = root.parent().expect("ancestor directory");
        // A directory needs backup semantics to be opened at all. Share mode 0 denies every other
        // open of the ancestor, so pinning the root fails with a sharing violation.
        let holder = OpenOptions::new()
            .access_mode(0x0000_0001) // FILE_LIST_DIRECTORY: a data read, which share modes check
            .share_mode(0)
            .custom_flags(0x0200_0000) // FILE_FLAG_BACKUP_SEMANTICS
            .open(ancestor)
            .expect("hold ancestor");
        let failure = files
            .qualify_owned_root()
            .expect_err("a busy ancestor is retryable");
        assert_eq!(failure.category(), RemovalFailureCategory::FileDeleteFailed);
        drop(holder);
        files
            .qualify_owned_root()
            .expect("the root qualifies once the ancestor is free");
    }

    #[test]
    fn a_junctioned_root_is_never_followed() {
        let (scratch, root, identity, files) = owned_with_asset();
        let alias = scratch.join("alias");
        if let Err(error) = make_junction(&alias, &root) {
            skip_or_fail_without("junction creation", &error);
            return;
        }
        let through_alias = OwnedAssetFiles::new(
            Some(alias.to_string_lossy().into_owned()),
            Some(identity.root_id()),
        );
        assert_eq!(
            through_alias
                .remove_owned(&identity)
                .map_err(|e| e.category()),
            Err(RemovalFailureCategory::OwnershipUnproven)
        );
        assert!(root.join(identity.relative_path()).exists());
        assert!(files.qualify_owned_root().is_ok());
    }

    #[test]
    fn a_junctioned_ancestor_is_refused() {
        let (scratch, root, identity, _files) = owned_with_asset();
        let parent = root.parent().expect("parent").to_path_buf();
        let alias_parent = scratch.join("alias-parent");
        if let Err(error) = make_junction(&alias_parent, &parent) {
            skip_or_fail_without("junction creation", &error);
            return;
        }
        let through = OwnedAssetFiles::new(
            Some(
                alias_parent
                    .join("generated")
                    .to_string_lossy()
                    .into_owned(),
            ),
            Some(identity.root_id()),
        );
        assert_eq!(
            through.qualify_owned_root().map_err(|e| e.category()),
            Err(RemovalFailureCategory::OwnershipUnproven)
        );
        assert!(root.join(identity.relative_path()).exists());
    }

    fn grant(path: &Path, principal_and_rights: &str) {
        let status = std::process::Command::new("icacls")
            .arg(path)
            .arg("/grant")
            .arg(principal_and_rights)
            .status()
            .expect("run icacls");
        assert!(status.success(), "icacls grant applied");
    }

    #[test]
    fn a_root_readable_or_writable_by_everyone_fails_closed() {
        for rights in ["*S-1-1-0:(OI)(CI)R", "*S-1-1-0:(OI)(CI)W"] {
            let (_scratch, root, identity, files) = owned_with_asset();
            grant(&root, rights);
            assert_eq!(
                files.qualify_owned_root().map_err(|e| e.category()),
                Err(RemovalFailureCategory::OwnershipUnproven),
                "{rights}"
            );
            assert_eq!(
                files.remove_owned(&identity).map_err(|e| e.category()),
                Err(RemovalFailureCategory::OwnershipUnproven),
                "{rights}"
            );
            assert!(root.join(identity.relative_path()).exists());
        }
    }

    #[test]
    fn a_stale_owned_temporary_file_does_not_block_initialization() {
        let scratch = Scratch::new();
        let root = scratch.join("generated");
        let root_text = root.to_string_lossy().into_owned();
        assert!(ensure_private_directory(&root_text));
        let stale = root.join(format!(".jobniac-tmp-{}", "0123456789abcdef".repeat(2)));
        fs::write(&stale, b"partial").expect("stale temp");
        OwnedAssetFiles::initialize_owned_root(&root_text, Uuid::new_v4())
            .expect("an owned temporary name is tolerated");
        assert!(stale.exists());
        assert!(root.join(".jobniac-generated-root").exists());
    }

    #[test]
    fn controls_carry_the_windows_capability_mechanism() {
        let (_scratch, root, _identity, _files) = owned_with_asset();
        for name in [".jobniac-generated-root", ".jobniac-generated-lock"] {
            let metadata = fs::metadata(root.join(name)).expect("control");
            assert!(metadata.is_file());
        }
        let capability =
            fs::read_to_string(root.join(".jobniac-generated-capability")).expect("capability");
        assert!(capability.contains("windows-createnew-rename-noreplace"));
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn a_symbolic_link_root_is_never_followed() {
        let (scratch, root, identity, files) = owned_with_asset();
        let alias = scratch.join("alias");
        symlink(&root, &alias).expect("symlink");
        let through = OwnedAssetFiles::new(
            Some(alias.to_string_lossy().into_owned()),
            Some(identity.root_id()),
        );
        assert_eq!(
            through.remove_owned(&identity).map_err(|e| e.category()),
            Err(RemovalFailureCategory::OwnershipUnproven)
        );
        assert!(root.join(identity.relative_path()).exists());
        assert!(files.qualify_owned_root().is_ok());
    }

    #[test]
    fn a_group_or_world_accessible_root_fails_closed() {
        let (_scratch, root, identity, files) = owned_with_asset();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(
            files.qualify_owned_root().map_err(|e| e.category()),
            Err(RemovalFailureCategory::OwnershipUnproven)
        );
        assert_eq!(
            files.remove_owned(&identity).map_err(|e| e.category()),
            Err(RemovalFailureCategory::OwnershipUnproven)
        );
        assert!(root.join(identity.relative_path()).exists());
    }

    #[test]
    fn a_stale_temporary_file_is_refused_on_linux() {
        let scratch = Scratch::new();
        let root = scratch.join("generated");
        let root_text = root.to_string_lossy().into_owned();
        assert!(ensure_private_directory(&root_text));
        fs::write(
            root.join(".jobniac-tmp-0123456789abcdef0123456789abcdef"),
            b"partial",
        )
        .expect("stale temp");
        assert!(OwnedAssetFiles::initialize_owned_root(&root_text, Uuid::new_v4()).is_err());
    }

    #[test]
    fn controls_and_published_assets_are_owner_only() {
        let (_scratch, root, identity, _files) = owned_with_asset();
        let asset_name = identity.relative_path();
        let names = [
            ".jobniac-generated-root",
            ".jobniac-generated-lock",
            ".jobniac-generated-capability",
            asset_name.as_str(),
        ];
        for name in names {
            let mode = fs::metadata(root.join(name))
                .expect("file")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{name}");
        }
    }
}
