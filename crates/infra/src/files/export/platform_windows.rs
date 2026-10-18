//! Windows publication through a pinned directory chain and a no-replace rename.
//!
//! All Win32 calls of the exporter live here, each block with a `SAFETY:` comment. The shared
//! orchestration (destination syntax, protected roots, outcome mapping) is in the parent `export`
//! module.
//!
//! Follow-up: the generated-root platform layer (`crate::generated::platform::windows`) repeats
//! the private-DACL, SID and handle-identity primitives of this module; merge them into one
//! platform layer.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::Path;
use std::ptr::null_mut;

use jobniac_core::artifacts::{DocumentArtifact, validate_artifact};
use uuid::Uuid;
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION, GetAce,
    GetAclInformation, GetTokenInformation, OWNER_SECURITY_INFORMATION, SECURITY_ATTRIBUTES,
    TOKEN_ELEVATION, TOKEN_USER, TokenElevation, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_DISPOSITION_INFO,
    FILE_RENAME_INFO, FileDispositionInfo, FileRenameInfo, GetFileInformationByHandle,
    GetFinalPathNameByHandleW, SetFileInformationByHandle,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::{AtomicExporter, Failure, TEMP_PREFIX, inside_protected, protected_snapshot};

const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_LIST_DIRECTORY: u32 = 0x0000_0001;
const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
const READ_CONTROL: u32 = 0x0002_0000;
const DELETE: u32 = 0x0001_0000;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const TOKEN_QUERY: u32 = 8;
/// The rights that would let a broad principal alter a protected directory (`_WRITE_RIGHTS`).
const WRITE_RIGHTS: u32 = 0x02
    | 0x04
    | 0x10
    | 0x40
    | 0x100
    | 0x0001_0000
    | 0x0004_0000
    | 0x0008_0000
    | 0x1000_0000
    | 0x4000_0000;
const INHERIT_ONLY_ACE: u8 = 0x08;
const CHILD_INHERIT_ACES: u8 = 0x03;
const ADMINISTRATORS: &str = "S-1-5-32-544";
const SYSTEM: &str = "S-1-5-18";
const CREATOR_OWNER: &str = "S-1-3-0";
const OWNER_RIGHTS: &str = "S-1-3-4";
/// The largest final path accepted (32768 units).
const MAX_FINAL_PATH_UNITS: usize = 32768;
/// The first buffer size of a final-path query (the starting size).
const FIRST_FINAL_PATH_UNITS: usize = 512;

/// Identity and attributes of an open handle, from `GetFileInformationByHandle`.
#[derive(Clone, Copy)]
struct Info {
    attributes: u32,
    volume: u32,
    index: u64,
    size: u64,
    links: u32,
}

/// The pinned directories of the destination, from the drive root down to the parent. `identities`
/// holds the `(volume, index)` pair of each handle when it was pinned.
struct Chain {
    path: String,
    handles: Vec<File>,
    identities: Vec<(u32, u64)>,
}

/// The points where the publication can be observed or made to fail. Production uses
/// [`Production`]; the unit tests supply their own to drive the refusal and uncertainty paths
/// (short or zero writes, a permission change or a rename between steps, a failed sync or rename,
/// and a foreign owner).
pub(super) trait Seam {
    /// Writes a piece of content and returns the number of bytes accepted.
    fn write(&self, file: &mut File, piece: &[u8]) -> io::Result<usize> {
        file.write(piece)
    }

    /// Called once all content is written, before the file is synced and the chain re-verified.
    fn after_write(&self, _parent: &str) {}

    /// Flushes the held temporary file to disk.
    fn sync(&self, file: &File) -> io::Result<()> {
        file.sync_all()
    }

    /// Called after the rename succeeded: a failure here is an uncertain outcome.
    fn after_rename(&self) -> io::Result<()> {
        Ok(())
    }

    /// The SID string of the user who must own the parent and hold its only write rights.
    fn current_user(&self) -> io::Result<String> {
        current_user_sid()
    }
}

/// The production seam: every hook is the default.
struct Production;

impl Seam for Production {}

/// Publish `artifact` as `name` under `parent` (a normalized drive path).
pub(super) fn publish(
    exporter: &AtomicExporter,
    artifact: &DocumentArtifact,
    parent: &str,
    name: &str,
    published: &mut bool,
) -> Result<String, Failure> {
    publish_with(exporter, artifact, parent, name, published, &Production)
}

