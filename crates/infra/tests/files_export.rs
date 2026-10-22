//! Native PDF export: publication of a new, private, complete file.
//!
//! The Windows cases run on Windows, the POSIX cases on Linux and macOS. Each test works in its own
//! directory under the system temporary folder, and all content is synthetic.

use std::error::Error;
use std::path::{Path, PathBuf};

use jobniac_core::artifacts::{ArtifactExporter, DocumentArtifact, ExportFailure};
use jobniac_core::clock::format_instant;
use jobniac_core::documents::{DocumentMetadata, sha256_hex};
use jobniac_infra::files::export::{AtomicExporter, create_private_directory};
use time::macros::datetime;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const REFUSED: ExportFailure = ExportFailure::Refused;

/// A consistent artifact with a valid PDF body of `size` bytes.
fn artifact_of(size: usize) -> DocumentArtifact {
    let mut content = b"%PDF-1.4\n".to_vec();
    content.resize(size, b'a');
    DocumentArtifact {
        document: DocumentMetadata {
            id: Uuid::from_u128(0xAA),
            original_file_name: "cv.pdf".to_owned(),
            media_type: "application/pdf".to_owned(),
            byte_size: i64::try_from(size).unwrap_or(i64::MAX),
            sha256: sha256_hex(&content),
            created_at: format_instant(datetime!(2026-01-02 03:04:05 UTC)),
            updated_at: format_instant(datetime!(2026-01-02 03:04:06 UTC)),
        },
        content,
    }
}

/// The system temporary folder. On Windows it is returned in its final-path form: the export
/// refuses a parent whose text differs from its final path, and `%TEMP%` can be an 8.3 alias
/// (`RUNNER~1`) that the final path expands. On macOS it is canonicalized: `/var` and `/tmp` are
/// symbolic links to `/private/...`, and the export (correctly) refuses a link in the parent.
fn temp_base() -> Result<PathBuf, Box<dyn Error>> {
    let base = std::env::temp_dir();
    if cfg!(target_os = "macos") {
        return Ok(std::fs::canonicalize(base)?);
    }
    if cfg!(windows) {
        let canonical = std::fs::canonicalize(base)?.to_string_lossy().into_owned();
        let plain = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
        return Ok(PathBuf::from(plain));
    }
    Ok(base)
}

/// A fresh, empty, private working directory under the system temporary folder.
fn fresh_dir(label: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = temp_base()?.join(format!(
        "jobniac-export-{label}-{}",
        Uuid::new_v4().simple()
    ));
    create_private_directory(&path)?;
    Ok(path)
}

