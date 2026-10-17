//! Reads of document import files from the configured import root.
//!
//! The checks run in this order: root configured, platform available, path shape, root shape,
//! traversal, containment, pinned no-follow open, regular file, size, bounded read, identity
//! re-check. The refusal reasons are wire-visible
//! strings, including those for a relative path, a hard-linked file and a Windows path that the
//! lexical rules refuse.

use std::fmt;
use std::io;
use std::path::{Component, Path};

use super::no_follow::{self, DirChain, NodeInfo, NodeKind, PinnedFile};
#[cfg(windows)]
use super::windows_path;

/// Largest import accepted, in bytes (10 MiB).
pub const MAX_IMPORT_BYTES: u64 = 10 * 1024 * 1024;

/// A file read from the import root: its last path component and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedFile {
    /// The last path component of the requested file.
    pub file_name: String,
    /// The file content, at most [`MAX_IMPORT_BYTES`] bytes.
    pub content: Vec<u8>,
}

/// Why an import read was refused. [`ImportRefusal::reason`] gives the caller-visible text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportRefusal {
    /// No import root is configured (unset or empty).
    RootNotConfigured,
    /// Pinned no-follow reads are not available on this platform.
    PlatformUnavailable,
    /// The path is blank, contains NUL, or is not representable as text.
    InvalidPath,
    /// The import root is relative or contains a `..` component.
    RootNotAbsolute,
    /// The requested path contains a `..` component.
    PathTraversal,
    /// The requested path is relative. It is refused rather than resolved against the
    /// working directory.
    PathNotAbsolute,
    /// The requested path is not inside the import root, or is the root itself.
    OutsideRoot,
    /// The final object is a directory, a FIFO, a device or another non-regular object.
    NotRegularFile,
    /// The final file has another hard link.
    HardLinked,
    /// The file is larger than [`MAX_IMPORT_BYTES`].
    TooLarge,
    /// The file or its identity changed while it was read.
    ChangedWhileReading,
    /// A path component or the file does not exist.
    NotFound,
    /// A component or the file is a link, a reparse point, or cannot be opened or read.
    NotReadable,
}

impl ImportRefusal {
    /// The caller-visible reason text, in the PDF extraction form (the storage form
    /// shortens `PDF file size` to `file size`).
    /// the storage form shortens `PDF file size` to `file size`).
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::RootNotConfigured => "document import root is not configured",
            Self::PlatformUnavailable => "safe document import is unavailable on this platform",
            Self::InvalidPath => "must be a valid file path",
            Self::RootNotAbsolute => {
                "document import root must be an absolute path without traversal"
            }
            Self::PathTraversal => "path traversal is not allowed",
            Self::PathNotAbsolute => "document import path must be absolute",
            Self::OutsideRoot => "file must be inside the document import root",
            Self::NotRegularFile => "must identify a regular file",
            Self::HardLinked => "file must not have more than one hard link",
            Self::TooLarge => "PDF file size exceeds limit of 10485760 bytes",
            Self::ChangedWhileReading => "file changed while being read",
            Self::NotFound => "file was not found",
            Self::NotReadable => "file is not readable or contains a symbolic link",
        }
    }
}

impl fmt::Display for ImportRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.reason())
    }
}

impl std::error::Error for ImportRefusal {}

/// Reads files from one configured import root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRoot {
    root: Option<String>,
}

impl ImportRoot {
    /// Creates a reader for `root`. `None` or an empty root refuses every read with
    /// [`ImportRefusal::RootNotConfigured`]. Nothing is created and nothing is read here.
    #[must_use]
    pub const fn new(root: Option<String>) -> Self {
        Self { root }
    }

    /// Reads `path`, which must be an absolute path to a regular file inside the import root.
    ///
    /// The read never writes, never creates the root, and never follows a link in any component.
    /// The size is checked before the read, the read is capped at [`MAX_IMPORT_BYTES`] plus one
    /// byte, and the identity is sampled before and after the read.
    ///
    /// # Errors
    ///
    /// Returns the [`ImportRefusal`] for the first rule that fails, in check order.
    pub fn read(&self, path: &str) -> Result<ImportedFile, ImportRefusal> {
        let root_text = self
            .root
            .as_deref()
            .filter(|text| !text.is_empty())
            .ok_or(ImportRefusal::RootNotConfigured)?;
        if !no_follow::available() {
            return Err(ImportRefusal::PlatformUnavailable);
        }
        if jobniac_text::str_rules::strip_text(path).is_empty() || path.contains('\0') {
            return Err(ImportRefusal::InvalidPath);
        }
        let root = Path::new(root_text);
        if !root.is_absolute() || has_parent_component(root) {
            return Err(ImportRefusal::RootNotAbsolute);
        }
        let source = Path::new(path);
        if has_parent_component(source) {
            return Err(ImportRefusal::PathTraversal);
        }
        if !source.is_absolute() {
            return Err(ImportRefusal::PathNotAbsolute);
        }
        let relative = relative_names(root, source)?;
        platform_read(root, source, &relative)
    }
}

