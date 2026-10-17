//! Import root reads on Linux: symbolic links in every position, FIFOs and devices, hard links and
//! directories.
//!
//! The tests run only on the targets where the pinned no-follow reads exist (x86-64 and AArch64).

#![cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]

mod files_support;

use std::os::unix::fs::symlink;
use std::process::Command;

use files_support::{SAMPLE_PDF, Scratch, skip_or_fail_without};
use jobniac_infra::files::import_root::{ImportRefusal, ImportRoot};

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
