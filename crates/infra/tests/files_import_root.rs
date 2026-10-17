//! Import root rules that hold on every platform: root configuration, path shape, containment,
//! size limit, regular-file classification and the exact refusal reasons.

mod files_support;

use files_support::{SAMPLE_PDF, Scratch};
use jobniac_infra::files::import_root::{ImportRefusal, ImportRoot, MAX_IMPORT_BYTES};

fn root_for(scratch: &Scratch) -> ImportRoot {
    ImportRoot::new(Some(scratch.root_text()))
}

#[test]
fn unset_or_empty_root_refuses_every_read() {
    let scratch = Scratch::new();
    let never = scratch.join("never-created");
    for root in [None, Some(String::new())] {
        let reader = ImportRoot::new(root);
        let refusal = reader
            .read(&scratch.text("never-created/resume.pdf"))
            .expect_err("refused");
        assert_eq!(refusal, ImportRefusal::RootNotConfigured);
    }
    assert!(!never.exists(), "no root is created");
}

#[test]
fn relative_root_and_root_with_parent_component_are_refused() {
    let scratch = Scratch::new();
    scratch.write("resume.pdf", SAMPLE_PDF);
    let relative = ImportRoot::new(Some("imports".to_string()));
    assert_eq!(
        relative.read(&scratch.text("resume.pdf")),
        Err(ImportRefusal::RootNotAbsolute)
    );
    let traversal = ImportRoot::new(Some(format!("{}/../other", scratch.root_text())));
    assert_eq!(
        traversal.read(&scratch.text("resume.pdf")),
        Err(ImportRefusal::RootNotAbsolute)
    );
}

#[test]
fn parent_component_in_the_request_is_refused_before_normalization() {
    let scratch = Scratch::new();
    scratch.write("resume.pdf", SAMPLE_PDF);
    let reader = root_for(&scratch);
    let text = format!("{}/sub/../resume.pdf", scratch.root_text());
    assert_eq!(reader.read(&text), Err(ImportRefusal::PathTraversal));
}

#[test]
fn relative_request_is_refused_instead_of_using_the_working_directory() {
    let scratch = Scratch::new();
    scratch.write("resume.pdf", SAMPLE_PDF);
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read("resume.pdf"),
        Err(ImportRefusal::PathNotAbsolute)
    );
}

#[test]
fn blank_and_nul_requests_are_invalid_paths() {
    let scratch = Scratch::new();
    let reader = root_for(&scratch);
    for text in ["", "   ", "\t"] {
        assert_eq!(
            reader.read(text),
            Err(ImportRefusal::InvalidPath),
            "{text:?}"
        );
    }
    let with_nul = format!("{}/resume\0.pdf", scratch.root_text());
    assert_eq!(reader.read(&with_nul), Err(ImportRefusal::InvalidPath));
}

#[test]
fn files_outside_the_root_and_the_root_itself_are_refused() {
    let scratch = Scratch::new();
    let outside = Scratch::new();
    outside.write("private.pdf", SAMPLE_PDF);
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&outside.text("private.pdf")),
        Err(ImportRefusal::OutsideRoot)
    );
    // A sibling whose name only starts with the root name is outside as well.
    let sibling = format!("{}x/resume.pdf", scratch.root_text());
    assert_eq!(reader.read(&sibling), Err(ImportRefusal::OutsideRoot));
    assert_eq!(
        reader.read(&scratch.root_text()),
        Err(ImportRefusal::NotRegularFile)
    );
    assert_eq!(
        reader.read(&format!("{}/", scratch.root_text())),
        Err(ImportRefusal::NotRegularFile)
    );
}

#[test]
fn missing_file_and_missing_directory_are_not_found() {
    let scratch = Scratch::new();
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("missing.pdf")),
        Err(ImportRefusal::NotFound)
    );
    assert_eq!(
        reader.read(&scratch.text("missing/resume.pdf")),
        Err(ImportRefusal::NotFound)
    );
}

#[test]
fn regular_file_is_read_with_its_last_component_as_name() {
    let scratch = Scratch::new();
    scratch.write("nested/resume.pdf", SAMPLE_PDF);
    let reader = root_for(&scratch);
    let mut text = scratch.text("nested/resume.pdf");
    if cfg!(windows) {
        // Drive letters are case-insensitive on Windows; the containment check must accept either.
        text.replace_range(..1, &text[..1].to_lowercase());
    }
    let imported = reader.read(&text).expect("regular file is read");
    assert_eq!(imported.file_name, "resume.pdf");
    assert_eq!(imported.content, SAMPLE_PDF);
}