/// Removes a working directory, ignoring failures of the cleanup itself.
fn remove_dir(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

/// The destination text of `name` inside `dir`, in the platform's absolute form.
fn destination(dir: &Path, name: &str) -> Result<String, Box<dyn Error>> {
    let text = dir.join(name).to_string_lossy().into_owned();
    Ok(text)
}

/// The names in a directory, sorted (no partial or temporary names may remain).
fn names_in(dir: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut names = std::fs::read_dir(dir)?
        .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

#[test]
fn a_new_export_is_complete_and_returns_only_the_basename() -> TestResult {
    let dir = fresh_dir("new")?;
    let exporter = AtomicExporter::default();
    let artifact = artifact_of(256);
    let name = exporter
        .export(&artifact, &destination(&dir, "saved.pdf")?)
        .map_err(|_| "export refused")?;
    assert_eq!(name, "saved.pdf");
    assert_eq!(std::fs::read(dir.join("saved.pdf"))?, artifact.content);
    assert_eq!(names_in(&dir)?, ["saved.pdf"]);
    remove_dir(&dir);
    Ok(())
}

#[test]
fn an_existing_name_is_never_replaced_even_with_another_case() -> TestResult {
    let dir = fresh_dir("exists")?;
    std::fs::write(dir.join("keep.pdf"), b"original")?;
    let exporter = AtomicExporter::default();
    assert_eq!(
        exporter.export(&artifact_of(64), &destination(&dir, "keep.pdf")?),
        Err(REFUSED)
    );
    assert_eq!(std::fs::read(dir.join("keep.pdf"))?, b"original");
    assert_eq!(names_in(&dir)?, ["keep.pdf"]);
    remove_dir(&dir);
    Ok(())
}

#[test]
fn a_directory_with_the_target_name_is_refused() -> TestResult {
    let dir = fresh_dir("dirname")?;
    std::fs::create_dir(dir.join("taken.pdf"))?;
    let exporter = AtomicExporter::default();
    assert_eq!(
        exporter.export(&artifact_of(64), &destination(&dir, "taken.pdf")?),
        Err(REFUSED)
    );
    assert_eq!(names_in(&dir)?, ["taken.pdf"]);
    remove_dir(&dir);
    Ok(())
}

#[test]
fn invalid_destinations_are_refused_without_echoing_the_path() -> TestResult {
    let dir = fresh_dir("invalid")?;
    let exporter = AtomicExporter::default();
    let base = dir.to_string_lossy().into_owned();
    let cases = [
        "relative/saved.pdf".to_owned(),
        format!("{base}/../saved.pdf"),
        format!("{base}/saved.txt"),
        format!("{base}/.pdf"),
        format!("{base}/saved\0.pdf"),
        format!("file://{base}/saved.pdf"),
    ];
    for path in cases {
        assert_eq!(
            exporter.export(&artifact_of(64), &path),
            Err(REFUSED),
            "{path}"
        );
    }
    assert!(names_in(&dir)?.is_empty());
    remove_dir(&dir);
    Ok(())
}

#[test]
fn a_corrupt_artifact_never_creates_the_destination() -> TestResult {
    let dir = fresh_dir("corrupt")?;
    let mut artifact = artifact_of(64);
    artifact.content[0] = b'X';
    let exporter = AtomicExporter::default();
    assert_eq!(
        exporter.export(&artifact, &destination(&dir, "bad.pdf")?),
        Err(REFUSED)
    );
    assert!(names_in(&dir)?.is_empty());
    remove_dir(&dir);
    Ok(())
}

#[test]
fn a_protected_root_and_its_children_are_refused() -> TestResult {
    let dir = fresh_dir("protected")?;
    let inside = dir.join("import");
    create_private_directory(&inside)?;
    let exporter = AtomicExporter::new(vec![dir.to_string_lossy().into_owned()]);
    assert_eq!(
        exporter.export(&artifact_of(64), &destination(&dir, "root.pdf")?),
        Err(REFUSED)
    );
    assert_eq!(
        exporter.export(&artifact_of(64), &destination(&inside, "child.pdf")?),
        Err(REFUSED)
    );
    assert!(names_in(&dir)?.iter().all(|name| name == "import"));
    remove_dir(&dir);
    Ok(())
}

#[cfg(unix)]
mod posix {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    #[test]
    fn the_export_is_owner_only_and_leaves_no_temporary_name() -> TestResult {
        let dir = fresh_dir("mode")?;
        AtomicExporter::default()
            .export(&artifact_of(64), &destination(&dir, "owner.pdf")?)
            .map_err(|_| "export refused")?;
        let mode = std::fs::metadata(dir.join("owner.pdf"))?
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(names_in(&dir)?, ["owner.pdf"]);
        remove_dir(&dir);
        Ok(())
    }

    #[test]
    fn a_symlink_or_dangling_symlink_target_is_never_followed() -> TestResult {
        let dir = fresh_dir("links")?;
        let elsewhere = dir.join("elsewhere.pdf");
        std::fs::write(&elsewhere, b"other")?;
        symlink(&elsewhere, dir.join("link.pdf"))?;
        symlink(dir.join("missing.pdf"), dir.join("dangling.pdf"))?;
        let exporter = AtomicExporter::default();
        assert_eq!(
            exporter.export(&artifact_of(64), &destination(&dir, "link.pdf")?),
            Err(REFUSED)
        );
        assert_eq!(
            exporter.export(&artifact_of(64), &destination(&dir, "dangling.pdf")?),
            Err(REFUSED)
        );
        assert_eq!(std::fs::read(&elsewhere)?, b"other");
        assert!(!dir.join("missing.pdf").exists());
        remove_dir(&dir);
        Ok(())
    }

    #[test]
    fn a_symlink_parent_and_a_group_or_world_writable_parent_are_refused() -> TestResult {
        let dir = fresh_dir("parents")?;
        let real = dir.join("real");
        create_private_directory(&real)?;
        symlink(&real, dir.join("alias"))?;
        let exporter = AtomicExporter::default();
        assert_eq!(
            exporter.export(&artifact_of(64), &destination(&dir.join("alias"), "a.pdf")?),
            Err(REFUSED)
        );
        let open = dir.join("open");
        std::fs::create_dir(&open)?;
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777))?;
        assert_eq!(
            exporter.export(&artifact_of(64), &destination(&open, "b.pdf")?),
            Err(REFUSED)
        );
        assert!(names_in(&real)?.is_empty());
        remove_dir(&dir);
        Ok(())
    }
}

