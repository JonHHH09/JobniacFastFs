//! Lint: `unsafe` code and `allow(unsafe_code)` appear only in platform modules. The scan is textual: it reads every `.rs` file under the source roots of the
//! workspace and flags `unsafe` blocks, functions, impls, traits and `allow(unsafe_code)` attributes
//! outside the allowed files.
//!
//! A file is a platform module when a path component is `platform`, or its file stem starts with
//! `platform` (for example `export/platform_linux.rs`).
//!
//! The known-violation list is a shrinking allow-list: an entry must still contain unsafe code,
//! and an entry that no longer does is an error, so a fixed violation is removed from the list in
//! the same change. New violations are never added to it; move the code into a platform module.

use std::path::{Path, PathBuf};

/// The source roots scanned, relative to the workspace root.
const ROOTS: &[&str] = &[
    "crates/infra/src",
    "crates/runtime/src",
    "crates/core/src",
    "crates/mcp/src",
    "crates/cli/src",
    "src-tauri/src",
];

/// Files outside platform modules that still contain unsafe code, each with its reason. Shrinking:
/// an entry is removed when its unsafe code moves into a platform module.
const KNOWN_VIOLATIONS: &[(&str, &str)] = &[(
    "crates/infra/src/files/no_follow.rs",
    "one GetFileInformationByHandle call in a function-level allow (pinned-read identity); \
     it is to move into a platform module, which is outside the exporter fix that added this lint",
)];

/// The 1-based line numbers of lines that contain unsafe code, or an allow of `unsafe_code`.
/// Comment lines are skipped. String literals are not parsed, so a literal that contains the
/// keyword would also be reported; the lint is conservative in that direction.
fn unsafe_lines(text: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let code = line.trim_start();
            if code.starts_with("//") {
                return false;
            }
            has_unsafe_keyword(code) || (code.contains("allow(") && code.contains("unsafe_code"))
        })
        .map(|(index, _)| index + 1)
        .collect()
}

/// Whether a code line has `unsafe` as a whole word followed by a block, fn, impl, trait or
/// extern item.
fn has_unsafe_keyword(code: &str) -> bool {
    let bytes = code.as_bytes();
    let mut start = 0;
    while let Some(found) = code[start..].find("unsafe") {
        let at = start + found;
        let end = at + "unsafe".len();
        let before_ok = at == 0 || !is_word(bytes[at - 1]);
        let after_ok = end >= bytes.len() || !is_word(bytes[end]);
        if before_ok && after_ok {
            let rest = code[end..].trim_start();
            if rest.starts_with('{')
                || rest.starts_with("fn ")
                || rest.starts_with("impl")
                || rest.starts_with("trait ")
                || rest.starts_with("extern")
            {
                return true;
            }
        }
        start = end;
    }
    false
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Whether a workspace-relative path is a platform module.
fn is_platform(relative: &str) -> bool {
    let components: Vec<&str> = relative.split('/').collect();
    components.contains(&"platform")
        || components.last().is_some_and(|file| {
            Path::new(file)
                .file_stem()
                .is_some_and(|stem| stem.to_string_lossy().starts_with("platform"))
        })
}

/// Every `.rs` file under `dir`, recursively.
fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn the_detector_recognizes_blocks_fns_impls_and_allows() {
    assert_eq!(unsafe_lines("unsafe { x() }"), [1]);
    assert_eq!(unsafe_lines("pub unsafe fn f() {}"), [1]);
    assert_eq!(unsafe_lines("unsafe impl Send for T {}"), [1]);
    assert_eq!(unsafe_lines("#![allow(unsafe_code)]"), [1]);
    assert_eq!(unsafe_lines("#[allow(clippy::x, unsafe_code)]"), [1]);
    assert!(unsafe_lines("#![forbid(unsafe_code)]").is_empty());
    assert!(unsafe_lines("// unsafe { not code }").is_empty());
    assert!(unsafe_lines("let unsafe_count = 1;").is_empty());
    assert!(unsafe_lines("//! The unsafe calls are linkat.").is_empty());
}

#[test]
fn the_platform_path_rule_is_exact() {
    assert!(is_platform("crates/infra/src/workers/platform/linux.rs"));
    assert!(is_platform(
        "crates/infra/src/generated/platform/windows.rs"
    ));
    assert!(is_platform(
        "crates/infra/src/files/export/platform_linux.rs"
    ));
    assert!(is_platform(
        "crates/infra/src/files/export/platform_windows.rs"
    ));
    assert!(!is_platform("crates/infra/src/files/export.rs"));
    assert!(!is_platform("crates/infra/src/files/no_follow.rs"));
    assert!(!is_platform("crates/infra/src/platforms_list.rs/export.rs"));
}

#[test]
fn unsafe_code_is_confined_to_platform_modules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ROOTS {
        collect(&root.join(dir), &mut files);
    }
    assert!(
        !files.is_empty(),
        "no source files found under {}",
        root.display()
    );

    let mut violations = Vec::new();
    let mut known_still_present = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(&root)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| file.to_string_lossy().replace('\\', "/"));
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("cannot read {relative}: {error}"));
        let lines = unsafe_lines(&text);
        if lines.is_empty() || is_platform(&relative) {
            continue;
        }
        if KNOWN_VIOLATIONS.iter().any(|(path, _)| *path == relative) {
            known_still_present.push(relative);
            continue;
        }
        violations.push(format!("{relative}: lines {lines:?}"));
    }

    for (path, _) in KNOWN_VIOLATIONS {
        assert!(
            known_still_present.iter().any(|present| present == path),
            "{path} no longer contains unsafe code outside a platform module: remove it from \
             KNOWN_VIOLATIONS"
        );
    }
    assert!(
        violations.is_empty(),
        "unsafe code outside platform modules (move it into a platform module):\n{}",
        violations.join("\n")
    );
}
