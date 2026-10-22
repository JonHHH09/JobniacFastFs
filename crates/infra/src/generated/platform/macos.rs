//! macOS primitives of the owned generated-asset namespace.
//!
//! Every object is reached through a descriptor held open for the whole operation: the root chain
//! is pinned with `O_DIRECTORY | O_NOFOLLOW`, and each child is resolved with the `*at` calls
//! (`openat`, `fstatat`, `unlinkat`, `renameatx_np`) relative to the held directory descriptor and
//! with `O_NOFOLLOW` or `AT_SYMLINK_NOFOLLOW`. macOS has no anonymous `O_TMPFILE` inode, so
//! publication writes a private temporary file (`.jobniac-tmp-<32 hex digits>`, the same name the
//! Windows module uses), flushes it with `F_FULLFSYNC` and moves it into place with
//! `renameatx_np(RENAME_EXCL)`, which never replaces an existing name. The temporary name is removed
//! on every failure path, and a name that survives a crash is a stale owned temporary file that
//! initialization tolerates (as on Windows).
//!
//! macOS volumes are case-insensitive by default. Every name here is resolved by the kernel, so a
//! case variant of a control, fence or asset name resolves to the same object and can neither hide
//! a fence nor be created next to an existing name (`RENAME_EXCL` fails with `EEXIST` for it).
//!
//! Extended ACLs can grant access that the mode bits do not show (`chmod +a`, Finder sharing), so
//! a root, control or asset that carries any ACL entry is refused as not private.
//!
//! Calls with no safe std equivalent are `unsafe` below, each with a SAFETY comment: `geteuid`,
//! `openat`, `fstat`, `fstatat`, `unlinkat`, `renameatx_np`, `fcntl(F_FULLFSYNC)`, `fstatfs`,
//! `fdopendir`. Declarations come from the `libc` crate except for two constants, which are
//! declared by hand next to their use with the SDK header they come from. The ACL probe lives in
//! [`crate::platform_macos_acl`].

#![allow(unsafe_code)]

use super::{Access, Publish, PublishError};
use crate::platform_macos_acl::has_extended_acl;
use jobniac_core::generated_root::{TEMP_PREFIX, macos_capability_bytes};
use std::ffi::{CStr, CString, OsStr, OsString, c_int, c_uint};
use std::fs::{DirBuilder, File};
use std::io::{self, Write};
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::{Component, Path};
use uuid::Uuid;

/// `RENAME_EXCL` of `renameatx_np`: fail with `EEXIST` instead of replacing the target
/// (`<sys/stdio.h>` in the macOS SDK: `#define RENAME_EXCL 0x00000004`).
const RENAME_EXCL: c_uint = 0x0000_0004;

/// `MNT_RDONLY` of `statfs.f_flags`: a read-only filesystem
/// (`<sys/mount.h>` in the macOS SDK: `#define MNT_RDONLY 0x00000001`).
const MNT_RDONLY: u32 = 0x0000_0001;

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

fn stamp_from_stat(info: &libc::stat) -> Stamp {
    Stamp {
        device: u64::from(info.st_dev as u32),
        inode: info.st_ino,
        links: u64::from(info.st_nlink),
        size: info.st_size as u64,
        modified: i128::from(info.st_mtime) * 1_000_000_000 + i128::from(info.st_mtime_nsec),
        changed: i128::from(info.st_ctime) * 1_000_000_000 + i128::from(info.st_ctime_nsec),
    }
}

/// Whether publication and namespace control are available: always on macOS, where the operations
/// fail closed individually when the filesystem lacks `RENAME_EXCL`.
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

/// A child name must be one plain component: no separators, NUL, `.` or `..`.
fn checked_name(name: &str) -> io::Result<CString> {
    let plain = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\0');
    if plain {
        CString::new(name).map_err(|_| invalid_name())
    } else {
        Err(invalid_name())
    }
}

