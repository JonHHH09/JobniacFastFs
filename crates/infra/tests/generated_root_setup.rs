//! Generated-root directory handling and the filesystem side of setup, on synthetic data.

// The tests that need the files and generated backends run on Linux and Windows only; macOS fails
// closed there, so their helpers are unused on macOS.
#![cfg_attr(
    not(any(target_os = "linux", windows)),
    allow(dead_code, unused_imports)
)]

mod files_support;

use files_support::Scratch;
use jobniac_core::generated_root::{RootPlatform, marker_bytes};
use jobniac_infra::generated::root::{ensure_private_directory, host_platform, is_plain_directory};
use jobniac_infra::generated::setup::{RootDirectoryState, RootSetupFiles};
use std::fs;
use uuid::Uuid;

#[cfg(any(target_os = "linux", windows))]
#[test]
fn ensure_creates_the_missing_chain_and_leaves_an_existing_directory_alone() {
    let scratch = Scratch::new();
    let target = scratch.join("a/b/generated");
    let text = target.to_string_lossy().into_owned();
    assert!(ensure_private_directory(&text));
    assert!(is_plain_directory(&text));
    fs::write(target.join("kept.txt"), b"keep").expect("file inside");
    assert!(ensure_private_directory(&text));
    assert!(target.join("kept.txt").exists());
}

#[test]
fn ensure_refuses_a_file_in_the_way() {
    let scratch = Scratch::new();
    let blocker = scratch.write("file", b"x");
    let text = blocker.to_string_lossy().into_owned();
    assert!(!ensure_private_directory(&text));
    assert!(!ensure_private_directory(&format!("{text}/child")));
    assert!(!is_plain_directory(&text));
}

#[test]
fn plain_directory_check_refuses_missing_and_file_paths() {
    let scratch = Scratch::new();
    assert!(!is_plain_directory(&scratch.text("missing")));
    assert!(is_plain_directory(&scratch.root_text()));
}

#[test]
fn host_platform_matches_the_compiled_target() {
    let expected = if cfg!(windows) {
        RootPlatform::Windows
    } else if cfg!(target_os = "macos") {
        RootPlatform::MacOs
    } else {
        RootPlatform::Other
    };
    assert_eq!(host_platform(), expected);
}

#[test]
fn an_absent_explicit_root_is_unusable_and_never_created() {
    let scratch = Scratch::new();
    let explicit = scratch.text("explicit-missing");
    let setup = RootSetupFiles::new(explicit.clone(), false);
    assert_eq!(setup.inspect(), RootDirectoryState::Unusable);
    assert!(!scratch.join("explicit-missing").exists());
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn a_missing_default_root_is_created_privately_and_then_reads_as_empty() {
    let scratch = Scratch::new();
    let default = scratch.text("default/generated");
    let setup = RootSetupFiles::new(default.clone(), true);
    assert_eq!(setup.inspect(), RootDirectoryState::Empty);
    assert!(is_plain_directory(&default));
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn setup_initializes_qualifies_and_reports_the_marker_identity() {
    let scratch = Scratch::new();
    let root = scratch.text("generated");
    assert!(ensure_private_directory(&root));
    let root_id = Uuid::new_v4();
    let setup = RootSetupFiles::new(root.clone(), false);
    assert_eq!(setup.inspect(), RootDirectoryState::Empty);
    setup.initialize(root_id).expect("initialize");
    setup.qualify(root_id).expect("qualify");
    assert_eq!(setup.inspect(), RootDirectoryState::Marked);
    assert_eq!(setup.read_marker_identity(), Some(root_id));
    assert!(
        setup.qualify(Uuid::new_v4()).is_err(),
        "another identity is not proven"
    );
}

#[test]
fn a_non_empty_root_without_a_marker_is_foreign_and_untouched() {
    let scratch = Scratch::new();
    let root = scratch.write("generated/user.txt", b"user content");
    let setup = RootSetupFiles::new(
        root.parent()
            .expect("parent")
            .to_string_lossy()
            .into_owned(),
        false,
    );
    assert_eq!(setup.inspect(), RootDirectoryState::Foreign);
    assert_eq!(fs::read(&root).expect("user file"), b"user content");
    assert_eq!(setup.read_marker_identity(), None);
}

#[test]
fn malformed_markers_give_no_identity_and_make_the_root_foreign() {
    let good = marker_bytes(Uuid::new_v4());
    let cases: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"jobniac-generated-root-v2\n00000000-0000-4000-8000-000000000001\n".to_vec(),
        b"jobniac-generated-root-v1\n00000000-0000-4000-8000-00000000000A\n".to_vec(),
        b"jobniac-generated-root-v1\n00000000-0000-4000-8000-000000000001".to_vec(),
        [good.clone(), b"x".to_vec()].concat(),
        [good.clone(), vec![b' '; 128]].concat(),
    ];
    for content in cases {
        let scratch = Scratch::new();
        let root = scratch.dir("generated");
        fs::write(root.join(".jobniac-generated-root"), &content).expect("marker");
        let setup = RootSetupFiles::new(root.to_string_lossy().into_owned(), false);
        assert_eq!(setup.read_marker_identity(), None, "content {content:?}");
        assert_eq!(
            setup.inspect(),
            RootDirectoryState::Foreign,
            "content {content:?}"
        );
    }
}

#[test]
fn a_well_formed_marker_yields_only_a_candidate() {
    let scratch = Scratch::new();
    let root = scratch.dir("generated");
    let identity = Uuid::new_v4();
    fs::write(root.join(".jobniac-generated-root"), marker_bytes(identity)).expect("marker");
    let setup = RootSetupFiles::new(root.to_string_lossy().into_owned(), false);
    assert_eq!(setup.read_marker_identity(), Some(identity));
    assert_eq!(setup.inspect(), RootDirectoryState::Marked);
    assert!(
        setup.qualify(identity).is_err(),
        "a marker without controls is never proof"
    );
}
