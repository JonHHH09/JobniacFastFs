//! Reads of a picked file (`read_picked`): the import root rules without the root, on every
//! platform. A symbolic link, a hard link and an oversized file are refused; a normal file is read.
//!
//! Symbolic links need privileges on Windows; that test prints a note and returns when the host does
//! not allow them.

mod files_support;

use files_support::{SAMPLE_PDF, Scratch, skip_or_fail_without};
use jobniac_infra::files::import_root::{ImportRefusal, MAX_IMPORT_BYTES, read_picked};

#[cfg(unix)]
fn make_file_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn make_file_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[test]
fn regular_file_is_read_with_its_last_component_as_name() {
    let scratch = Scratch::new();
    scratch.write("nested/resume.pdf", SAMPLE_PDF);
    let imported = read_picked(&scratch.text("nested/resume.pdf")).expect("picked file is read");
    assert_eq!(imported.file_name, "resume.pdf");
    assert_eq!(imported.content, SAMPLE_PDF);
}

#[test]
fn path_shapes_are_refused_before_any_open() {
    let scratch = Scratch::new();
    scratch.write("resume.pdf", SAMPLE_PDF);
    for text in ["", "   "] {
        assert_eq!(read_picked(text), Err(ImportRefusal::InvalidPath));
    }
    let with_nul = format!("{}/resume\0.pdf", scratch.root_text());
    assert_eq!(read_picked(&with_nul), Err(ImportRefusal::InvalidPath));
    assert_eq!(
        read_picked("resume.pdf"),
        Err(ImportRefusal::PathNotAbsolute)
    );
    let traversal = format!("{}/sub/../resume.pdf", scratch.root_text());
    assert_eq!(read_picked(&traversal), Err(ImportRefusal::PathTraversal));
}

#[test]
fn missing_file_is_not_found() {
    let scratch = Scratch::new();
    assert_eq!(
        read_picked(&scratch.text("missing.pdf")),
        Err(ImportRefusal::NotFound)
    );
}

#[test]
fn symbolic_link_as_the_file_is_refused() {
    let scratch = Scratch::new();
    let target = scratch.write("real/resume.pdf", SAMPLE_PDF);
    if let Err(error) = make_file_symlink(&target, &scratch.join("link.pdf")) {
        skip_or_fail_without("symbolic links", &error);
        return;
    }
    assert_eq!(
        read_picked(&scratch.text("link.pdf")),
        Err(ImportRefusal::NotReadable)
    );
}

#[test]
fn oversized_file_is_refused() {
    let scratch = Scratch::new();
    let over = scratch.write("over-limit.pdf", b"");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&over)
        .expect("open");
    file.set_len(MAX_IMPORT_BYTES + 1).expect("size over limit");
    drop(file);
    assert_eq!(
        read_picked(&scratch.text("over-limit.pdf")),
        Err(ImportRefusal::TooLarge)
    );
}

#[test]
fn hard_linked_file_is_refused_and_left_in_place() {
    let scratch = Scratch::new();
    let original = scratch.write("resume.pdf", SAMPLE_PDF);
    let second = scratch.join("second-name.pdf");
    std::fs::hard_link(&original, &second).expect("create hard link");
    assert_eq!(
        read_picked(&scratch.text("resume.pdf")),
        Err(ImportRefusal::HardLinked)
    );
    assert!(
        original.exists() && second.exists(),
        "a refused import removes nothing"
    );
}