fn publish_with(
    exporter: &AtomicExporter,
    artifact: &DocumentArtifact,
    parent: &str,
    name: &str,
    published: &mut bool,
    seam: &dyn Seam,
) -> Result<String, Failure> {
    let snapshot = protected_snapshot(&exporter.protected_roots);
    let chain = Chain::pin(parent).map_err(|_| Failure::Refused)?;
    check_parent(&chain, exporter, &snapshot, seam)?;
    let directory = parent.trim_end_matches('\\');
    let target = format!("{directory}\\{name}");
    if exists_no_follow(&target)? {
        return Err(Failure::Refused);
    }
    let temp = format!("{directory}\\{TEMP_PREFIX}{}", temp_suffix());
    let mut file = create_new_private(&temp).map_err(|_| Failure::Refused)?;
    let outcome = finish(
        &mut file, &chain, &target, artifact, exporter, &snapshot, published, seam,
    );
    if outcome.is_err() && !*published {
        let _ = delete_on_handle(&file);
    }
    drop(file);
    outcome.map(|()| name.to_owned())
}

/// Writes, verifies and renames the held temporary file.
#[allow(clippy::too_many_arguments)]
fn finish(
    file: &mut File,
    chain: &Chain,
    target: &str,
    artifact: &DocumentArtifact,
    exporter: &AtomicExporter,
    snapshot: &[String],
    published: &mut bool,
    seam: &dyn Seam,
) -> Result<(), Failure> {
    for piece in artifact.content.chunks(1 << 20) {
        write_all_with(seam, file, piece)?;
    }
    seam.after_write(&chain.path);
    seam.sync(file).map_err(|_| Failure::Refused)?;
    validate_artifact(artifact.clone()).map_err(|_| Failure::Refused)?;
    check_parent(chain, exporter, snapshot, seam)?;
    let info = info_of(file).map_err(|_| Failure::Refused)?;
    if info.size != artifact.content.len() as u64
        || info.links != 1
        || info.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
        || info.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(Failure::Refused);
    }
    rename_no_replace(file, target).map_err(|_| Failure::Refused)?;
    *published = true;
    seam.after_rename().map_err(|_| Failure::Published)?;
    let current = info_of(file).map_err(|_| Failure::Published)?;
    if !same_path(&final_path(file).map_err(|_| Failure::Published)?, target)
        || current.links != 1
        || current.size != artifact.content.len() as u64
    {
        return Err(Failure::Published);
    }
    Ok(())
}

/// Writes one chunk completely. A zero-progress write is a refusal, and short writes continue.
fn write_all_with(seam: &dyn Seam, file: &mut File, mut piece: &[u8]) -> Result<(), Failure> {
    while !piece.is_empty() {
        let written = seam.write(file, piece).map_err(|_| Failure::Refused)?;
        if written == 0 {
            return Err(Failure::Refused);
        }
        piece = piece.get(written..).ok_or(Failure::Refused)?;
    }
    Ok(())
}

/// Re-verifies the pinned chain, the private DACL of the leaf and the protected-root rules.
fn check_parent(
    chain: &Chain,
    exporter: &AtomicExporter,
    snapshot: &[String],
    seam: &dyn Seam,
) -> Result<(), Failure> {
    chain.verify().map_err(|_| Failure::Refused)?;
    let leaf = chain.handles.last().ok_or(Failure::Refused)?;
    let user = seam.current_user().map_err(|_| Failure::Refused)?;
    check_private(leaf, WRITE_RIGHTS, &user).map_err(|_| Failure::Refused)?;
    if protected_snapshot(&exporter.protected_roots) != snapshot {
        return Err(Failure::Refused);
    }
    if inside_protected(Path::new(&chain.path), &exporter.protected_roots, snapshot) {
        return Err(Failure::Refused);
    }
    for root in &exporter.protected_roots {
        let identity = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(root)
            .ok()
            .and_then(|file| info_of(&file).ok())
            .map(|info| (info.volume, info.index));
        if identity.is_some_and(|id| chain.identities.contains(&id)) {
            return Err(Failure::Refused);
        }
    }
    Ok(())
}