fn has_parent_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}

fn component_text(component: Component<'_>) -> Result<&str, ImportRefusal> {
    component
        .as_os_str()
        .to_str()
        .ok_or(ImportRefusal::InvalidPath)
}

/// Compares two components the way the platform compares paths: Windows components are compared
/// case-insensitively, POSIX components exactly.
fn same_component(left: Component<'_>, right: Component<'_>) -> Result<bool, ImportRefusal> {
    let left = component_text(left)?;
    let right = component_text(right)?;
    if cfg!(windows) {
        Ok(left.to_lowercase() == right.to_lowercase())
    } else {
        Ok(left == right)
    }
}

/// The components of `source` below `root`. Refuses a source that is not inside the root and a
/// source that is the root itself.
fn relative_names(root: &Path, source: &Path) -> Result<Vec<String>, ImportRefusal> {
    let root_parts: Vec<Component<'_>> = root.components().collect();
    let source_parts: Vec<Component<'_>> = source.components().collect();
    if source_parts.len() < root_parts.len() {
        return Err(ImportRefusal::OutsideRoot);
    }
    for (expected, actual) in root_parts.iter().zip(&source_parts) {
        if !same_component(*expected, *actual)? {
            return Err(ImportRefusal::OutsideRoot);
        }
    }
    if source_parts.len() == root_parts.len() {
        return Err(ImportRefusal::NotRegularFile);
    }
    source_parts[root_parts.len()..]
        .iter()
        .map(|component| component_text(*component).map(str::to_string))
        .collect()
}

/// Maps an OS error to a refusal: a missing object is `file was not found`,
/// every other failure (links included) is `file is not readable or contains a symbolic link`.
fn map_io(error: &io::Error) -> ImportRefusal {
    if error.kind() == io::ErrorKind::NotFound {
        ImportRefusal::NotFound
    } else {
        ImportRefusal::NotReadable
    }
}

/// The handle operations that [`read_leaf`] needs. [`PinnedFile`] implements it; the unit tests
/// substitute a scripted handle so that the identity and size rules run deterministically.
trait ReadableFile {
    /// Samples the metadata of the handle.
    fn sample(&mut self) -> io::Result<NodeInfo>;
    /// Reads at most `limit` bytes from the start of the handle.
    fn read_bounded(&mut self, limit: u64) -> io::Result<Vec<u8>>;
}

impl ReadableFile for PinnedFile {
    fn sample(&mut self) -> io::Result<NodeInfo> {
        self.info()
    }

    fn read_bounded(&mut self, limit: u64) -> io::Result<Vec<u8>> {
        self.read_up_to(limit)
    }
}

/// The refusal for an opened directory. On Windows, opening a directory fails inside the
/// open helper (the reader reports it as unreadable), while the POSIX open succeeds and fails the
/// regular-file test.
const fn directory_refusal() -> ImportRefusal {
    if cfg!(windows) {
        ImportRefusal::NotReadable
    } else {
        ImportRefusal::NotRegularFile
    }
}

/// Classifies the opened file, re-checks the pinned chain and the final path, then reads it with
/// the size cap and the identity re-check.
fn read_leaf<F: ReadableFile>(
    leaf: &mut F,
    verify_chain: &dyn Fn() -> io::Result<()>,
    final_path_matches: &dyn Fn() -> io::Result<bool>,
) -> Result<Vec<u8>, ImportRefusal> {
    let before = leaf.sample().map_err(|error| map_io(&error))?;
    match before.kind {
        NodeKind::Regular => {}
        NodeKind::ReparsePoint => return Err(ImportRefusal::NotReadable),
        NodeKind::Directory => return Err(directory_refusal()),
        NodeKind::Other => return Err(ImportRefusal::NotRegularFile),
    }
    // A link count other than one is refused on both platforms. The count is sampled
    // from the same handle as the identity, so a link added after the sample changes the identity.
    if before.links.is_some_and(|links| links != 1) {
        return Err(ImportRefusal::HardLinked);
    }
    verify_chain().map_err(|error| map_io(&error))?;
    let path_matches = final_path_matches().map_err(|error| map_io(&error))?;
    if !path_matches {
        return Err(ImportRefusal::NotReadable);
    }
    if before.size() > MAX_IMPORT_BYTES {
        return Err(ImportRefusal::TooLarge);
    }
    let content = leaf
        .read_bounded(MAX_IMPORT_BYTES + 1)
        .map_err(|error| map_io(&error))?;
    let after = leaf.sample().map_err(|error| map_io(&error))?;
    verify_chain().map_err(|error| map_io(&error))?;
    let read_length = u64::try_from(content.len()).unwrap_or(u64::MAX);
    if before.identity != after.identity || read_length != after.size() {
        return Err(ImportRefusal::ChangedWhileReading);
    }
    if read_length > MAX_IMPORT_BYTES {
        return Err(ImportRefusal::TooLarge);
    }
    Ok(content)
}

