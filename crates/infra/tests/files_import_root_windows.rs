//! Import root reads on Windows: junctions and symbolic links in every position, reserved device
//! names, alternate data streams, reserved characters and directories.
//!
//! Symbolic links need the `SeCreateSymbolicLinkPrivilege` (or developer mode); those tests print a
//! note and return when the host does not allow them. Junctions need no privilege.

#![cfg(windows)]

mod files_support;

use std::os::windows::fs::{symlink_dir, symlink_file};

use files_support::{SAMPLE_PDF, Scratch, make_junction, skip_or_fail_without};
use jobniac_infra::files::import_root::{ImportRefusal, ImportRoot};
use jobniac_infra::files::no_follow::DirChain;

fn root_for(scratch: &Scratch) -> ImportRoot {
    ImportRoot::new(Some(scratch.root_text()))
}

#[test]
fn junction_in_a_directory_component_is_refused() {
    let scratch = Scratch::new();
    let real = scratch.dir("real");
    scratch.write("real/resume.pdf", SAMPLE_PDF);
    make_junction(&scratch.join("linked"), &real).expect("create junction");
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("linked/resume.pdf")),
        Err(ImportRefusal::NotReadable)
    );
}

#[test]
fn junction_as_the_final_component_is_refused() {
    let scratch = Scratch::new();
    let real = scratch.dir("real");
    make_junction(&scratch.join("linked"), &real).expect("create junction");
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("linked")),
        Err(ImportRefusal::NotReadable)
    );
}

#[test]
fn junction_that_resolves_the_root_elsewhere_is_refused() {
    let scratch = Scratch::new();
    let real_root = scratch.dir("real-root");
    scratch.write("real-root/resume.pdf", SAMPLE_PDF);
    let linked_root = scratch.join("linked-root");
    make_junction(&linked_root, &real_root).expect("create junction");
    let reader = ImportRoot::new(Some(linked_root.to_string_lossy().into_owned()));
    let text = format!("{}\\resume.pdf", linked_root.to_string_lossy());
    assert_eq!(reader.read(&text), Err(ImportRefusal::NotReadable));
}

#[test]
fn symbolic_link_as_the_file_is_refused_when_the_host_allows_links() {
    let scratch = Scratch::new();
    let target = scratch.write("real/resume.pdf", SAMPLE_PDF);
    if let Err(error) = symlink_file(&target, scratch.join("link.pdf")) {
        skip_or_fail_without("symbolic links", &error);
        return;
    }
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("link.pdf")),
        Err(ImportRefusal::NotReadable)
    );
}

#[test]
fn symbolic_link_as_a_directory_component_is_refused_when_the_host_allows_links() {
    let scratch = Scratch::new();
    let real = scratch.dir("real");
    scratch.write("real/resume.pdf", SAMPLE_PDF);
    if let Err(error) = symlink_dir(&real, scratch.join("linked")) {
        skip_or_fail_without("symbolic links", &error);
        return;
    }
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("linked/resume.pdf")),
        Err(ImportRefusal::NotReadable)
    );
}

#[test]
fn reserved_device_names_are_refused_without_opening_devices() {
    let scratch = Scratch::new();
    let reader = root_for(&scratch);
    for name in [
        "CON",
        "con.pdf",
        "NUL",
        "NUL.pdf",
        "PRN.txt",
        "AUX",
        "COM1",
        "lpt9.pdf",
        "CONIN$",
        "COM\u{b9}",
    ] {
        assert_eq!(
            reader.read(&scratch.text(name)),
            Err(ImportRefusal::NotReadable),
            "{name}"
        );
    }
}

#[test]
fn alternate_data_stream_of_an_existing_file_is_refused() {
    let scratch = Scratch::new();
    scratch.write("resume.pdf", SAMPLE_PDF);
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("resume.pdf:stream")),
        Err(ImportRefusal::NotReadable)
    );
    assert_eq!(
        reader.read(&scratch.text("resume.pdf::$DATA")),
        Err(ImportRefusal::NotReadable)
    );
}

#[test]
fn trailing_dot_space_and_reserved_characters_are_refused() {
    let scratch = Scratch::new();
    scratch.write("resume.pdf", SAMPLE_PDF);
    let reader = root_for(&scratch);
    for name in [
        "resume.pdf.",
        "resume.pdf ",
        "res*.pdf",
        "re?ume.pdf",
        "resume|.pdf",
    ] {
        assert_eq!(
            reader.read(&scratch.text(name)),
            Err(ImportRefusal::NotReadable),
            "{name}"
        );
    }
}

#[test]
fn forward_slashes_in_the_request_are_accepted() {
    let scratch = Scratch::new();
    scratch.write("nested/resume.pdf", SAMPLE_PDF);
    let reader = root_for(&scratch);
    let text = scratch.text("nested/resume.pdf").replace('\\', "/");
    let imported = reader.read(&text).expect("forward slashes are normalized");
    assert_eq!(imported.content, SAMPLE_PDF);
}

/// A hard-linked file is refused on Windows: the link count is sampled from the handle,
/// so a second name for the file outside the import root cannot be read through the root.
#[test]
fn hard_linked_file_is_refused_on_windows() {
    let scratch = Scratch::new();
    let original = scratch.write("resume.pdf", SAMPLE_PDF);
    std::fs::hard_link(&original, scratch.join("second.pdf")).expect("create hard link");
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("resume.pdf")),
        Err(ImportRefusal::HardLinked)
    );
}

#[test]
fn pinned_chain_refuses_a_junction_at_pin_time() {
    let scratch = Scratch::new();
    let real = scratch.dir("real");
    make_junction(&scratch.join("linked"), &real).expect("create junction");
    let names = vec!["linked".to_string()];
    assert!(DirChain::pin(scratch.path(), &names).is_err());
}

#[test]
fn pinned_ancestor_cannot_be_renamed_while_the_chain_is_held() {
    let scratch = Scratch::new();
    scratch.write("nested/resume.pdf", SAMPLE_PDF);
    let names = vec!["nested".to_string()];
    let chain = DirChain::pin(scratch.path(), &names).expect("pin chain");
    let moved = std::fs::rename(scratch.join("nested"), scratch.join("moved"));
    assert!(moved.is_err(), "a pinned ancestor must not be renamed");
    chain.verify().expect("the pinned chain still verifies");
}