impl Chain {
    /// Pins the drive root and each component, refusing reparse points and non-directories. The
    /// leaf must also report the requested path as its final path.
    fn pin(parent: &str) -> io::Result<Self> {
        let mut prefixes = Vec::new();
        let mut current = String::new();
        for (index, part) in parent.split('\\').enumerate() {
            if index == 0 {
                current = format!("{part}\\");
            } else if !part.is_empty() {
                if !current.ends_with('\\') {
                    current.push('\\');
                }
                current.push_str(part);
            }
            prefixes.push(current.clone());
        }
        let mut handles = Vec::new();
        let mut identities = Vec::new();
        for prefix in &prefixes {
            let file = OpenOptions::new()
                .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(prefix)?;
            let info = info_of(&file)?;
            if info.attributes & FILE_ATTRIBUTE_DIRECTORY == 0
                || info.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            {
                return Err(io::Error::other("component is not a plain directory"));
            }
            handles.push(file);
            identities.push((info.volume, info.index));
        }
        let chain = Self {
            path: parent.to_owned(),
            handles,
            identities,
        };
        let leaf = chain
            .handles
            .last()
            .ok_or_else(|| io::Error::other("empty directory chain"))?;
        if !same_path(&final_path(leaf)?, parent) {
            return Err(io::Error::other(
                "final path differs from the requested path",
            ));
        }
        Ok(chain)
    }

    /// Each pinned handle still reports the identity it had when pinned, is not a reparse point,
    /// and the leaf still reports the requested path. Only identity is compared: a directory's
    /// size and link count change when its entries change, which is not drift.
    fn verify(&self) -> io::Result<()> {
        for (file, identity) in self.handles.iter().zip(&self.identities) {
            let info = info_of(file)?;
            if (info.volume, info.index) != *identity
                || info.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            {
                return Err(io::Error::other("pinned directory changed"));
            }
        }
        let leaf = self
            .handles
            .last()
            .ok_or_else(|| io::Error::other("empty directory chain"))?;
        if !same_path(&final_path(leaf)?, &self.path) {
            return Err(io::Error::other(
                "final path differs from the requested path",
            ));
        }
        Ok(())
    }
}

/// Opens `path` without following a final reparse point and reports whether it exists.
fn exists_no_follow(path: &str) -> Result<bool, Failure> {
    match OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
    {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(Failure::Refused),
    }
}

/// The extended-length form (`\\?\`) of a drive-absolute path, so that paths past the classic
/// limit reach the Win32 calls that take a raw name. Other text is returned unchanged.
fn extended(path: &str) -> String {
    let drive_absolute = path.len() >= 3 && path.as_bytes()[1] == b':';
    if drive_absolute && !path.starts_with(r"\\") {
        format!(r"\\?\{path}")
    } else {
        path.to_owned()
    }
}

/// The owner-only descriptor text: SYSTEM and the current user, with the given inheritance.
fn private_sddl(inherit: &str) -> io::Result<String> {
    let user = current_user_sid()?;
    Ok(format!(
        "D:P(A;{inherit};FA;;;{SYSTEM})(A;{inherit};FA;;;{user})"
    ))
}

/// Converts an SDDL string into a LocalAlloc'd descriptor, which the caller frees.
fn convert_sddl(sddl: &str) -> io::Result<*mut c_void> {
    let encoded = wide(sddl);
    let mut descriptor: *mut c_void = null_mut();
    // SAFETY: `encoded` is NUL-terminated and lives across the call; `descriptor` receives a
    // LocalAlloc'd descriptor that the caller frees with LocalFree.
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            encoded.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(descriptor)
}

/// A new directory with the owner-only DACL (inherited by children).
pub(super) fn create_private_directory(path: &Path) -> io::Result<()> {
    let descriptor = convert_sddl(&private_sddl("OICI")?)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let encoded = wide(&extended(&path.to_string_lossy()));
    // SAFETY: `encoded` is NUL-terminated and lives across the call; `attributes` points at the
    // descriptor, which stays allocated until it is freed below.
    let created = unsafe { CreateDirectoryW(encoded.as_ptr(), &attributes) };
    let error = io::Error::last_os_error();
    // SAFETY: `descriptor` came from the conversion above and is released exactly once.
    unsafe { LocalFree(descriptor) };
    if created == 0 { Err(error) } else { Ok(()) }
}

