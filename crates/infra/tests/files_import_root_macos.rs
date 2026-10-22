//! Import root reads on macOS: symbolic links in every position, FIFOs and devices, hard links,
//! resource-fork paths, case variants, firmlinks and a chain that is renamed or replaced while it
//! is held.
//!
//! Scratch folders are canonicalized (see `files_support`): `/var` and `/tmp` are symbolic links
//! to `/private/...`, so a path built from `std::env::temp_dir()` would contain a link and be
//! refused, which is the production behaviour.

#![cfg(target_os = "macos")]

mod files_support;

use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;

use files_support::{SAMPLE_PDF, Scratch, skip_or_fail_without};
use jobniac_infra::files::import_root::{ImportRefusal, ImportRoot};
use jobniac_infra::files::no_follow::DirChain;

fn root_for(scratch: &Scratch) -> ImportRoot {
    ImportRoot::new(Some(scratch.root_text()))
}

#[test]
fn symlink_in_a_directory_component_is_refused() {
    let scratch = Scratch::new();
    let real = scratch.dir("real");
    scratch.write("real/resume.pdf", SAMPLE_PDF);
    symlink(&real, scratch.join("linked")).expect("create directory symlink");
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("linked/resume.pdf")),
        Err(ImportRefusal::NotReadable)
    );
}

#[test]
fn symlink_as_the_file_is_refused() {
    let scratch = Scratch::new();
    scratch.write("real/resume.pdf", SAMPLE_PDF);
    symlink(scratch.join("real/resume.pdf"), scratch.join("link.pdf"))
        .expect("create file symlink");
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("link.pdf")),
        Err(ImportRefusal::NotReadable)
    );
}

#[test]
fn dangling_symlink_as_the_file_is_refused_not_reported_missing() {
    let scratch = Scratch::new();
    symlink(
        scratch.join("no-such-target.pdf"),
        scratch.join("dangling.pdf"),
    )
    .expect("create dangling symlink");
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("dangling.pdf")),
        Err(ImportRefusal::NotReadable)
    );
}

#[test]
fn symlink_as_the_final_directory_is_refused() {
    let scratch = Scratch::new();
    let real = scratch.dir("real");
    symlink(&real, scratch.join("linked-folder")).expect("create directory symlink");
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("linked-folder")),
        Err(ImportRefusal::NotReadable)
    );
}

#[test]
fn symlinked_root_is_refused() {
    let scratch = Scratch::new();
    let real_root = scratch.dir("real-root");
    scratch.write("real-root/resume.pdf", SAMPLE_PDF);
    let linked_root = scratch.join("linked-root");
    symlink(&real_root, &linked_root).expect("create root symlink");
    let reader = ImportRoot::new(Some(linked_root.to_string_lossy().into_owned()));
    let text = format!("{}/resume.pdf", linked_root.to_string_lossy());
    assert_eq!(reader.read(&text), Err(ImportRefusal::NotReadable));
}

#[test]
fn fifo_is_refused_without_blocking() {
    let scratch = Scratch::new();
    let fifo = scratch.join("pipe.pdf");
    let status = Command::new("mkfifo").arg(&fifo).status();
    match status {
        Ok(result) if result.success() => {}
        Ok(result) => {
            skip_or_fail_without("mkfifo", &format!("exit status {result}"));
            return;
        }
        Err(error) => {
            skip_or_fail_without("mkfifo", &error);
            return;
        }
    }
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("pipe.pdf")),
        Err(ImportRefusal::NotRegularFile)
    );
}

#[test]
fn character_device_is_refused_without_reading_it() {
    let reader = ImportRoot::new(Some("/".to_string()));
    assert_eq!(reader.read("/dev/null"), Err(ImportRefusal::NotRegularFile));
}

#[test]
fn hard_linked_file_is_refused_and_left_in_place() {
    let scratch = Scratch::new();
    let original = scratch.write("resume.pdf", SAMPLE_PDF);
    let second = scratch.join("second-name.pdf");
    std::fs::hard_link(&original, &second).expect("create hard link");
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("resume.pdf")),
        Err(ImportRefusal::HardLinked)
    );
    assert!(
        original.exists() && second.exists(),
        "a refused import removes nothing"
    );
}

