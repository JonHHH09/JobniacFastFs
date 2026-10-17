//! Linux primitives of the owned generated-asset namespace.
//!
//! Every object is reached through a descriptor held open for the whole operation: the root chain
//! is pinned with `O_DIRECTORY | O_NOFOLLOW`, and each child is named as
//! `/proc/self/fd/<held>/<name>` with `O_NOFOLLOW` (the same relation as `openat(2)`). Publication
//! uses an anonymous `O_TMPFILE` inode linked into place with `linkat(2)` through the proc link,
//! so no named partial file is ever visible. Three calls have no safe std equivalent and are
//! `unsafe` below, each with a SAFETY comment: `geteuid`, `fstatvfs` (read-only filesystem check)
//! and `linkat` (the unprivileged link of an anonymous inode, which needs `AT_SYMLINK_FOLLOW`).

#![allow(unsafe_code)]

use super::{Access, Publish, PublishError};
use jobniac_core::generated_root::linux_capability_bytes;
use std::ffi::{CString, OsString};
use std::fs::{self, DirBuilder, File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::mem::MaybeUninit;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

/// Identity of an object as compared by the generated-asset protocol: device, inode, link count,
/// size and both timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    device: u64,
    inode: u64,
    links: u64,
    size: u64,
    modified: i128,
    changed: i128,
}

fn stamp_from(metadata: &Metadata) -> Stamp {
    Stamp {
        device: metadata.dev(),
        inode: metadata.ino(),
        links: metadata.nlink(),
        size: metadata.size(),
        modified: i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec()),
        changed: i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec()),
    }
}

/// Whether publication and namespace control are available: always on Linux, where the
/// operations fail closed individually when the filesystem lacks `O_TMPFILE` or `linkat`.
pub(crate) fn supported() -> bool {
    true
}

/// The effective user id of the process.
fn effective_uid() -> u32 {
    // SAFETY: geteuid has no arguments, cannot fail and touches no memory.
    unsafe { libc::geteuid() }
}

fn permission_denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "generated root object is not private",
    )
}

fn invalid_name() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "generated root child name is invalid",
    )
}

/// The proc path that resolves `name` inside the held directory `parent`.
fn held_child(parent: &File, name: &str) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name)
}

/// A child name must be one plain component: no separators, NUL, `.` or `..`.
fn checked_name(name: &str) -> io::Result<&str> {
    let plain = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\0');
    if plain { Ok(name) } else { Err(invalid_name()) }
}

/// The pinned generated root: every ancestor held open without following links.
#[derive(Debug)]
pub(crate) struct Root {
    ancestors: Vec<File>,
    leaf: File,
}

