//! Pinned no-follow primitives on real files.
//!
//! These tests drive [`DirChain`] and [`PinnedFile`] directly, so they cover the handle rules
//! themselves rather than the import reader's refusal order. They run on the targets where the
//! pinned reads exist.

#![cfg(any(
    windows,
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
))]

mod files_support;

use std::fs::OpenOptions;
use std::io::Write;

use files_support::{SAMPLE_PDF, Scratch};
use jobniac_infra::files::no_follow::{DirChain, NodeKind};

#[test]
fn pinned_leaf_reports_a_regular_file_with_its_size() {
    let scratch = Scratch::new();
    scratch.write("nested/resume.pdf", SAMPLE_PDF);
    let names = vec!["nested".to_string()];
    let chain = DirChain::pin(scratch.path(), &names).expect("pin chain");
    let file = chain.open_file("resume.pdf").expect("open leaf");
    let info = file.info().expect("sample handle");
    assert_eq!(info.kind, NodeKind::Regular);
    assert_eq!(info.size(), SAMPLE_PDF.len() as u64);
}

#[test]
fn missing_leaf_is_reported_as_not_found() {
    let scratch = Scratch::new();
    let chain = DirChain::pin(scratch.path(), &[]).expect("pin base");
    let error = chain.open_file("absent.pdf").expect_err("missing leaf");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn reads_are_bounded_by_the_requested_limit() {
    let scratch = Scratch::new();
    scratch.write("resume.pdf", SAMPLE_PDF);
    let chain = DirChain::pin(scratch.path(), &[]).expect("pin base");
    let mut file = chain.open_file("resume.pdf").expect("open leaf");
    let head = file.read_up_to(4).expect("bounded read");
    assert_eq!(head, &SAMPLE_PDF[..4]);
}

/// A file changed between the two identity samples must not look unchanged. On Linux the writer
/// is admitted and the size or timestamps differ. On Windows the handle shares read only, so the
/// writer is refused while the file is held.
#[test]
fn change_to_the_held_file_is_detected_or_blocked() {
    let scratch = Scratch::new();
    let path = scratch.write("resume.pdf", SAMPLE_PDF);
    let chain = DirChain::pin(scratch.path(), &[]).expect("pin base");
    let file = chain.open_file("resume.pdf").expect("open leaf");
    let before = file.info().expect("first sample");

    let writer = OpenOptions::new().append(true).open(&path);
    if cfg!(windows) {
        assert!(
            writer.is_err(),
            "a writer must be refused while the file is held"
        );
        let after = file.info().expect("second sample");
        assert_eq!(before.identity, after.identity);
    } else {
        let mut writer = writer.expect("Linux admits the writer");
        writer.write_all(b"more").expect("append");
        writer.sync_all().expect("sync");
        drop(writer);
        let after = file.info().expect("second sample");
        assert_ne!(before.identity, after.identity);
    }
}

#[test]
fn opening_the_leaf_does_not_modify_the_file() {
    let scratch = Scratch::new();
    let path = scratch.write("resume.pdf", SAMPLE_PDF);
    let modified_before = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let chain = DirChain::pin(scratch.path(), &[]).expect("pin base");
    let mut file = chain.open_file("resume.pdf").expect("open leaf");
    let content = file.read_up_to(1024).expect("read");
    assert_eq!(content, SAMPLE_PDF);
    drop(file);
    let modified_after = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    assert_eq!(modified_before, modified_after);
    assert_eq!(std::fs::read(&path).expect("reread"), SAMPLE_PDF);
}

#[test]
fn chain_names_must_be_plain_components() {
    let scratch = Scratch::new();
    scratch.write("inner/resume.pdf", SAMPLE_PDF);
    for bad in ["..", ".", "", "a/b", "/etc", "nul\0name"] {
        let names = vec![bad.to_string()];
        let error = DirChain::pin(scratch.path(), &names).expect_err("bad pin name");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{bad:?}");
    }
    let names = vec!["inner".to_string()];
    let chain = DirChain::pin(scratch.path(), &names).expect("pin chain");
    for bad in ["../inner/resume.pdf", "/etc/hostname", "..", "", "a/b"] {
        let error = chain.open_file(bad).expect_err("bad leaf name");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{bad:?}");
    }
}

/// A link at the leaf or inside the chain is refused on every architecture: the open flags come
/// from `libc`, not from hand-written x86 values.
#[cfg(target_os = "linux")]
#[test]
fn links_are_refused_at_the_leaf_and_in_the_chain() {
    let scratch = Scratch::new();
    scratch.write("real/resume.pdf", SAMPLE_PDF);
    std::os::unix::fs::symlink(
        scratch.join("real/resume.pdf"),
        scratch.join("real/link.pdf"),
    )
    .expect("leaf link");
    std::os::unix::fs::symlink(scratch.join("real"), scratch.join("dirlink")).expect("dir link");
    let chain = DirChain::pin(scratch.path(), &["real".to_string()]).expect("pin chain");
    let error = chain.open_file("link.pdf").expect_err("leaf link refused");
    assert_eq!(error.raw_os_error(), Some(libc::ELOOP));
    assert!(DirChain::pin(scratch.path(), &["dirlink".to_string()]).is_err());
}