#[test]
fn single_link_file_is_read() {
    let scratch = Scratch::new();
    scratch.write("resume.pdf", SAMPLE_PDF);
    let reader = root_for(&scratch);
    let imported = reader
        .read(&scratch.text("resume.pdf"))
        .expect("single-link file is read");
    assert_eq!(imported.content, SAMPLE_PDF);
}

/// `file/..namedfork/rsrc` reads the resource fork of `file`; it is the macOS analogue of a
/// Windows alternate data stream and must never be reachable through the import.
///
/// The real fork path cannot be built as a directory tree (the kernel intercepts it), so each
/// case is made to fail for the fork reason alone and the reason is asserted at the chain, which
/// reports `InvalidInput` for a fork name and would report not-a-directory or not-found for any
/// other fault. `resume.pdf` is a real readable file, so `resume.pdf/..namedfork/rsrc` fails
/// only on its fork component; `x..namedfork` and `..NamedFork` are ordinary directories to the
/// kernel that exist and hold a readable file, so only the guard refuses them. The control
/// (`forkless/rsrc`) reads.
#[test]
fn resource_fork_paths_are_refused() {
    let scratch = Scratch::new();
    scratch.write("resume.pdf", SAMPLE_PDF);
    scratch.write("forkless/rsrc", SAMPLE_PDF);
    let reader = root_for(&scratch);
    for control in ["resume.pdf", "forkless/rsrc"] {
        assert_eq!(
            reader
                .read(&scratch.text(control))
                .expect("control reads")
                .content,
            SAMPLE_PDF,
            "{control}"
        );
    }
    for directory in ["x..namedfork", "..NamedFork"] {
        scratch.write(&format!("{directory}/rsrc"), SAMPLE_PDF);
        assert_eq!(
            reader.read(&scratch.text(&format!("{directory}/rsrc"))),
            Err(ImportRefusal::NotReadable),
            "{directory}/rsrc"
        );
        let error = DirChain::pin(scratch.path(), &[directory.to_string()])
            .expect_err("fork directory pin");
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::InvalidInput,
            "{directory}"
        );
    }
    for relative in [
        "resume.pdf/..namedfork/rsrc",
        "resume.pdf/..namedfork/data",
        "resume.pdf/..NamedFork/RSRC",
        "resume.pdf/..namedfork",
    ] {
        assert_eq!(
            reader.read(&scratch.text(relative)),
            Err(ImportRefusal::NotReadable),
            "{relative}"
        );
    }
    // The chain refuses the fork component before it opens anything: a plain file as a directory
    // component would otherwise fail with `ENOTDIR`, a missing one with not-found.
    let names = ["resume.pdf", "..namedfork"].map(str::to_string);
    let error = DirChain::pin(scratch.path(), &names).expect_err("fork chain");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    let plain = ["resume.pdf", "plain"].map(str::to_string);
    let error = DirChain::pin(scratch.path(), &plain).expect_err("plain chain through a file");
    assert_ne!(error.kind(), std::io::ErrorKind::InvalidInput);
    // A fork name as the file itself (an existing, readable regular file).
    scratch.write("a..namedfork", SAMPLE_PDF);
    assert_eq!(
        reader.read(&scratch.text("a..namedfork")),
        Err(ImportRefusal::NotReadable)
    );
    let chain = DirChain::pin(scratch.path(), &[]).expect("pin base");
    let error = chain.open_file("a..namedfork").expect_err("fork leaf");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn fork_names_are_refused_by_the_chain_itself() {
    let scratch = Scratch::new();
    scratch.write("resume.pdf", SAMPLE_PDF);
    let pinned = DirChain::pin(scratch.path(), &[]).expect("pin base");
    for bad in ["..namedfork", "x..namedfork", "..NAMEDFORK"] {
        let error = pinned.open_file(bad).expect_err("fork leaf name");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{bad:?}");
        let error = DirChain::pin(scratch.path(), &[bad.to_string()]).expect_err("fork pin name");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{bad:?}");
    }
}