/// An exclusive temporary file with an owner-only DACL.
fn create_new_private(path: &str) -> io::Result<File> {
    let descriptor = convert_sddl(&private_sddl("")?)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let encoded = wide(&extended(path));
    // SAFETY: `encoded` is NUL-terminated and lives across the call; `attributes` and its
    // descriptor stay valid for the call. CREATE_NEW with no sharing makes the handle exclusive,
    // and the returned handle is owned by the File below.
    let handle = unsafe {
        CreateFileW(
            encoded.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | DELETE | READ_CONTROL,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    // SAFETY: `descriptor` came from the conversion above and is released exactly once.
    unsafe { LocalFree(descriptor.cast()) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the handle is a fresh, owned file handle that nothing else holds.
    Ok(unsafe { File::from_raw_handle(handle as _) })
}

/// Renames the held file to `target` without replacing an existing name (ReplaceIfExists FALSE).
fn rename_no_replace(file: &File, target: &str) -> io::Result<()> {
    let encoded: Vec<u16> = extended(target).encode_utf16().collect();
    let name_bytes = encoded.len() * 2;
    let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let total = offset + name_bytes + 2;
    // Aligned storage for the variable-length structure.
    let mut buffer = vec![0_u64; total.div_ceil(8)];
    let base = buffer.as_mut_ptr().cast::<u8>();
    // SAFETY: `buffer` holds at least `total` bytes and is 8-byte aligned, which covers the
    // structure. The name is copied inside the buffer, after the fixed header, and the length
    // prefix counts only the name bytes.
    unsafe {
        let info = base.cast::<FILE_RENAME_INFO>();
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).Anonymous.Flags = 0;
        (*info).RootDirectory = null_mut();
        (*info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(encoded.as_ptr().cast::<u8>(), base.add(offset), name_bytes);
        if SetFileInformationByHandle(
            file.as_raw_handle() as HANDLE,
            FileRenameInfo,
            base.cast(),
            total as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Marks the held file for deletion on close (no path involved).
fn delete_on_handle(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: `info` is a valid FILE_DISPOSITION_INFO for the duration of the call, and the
    // handle is open with DELETE access.
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as HANDLE,
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// The identity sample of an open handle.
fn info_of(file: &File) -> io::Result<Info> {
    let mut raw = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    // SAFETY: the handle is open and `raw` is a writable structure of the right type.
    let ok =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, raw.as_mut_ptr()) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so the structure is initialized.
    let raw = unsafe { raw.assume_init() };
    Ok(Info {
        attributes: raw.dwFileAttributes,
        volume: raw.dwVolumeSerialNumber,
        index: (u64::from(raw.nFileIndexHigh) << 32) | u64::from(raw.nFileIndexLow),
        size: (u64::from(raw.nFileSizeHigh) << 32) | u64::from(raw.nFileSizeLow),
        links: raw.nNumberOfLinks,
    })
}

/// The final path of a handle, without the extended prefix. The buffer starts at 512 units and
/// grows as Win32 reports the required size, up to a limit of 32768 units; a longer
/// path is an error.
fn final_path(file: &File) -> io::Result<String> {
    let mut size = FIRST_FINAL_PATH_UNITS;
    loop {
        let mut buffer = vec![0_u16; size];
        // SAFETY: the buffer is writable for the length passed, and the handle is open.
        let length = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle() as HANDLE,
                buffer.as_mut_ptr(),
                size as u32,
                0,
            )
        } as usize;
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        if length < size {
            let text = String::from_utf16_lossy(&buffer[..length]);
            return Ok(text
                .strip_prefix(r"\\?\UNC\")
                .map(|rest| format!(r"\\{rest}"))
                .or_else(|| text.strip_prefix(r"\\?\").map(str::to_owned))
                .unwrap_or(text));
        }
        // On failure for lack of room, Win32 returns the size needed, terminator included.
        size = length + 1;
        if size > MAX_FINAL_PATH_UNITS {
            return Err(io::Error::other("final path too long"));
        }
    }
}

/// Case-insensitive path equality (`ntpath.normcase`).
fn same_path(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

/// The SID string of the current user.
fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = null_mut();
    // SAFETY: the current process handle is a pseudo-handle; `token` receives an open token
    // that is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut size = 0_u32;
    // SAFETY: a null buffer with zero length only queries the required size.
    unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut size) };
    let mut buffer = vec![0_u64; (size as usize).div_ceil(8).max(1)];
    // SAFETY: the buffer holds `size` bytes and is 8-byte aligned for TOKEN_USER.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    };
    let result = if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: the call filled the buffer with a TOKEN_USER whose SID stays valid while the
        // buffer lives.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        sid_string(user.User.Sid.cast())
    };
    // SAFETY: `token` is the handle opened above and is closed once.
    unsafe { windows_sys::Win32::Foundation::CloseHandle(token) };
    result
}

/// The string form of a SID pointer, freed with LocalFree. A null SID is an error.
fn sid_string(sid: *mut c_void) -> io::Result<String> {
    if sid.is_null() {
        return Err(io::Error::other("missing security identifier"));
    }
    let mut text: *mut u16 = null_mut();
    // SAFETY: `sid` is non-null and points to a valid SID owned by a live buffer; `text` receives
    // a LocalAlloc'd string freed below.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut length = 0;
    // SAFETY: the converted string is NUL-terminated.
    unsafe {
        while *text.add(length) != 0 {
            length += 1;
        }
    }
    // SAFETY: `length` code units precede the terminator, and the buffer is still allocated.
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    // SAFETY: `text` came from ConvertSidToStringSidW and is freed once.
    unsafe { LocalFree(text.cast()) };
    Ok(value)
}

fn token_is_elevated() -> bool {
    let mut token: HANDLE = null_mut();
    // SAFETY: as in current_user_sid.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
    // SAFETY: the structure and size match TokenElevation.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size,
            &mut size,
        )
    };
    // SAFETY: the token opened above is closed once.
    unsafe { windows_sys::Win32::Foundation::CloseHandle(token) };
    ok != 0 && elevation.TokenIsElevated != 0
}

/// Owner-and-DACL allowlist of a directory (`check_private_directory`): the owner must be `user`,
/// or Administrators when elevated; a write-capable allow ACE that applies to the directory or its
/// new children may name only `user`, SYSTEM, Administrators or the owner rights principals.
fn check_private(file: &File, rights: u32, user: &str) -> io::Result<()> {
    let info = info_of(file)?;
    let is_directory = info.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    let mut owner: *mut c_void = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: *mut c_void = null_mut();
    // SAFETY: the handle is open with READ_CONTROL; the out pointers receive pointers into the
    // descriptor, which is freed below.
    let code = unsafe {
        GetSecurityInfo(
            file.as_raw_handle() as HANDLE,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let result = (|| -> io::Result<()> {
        let owner_sid = sid_string(owner)?;
        let owner_ok = owner_sid == user || (owner_sid == ADMINISTRATORS && token_is_elevated());
        if !owner_ok {
            return Err(io::Error::other("owner is not the current user"));
        }
        if dacl.is_null() {
            return Err(io::Error::other("null DACL grants everyone access"));
        }
        let mut size_info = ACL_SIZE_INFORMATION {
            AceCount: 0,
            AclBytesInUse: 0,
            AclBytesFree: 0,
        };
        // SAFETY: `dacl` is non-null and valid while the descriptor lives.
        if unsafe {
            GetAclInformation(
                dacl,
                (&mut size_info as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        for index in 0..size_info.AceCount {
            let mut ace: *mut c_void = null_mut();
            // SAFETY: the index is below AceCount.
            if unsafe { GetAce(dacl, index, &mut ace) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if ace.is_null() {
                return Err(io::Error::other("missing access control entry"));
            }
            // SAFETY: an ACE begins with AceType (byte 0), AceFlags (byte 1) and Mask (offset 4).
            let (ace_type, flags, mask) = unsafe {
                let base = ace.cast::<u8>();
                (
                    *base,
                    *base.add(1),
                    base.add(4).cast::<u32>().read_unaligned(),
                )
            };
            if matches!(ace_type, 4 | 5 | 11) {
                return Err(io::Error::other("unsupported allow ACE"));
            }
            if !matches!(ace_type, 0 | 9) {
                continue;
            }
            let applies_to_self = flags & INHERIT_ONLY_ACE == 0;
            let applies_to_children = is_directory && flags & CHILD_INHERIT_ACES != 0;
            if !(applies_to_self || applies_to_children) || mask & rights == 0 {
                continue;
            }
            // SAFETY: the SID starts at offset 8 of an allow ACE, inside the descriptor.
            let sid = sid_string(unsafe { ace.cast::<u8>().add(8) }.cast())?;
            let trusted = [
                SYSTEM,
                ADMINISTRATORS,
                CREATOR_OWNER,
                OWNER_RIGHTS,
                user,
                owner_sid.as_str(),
            ];
            if !trusted.contains(&sid.as_str()) {
                return Err(io::Error::other("untrusted principal has access"));
            }
        }
        Ok(())
    })();
    // SAFETY: the descriptor came from GetSecurityInfo and is freed once.
    unsafe { LocalFree(descriptor) };
    result
}

fn wide(text: &str) -> Vec<u16> {
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// A random hex suffix for the temporary name (32 hex digits).
fn temp_suffix() -> String {
    Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jobniac_core::clock::format_instant;
    use jobniac_core::documents::{DocumentMetadata, sha256_hex};
    use time::macros::datetime;

    use super::*;

    /// A fresh owner-only directory under the system temporary folder, in its final-path form. The
    /// publication refuses a parent whose text differs from its final path, and `%TEMP%` can be an
    /// 8.3 alias (`RUNNER~1`) that the final path expands, so the base is canonicalized first.
    fn fresh_dir(label: &str) -> PathBuf {
        let base = std::fs::canonicalize(std::env::temp_dir()).expect("canonical temp directory");
        let base = base.to_string_lossy();
        let base = Path::new(base.strip_prefix(r"\\?\").unwrap_or(&base)).to_path_buf();
        let path = base.join(format!(
            "jobniac-platform-{label}-{}",
            Uuid::new_v4().simple()
        ));
        create_private_directory(&path).expect("create test directory");
        path
    }

    fn artifact_of(size: usize) -> DocumentArtifact {
        let mut content = b"%PDF-1.4\n".to_vec();
        content.resize(size, b'a');
        DocumentArtifact {
            document: DocumentMetadata {
                id: Uuid::from_u128(0xBB),
                original_file_name: "cv.pdf".to_owned(),
                media_type: "application/pdf".to_owned(),
                byte_size: size as i64,
                sha256: sha256_hex(&content),
                created_at: format_instant(datetime!(2026-01-02 03:04:05 UTC)),
                updated_at: format_instant(datetime!(2026-01-02 03:04:06 UTC)),
            },
            content,
        }
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("read test directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    fn remove(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Runs one publication with `seam` into `dir` and returns the outcome and the flag.
    fn run(dir: &Path, seam: &dyn Seam, size: usize) -> (Result<String, Failure>, bool) {
        let parent = dir.to_string_lossy().into_owned();
        let mut published = false;
        let outcome = publish_with(
            &AtomicExporter::default(),
            &artifact_of(size),
            &parent,
            "selected.pdf",
            &mut published,
            seam,
        );
        (outcome, published)
    }

    struct ShortWrites;

    impl Seam for ShortWrites {
        fn write(&self, file: &mut File, piece: &[u8]) -> io::Result<usize> {
            file.write(&piece[..piece.len().min(17)])
        }
    }

    struct ZeroProgress;

    impl Seam for ZeroProgress {
        fn write(&self, _file: &mut File, _piece: &[u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    struct FailingWrite;

    impl Seam for FailingWrite {
        fn write(&self, _file: &mut File, _piece: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("write failed"))
        }
    }

    struct FailingSync;

    impl Seam for FailingSync {
        fn sync(&self, _file: &File) -> io::Result<()> {
            Err(io::Error::other("sync failed"))
        }
    }

    struct FailingAfterRename;

    impl Seam for FailingAfterRename {
        fn after_rename(&self) -> io::Result<()> {
            Err(io::Error::other("post-rename check failed"))
        }
    }

    struct ForeignUser;

    impl Seam for ForeignUser {
        fn current_user(&self) -> io::Result<String> {
            Ok("S-1-5-21-1-2-3-1001".to_owned())
        }
    }

    /// Grants Everyone write rights on the parent between the writes and the verification.
    struct PermissionChange;

    impl Seam for PermissionChange {
        fn after_write(&self, parent: &str) {
            let status = std::process::Command::new("icacls")
                .arg(parent)
                .arg("/grant")
                .arg("*S-1-1-0:(W)")
                .status();
            assert!(status.is_ok_and(|code| code.success()), "icacls failed");
        }
    }

    /// Tries to rename the parent between the writes and the verification.
    struct RenameParent {
        moved: PathBuf,
    }

    impl Seam for RenameParent {
        fn after_write(&self, parent: &str) {
            let _ = std::fs::rename(parent, &self.moved);
        }
    }

    /// Writes 17 bytes per call: every write is short, and the content must still be complete.
    #[test]
    fn partial_writes_are_completed_without_a_named_partial_file() {
        let dir = fresh_dir("partial");
        let (outcome, published) = run(&dir, &ShortWrites, 4096);
        assert!(
            matches!(outcome.as_deref(), Ok("selected.pdf")),
            "{outcome:?}"
        );
        assert!(published);
        assert_eq!(names_in(&dir), ["selected.pdf"]);
        assert_eq!(
            std::fs::read(dir.join("selected.pdf")).expect("read"),
            artifact_of(4096).content
        );
        remove(&dir);
    }

    #[test]
    fn zero_progress_refuses_and_leaves_no_name() {
        let dir = fresh_dir("zero");
        let (outcome, published) = run(&dir, &ZeroProgress, 64);
        assert!(matches!(outcome, Err(Failure::Refused)));
        assert!(!published);
        assert!(names_in(&dir).is_empty());
        remove(&dir);
    }

    #[test]
    fn a_failed_write_or_sync_refuses_and_leaves_no_name() {
        let dir = fresh_dir("failed-write");
        let (outcome, _) = run(&dir, &FailingWrite, 64);
        assert!(matches!(outcome, Err(Failure::Refused)));
        assert!(names_in(&dir).is_empty());
        remove(&dir);

        let dir = fresh_dir("failed-sync");
        let (outcome, _) = run(&dir, &FailingSync, 64);
        assert!(matches!(outcome, Err(Failure::Refused)));
        assert!(names_in(&dir).is_empty());
        remove(&dir);
    }

    /// A failure after the rename is the uncertain outcome: the complete file stays in place.
    #[test]
    fn a_failure_after_the_rename_is_uncertain_and_keeps_the_complete_file() {
        let dir = fresh_dir("uncertain");
        let (outcome, published) = run(&dir, &FailingAfterRename, 64);
        assert!(matches!(outcome, Err(Failure::Published)), "{outcome:?}");
        assert!(published);
        assert_eq!(names_in(&dir), ["selected.pdf"]);
        assert_eq!(
            std::fs::read(dir.join("selected.pdf")).expect("read"),
            artifact_of(64).content
        );
        remove(&dir);
    }

    #[test]
    fn a_foreign_owner_or_writable_parent_refuses_and_leaves_no_name() {
        let dir = fresh_dir("foreign");
        let (outcome, _) = run(&dir, &ForeignUser, 64);
        assert!(matches!(outcome, Err(Failure::Refused)));
        assert!(names_in(&dir).is_empty());
        remove(&dir);

        let dir = fresh_dir("granted");
        let (outcome, _) = run(&dir, &PermissionChange, 64);
        assert!(matches!(outcome, Err(Failure::Refused)));
        assert!(names_in(&dir).is_empty());
        remove(&dir);
    }

    /// A pinned directory cannot be renamed while it is held. If the rename does go through, the
    /// verification refuses the publication. Either way no file is published under the old name.
    #[test]
    fn a_renamed_parent_is_refused_before_publication() {
        let dir = fresh_dir("renamed");
        let moved = dir.with_file_name(format!(
            "{}-moved",
            dir.file_name().expect("name").to_string_lossy()
        ));
        let (outcome, published) = run(
            &dir,
            &RenameParent {
                moved: moved.clone(),
            },
            64,
        );
        if moved.exists() {
            assert!(matches!(outcome, Err(Failure::Refused)));
            assert!(!published);
            assert!(names_in(&moved).is_empty());
            remove(&moved);
        } else {
            assert!(outcome.is_ok(), "{outcome:?}");
        }
        remove(&dir);
    }
}
