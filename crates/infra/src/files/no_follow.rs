//! Platform primitives for no-follow, pinned reads of document imports.
//!
//! Std only, except one Windows call: `GetFileInformationByHandle` samples the link count, the
//! volume serial number and the file index of an open handle. It is the only
//! `unsafe` of this module, documented at the call. Two implementations sit behind one interface:
//!
//! - Linux (x86-64 and AArch64): every directory of the chain is held open as a descriptor. The
//!   next name is opened as `/proc/self/fd/<held>/<name>` with `O_NOFOLLOW`, which resolves the
//!   name inside the held directory (the `openat(2)` relation). The flag
//!   values come from `libc`, because they differ between architectures.
//! - Windows: every directory is opened with `FILE_FLAG_OPEN_REPARSE_POINT` and share mode
//!   read|write (no `FILE_SHARE_DELETE`), so no ancestor can be renamed while the chain is held.
//!   Regular files are opened with share mode read only, which denies writers while they are read.
//! - macOS: every directory is held as a descriptor, starting at `/`, and each next name is opened
//!   with `openat(2)` relative to the held descriptor (`O_DIRECTORY | O_NOFOLLOW`; there is no
//!   `/proc`). Descriptors cannot stop a rename, so [`DirChain::verify`] compares every held
//!   directory with the name it was opened under (`fstatat` with `AT_SYMLINK_NOFOLLOW`). It uses
//!   the rustix safe API and contains no `unsafe` code.
//!
//! Every other target reports [`available`] as `false` and the import is refused before any open.

use std::io;
use std::path::Path;

/// The kind of object behind an opened handle, as far as import needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A regular file.
    Regular,
    /// A directory.
    Directory,
    /// A symbolic link, junction or other reparse point (Windows only; Linux refuses links at
    /// open time with `ELOOP`).
    ReparsePoint,
    /// Anything else, such as a FIFO, a socket or a device.
    Other,
}

/// Identity of an opened object. Two samples are equal only when the object and its content
/// metadata are unchanged: Linux compares device, inode, size and both timestamps (`st_mtime_ns`,
/// `st_ctime_ns`); Windows compares volume serial, file index, size and last write time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    /// Device number (Linux) or volume serial number (Windows).
    pub volume: u64,
    /// Inode number (Linux) or file index (Windows).
    pub index: u64,
    /// Size in bytes.
    pub size: u64,
    /// Modification time in nanoseconds (Linux) or the last write time in 100 ns units (Windows).
    pub written: i128,
    /// Status change time in nanoseconds (Linux); always zero on Windows.
    pub changed: i128,
}

/// Metadata of an opened object, sampled from the handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeInfo {
    /// The object kind.
    pub kind: NodeKind,
    /// The number of hard links, sampled from the handle on both platforms. A link count other
    /// than one is refused by the import.
    pub links: Option<u64>,
    /// The identity sample used for the before and after comparison.
    pub identity: FileIdentity,
}

impl NodeInfo {
    /// The size in bytes from the same sample.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.identity.size
    }
}

/// Whether this build can perform the pinned, no-follow reads at all.
#[must_use]
pub fn available() -> bool {
    platform::available()
}

/// A chain of pinned directories, from a base directory down to the directory that holds the file.
#[derive(Debug)]
pub struct DirChain {
    inner: platform::Chain,
}

impl DirChain {
    /// Pins `base` and then each of `names` below it. Any link, reparse point or non-directory in
    /// the chain is refused; a missing name fails with [`io::ErrorKind::NotFound`].
    ///
    /// # Errors
    ///
    /// Fails when a component cannot be opened as a plain directory.
    pub fn pin(base: &Path, names: &[String]) -> io::Result<Self> {
        for name in names {
            plain_name(name)?;
        }
        platform::Chain::pin(base, names).map(|inner| Self { inner })
    }

    /// Opens `name` inside the innermost pinned directory without following links. The file is
    /// opened for reading only, and FIFOs and devices open without blocking.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::NotFound`] for a missing name and with another kind for a link
    /// or an unreadable object.
    pub fn open_file(&self, name: &str) -> io::Result<PinnedFile> {
        plain_name(name)?;
        self.inner.open_file(name).map(|inner| PinnedFile { inner })
    }