impl Root {
    /// Pins the absolute `path` component by component and checks that the leaf belongs to the
    /// process user and has no group or other permission bits (`_open_root`).
    pub(crate) fn open(path: &str) -> io::Result<Self> {
        if path.is_empty() || path.contains('\0') {
            return Err(invalid_name());
        }
        let parsed = Path::new(path);
        if !parsed.is_absolute()
            || parsed
                .components()
                .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(invalid_name());
        }
        let flags = libc::O_DIRECTORY | libc::O_NOFOLLOW;
        let mut current = OpenOptions::new()
            .read(true)
            .custom_flags(flags)
            .open("/")?;
        let mut ancestors = Vec::new();
        for part in parsed.components() {
            let Component::Normal(name) = part else {
                continue;
            };
            let name = name.to_str().ok_or_else(invalid_name)?;
            let next = OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open(held_child(&current, name))?;
            ancestors.push(current);
            current = next;
        }
        let info = current.metadata()?;
        if info.uid() != effective_uid() || info.mode() & 0o077 != 0 {
            return Err(permission_denied());
        }
        Ok(Self {
            ancestors,
            leaf: current,
        })
    }

    /// The names of the entries of the held directory. The listing goes through the proc link of
    /// the leaf descriptor, so it reads the directory that was pinned even if the path was renamed.
    pub(crate) fn list_names(&self) -> io::Result<Vec<OsString>> {
        let listing = fs::read_dir(format!("/proc/self/fd/{}", self.leaf.as_raw_fd()))?;
        listing
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect()
    }

    /// Linux holds descriptors, so the chain cannot drift and nothing is re-checked.
    pub(crate) fn verify(&self) -> io::Result<()> {
        let _ = &self.ancestors;
        Ok(())
    }

    /// Owner read, write and search, on a filesystem that is not read-only.
    pub(crate) fn qualifies_for_controls(&self) -> io::Result<bool> {
        let mode = self.leaf.metadata()?.mode();
        if mode & 0o700 != 0o700 {
            return Ok(false);
        }
        let mut buffer = MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: fstatvfs writes one complete statvfs structure into `buffer` on success, and the
        // descriptor is open for the lifetime of `self`.
        let status = unsafe { libc::fstatvfs(self.leaf.as_raw_fd(), buffer.as_mut_ptr()) };
        if status != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the previous call succeeded, so the structure is fully initialized.
        let flags = unsafe { buffer.assume_init() }.f_flag;
        let read_only = libc::ST_RDONLY as std::ffi::c_ulong;
        Ok(flags & read_only == 0)
    }

    /// Confirms that `/proc/self/fd/<leaf>` resolves to the same directory as the held descriptor
    /// (`_require_proc_descriptor`).
    pub(crate) fn require_capability_link(&self) -> io::Result<()> {
        let through_proc = fs::metadata(format!("/proc/self/fd/{}", self.leaf.as_raw_fd()))?;
        let held = self.leaf.metadata()?;
        if !through_proc.is_dir()
            || (through_proc.dev(), through_proc.ino()) != (held.dev(), held.ino())
        {
            return Err(permission_denied());
        }
        Ok(())
    }

    /// Opens an existing child without following links and checks that it is a private regular
    /// file (`_private_file`). A link is refused with `PermissionDenied`.
    pub(crate) fn open_child(&self, name: &str, _access: Access) -> io::Result<File> {
        let name = checked_name(name)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(held_child(&self.leaf, name))
            .map_err(|error| {
                if error.raw_os_error() == Some(libc::ELOOP) {
                    permission_denied()
                } else {
                    error
                }
            })?;
        verify_private_file(&file)?;
        Ok(file)
    }

    /// Creates a new private child, failing if the name exists (`O_CREAT | O_EXCL | O_NOFOLLOW`).
    pub(crate) fn create_child_with(&self, name: &str, data: &[u8]) -> io::Result<()> {
        let name = checked_name(name)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(held_child(&self.leaf, name))?;
        file.write_all(data)?;
        file.sync_all()
    }

    /// Partial controls are left in place on Linux and keep the root unqualified; nothing is
    /// rolled back.
    pub(crate) fn discard_child(&self, _name: &str) -> io::Result<()> {
        Ok(())
    }

    /// Opens an anonymous `O_TMPFILE` inode in the held directory. A filesystem or kernel without
    /// support is `Unsupported`.
    fn open_anonymous(&self) -> Result<File, PublishError> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_TMPFILE)
            .open(format!("/proc/self/fd/{}", self.leaf.as_raw_fd()))
            .map_err(|error| match error.raw_os_error() {
                Some(code)
                    if [libc::EOPNOTSUPP, libc::ENOSYS, libc::EINVAL, libc::EISDIR]
                        .contains(&code) =>
                {
                    PublishError::Unsupported
                }
                _ => PublishError::Io(error),
            })
    }

    /// Checks that anonymous publication can work here without creating a named entry: an
    /// `O_TMPFILE` inode is opened and dropped. Initialization runs this before it creates any
    /// control, so an unsupported filesystem leaves the root untouched.
    pub(crate) fn probe_publication(&self) -> Result<(), PublishError> {
        self.open_anonymous().map(drop)
    }

    /// Publishes `content` under `name` without a named partial file: an anonymous `O_TMPFILE`
    /// inode is written, flushed and linked in with `linkat(2)` through the proc link.
    pub(crate) fn publish_anonymous(
        &self,
        name: &str,
        content: &[u8],
    ) -> Result<Publish, PublishError> {
        let name = checked_name(name).map_err(PublishError::Io)?;
        let mut anonymous = self.open_anonymous()?;
        anonymous.write_all(content).map_err(PublishError::Io)?;
        anonymous.sync_all().map_err(PublishError::Io)?;
        let source = CString::new(format!("/proc/self/fd/{}", anonymous.as_raw_fd()))
            .map_err(|_| PublishError::Unsupported)?;
        let target = CString::new(format!("/proc/self/fd/{}/{name}", self.leaf.as_raw_fd()))
            .map_err(|_| PublishError::Unsupported)?;
        // SAFETY: both arguments are NUL-terminated strings that live across the call. The source
        // is the proc link of this process's own anonymous descriptor, and AT_SYMLINK_FOLLOW makes
        // the kernel follow only that link, never a caller-supplied path. The target is resolved
        // inside the held root descriptor. AT_FDCWD is ignored because both paths are absolute.
        let status = unsafe {
            libc::linkat(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                target.as_ptr(),
                libc::AT_SYMLINK_FOLLOW,
            )
        };
        if status != 0 {
            let error = io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::EEXIST) => Ok(Publish::NameExists),
                Some(code)
                    if [
                        libc::ENOSYS,
                        libc::EOPNOTSUPP,
                        libc::EINVAL,
                        libc::EXDEV,
                        libc::ENOENT,
                    ]
                    .contains(&code) =>
                {
                    Err(PublishError::Unsupported)
                }
                _ => Err(PublishError::Io(error)),
            };
        }
        let written = anonymous.metadata().map_err(PublishError::Io)?;
        let current =
            fs::symlink_metadata(held_child(&self.leaf, name)).map_err(PublishError::Io)?;
        if written.nlink() != 1
            || written.size() != content.len() as u64
            || stamp_from(&written) != stamp_from(&current)
        {
            // Deliberately no unlink here: the name was resolved by path after the link, so it may
            // now be a foreign object, and a name-based removal could destroy it. The mismatch is
            // reported and the object is left for an operator.
            return Err(PublishError::Mismatch);
        }
        self.leaf.sync_all().map_err(PublishError::Io)?;
        Ok(Publish::Created)
    }

    /// Removes a child by name through the held directory, after re-checking that the name still
    /// refers to the verified `file`.
    ///
    /// POSIX has no removal by inode, so a name-based `unlink` after the check cannot be made
    /// atomic: a process of the same user that can write the root could swap a foreign object onto
    /// the name between the re-check and the removal. That window is accepted under the same-user
    /// threat model (such a process can already delete or replace anything in the root, and the
    /// namespace lock only excludes other Jobniac processes). The re-check narrows it to the gap
    /// between two system calls and fails closed on any mismatch.
    pub(crate) fn unlink(&self, name: &str, file: &File) -> io::Result<()> {
        if !self.name_refers_to(name, file)? {
            return Err(permission_denied());
        }
        let name = checked_name(name)?;
        fs::remove_file(held_child(&self.leaf, name))
    }

    /// Whether no object of that name exists (without following a link at that name).
    pub(crate) fn absent(&self, name: &str) -> io::Result<bool> {
        let name = checked_name(name)?;
        match fs::symlink_metadata(held_child(&self.leaf, name)) {
            Ok(_) => Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(error),
        }
    }

    /// Flushes the held directory so a completed link or removal is durable.
    pub(crate) fn sync(&self) -> io::Result<()> {
        self.leaf.sync_all()
    }

    /// Whether `name` currently resolves, without following links, to the object `file` holds.
    pub(crate) fn name_refers_to(&self, name: &str, file: &File) -> io::Result<bool> {
        let name = checked_name(name)?;
        match fs::symlink_metadata(held_child(&self.leaf, name)) {
            Ok(current) => Ok(stamp_from(&current) == stamp_of(file)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Takes the exclusive, non-blocking namespace lock on an open lock file.
    pub(crate) fn lock_exclusive(&self, file: &File) -> io::Result<()> {
        let _ = self;
        match file.try_lock() {
            Ok(()) => Ok(()),
            Err(fs::TryLockError::WouldBlock) => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "generated namespace lock is busy",
            )),
            Err(fs::TryLockError::Error(error)) => Err(error),
        }
    }

    /// Identity `(device, inode)` of the pinned root, for the capability content.
    pub(crate) fn identity(&self) -> io::Result<(u64, u64)> {
        let info = self.leaf.metadata()?;
        Ok((info.dev(), info.ino()))
    }
}