/// The macOS-only properties: no replacement by case on a case-insensitive volume, resource-fork
/// names refused, a temporary name that never survives a failure, and no write through a link.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)] // the test sets an ACL through <sys/acl.h>
mod macos {
    use std::os::unix::fs::symlink;

    use super::*;

    /// Whether the volume of `dir` treats `A` and `a` as one name (APFS default).
    fn is_case_insensitive(dir: &Path) -> Result<bool, Box<dyn Error>> {
        std::fs::write(dir.join("Probe-Case"), b"")?;
        let same = dir.join("probe-case").exists();
        std::fs::remove_file(dir.join("Probe-Case"))?;
        Ok(same)
    }

    #[test]
    fn a_name_that_differs_only_by_case_never_replaces_the_existing_file() -> TestResult {
        let dir = fresh_dir("case")?;
        std::fs::write(dir.join("keep.pdf"), b"original")?;
        let exporter = AtomicExporter::default();
        let outcome = exporter.export(&artifact_of(64), &destination(&dir, "KEEP.pdf")?);
        if is_case_insensitive(&dir)? {
            assert_eq!(outcome, Err(REFUSED));
            assert_eq!(names_in(&dir)?, ["keep.pdf"]);
        } else {
            // A case-sensitive volume holds two distinct names; neither may be touched.
            assert!(outcome.is_ok());
            assert_eq!(names_in(&dir)?, ["KEEP.pdf", "keep.pdf"]);
        }
        assert_eq!(std::fs::read(dir.join("keep.pdf"))?, b"original");
        remove_dir(&dir);
        Ok(())
    }

    /// Every case is otherwise valid (the parent exists, is private and the name ends in `.pdf`),
    /// so only the `..namedfork` component can refuse it. The control proves the same layout
    /// without the fork name publishes.
    #[test]
    fn resource_fork_and_named_fork_paths_are_refused() -> TestResult {
        let dir = fresh_dir("fork")?;
        let exporter = AtomicExporter::default();
        let control = dir.join("forkless");
        create_private_directory(&control)?;
        exporter
            .export(&artifact_of(64), &destination(&control, "x.pdf")?)
            .map_err(|_| "control export refused")?;
        for parent in ["..namedfork", "..NamedFork2", "holder..namedfork"] {
            create_private_directory(&dir.join(parent))?;
            assert_eq!(
                exporter.export(&artifact_of(64), &destination(&dir.join(parent), "x.pdf")?),
                Err(REFUSED),
                "{parent}"
            );
            assert!(names_in(&dir.join(parent))?.is_empty(), "{parent}");
        }
        for name in ["..namedfork.pdf", "x..NAMEDFORK.pdf"] {
            assert_eq!(
                exporter.export(&artifact_of(64), &destination(&dir, name)?),
                Err(REFUSED),
                "{name}"
            );
        }
        assert_eq!(
            names_in(&dir)?,
            [
                "..NamedFork2",
                "..namedfork",
                "forkless",
                "holder..namedfork"
            ]
        );
        remove_dir(&dir);
        Ok(())
    }