    /// Re-checks every pinned directory. On Linux the held descriptors keep the chain fixed, so
    /// this always succeeds; on Windows it compares each held handle with its first sample; on
    /// macOS it compares each held descriptor with the name it was opened under.
    ///
    /// # Errors
    ///
    /// Fails when a pinned directory changed identity or stopped being a plain directory.
    pub fn verify(&self) -> io::Result<()> {
        self.inner.verify()
    }
}

/// A chain name must be one plain component: not empty, not `.` or `..`, and without separators
/// or NUL, so it can neither leave the pinned directory nor name an absolute path.
fn plain_name(name: &str) -> io::Result<()> {
    let separator =
        name.contains('/') || (cfg!(windows) && (name.contains('\\') || name.contains(':')));
    if name.is_empty()
        || name == "."
        || name == ".."
        || separator
        || name.contains('\0')
        || is_fork_component(name)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path component is not a plain name",
        ));
    }
    Ok(())
}

/// Whether a name addresses a macOS resource fork or named fork (`file/..namedfork/rsrc`, the
/// analogue of a Windows alternate data stream). Any name that contains `..namedfork`, in any
/// case, is refused on macOS; other targets have no such namespace.
#[must_use]
pub(crate) fn is_fork_component(name: &str) -> bool {
    cfg!(target_os = "macos") && name.to_ascii_lowercase().contains("..namedfork")
}

/// A file candidate opened without following links.
#[derive(Debug)]
pub struct PinnedFile {
    inner: platform::Leaf,
}

impl PinnedFile {
    /// Samples the metadata of the opened handle.
    ///
    /// # Errors
    ///
    /// Fails when the handle cannot report its metadata.
    pub fn info(&self) -> io::Result<NodeInfo> {
        self.inner.info()
    }

    /// Reads at most `limit` bytes from the start of the handle, stopping at the end of the file.
    ///
    /// # Errors
    ///
    /// Fails when the handle cannot be read.
    pub fn read_up_to(&mut self, limit: u64) -> io::Result<Vec<u8>> {
        self.inner.read_up_to(limit)
    }
}