/// A path under another case of the root is not inside the root: the containment check is
/// textual, so it holds on both case-insensitive and case-sensitive volumes.
#[test]
fn root_in_another_case_is_outside_the_root() {
    let scratch = Scratch::new();
    scratch.write("resume.pdf", SAMPLE_PDF);
    let reader = root_for(&scratch);
    let upper = scratch.text("resume.pdf").to_uppercase();
    assert_eq!(reader.read(&upper), Err(ImportRefusal::OutsideRoot));
}

/// A leaf in another case reads the same file on a case-insensitive volume (APFS default) and is
/// missing on a case-sensitive one. It never reads anything else.
#[test]
fn leaf_in_another_case_reads_the_same_file_or_is_missing() {
    let scratch = Scratch::new();
    scratch.write("resume.pdf", SAMPLE_PDF);
    let reader = root_for(&scratch);
    match reader.read(&scratch.text("RESUME.PDF")) {
        Ok(imported) => assert_eq!(imported.content, SAMPLE_PDF),
        Err(refusal) => assert_eq!(refusal, ImportRefusal::NotFound),
    }
}

/// `/tmp` is a link to `/private/tmp`: a root or path reached through it is refused.
#[test]
fn a_root_reached_through_the_tmp_alias_is_refused() {
    let leaf = format!("jobniac-files-alias-{}", std::process::id());
    let real = Path::new("/private/tmp").join(&leaf);
    std::fs::create_dir_all(&real).expect("create scratch below /private/tmp");
    std::fs::write(real.join("resume.pdf"), SAMPLE_PDF).expect("write file");
    let through_link = format!("/tmp/{leaf}");
    let reader = ImportRoot::new(Some(through_link.clone()));
    let refused = reader.read(&format!("{through_link}/resume.pdf"));
    let direct = ImportRoot::new(Some(real.to_string_lossy().into_owned()))
        .read(&real.join("resume.pdf").to_string_lossy());
    std::fs::remove_dir_all(&real).expect("clean up");
    assert_eq!(refused, Err(ImportRefusal::NotReadable));
    assert_eq!(direct.expect("real path reads").content, SAMPLE_PDF);
}

/// Firmlinks (`/Users`, `/private`, ...) are transparent to `openat` and are not symbolic links,
/// so a chain through them pins. Every canonicalized scratch folder below `/private/var` already
/// crosses one; this checks the `/Users` firmlink and a system file explicitly.
#[test]
fn firmlinked_directories_pin_and_system_files_read() {
    DirChain::pin(Path::new("/"), &["Users".to_string()]).expect("pin through /Users");
    let reader = ImportRoot::new(Some("/System/Library/CoreServices".to_string()));
    let imported = reader
        .read("/System/Library/CoreServices/SystemVersion.plist")
        .expect("system file below the sealed volume reads");
    assert!(!imported.content.is_empty());
}

/// Descriptors do not stop a rename on macOS, so `verify` must notice an ancestor that was
/// renamed away or replaced by another directory with the same name.
#[test]
fn ancestor_renamed_or_replaced_while_held_is_detected_by_verify() {
    let scratch = Scratch::new();
    scratch.write("outer/inner/resume.pdf", SAMPLE_PDF);
    let names = vec!["outer".to_string(), "inner".to_string()];
    let chain = DirChain::pin(scratch.path(), &names).expect("pin chain");
    chain.verify().expect("unchanged chain verifies");

    std::fs::rename(scratch.join("outer"), scratch.join("moved")).expect("rename ancestor");
    assert!(chain.verify().is_err(), "a renamed ancestor is detected");

    scratch.write("outer/inner/resume.pdf", b"%PDF-1.4 replacement\n");
    assert!(chain.verify().is_err(), "a replaced ancestor is detected");
}

/// The import read re-checks the chain after the leaf was opened: a directory replaced by a
/// link between the pin and the check cannot be exercised without a race, so the contract is
/// covered through `verify` above; here the unchanged read passes the same check end to end.
#[test]
fn unchanged_nested_read_passes_the_chain_check() {
    let scratch = Scratch::new();
    scratch.write("a/b/resume.pdf", SAMPLE_PDF);
    let reader = root_for(&scratch);
    let imported = reader
        .read(&scratch.text("a/b/resume.pdf"))
        .expect("nested file is read");
    assert_eq!(imported.file_name, "resume.pdf");
    assert_eq!(imported.content, SAMPLE_PDF);
}