    unsafe extern "C" {
        // <sys/acl.h> of the macOS SDK: acl_from_text, acl_set_file and acl_free.
        fn acl_from_text(text: *const std::ffi::c_char) -> *mut std::ffi::c_void;
        fn acl_set_file(
            path: *const std::ffi::c_char,
            acl_type: std::ffi::c_uint,
            acl: *mut std::ffi::c_void,
        ) -> std::ffi::c_int;
        fn acl_free(object: *mut std::ffi::c_void) -> std::ffi::c_int;
    }

    /// Adds an ACL entry that lets everyone delete children, through the ACL API (not a `chmod` child).
    fn grant_everyone_delete_child(path: &Path) {
        use std::os::unix::ffi::OsStrExt;
        const ACL_TYPE_EXTENDED: std::ffi::c_uint = 0x0000_0100;
        let text =
            c"!#acl 1\ngroup:ABCDEFAB-CDEF-ABCD-EFAB-CDEF0000000C:everyone:12:allow:delete_child";
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("path");
        // SAFETY: both strings are NUL-terminated and outlive the calls; the ACL is released
        // exactly once after it was applied.
        unsafe {
            let acl = acl_from_text(text.as_ptr());
            assert!(!acl.is_null(), "ACL text parsed");
            let status = acl_set_file(path.as_ptr(), ACL_TYPE_EXTENDED, acl);
            acl_free(acl);
            assert_eq!(status, 0, "ACL entry applied");
        }
    }

    /// Mode bits do not show an extended ACL, so a parent with one is refused (macOS is stricter
    /// than Linux here) and nothing is left behind.
    #[test]
    fn a_parent_with_an_extended_acl_is_refused() -> TestResult {
        let dir = fresh_dir("acl")?;
        let exporter = AtomicExporter::default();
        let parent = dir.join("shared");
        create_private_directory(&parent)?;
        exporter
            .export(&artifact_of(64), &destination(&parent, "before.pdf")?)
            .map_err(|_| "export before the ACL refused")?;
        std::fs::remove_file(parent.join("before.pdf"))?;
        grant_everyone_delete_child(&parent);
        assert_eq!(
            exporter.export(&artifact_of(64), &destination(&parent, "after.pdf")?),
            Err(REFUSED)
        );
        assert!(names_in(&parent)?.is_empty());
        remove_dir(&dir);
        Ok(())
    }

    #[test]
    fn no_temporary_name_survives_a_successful_or_refused_export() -> TestResult {
        let dir = fresh_dir("temp")?;
        std::fs::write(dir.join("taken.pdf"), b"original")?;
        let exporter = AtomicExporter::default();
        exporter
            .export(&artifact_of(64), &destination(&dir, "clean.pdf")?)
            .map_err(|_| "export refused")?;
        assert_eq!(
            exporter.export(&artifact_of(64), &destination(&dir, "taken.pdf")?),
            Err(REFUSED)
        );
        assert_eq!(names_in(&dir)?, ["clean.pdf", "taken.pdf"]);
        remove_dir(&dir);
        Ok(())
    }

    #[test]
    fn the_temporary_directory_alias_is_refused_but_its_real_path_works() -> TestResult {
        // `/tmp` is a link to `/private/tmp`: a parent reached through it is refused.
        let leaf = format!("jobniac-export-alias-{}", Uuid::new_v4().simple());
        let real = PathBuf::from("/private/tmp").join(&leaf);
        create_private_directory(&real)?;
        let through_link = PathBuf::from("/tmp").join(&leaf);
        let exporter = AtomicExporter::default();
        assert_eq!(
            exporter.export(&artifact_of(64), &destination(&through_link, "a.pdf")?),
            Err(REFUSED)
        );
        assert!(
            exporter
                .export(&artifact_of(64), &destination(&real, "a.pdf")?)
                .is_ok()
        );
        remove_dir(&real);
        Ok(())
    }