/// Whether the final path reported for a Windows file equals the requested path, compared
/// case-insensitively. Junctions, short-name aliases and drive substitutions give another final
/// path and therefore fail. The requested path must already be normalized.
///
/// # Errors
///
/// Fails when the file cannot be opened to ask for its final path.
#[cfg(windows)]
pub fn final_path_matches(requested: &Path) -> io::Result<bool> {
    let canonical = std::fs::canonicalize(requested)?;
    let text = canonical.to_string_lossy();
    let stripped = text.strip_prefix(r"\\?\").unwrap_or(&text);
    Ok(stripped.to_lowercase() == requested.to_string_lossy().to_lowercase())
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod platform {
    use super::{FileIdentity, NodeInfo, NodeKind};
    use std::fs::{File, Metadata, OpenOptions};
    use std::io::{self, Read};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::os::unix::io::AsRawFd;
    use std::path::{Path, PathBuf};

    // The `open(2)` flag values differ between architectures (AArch64 does not use the
    // asm-generic ones), so they come from `libc`.
    use libc::{O_DIRECTORY, O_NOFOLLOW, O_NONBLOCK};

    pub fn available() -> bool {
        Path::new("/proc/self/fd").is_dir()
    }

    #[derive(Debug)]
    pub struct Chain {
        directories: Vec<File>,
    }

    impl Chain {
        pub fn pin(base: &Path, names: &[String]) -> io::Result<Self> {
            let root = OpenOptions::new()
                .read(true)
                .custom_flags(O_DIRECTORY | O_NOFOLLOW)
                .open(base)?;
            let mut directories = vec![root];
            for name in names {
                let parent = leaf_directory(&directories)?;
                let directory = OpenOptions::new()
                    .read(true)
                    .custom_flags(O_DIRECTORY | O_NOFOLLOW)
                    .open(held_path(parent, name))?;
                directories.push(directory);
            }
            Ok(Self { directories })
        }

        pub fn open_file(&self, name: &str) -> io::Result<Leaf> {
            let parent = leaf_directory(&self.directories)?;
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(O_NOFOLLOW | O_NONBLOCK)
                .open(held_path(parent, name))?;
            Ok(Leaf { file })
        }

        pub fn verify(&self) -> io::Result<()> {
            // The held descriptors cannot be renamed or replaced, so the chain is fixed.
            Ok(())
        }
    }

    #[derive(Debug)]
    pub struct Leaf {
        file: File,
    }

    impl Leaf {
        pub fn info(&self) -> io::Result<NodeInfo> {
            Ok(info_of(&self.file.metadata()?))
        }

        pub fn read_up_to(&mut self, limit: u64) -> io::Result<Vec<u8>> {
            let mut content = Vec::new();
            (&mut self.file).take(limit).read_to_end(&mut content)?;
            Ok(content)
        }
    }

    impl Drop for Chain {
        fn drop(&mut self) {
            // Close the innermost directory first, as on Windows.
            while self.directories.pop().is_some() {}
        }
    }

    fn leaf_directory(directories: &[File]) -> io::Result<&File> {
        directories
            .last()
            .ok_or_else(|| io::Error::other("directory chain is empty"))
    }

    /// The path that makes the kernel resolve `name` inside the held directory.
    fn held_path(parent: &File, name: &str) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}/{}", parent.as_raw_fd(), name))
    }

    fn info_of(metadata: &Metadata) -> NodeInfo {
        let kind = if metadata.is_file() {
            NodeKind::Regular
        } else if metadata.is_dir() {
            NodeKind::Directory
        } else {
            NodeKind::Other
        };
        let written =
            i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec());
        let changed =
            i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec());
        NodeInfo {
            kind,
            links: Some(metadata.nlink()),
            identity: FileIdentity {
                volume: metadata.dev(),
                index: metadata.ino(),
                size: metadata.size(),
                written,
                changed,
            },
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::{FileIdentity, NodeInfo, NodeKind};
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read};
    use std::mem::MaybeUninit;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    const GENERIC_READ: u32 = 0x8000_0000;
    const FILE_LIST_DIRECTORY: u32 = 0x0000_0001;
    const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
    const READ_CONTROL: u32 = 0x0002_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

    pub fn available() -> bool {
        true
    }

    /// A pinned directory with the identity sampled when it was pinned.
    #[derive(Debug)]
    struct PinnedDirectory {
        file: File,
        volume: u64,
        index: u64,
    }

    /// The pinned directories. Each handle has no `FILE_SHARE_DELETE`, so it cannot be renamed or
    /// removed while the chain is held; the chain is re-checked for reparse points, kind and
    /// identity (volume serial and file index, not the write time, which a new entry changes).
    #[derive(Debug)]
    pub struct Chain {
        directories: Vec<PinnedDirectory>,
        leaf_directory: PathBuf,
    }

    impl Chain {
        pub fn pin(base: &Path, names: &[String]) -> io::Result<Self> {
            let mut paths = vec![base.to_path_buf()];
            let mut current = base.to_path_buf();
            for name in names {
                current.push(name);
                paths.push(current.clone());
            }
            let mut directories = Vec::with_capacity(paths.len());
            for path in &paths {
                let file = OpenOptions::new()
                    .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL)
                    .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                    .open(path)?;
                let info = info_of(&file)?;
                if info.kind != NodeKind::Directory {
                    return Err(io::Error::other("component is not a plain directory"));
                }
                directories.push(PinnedDirectory {
                    file,
                    volume: info.identity.volume,
                    index: info.identity.index,
                });
            }
            Ok(Self {
                directories,
                leaf_directory: current,
            })
        }

        pub fn open_file(&self, name: &str) -> io::Result<Leaf> {
            let file = OpenOptions::new()
                .access_mode(GENERIC_READ)
                .share_mode(FILE_SHARE_READ)
                // No backup semantics: a directory at the leaf fails to open.
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(self.leaf_directory.join(name))?;
            Ok(Leaf { file })
        }

        pub fn verify(&self) -> io::Result<()> {
            for pinned in &self.directories {
                let info = info_of(&pinned.file)?;
                if info.kind != NodeKind::Directory
                    || info.identity.volume != pinned.volume
                    || info.identity.index != pinned.index
                {
                    return Err(io::Error::other("directory chain changed"));
                }
            }
            Ok(())
        }
    }

    #[derive(Debug)]
    pub struct Leaf {
        file: File,
    }

    impl Leaf {
        pub fn info(&self) -> io::Result<NodeInfo> {
            info_of(&self.file)
        }

        pub fn read_up_to(&mut self, limit: u64) -> io::Result<Vec<u8>> {
            let mut content = Vec::new();
            (&mut self.file).take(limit).read_to_end(&mut content)?;
            Ok(content)
        }
    }

    impl Drop for Chain {
        fn drop(&mut self) {
            // Close the leaf directory first and then each ancestor.
            while self.directories.pop().is_some() {}
        }
    }

    /// Maps an open handle to a node. Reparse points are reported before directories, so a
    /// junction is never mistaken for a plain directory. The link count, volume serial number and
    /// file index come from the handle; the identity is those with the size and last write time,
    /// and the read share mode denies writers while the file is open.
    fn info_of(file: &File) -> io::Result<NodeInfo> {
        let metadata = file.metadata()?;
        let information = handle_information(file)?;
        let attributes = metadata.file_attributes();
        let kind = if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            NodeKind::ReparsePoint
        } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            NodeKind::Directory
        } else {
            NodeKind::Regular
        };
        let index =
            (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
        Ok(NodeInfo {
            kind,
            links: Some(u64::from(information.nNumberOfLinks)),
            identity: FileIdentity {
                volume: u64::from(information.dwVolumeSerialNumber),
                index,
                size: metadata.file_size(),
                written: i128::from(metadata.last_write_time()),
                changed: 0,
            },
        })
    }

    /// Reads the handle information of an open file (link count, volume serial, file index).
    #[allow(unsafe_code)]
    fn handle_information(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
        let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: `file` is an open handle owned by this process and it stays open for the whole
        // call. `information` points to writable storage of exactly the structure the call fills,
        // and the call writes to it only when it returns nonzero.
        let ok =
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call returned nonzero, so every field of the structure has been written.
        Ok(unsafe { information.assume_init() })
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{FileIdentity, NodeInfo, NodeKind};
    use std::fs::File;
    use std::io::{self, Read};
    use std::os::unix::fs::MetadataExt;
    use std::path::{Component, Path};

    use rustix::fs::{AtFlags, Mode, OFlags, Stat, fstat, open, openat, statat};

    pub fn available() -> bool {
        true
    }

    /// The flags of every directory open: a directory, never a followed final link, close-on-exec.
    fn directory_flags() -> OFlags {
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
    }

    /// One pinned directory and the name it was opened under in its parent.
    #[derive(Debug)]
    struct Pinned {
        file: File,
        name: Option<String>,
    }

    #[derive(Debug)]
    pub struct Chain {
        directories: Vec<Pinned>,
        /// Whether the first directory is `/` (an absolute base), so it can be re-checked by name.
        anchored: bool,
    }

    impl Chain {
        /// Pins `base` and then each of `names`. An absolute `base` is walked component by
        /// component from `/`, so a link anywhere in it is refused too. A relative `base` is
        /// opened without following a final link only, as on Linux.
        pub fn pin(base: &Path, names: &[String]) -> io::Result<Self> {
            let mut directories = Vec::new();
            if base.is_absolute() {
                directories.push(Pinned {
                    file: File::from(open("/", directory_flags(), Mode::empty())?),
                    name: None,
                });
                for component in base.components() {
                    match component {
                        Component::RootDir | Component::CurDir => {}
                        Component::Normal(name) => {
                            let name = name.to_str().ok_or_else(not_plain)?;
                            if super::is_fork_component(name) {
                                return Err(not_plain());
                            }
                            push_child(&mut directories, name)?;
                        }
                        Component::ParentDir | Component::Prefix(_) => return Err(not_plain()),
                    }
                }
            } else {
                directories.push(Pinned {
                    file: File::from(open(base, directory_flags(), Mode::empty())?),
                    name: None,
                });
            }
            for name in names {
                push_child(&mut directories, name)?;
            }
            Ok(Self {
                directories,
                anchored: base.is_absolute(),
            })
        }

        pub fn open_file(&self, name: &str) -> io::Result<Leaf> {
            let parent = leaf_directory(&self.directories)?;
            // O_NONBLOCK: a FIFO opens without waiting for a writer. O_NOFOLLOW: a link at the
            // leaf fails with ELOOP, a dangling one included.
            let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
            let opened = openat(&parent.file, name, flags, Mode::empty())?;
            Ok(Leaf {
                file: File::from(opened),
            })
        }

        /// Every held directory must still be the directory its parent holds under that name:
        /// a rename or replacement of any ancestor after the pin is detected here.
        pub fn verify(&self) -> io::Result<()> {
            let root = self
                .directories
                .first()
                .ok_or_else(|| io::Error::other("directory chain is empty"))?;
            if self.anchored {
                let at_root = open("/", directory_flags(), Mode::empty())?;
                if !same_object(&fstat(&at_root)?, &fstat(&root.file)?) {
                    return Err(changed());
                }
            }
            for pair in self.directories.windows(2) {
                let (parent, child) = (&pair[0], &pair[1]);
                let name = child.name.as_deref().ok_or_else(changed)?;
                let current = statat(&parent.file, name, AtFlags::SYMLINK_NOFOLLOW)?;
                if !is_directory(&current) || !same_object(&current, &fstat(&child.file)?) {
                    return Err(changed());
                }
            }
            Ok(())
        }
    }

    /// Opens `name` inside the innermost held directory and holds it as the new innermost one.
    fn push_child(directories: &mut Vec<Pinned>, name: &str) -> io::Result<()> {
        let parent = leaf_directory(directories)?;
        let opened = openat(&parent.file, name, directory_flags(), Mode::empty())?;
        directories.push(Pinned {
            file: File::from(opened),
            name: Some(name.to_owned()),
        });
        Ok(())
    }

    fn leaf_directory(directories: &[Pinned]) -> io::Result<&Pinned> {
        directories
            .last()
            .ok_or_else(|| io::Error::other("directory chain is empty"))
    }

    fn same_object(left: &Stat, right: &Stat) -> bool {
        (left.st_dev, left.st_ino) == (right.st_dev, right.st_ino)
    }

    fn is_directory(stat: &Stat) -> bool {
        rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::Directory
    }

    fn not_plain() -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "path component is not a plain name",
        )
    }

    fn changed() -> io::Error {
        io::Error::other("directory chain changed")
    }

    #[derive(Debug)]
    pub struct Leaf {
        file: File,
    }

    impl Leaf {
        pub fn info(&self) -> io::Result<NodeInfo> {
            let metadata = self.file.metadata()?;
            let kind = if metadata.is_file() {
                NodeKind::Regular
            } else if metadata.is_dir() {
                NodeKind::Directory
            } else {
                NodeKind::Other
            };
            let written =
                i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec());
            let changed =
                i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec());
            Ok(NodeInfo {
                kind,
                links: Some(metadata.nlink()),
                identity: FileIdentity {
                    volume: metadata.dev(),
                    index: metadata.ino(),
                    size: metadata.size(),
                    written,
                    changed,
                },
            })
        }

        pub fn read_up_to(&mut self, limit: u64) -> io::Result<Vec<u8>> {
            let mut content = Vec::new();
            (&mut self.file).take(limit).read_to_end(&mut content)?;
            Ok(content)
        }
    }

    impl Drop for Chain {
        fn drop(&mut self) {
            // Close the innermost directory first, as on the other platforms.
            while self.directories.pop().is_some() {}
        }
    }
}

#[cfg(not(any(
    windows,
    target_os = "macos",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
)))]
mod platform {
    use super::NodeInfo;
    use std::io;
    use std::path::Path;

    pub fn available() -> bool {
        false
    }

    #[derive(Debug)]
    pub struct Chain;

    impl Chain {
        pub fn pin(_base: &Path, _names: &[String]) -> io::Result<Self> {
            Err(unsupported())
        }

        pub fn open_file(&self, _name: &str) -> io::Result<Leaf> {
            Err(unsupported())
        }

        pub fn verify(&self) -> io::Result<()> {
            Err(unsupported())
        }
    }

    #[derive(Debug)]
    pub struct Leaf;

    impl Leaf {
        pub fn info(&self) -> io::Result<NodeInfo> {
            Err(unsupported())
        }

        pub fn read_up_to(&mut self, _limit: u64) -> io::Result<Vec<u8>> {
            Err(unsupported())
        }
    }

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "pinned reads are not available on this platform",
        )
    }
}