/// The Windows text of the requested path, built from its components so that `/`, repeated
/// separators and `.` are treated like in a `Path`.
#[cfg(windows)]
fn windows_text(source: &Path) -> Result<String, ImportRefusal> {
    let mut text = String::new();
    for component in source.components() {
        match component {
            Component::Prefix(_) => text.push_str(component_text(component)?),
            Component::RootDir => text.push('\\'),
            Component::Normal(_) => {
                if !text.ends_with('\\') {
                    text.push('\\');
                }
                text.push_str(component_text(component)?);
            }
            Component::CurDir | Component::ParentDir => return Err(ImportRefusal::NotReadable),
        }
    }
    Ok(text)
}

/// Opens and reads the file on Windows: the lexical rules first, then a pinned chain from the
/// drive root, then the leaf opened without following reparse points, then the final-path check.
#[cfg(windows)]
fn platform_read(
    _root: &Path,
    source: &Path,
    _relative: &[String],
) -> Result<ImportedFile, ImportRefusal> {
    let text = windows_text(source)?;
    let normalized =
        windows_path::normalize_drive_path(&text).map_err(|_| ImportRefusal::NotReadable)?;
    // The drive root is `X:\` (three bytes, ASCII); the remainder holds every name down to the file.
    let names: Vec<String> = normalized[3..].split('\\').map(str::to_string).collect();
    let (leaf_name, directories) = names.split_last().ok_or(ImportRefusal::NotRegularFile)?;
    let requested = std::path::PathBuf::from(&normalized);
    let chain =
        DirChain::pin(Path::new(&normalized[..3]), directories).map_err(|error| map_io(&error))?;
    let mut leaf = chain.open_file(leaf_name).map_err(|error| map_io(&error))?;
    let content = read_leaf(&mut leaf, &|| chain.verify(), &|| {
        no_follow::final_path_matches(&requested)
    })?;
    Ok(ImportedFile {
        file_name: leaf_name.clone(),
        content,
    })
}

/// The directory names between the root and the requested file's parent, for a POSIX root.
#[cfg(not(windows))]
fn root_names(root: &Path) -> Result<Vec<String>, ImportRefusal> {
    let mut names = Vec::new();
    for component in root.components() {
        if let Component::Normal(name) = component {
            names.push(name.to_str().ok_or(ImportRefusal::InvalidPath)?.to_string());
        }
    }
    Ok(names)
}