    #[test]
    fn a_symlinked_ancestor_is_refused() -> TestResult {
        let dir = fresh_dir("ancestor")?;
        let real = dir.join("real");
        create_private_directory(&real)?;
        create_private_directory(&real.join("inner"))?;
        symlink(&real, dir.join("alias"))?;
        let exporter = AtomicExporter::default();
        assert_eq!(
            exporter.export(
                &artifact_of(64),
                &destination(&dir.join("alias").join("inner"), "a.pdf")?
            ),
            Err(REFUSED)
        );
        assert!(names_in(&real.join("inner"))?.is_empty());
        remove_dir(&dir);
        Ok(())
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    #[test]
    fn a_forward_slash_destination_is_accepted() -> TestResult {
        let dir = fresh_dir("slash")?;
        let text = destination(&dir, "slash.pdf")?.replace('\\', "/");
        AtomicExporter::default()
            .export(&artifact_of(64), &text)
            .map_err(|_| "export refused")?;
        assert_eq!(names_in(&dir)?, ["slash.pdf"]);
        remove_dir(&dir);
        Ok(())
    }

    #[test]
    fn unc_device_and_stream_forms_are_refused() -> TestResult {
        let exporter = AtomicExporter::default();
        for path in [
            r"\\server\share\a.pdf",
            r"\\?\C:\a.pdf",
            r"C:\a.pdf:stream.pdf",
            r"C:\x\..\a.pdf",
        ] {
            assert_eq!(
                exporter.export(&artifact_of(64), path),
                Err(REFUSED),
                "{path}"
            );
        }
        Ok(())
    }

    /// Builds nested private directories until the destination text reaches about `total`
    /// characters, and returns the parent and the file name. Each component is 200 characters.
    fn long_destination(base: &Path, total: usize) -> Result<(PathBuf, String), Box<dyn Error>> {
        let mut dir = base.to_path_buf();
        loop {
            let used = dir.to_string_lossy().len() + 1;
            if total.saturating_sub(used) <= 201 {
                break;
            }
            dir = dir.join("d".repeat(200));
            create_private_directory(&dir)?;
        }
        let used = dir.to_string_lossy().len() + 1;
        let stem = total.saturating_sub(used + ".pdf".len());
        Ok((dir, format!("{}.pdf", "n".repeat(stem))))
    }

    /// A destination past the 512-unit final-path buffer must publish and verify. The buffer
    /// grows up to 32768 units, so these names must not be refused.
    #[test]
    fn long_destinations_publish_and_verify() -> TestResult {
        for total in [400_usize, 1000] {
            let base = fresh_dir(&format!("long{total}"))?;
            let (dir, name) = match long_destination(&base, total) {
                Ok(built) => built,
                Err(error) => {
                    println!(
                        "skipped long destination of {total}: the OS refused long paths ({error})"
                    );
                    remove_dir(&base);
                    continue;
                }
            };
            let text = dir.join(&name).to_string_lossy().into_owned();
            assert!(
                text.len() >= total - 5,
                "destination too short: {}",
                text.len()
            );
            let artifact = artifact_of(64);
            let published = AtomicExporter::default()
                .export(&artifact, &text)
                .map_err(|_| format!("long destination of {total} refused"))?;
            assert_eq!(published, name);
            assert_eq!(std::fs::read(dir.join(&name))?, artifact.content);
            assert_eq!(names_in(&dir)?, [name.as_str()]);
            remove_dir(&base);
        }
        Ok(())
    }

    #[test]
    fn no_temporary_name_survives_a_successful_export() -> TestResult {
        let dir = fresh_dir("temp")?;
        AtomicExporter::default()
            .export(&artifact_of(64), &destination(&dir, "clean.pdf")?)
            .map_err(|_| "export refused")?;
        assert!(
            names_in(&dir)?
                .iter()
                .all(|name| !name.starts_with(".jobniac-tmp-"))
        );
        remove_dir(&dir);
        Ok(())
    }
}