/// Opens nothing; the identity of an opened object for before-and-after comparisons.
pub(crate) fn stamp_of(file: &File) -> io::Result<Stamp> {
    Ok(stamp_from(&file.metadata()?))
}

/// Flushes an existing object that was just compared byte for byte (a read-only descriptor can be
/// flushed on Linux).
pub(crate) fn sync_existing(file: &File) -> io::Result<()> {
    file.sync_all()
}

/// Ownership and privacy of an opened control or asset: a single-link regular file owned by the
/// process user with no group or other permission bits.
pub(crate) fn verify_private_file(file: &File) -> io::Result<()> {
    let info = file.metadata()?;
    if !info.is_file()
        || info.nlink() != 1
        || info.uid() != effective_uid()
        || info.mode() & 0o077 != 0
    {
        return Err(permission_denied());
    }
    Ok(())
}

/// Creates one directory with owner-only permissions (`mkdir 0700`).
pub(crate) fn create_private_directory(path: &str) -> io::Result<()> {
    DirBuilder::new().mode(0o700).create(path)
}

/// The exact capability content of a root published on this platform.
pub(crate) fn capability_bytes(root: &Root, root_id: Uuid) -> io::Result<Vec<u8>> {
    let (device, inode) = root.identity()?;
    Ok(linux_capability_bytes(root_id, device, inode))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh private directory under the temporary directory, removed when dropped.
    struct Private(PathBuf);

    impl Private {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("jobniac-linux-{}", Uuid::new_v4()));
            create_private_directory(path.to_str().expect("utf-8 temp path")).expect("mkdir");
            Self(path)
        }

        fn root(&self) -> Root {
            Root::open(self.0.to_str().expect("utf-8")).expect("open root")
        }
    }

    impl Drop for Private {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unlink_removes_the_verified_object() {
        let dir = Private::new();
        let root = dir.root();
        root.create_child_with("asset", b"x").expect("create");
        let file = root.open_child("asset", Access::ReadDelete).expect("open");
        root.unlink("asset", &file).expect("unlink");
        assert!(root.absent("asset").expect("absent"));
    }

    #[test]
    fn unlink_refuses_a_name_that_now_holds_another_object() {
        let dir = Private::new();
        let root = dir.root();
        root.create_child_with("asset", b"x").expect("create");
        let file = root.open_child("asset", Access::ReadDelete).expect("open");
        // A foreign object takes over the name after the object was verified.
        fs::remove_file(dir.0.join("asset")).expect("swap out");
        root.create_child_with("asset", b"foreign")
            .expect("swap in");
        let error = root.unlink("asset", &file).expect_err("fails closed");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(dir.0.join("asset")).expect("kept"), b"foreign");
    }

    #[test]
    fn the_publication_probe_creates_no_entry() {
        let dir = Private::new();
        let root = dir.root();
        root.probe_publication()
            .expect("O_TMPFILE is supported here");
        assert!(root.list_names().expect("list").is_empty());
    }
}