#[test]
fn root_as_a_folder_with_trailing_separator_still_contains_the_file() {
    let scratch = Scratch::new();
    scratch.write("resume.pdf", SAMPLE_PDF);
    let reader = ImportRoot::new(Some(format!("{}/", scratch.root_text())));
    let imported = reader
        .read(&scratch.text("resume.pdf"))
        .expect("file is read");
    assert_eq!(imported.content, SAMPLE_PDF);
}

#[test]
fn directory_at_the_requested_path_is_refused() {
    let scratch = Scratch::new();
    scratch.dir("folder");
    let reader = root_for(&scratch);
    // The POSIX open succeeds and fails the regular-file test; the Windows open of a directory
    // fails inside the open helper, which is reported as unreadable.
    let expected = if cfg!(windows) {
        ImportRefusal::NotReadable
    } else {
        ImportRefusal::NotRegularFile
    };
    assert_eq!(reader.read(&scratch.text("folder")), Err(expected));
}

#[test]
fn file_used_as_a_directory_component_is_not_readable() {
    let scratch = Scratch::new();
    scratch.write("plain.pdf", SAMPLE_PDF);
    let reader = root_for(&scratch);
    assert_eq!(
        reader.read(&scratch.text("plain.pdf/resume.pdf")),
        Err(ImportRefusal::NotReadable)
    );
}

#[test]
fn size_limit_accepts_exactly_the_maximum_and_refuses_one_more() {
    let scratch = Scratch::new();
    let reader = root_for(&scratch);

    let at_limit = scratch.write("at-limit.pdf", b"");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&at_limit)
        .expect("open");
    file.set_len(MAX_IMPORT_BYTES).expect("size at limit");
    drop(file);
    let imported = reader
        .read(&scratch.text("at-limit.pdf"))
        .expect("limit is accepted");
    assert_eq!(
        u64::try_from(imported.content.len()).ok(),
        Some(MAX_IMPORT_BYTES)
    );

    let over = scratch.write("over-limit.pdf", b"");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&over)
        .expect("open");
    file.set_len(MAX_IMPORT_BYTES + 1).expect("size over limit");
    drop(file);
    assert_eq!(
        reader.read(&scratch.text("over-limit.pdf")),
        Err(ImportRefusal::TooLarge)
    );
}

#[test]
fn refusal_reasons_are_the_exact_strings() {
    let expected = [
        (
            ImportRefusal::RootNotConfigured,
            "document import root is not configured",
        ),
        (
            ImportRefusal::PlatformUnavailable,
            "safe document import is unavailable on this platform",
        ),
        (ImportRefusal::InvalidPath, "must be a valid file path"),
        (
            ImportRefusal::RootNotAbsolute,
            "document import root must be an absolute path without traversal",
        ),
        (
            ImportRefusal::PathTraversal,
            "path traversal is not allowed",
        ),
        (
            ImportRefusal::OutsideRoot,
            "file must be inside the document import root",
        ),
        (
            ImportRefusal::NotRegularFile,
            "must identify a regular file",
        ),
        (
            ImportRefusal::TooLarge,
            "PDF file size exceeds limit of 10485760 bytes",
        ),
        (
            ImportRefusal::ChangedWhileReading,
            "file changed while being read",
        ),
        (ImportRefusal::NotFound, "file was not found"),
        (
            ImportRefusal::NotReadable,
            "file is not readable or contains a symbolic link",
        ),
    ];
    for (refusal, text) in expected {
        assert_eq!(refusal.reason(), text);
        assert_eq!(refusal.to_string(), text);
    }
    assert_eq!(
        ImportRefusal::PathNotAbsolute.reason(),
        "document import path must be absolute"
    );
    assert_eq!(
        ImportRefusal::HardLinked.reason(),
        "file must not have more than one hard link"
    );
}

#[test]
fn refusals_never_echo_the_requested_path() {
    let scratch = Scratch::new();
    let reader = root_for(&scratch);
    let outside = Scratch::new();
    outside.write("private-person.pdf", SAMPLE_PDF);
    let refusal = reader
        .read(&outside.text("private-person.pdf"))
        .expect_err("refused");
    assert!(!refusal.reason().contains(&outside.root_text()));
    assert!(!refusal.reason().contains("private-person"));
}