/// `openat(2)` of `name` inside the held directory, close-on-exec, returned as an owned `File`.
fn open_at(parent: RawFd, name: &CStr, flags: c_int, mode: libc::mode_t) -> io::Result<File> {
    // SAFETY: `name` is NUL-terminated and outlives the call, `parent` is a descriptor the caller
    // holds open, and the mode is read only because every caller that passes O_CREAT supplies it.
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            flags | libc::O_CLOEXEC,
            c_uint::from(mode),
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a freshly opened descriptor that nothing else owns.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// `fstat(2)` of an open descriptor.
fn fstat(fd: RawFd) -> io::Result<libc::stat> {
    let mut buffer = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat writes one complete stat structure into `buffer` on success, and the caller
    // holds `fd` open.
    let status = unsafe { libc::fstat(fd, buffer.as_mut_ptr()) };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so the structure is fully initialized.
    Ok(unsafe { buffer.assume_init() })
}

/// `fstatat(2)` of `name` inside the held directory without following a link at that name.
fn fstat_name(parent: RawFd, name: &CStr) -> io::Result<libc::stat> {
    let mut buffer = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `name` is NUL-terminated and outlives the call, `parent` is held open by the caller,
    // and fstatat writes one complete stat structure into `buffer` on success.
    let status = unsafe {
        libc::fstatat(
            parent,
            name.as_ptr(),
            buffer.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so the structure is fully initialized.
    Ok(unsafe { buffer.assume_init() })
}

/// `unlinkat(2)` of a non-directory `name` inside the held directory.
fn unlink_name(parent: RawFd, name: &CStr) -> io::Result<()> {
    // SAFETY: `name` is NUL-terminated and outlives the call, and `parent` is held open by the
    // caller. Flags 0 remove a non-directory entry and never follow the final component.
    let status = unsafe { libc::unlinkat(parent, name.as_ptr(), 0) };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Flushes `file` to the medium with `F_FULLFSYNC`; a filesystem that rejects it falls back to
/// `fsync(2)`, which is the strongest flush it offers.
fn full_sync(file: &File) -> io::Result<()> {
    // SAFETY: F_FULLFSYNC takes no argument and the descriptor is open for the lifetime of `file`.
    let status = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) };
    if status == 0 { Ok(()) } else { file.sync_all() }
}

/// The pinned generated root: every ancestor held open without following links.
#[derive(Debug)]
pub(crate) struct Root {
    ancestors: Vec<File>,
    leaf: File,
}

impl Root {
    /// Pins the absolute `path` component by component and checks that the leaf belongs to the
    /// process user, has no group or other permission bits and carries no ACL (`_open_root`).
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
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW;
        let mut current = open_at(libc::AT_FDCWD, c"/", flags, 0)?;
        let mut ancestors = Vec::new();
        for part in parsed.components() {
            let Component::Normal(name) = part else {
                continue;
            };
            let name = name.to_str().ok_or_else(invalid_name)?;
            let next = open_at(current.as_raw_fd(), &checked_name(name)?, flags, 0)?;
            ancestors.push(current);
            current = next;
        }
        let info = current.metadata()?;
        if info.uid() != effective_uid()
            || info.mode() & 0o077 != 0
            || has_extended_acl(current.as_raw_fd())?
        {
            return Err(permission_denied());
        }
        Ok(Self {
            ancestors,
            leaf: current,
        })
    }

    /// The names of the entries of the held directory, read through a duplicate of the leaf
    /// descriptor, so it lists the directory that was pinned even if the path was renamed.
    pub(crate) fn list_names(&self) -> io::Result<Vec<OsString>> {
        // SAFETY: dup returns a new descriptor or -1; the leaf descriptor is open.
        let duplicate = unsafe { libc::dup(self.leaf.as_raw_fd()) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `duplicate` is a directory descriptor this function owns; on success fdopendir
        // takes ownership and closedir releases it.
        let stream = unsafe { libc::fdopendir(duplicate) };
        if stream.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: fdopendir failed, so `duplicate` is still owned here and closed once.
            unsafe { libc::close(duplicate) };
            return Err(error);
        }
        // SAFETY: `stream` is a valid directory stream; the offset is shared with the held
        // descriptor, so it is rewound to list from the start.
        unsafe { libc::rewinddir(stream) };
        let mut names = Vec::new();
        loop {
            // readdir returns NULL at the end and on error; errno is cleared first to tell them
            // apart.
            // SAFETY: errno is thread-local and writable.
            unsafe { *libc::__error() = 0 };
            // SAFETY: `stream` is valid and used by this thread only.
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let error = io::Error::last_os_error();
                // SAFETY: `stream` is valid and closed exactly once (this also closes the dup).
                unsafe { libc::closedir(stream) };
                return if error.raw_os_error() == Some(0) {
                    Ok(names)
                } else {
                    Err(error)
                };
            }
            // SAFETY: readdir returned a valid entry whose d_name is NUL-terminated; it is copied
            // before the next readdir call.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name != b"." && name != b".." {
                names.push(OsStr::from_bytes(name).to_os_string());
            }
        }
    }

    /// Nothing is re-checked: every operation is relative to the held descriptors, so a root that
    /// was renamed after it was pinned keeps receiving its writes at the new location (a held
    /// descriptor does not prevent a rename of an ancestor on macOS). The capability content
    /// carries the device and inode of the pinned root, which a copy to another root cannot
    /// reproduce.
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
        let mut buffer = MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: fstatfs writes one complete statfs structure into `buffer` on success, and the
        // descriptor is open for the lifetime of `self`.
        let status = unsafe { libc::fstatfs(self.leaf.as_raw_fd(), buffer.as_mut_ptr()) };
        if status != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the previous call succeeded, so the structure is fully initialized.
        let flags = unsafe { buffer.assume_init() }.f_flags;
        Ok(flags & MNT_RDONLY == 0)
    }

    /// Confirms that descriptor-relative resolution works: `.` inside the held directory is the
    /// held directory (the counterpart of the proc-link check on Linux).
    pub(crate) fn require_capability_link(&self) -> io::Result<()> {
        let held = fstat(self.leaf.as_raw_fd())?;
        let through = fstat_name(self.leaf.as_raw_fd(), c".")?;
        if through.st_mode & libc::S_IFMT != libc::S_IFDIR
            || (through.st_dev, through.st_ino) != (held.st_dev, held.st_ino)
        {
            return Err(permission_denied());
        }
        Ok(())
    }

    /// Opens an existing child without following links and checks that it is a private regular
    /// file (`_private_file`). A link is refused with `PermissionDenied`; `O_NONBLOCK` keeps a
    /// FIFO from blocking the open, and the regular-file check then refuses it.
    pub(crate) fn open_child(&self, name: &str, _access: Access) -> io::Result<File> {
        let name = checked_name(name)?;
        let file = open_at(
            self.leaf.as_raw_fd(),
            &name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0,
        )
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
        let mut file = open_at(
            self.leaf.as_raw_fd(),
            &name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            0o600,
        )?;
        file.write_all(data)?;
        full_sync(&file)
    }

    /// Partial controls are left in place on macOS and keep the root unqualified; nothing is
    /// rolled back.
    pub(crate) fn discard_child(&self, _name: &str) -> io::Result<()> {
        Ok(())
    }

    /// Creates the private temporary file of one publication, never replacing a name.
    fn create_temporary(&self) -> io::Result<(CString, File)> {
        let name = CString::new(format!("{TEMP_PREFIX}{}", Uuid::new_v4().simple()))
            .map_err(|_| invalid_name())?;
        let file = open_at(
            self.leaf.as_raw_fd(),
            &name,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            0o600,
        )?;
        Ok((name, file))
    }

    /// Removes a temporary name that this call created, after re-checking that it still refers to
    /// the held `file`. Best effort: a failure leaves a stale owned temporary name, which
    /// initialization tolerates and which can never be mistaken for a published asset. The
    /// re-check has the same unavoidable window as [`Root::unlink`].
    fn discard_temporary(&self, name: &CStr, file: &File) {
        let still_ours = match (
            fstat_name(self.leaf.as_raw_fd(), name),
            fstat(file.as_raw_fd()),
        ) {
            (Ok(named), Ok(held)) => (named.st_dev, named.st_ino) == (held.st_dev, held.st_ino),
            _ => false,
        };
        if still_ours {
            let _ = unlink_name(self.leaf.as_raw_fd(), name);
        }
    }

    /// `renameatx_np(RENAME_EXCL)` of `from` onto `to` inside the held directory: it fails with
    /// `EEXIST` instead of replacing, and the filesystem compares names the way it resolves them
    /// (case-insensitively on the default APFS).
    fn rename_exclusive(&self, from: &CStr, to: &CStr) -> io::Result<()> {
        let leaf = self.leaf.as_raw_fd();
        // SAFETY: both names are NUL-terminated and outlive the call, both descriptors are the held
        // directory, and RENAME_EXCL makes the kernel refuse an existing target.
        let status =
            unsafe { libc::renameatx_np(leaf, from.as_ptr(), leaf, to.as_ptr(), RENAME_EXCL) };
        if status == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// Checks that publication can work here: a private temporary file is created, moved with
    /// `RENAME_EXCL` to a second temporary name and removed. A filesystem without `RENAME_EXCL` is
    /// `Unsupported`. Initialization runs this before it creates any control; at most owned
    /// temporary names exist during the probe, and none remains afterwards.
    pub(crate) fn probe_publication(&self) -> Result<(), PublishError> {
        let (first, file) = self.create_temporary().map_err(PublishError::Io)?;
        let second = CString::new(format!("{TEMP_PREFIX}{}", Uuid::new_v4().simple()))
            .map_err(|_| PublishError::Unsupported)?;
        match self.rename_exclusive(&first, &second) {
            Ok(()) => {
                self.discard_temporary(&second, &file);
                Ok(())
            }
            Err(error) => {
                self.discard_temporary(&first, &file);
                Err(publish_error(error))
            }
        }
    }

    /// Publishes `content` under `name` without ever exposing a partial file under it: a private
    /// temporary file is written and flushed, then moved with `renameatx_np(RENAME_EXCL)`. An
    /// existing name is `NameExists` and is never replaced; the temporary name is removed on every
    /// failure path.
    pub(crate) fn publish_anonymous(
        &self,
        name: &str,
        content: &[u8],
    ) -> Result<Publish, PublishError> {
        let target = checked_name(name).map_err(PublishError::Io)?;
        let (temporary, mut file) = self.create_temporary().map_err(PublishError::Io)?;
        let written = file
            .write_all(content)
            .and_then(|()| full_sync(&file))
            .and_then(|()| fstat(file.as_raw_fd()));
        match written {
            Ok(info) if info.st_nlink == 1 && info.st_size as u64 == content.len() as u64 => {}
            Ok(_) => {
                self.discard_temporary(&temporary, &file);
                return Err(PublishError::Mismatch);
            }
            Err(error) => {
                self.discard_temporary(&temporary, &file);
                return Err(PublishError::Io(error));
            }
        }
        if let Err(error) = self.rename_exclusive(&temporary, &target) {
            self.discard_temporary(&temporary, &file);
            return match error.raw_os_error() {
                Some(libc::EEXIST) => Ok(Publish::NameExists),
                _ => Err(publish_error(error)),
            };
        }
        let held = fstat(file.as_raw_fd()).map_err(PublishError::Io)?;
        let current = fstat_name(self.leaf.as_raw_fd(), &target).map_err(PublishError::Io)?;
        if stamp_from_stat(&held) != stamp_from_stat(&current) {
            // Deliberately no unlink here: the name was resolved by path after the rename, so it
            // may now be a foreign object, and a name-based removal could destroy it. The mismatch
            // is reported and the object is left for an operator.
            return Err(PublishError::Mismatch);
        }
        full_sync(&self.leaf).map_err(PublishError::Io)?;
        Ok(Publish::Created)
    }

    /// Removes a child by name through the held directory, after re-checking that the name still
    /// refers to the verified `file`.
    ///
    /// POSIX has no removal by inode, so a name-based `unlinkat` after the check cannot be made
    /// atomic: a process of the same user that can write the root could swap a foreign object onto
    /// the name between the re-check and the removal. That window is accepted under the same-user
    /// threat model (such a process can already delete or replace anything in the root, and the
    /// namespace lock only excludes other Jobniac processes). The re-check narrows it to the gap
    /// between two system calls and fails closed on any mismatch.
    pub(crate) fn unlink(&self, name: &str, file: &File) -> io::Result<()> {
        if !self.name_refers_to(name, file)? {
            return Err(permission_denied());
        }
        unlink_name(self.leaf.as_raw_fd(), &checked_name(name)?)
    }

    /// Whether no object of that name exists (without following a link at that name).
    pub(crate) fn absent(&self, name: &str) -> io::Result<bool> {
        match fstat_name(self.leaf.as_raw_fd(), &checked_name(name)?) {
            Ok(_) => Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(error),
        }
    }

    /// Flushes the held directory so a completed rename or removal is durable.
    pub(crate) fn sync(&self) -> io::Result<()> {
        full_sync(&self.leaf)
    }

    /// Whether `name` currently resolves, without following links, to the object `file` holds. The
    /// kernel resolves the name, so a case variant of the same entry refers to the same object.
    pub(crate) fn name_refers_to(&self, name: &str, file: &File) -> io::Result<bool> {
        match fstat_name(self.leaf.as_raw_fd(), &checked_name(name)?) {
            Ok(current) => Ok(stamp_from_stat(&current) == stamp_of(file)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Takes the exclusive, non-blocking namespace lock on an open lock file (`flock(2)` with
    /// `LOCK_EX | LOCK_NB`, which is what `File::try_lock` calls on macOS).
    pub(crate) fn lock_exclusive(&self, file: &File) -> io::Result<()> {
        let _ = self;
        match file.try_lock() {
            Ok(()) => Ok(()),
            Err(std::fs::TryLockError::WouldBlock) => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "generated namespace lock is busy",
            )),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        }
    }

    /// Identity `(device, inode)` of the pinned root, for the capability content.
    pub(crate) fn identity(&self) -> io::Result<(u64, u64)> {
        let info = fstat(self.leaf.as_raw_fd())?;
        Ok((stamp_from_stat(&info).device, info.st_ino))
    }
}

/// Maps a failed no-replace rename: a filesystem that cannot do it is `Unsupported`.
fn publish_error(error: io::Error) -> PublishError {
    match error.raw_os_error() {
        Some(code) if [libc::ENOTSUP, libc::EINVAL, libc::ENOSYS].contains(&code) => {
            PublishError::Unsupported
        }
        _ => PublishError::Io(error),
    }
}

/// Opens nothing; the identity of an opened object for before-and-after comparisons.
pub(crate) fn stamp_of(file: &File) -> io::Result<Stamp> {
    Ok(stamp_from_stat(&fstat(file.as_raw_fd())?))
}

/// Flushes an existing object that was just compared byte for byte (a read-only descriptor can be
/// flushed on macOS).
pub(crate) fn sync_existing(file: &File) -> io::Result<()> {
    full_sync(file)
}

/// Ownership and privacy of an opened control or asset: a single-link regular file owned by the
/// process user with no group or other permission bits and no ACL.
pub(crate) fn verify_private_file(file: &File) -> io::Result<()> {
    let info = file.metadata()?;
    if !info.is_file()
        || info.nlink() != 1
        || info.uid() != effective_uid()
        || info.mode() & 0o077 != 0
        || has_extended_acl(file.as_raw_fd())?
    {
        return Err(permission_denied());
    }
    Ok(())
}

/// Creates one directory with owner-only permissions (`mkdir 0700`).
pub(crate) fn create_private_directory(path: &str) -> io::Result<()> {
    DirBuilder::new().mode(0o700).create(path)
}

/// The exact capability content of a root published on this platform: root identity plus the
/// device and inode of the pinned directory.
pub(crate) fn capability_bytes(root: &Root, root_id: Uuid) -> io::Result<Vec<u8>> {
    let (device, inode) = root.identity()?;
    Ok(macos_capability_bytes(root_id, device, inode))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// A fresh private directory under the canonical temporary directory (`/var` is a link to
    /// `/private/var`, which the production open refuses), removed when dropped.
    struct Private(PathBuf);

    impl Private {
        fn new() -> Self {
            let base = fs::canonicalize(std::env::temp_dir()).expect("canonical temp dir");
            let path = base.join(format!("jobniac-macos-{}", Uuid::new_v4()));
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
    fn the_publication_probe_leaves_no_entry() {
        let dir = Private::new();
        let root = dir.root();
        root.probe_publication().expect("RENAME_EXCL is supported");
        assert!(root.list_names().expect("list").is_empty());
    }

    #[test]
    fn publication_never_replaces_and_leaves_no_temporary_name() {
        let dir = Private::new();
        let root = dir.root();
        assert!(matches!(
            root.publish_anonymous("asset", b"first"),
            Ok(Publish::Created)
        ));
        assert!(matches!(
            root.publish_anonymous("asset", b"second"),
            Ok(Publish::NameExists)
        ));
        assert_eq!(fs::read(dir.0.join("asset")).expect("kept"), b"first");
        assert_eq!(
            root.list_names().expect("list"),
            vec![OsString::from("asset")]
        );
    }

    #[test]
    fn a_case_variant_name_is_the_same_name() {
        let dir = Private::new();
        let root = dir.root();
        root.create_child_with("Fence", b"x").expect("create");
        let insensitive = !root.absent("fence").expect("absent");
        if !insensitive {
            // A case-sensitive volume: nothing to prove here.
            return;
        }
        assert!(matches!(
            root.publish_anonymous("FENCE", b"y"),
            Ok(Publish::NameExists)
        ));
        let file = root.open_child("fence", Access::Read).expect("open");
        assert!(root.name_refers_to("FENCE", &file).expect("same object"));
    }

    #[test]
    fn an_acl_on_the_root_or_a_child_is_not_private() {
        let dir = Private::new();
        let root = dir.root();
        root.create_child_with("asset", b"x").expect("create");
        let grant = |path: &Path| {
            std::process::Command::new("chmod")
                .args(["+a", "group:everyone allow read"])
                .arg(path)
                .status()
                .expect("run chmod")
                .success()
        };
        assert!(grant(&dir.0.join("asset")));
        let error = root
            .open_child("asset", Access::Read)
            .expect_err("an ACL'd child is refused");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(grant(&dir.0));
        let error = Root::open(dir.0.to_str().expect("utf-8")).expect_err("an ACL'd root");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn a_symbolic_link_component_is_never_followed() {
        let dir = Private::new();
        let alias = dir.0.with_file_name(format!("alias-{}", Uuid::new_v4()));
        std::os::unix::fs::symlink(&dir.0, &alias).expect("symlink");
        let result = Root::open(alias.to_str().expect("utf-8"));
        let _ = fs::remove_file(&alias);
        assert!(result.is_err());
    }
}
