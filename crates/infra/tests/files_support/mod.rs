//! Shared fixtures for the document import tests: unique scratch folders and synthetic files.
//!
//! The scratch folder is canonicalized, so the root handed to the reader is the real long path
//! even when the temporary directory itself is reached through an alias. Nothing here is personal
//! data: file contents are fixed synthetic bytes.

#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A scratch folder removed when dropped.
#[derive(Debug)]
pub struct Scratch {
    root: PathBuf,
}

impl Scratch {
    /// Creates a new, empty, canonical scratch folder under the system temporary directory.
    pub fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let unique = format!(
            "jobniac-files-{}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            nanos
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).expect("create scratch folder");
        let canonical = fs::canonicalize(&path).expect("canonicalize scratch folder");
        Self {
            root: strip_verbatim(canonical),
        }
    }

    /// The canonical scratch folder.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// The scratch folder as text, for use as an import root.
    pub fn root_text(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }

    /// The absolute text of `relative` below the scratch folder (`/` separators are accepted).
    pub fn text(&self, relative: &str) -> String {
        self.join(relative).to_string_lossy().into_owned()
    }

    /// The absolute path of `relative` below the scratch folder.
    pub fn join(&self, relative: &str) -> PathBuf {
        let mut path = self.root.clone();
        for part in relative.split('/').filter(|part| !part.is_empty()) {
            path.push(part);
        }
        path
    }

    /// Creates the folder `relative`, including its parents.
    pub fn dir(&self, relative: &str) -> PathBuf {
        let path = self.join(relative);
        fs::create_dir_all(&path).expect("create synthetic folder");
        path
    }

    /// Writes `bytes` to the file `relative`, creating parent folders.
    pub fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create synthetic parent");
        }
        let mut file = fs::File::create(&path).expect("create synthetic file");
        file.write_all(bytes).expect("write synthetic file");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a locked file on Windows must not fail the test run.
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Removes the `\\?\` verbatim prefix that `canonicalize` adds on Windows.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy().into_owned();
    match text.strip_prefix(r"\\?\") {
        Some(stripped) if !stripped.starts_with("UNC\\") => PathBuf::from(stripped),
        _ => path,
    }
}

/// Synthetic PDF-like bytes. The reader does not validate the PDF header; the bytes only make the
/// fixtures recognisable in failures.
pub const SAMPLE_PDF: &[u8] = b"%PDF-1.4 synthetic\n";

/// Handles a test prerequisite that the host lacks (symbolic-link privilege, `mkfifo`).
///
/// On a developer machine the test prints a note and returns, so the caller can skip. When the `CI`
/// environment variable is set the prerequisite must exist, so the test panics instead and the run
/// cannot report a silent pass.
pub fn skip_or_fail_without(prerequisite: &str, error: &dyn std::fmt::Display) {
    assert!(
        std::env::var_os("CI").is_none(),
        "{prerequisite} is required on CI but is unavailable: {error}"
    );
    eprintln!("skipped: {prerequisite} is not available here ({error})");
}

/// Creates a directory link (`mklink /J` on Windows, `ln -s` on Linux) from `link` to `target`.
#[cfg(windows)]
pub fn make_junction(link: &Path, target: &Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()?;
    if status.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("mklink /J failed"))
    }
}