/// Opens and reads the file below a POSIX root: every component from `/` down is pinned.
#[cfg(not(windows))]
fn platform_read(
    root: &Path,
    _source: &Path,
    relative: &[String],
) -> Result<ImportedFile, ImportRefusal> {
    let (leaf_name, parents) = relative.split_last().ok_or(ImportRefusal::NotRegularFile)?;
    let mut directories = root_names(root)?;
    directories.extend(parents.iter().cloned());
    let chain = DirChain::pin(Path::new("/"), &directories).map_err(|error| map_io(&error))?;
    let mut leaf = chain.open_file(leaf_name).map_err(|error| map_io(&error))?;
    let always_matches = || -> io::Result<bool> { Ok(true) };
    let content = read_leaf(&mut leaf, &|| chain.verify(), &always_matches)?;
    Ok(ImportedFile {
        file_name: leaf_name.clone(),
        content,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::super::no_follow::FileIdentity;
    use super::*;

    /// A handle that answers samples from a script and serves fixed bytes.
    struct Scripted {
        samples: VecDeque<io::Result<NodeInfo>>,
        content: Vec<u8>,
    }

    impl ReadableFile for Scripted {
        fn sample(&mut self) -> io::Result<NodeInfo> {
            self.samples
                .pop_front()
                .unwrap_or_else(|| Err(io::Error::other("script exhausted")))
        }

        fn read_bounded(&mut self, limit: u64) -> io::Result<Vec<u8>> {
            let take = usize::try_from(limit)
                .unwrap_or(usize::MAX)
                .min(self.content.len());
            Ok(self.content[..take].to_vec())
        }
    }

    fn node(kind: NodeKind, links: Option<u64>, size: u64, written: i128) -> NodeInfo {
        NodeInfo {
            kind,
            links,
            identity: FileIdentity {
                volume: 1,
                index: 2,
                size,
                written,
                changed: 3,
            },
        }
    }

    fn scripted(samples: Vec<NodeInfo>, content: &[u8]) -> Scripted {
        Scripted {
            samples: samples.into_iter().map(Ok).collect(),
            content: content.to_vec(),
        }
    }

    fn ok_chain() -> io::Result<()> {
        Ok(())
    }

    fn read_with(handle: &mut Scripted, path_matches: bool) -> Result<Vec<u8>, ImportRefusal> {
        read_leaf(handle, &ok_chain, &|| Ok(path_matches))
    }

    #[test]
    fn unchanged_regular_file_is_returned() {
        let mut handle = scripted(
            vec![
                node(NodeKind::Regular, Some(1), 3, 7),
                node(NodeKind::Regular, Some(1), 3, 7),
            ],
            b"abc",
        );
        assert_eq!(read_with(&mut handle, true), Ok(b"abc".to_vec()));
    }

    #[test]
    fn identity_change_between_the_samples_is_refused() {
        let mut handle = scripted(
            vec![
                node(NodeKind::Regular, Some(1), 3, 7),
                node(NodeKind::Regular, Some(1), 3, 8),
            ],
            b"abc",
        );
        assert_eq!(
            read_with(&mut handle, true),
            Err(ImportRefusal::ChangedWhileReading)
        );
    }

    #[test]
    fn content_length_different_from_the_post_read_size_is_refused() {
        let mut handle = scripted(
            vec![
                node(NodeKind::Regular, Some(1), 3, 7),
                node(NodeKind::Regular, Some(1), 3, 7),
            ],
            b"ab",
        );
        assert_eq!(
            read_with(&mut handle, true),
            Err(ImportRefusal::ChangedWhileReading)
        );
    }

    #[test]
    fn size_over_the_limit_is_refused_before_any_read() {
        let mut handle = scripted(
            vec![node(NodeKind::Regular, Some(1), MAX_IMPORT_BYTES + 1, 7)],
            b"",
        );
        assert_eq!(read_with(&mut handle, true), Err(ImportRefusal::TooLarge));
    }

    #[test]
    fn hard_linked_file_is_refused() {
        let mut handle = scripted(vec![node(NodeKind::Regular, Some(2), 3, 7)], b"abc");
        assert_eq!(read_with(&mut handle, true), Err(ImportRefusal::HardLinked));
    }

    #[test]
    fn unknown_link_count_is_not_refused() {
        let mut handle = scripted(
            vec![
                node(NodeKind::Regular, None, 3, 7),
                node(NodeKind::Regular, None, 3, 7),
            ],
            b"abc",
        );
        assert_eq!(read_with(&mut handle, true), Ok(b"abc".to_vec()));
    }

    #[test]
    fn object_kinds_map_to_the_refusal_reasons() {
        let mut reparse = scripted(vec![node(NodeKind::ReparsePoint, None, 0, 0)], b"");
        assert_eq!(
            read_with(&mut reparse, true),
            Err(ImportRefusal::NotReadable)
        );
        let mut other = scripted(vec![node(NodeKind::Other, Some(1), 0, 0)], b"");
        assert_eq!(
            read_with(&mut other, true),
            Err(ImportRefusal::NotRegularFile)
        );
        let mut directory = scripted(vec![node(NodeKind::Directory, None, 0, 0)], b"");
        assert_eq!(read_with(&mut directory, true), Err(directory_refusal()));
    }

    #[test]
    fn final_path_that_differs_is_not_readable() {
        let mut handle = scripted(vec![node(NodeKind::Regular, Some(1), 3, 7)], b"abc");
        assert_eq!(
            read_with(&mut handle, false),
            Err(ImportRefusal::NotReadable)
        );
    }

    #[test]
    fn chain_drift_is_not_readable() {
        let mut handle = scripted(vec![node(NodeKind::Regular, Some(1), 3, 7)], b"abc");
        let drifted = || Err(io::Error::other("directory chain changed"));
        assert_eq!(
            read_leaf(&mut handle, &drifted, &|| Ok(true)),
            Err(ImportRefusal::NotReadable)
        );
    }

    #[test]
    fn missing_object_during_the_sample_is_not_found() {
        let mut handle = Scripted {
            samples: VecDeque::from([Err(io::Error::from(io::ErrorKind::NotFound))]),
            content: Vec::new(),
        };
        assert_eq!(read_with(&mut handle, true), Err(ImportRefusal::NotFound));
    }

    #[test]
    fn size_limit_is_ten_mebibytes() {
        assert_eq!(MAX_IMPORT_BYTES, 10_485_760);
    }
}
