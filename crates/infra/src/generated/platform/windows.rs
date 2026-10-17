//! Windows primitives of the owned generated-asset namespace.
//!
//! The root is a chain of directory handles, each opened with `FILE_FLAG_OPEN_REPARSE_POINT` and
//! share mode read|write (no `FILE_SHARE_DELETE`), so no ancestor can be renamed while an operation
//! runs. Files are opened by exact normalized path with no-follow flags and checked for a single
//! link and a private DACL. Publication goes through a briefly visible `.jobniac-tmp-` file created
//! with a private DACL and moved into place with a no-replace rename. Win32 calls that std does not
//! expose are `unsafe` and confined to this module, each with a SAFETY comment: `CreateFileW` and
//! `CreateDirectoryW` with a protected SDDL descriptor, `SetFileInformationByHandle` (no-replace
//! rename and delete on a held handle), `GetSecurityInfo` with the ACL allowlist, the token and SID
//! queries, and `GetVolumeInformationByHandleW`.
//!
//! Sharing violations map to `io::ErrorKind::WouldBlock` (retryable); access denied maps to
//! `PermissionDenied` (unproven ownership). Identity and link counts come from the std handle
//! metadata, so no `unsafe` is needed for them.

#![allow(unsafe_code)]

use super::{Access, Publish, PublishError};
use crate::files::no_follow::final_path_matches;
use crate::files::windows_path::normalize_drive_path;
use jobniac_core::generated_root::{TEMP_PREFIX, windows_capability_bytes};
use std::ffi::{OsString, c_void};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::mem::MaybeUninit;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::Path;
use std::ptr::null_mut;
use uuid::Uuid;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
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
    GetVolumeInformationByHandleW, SetFileInformationByHandle,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const DELETE: u32 = 0x0001_0000;
const READ_CONTROL: u32 = 0x0002_0000;
const FILE_LIST_DIRECTORY: u32 = 0x0000_0001;
const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const FILE_SHARE_DELETE: u32 = 0x0000_0004;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;
const TOKEN_QUERY: u32 = 0x0008;
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;

/// Rights that let a principal alter a protected object.
const WRITE_RIGHTS: u32 = 0x0000_0002
    | 0x0000_0004
    | 0x0000_0010
    | 0x0000_0040
    | 0x0000_0100
    | 0x0001_0000
    | 0x0004_0000
    | 0x0008_0000
    | 0x1000_0000
    | 0x4000_0000;
/// Rights that let a principal read a protected object's data or extended attributes.
const READ_RIGHTS: u32 = 0x0000_0001 | 0x0000_0008 | 0x8000_0000 | 0x1000_0000;
/// ACE types whose compound or object forms are refused outright (fail closed).
const UNSUPPORTED_ALLOW_ACES: [u8; 3] = [4, 5, 11];
const ALLOW_ACE: u8 = 0;
const ALLOW_CALLBACK_ACE: u8 = 9;
const INHERIT_ONLY_ACE: u8 = 0x08;
const CHILD_INHERIT_ACES: u8 = 0x03;
const ADMINISTRATORS_SID: &str = "S-1-5-32-544";
const SYSTEM_SID: &str = "S-1-5-18";
const CREATOR_OWNER_SID: &str = "S-1-3-0";
const OWNER_RIGHTS_SID: &str = "S-1-3-4";

/// Identity of an object as compared by the protocol: volume, index, size and last write time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    volume: u64,
    index: u64,
    size: u64,
    written: u64,
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

/// Maps a Win32 sharing or lock violation to the retryable `WouldBlock` kind.
fn retryable(error: io::Error) -> io::Error {
    match error.raw_os_error() {
        Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION) => io::Error::new(
            io::ErrorKind::WouldBlock,
            "generated namespace object is busy",
        ),
        _ => error,
    }
}

/// The fields of `GetFileInformationByHandle` that the protocol compares.
#[derive(Debug, Clone, Copy)]
struct HandleInfo {
    attributes: u32,
    links: u32,
    volume: u64,
    index: u64,
    size: u64,
    written: u64,
}

impl HandleInfo {
    fn is_directory(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }

    fn is_reparse_point(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
}

/// Samples the handle itself (never the path), so a swapped name cannot answer for it.
fn handle_info(file: &File) -> io::Result<HandleInfo> {
    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: the handle is live for the call and `info` is a writable structure of the exact type;
    // the API fills it completely on success, which is checked before it is read.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, info.as_mut_ptr()) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so every field of the structure has been written.
    let info = unsafe { info.assume_init() };
    Ok(HandleInfo {
        attributes: info.dwFileAttributes,
        links: info.nNumberOfLinks,
        volume: u64::from(info.dwVolumeSerialNumber),
        index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        size: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
        written: (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
            | u64::from(info.ftLastWriteTime.dwLowDateTime),
    })
}

/// Identity of an opened object for before-and-after comparisons.
pub(crate) fn stamp_of(file: &File) -> io::Result<Stamp> {
    let info = handle_info(file)?;
    Ok(Stamp {
        volume: info.volume,
        index: info.index,
        size: info.size,
        written: info.written,
    })
}

/// NUL-terminated UTF-16 form of a path or SDDL string.
fn wide(text: &str) -> Vec<u16> {
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// A security descriptor allocated by `ConvertStringSecurityDescriptorToSecurityDescriptorW`.
struct Descriptor(*mut c_void);

impl Descriptor {
    fn from_sddl(sddl: &str) -> io::Result<Self> {
        let text = wide(sddl);
        let mut descriptor: *mut c_void = null_mut();
        // SAFETY: `text` is NUL-terminated and outlives the call; SDDL revision 1 is the only
        // revision. On success the out pointer receives a LocalAlloc'd descriptor owned by `Self`.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                1,
                &raw mut descriptor,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from the conversion above and is freed exactly once.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// Protected DACL for a new private file: the owner-only user, plus SYSTEM.
fn private_file_sddl(user: &str) -> String {
    format!("D:P(A;;FA;;;SY)(A;;FA;;;{user})")
}

/// Protected DACL for a new private directory: user, SYSTEM and Administrators, inherited by
/// children.
fn private_directory_sddl(user: &str) -> String {
    format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{user})")
}

/// Creates a new private file with an exclusive share mode, DELETE access and no-follow flags.
fn create_new_private(path: &str) -> io::Result<File> {
    let descriptor = Descriptor::from_sddl(&private_file_sddl(&current_user_sid()?))?;
    let attributes = descriptor.attributes();
    let text = wide(path);
    // SAFETY: `text` is NUL-terminated; `attributes` points at a live SECURITY_ATTRIBUTES whose
    // descriptor is owned by `descriptor` and outlives the call. CREATE_NEW never opens an
    // existing object. A non-invalid return is a new handle that this function now owns.
    let handle = unsafe {
        CreateFileW(
            text.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | DELETE | READ_CONTROL,
            0,
            &raw const attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `handle` is a valid, owned file handle returned by CreateFileW above.
    Ok(unsafe { File::from_raw_handle(handle as _) })
}

/// Moves a held file to `target` without replacing an existing name
/// (`FILE_RENAME_INFO` with `ReplaceIfExists` false).
fn rename_no_replace(file: &File, target: &str) -> io::Result<()> {
    let units: Vec<u16> = std::ffi::OsStr::new(target).encode_wide().collect();
    let name_bytes = units.len() * 2;
    let header = offset_of!(FILE_RENAME_INFO, FileName);
    let total = header + name_bytes + size_of::<u16>();
    // Zeroed u64 words give 8-byte alignment and clear ReplaceIfExists and RootDirectory.
    let mut buffer = vec![0u64; total.div_ceil(size_of::<u64>())];
    let base = buffer.as_mut_ptr().cast::<u8>();
    // SAFETY: the buffer is zeroed, 8-byte aligned and at least `total` bytes long. The info
    // header lies at the start and the UTF-16 name is written at `header`, inside the buffer. The
    // handle is a live file object opened with DELETE access.
    let ok = unsafe {
        let info = base.cast::<FILE_RENAME_INFO>();
        (*info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(units.as_ptr().cast::<u8>(), base.add(header), name_bytes);
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileRenameInfo,
            base.cast::<c_void>(),
            total as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Marks a held file for deletion; the file disappears when the last handle closes.
fn delete_on_handle(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: `info` is a live structure of exactly the size passed. The handle is a live file
    // object opened with DELETE access (every caller opens it that way).
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileDispositionInfo,
            (&raw const info).cast::<c_void>(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Owning wrapper of a token handle.
struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by OpenProcessToken and is closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

fn process_token() -> io::Result<Token> {
    let mut token: HANDLE = null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no closing. OpenProcessToken
    // writes one handle into `token` on success; the wrapper takes ownership of it.
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Token(token))
}

/// Copies one token information class into a buffer aligned for its structure.
fn token_information(token: &Token, class: i32) -> io::Result<Vec<u64>> {
    let mut needed = 0u32;
    // SAFETY: a null buffer with length zero asks only for the required size and writes `needed`.
    unsafe { GetTokenInformation(token.0, class, null_mut(), 0, &raw mut needed) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = vec![0u64; (needed as usize).div_ceil(size_of::<u64>())];
    // SAFETY: the buffer holds at least `needed` bytes and is 8-byte aligned.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            class,
            buffer.as_mut_ptr().cast::<c_void>(),
            needed,
            &raw mut needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(buffer)
}

/// The SID string of a SID pointer, freed with LocalFree.
fn sid_to_string(sid: *mut c_void) -> io::Result<String> {
    let mut text: *mut u16 = null_mut();
    // SAFETY: `sid` points at a valid SID for the duration of the call; on success `text` receives
    // a LocalAlloc'd, NUL-terminated string that is read and freed below.
    let ok = unsafe { ConvertSidToStringSidW(sid, &raw mut text) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `text` is a NUL-terminated UTF-16 string returned by the successful call above.
    let value = unsafe {
        let mut length = 0usize;
        while *text.add(length) != 0 {
            length += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(text, length))
    };
    // SAFETY: `text` came from ConvertSidToStringSidW and is freed exactly once.
    unsafe { LocalFree(text.cast::<c_void>()) };
    Ok(value)
}

/// The SID string of the process user.
fn current_user_sid() -> io::Result<String> {
    let token = process_token()?;
    let buffer = token_information(&token, TokenUser)?;
    // SAFETY: the buffer was filled with a TOKEN_USER structure by GetTokenInformation and is
    // aligned for it; the SID it points at lives inside the buffer for as long as it is read.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    sid_to_string(sid)
}

/// Whether the process token is an elevated administrator token.
fn is_elevated() -> io::Result<bool> {
    let token = process_token()?;
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned = 0u32;
    // SAFETY: `elevation` is a live TOKEN_ELEVATION and its exact size is passed.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&raw mut elevation).cast::<c_void>(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &raw mut returned,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(elevation.TokenIsElevated != 0)
}

/// Checks an opened object: the owner is the process user (or Administrators when elevated), the
/// DACL is present, and every allow ACE that grants `rights` names only trusted principals.
fn check_private(file: &File, rights: u32) -> io::Result<()> {
    let is_directory = file.metadata()?.is_dir();
    let mut owner: *mut c_void = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: *mut c_void = null_mut();
    // SAFETY: the handle is a live object opened with READ_CONTROL. The out pointers receive
    // pointers into `descriptor`, which is freed by `Descriptor` below once they are no longer used.
    let code = unsafe {
        GetSecurityInfo(
            file.as_raw_handle() as _,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &raw mut owner,
            null_mut(),
            &raw mut dacl,
            null_mut(),
            &raw mut descriptor,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let _descriptor = Descriptor(descriptor);
    let owner_text = sid_to_string(owner)?;
    let user = current_user_sid()?;
    if owner_text != user && !(owner_text == ADMINISTRATORS_SID && is_elevated()?) {
        return Err(permission_denied());
    }
    if dacl.is_null() {
        return Err(permission_denied());
    }
    let mut sizes = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: `sizes` is a live ACL_SIZE_INFORMATION and its exact size is passed.
    let ok = unsafe {
        GetAclInformation(
            dacl,
            (&raw mut sizes).cast::<c_void>(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let trusted = [
        SYSTEM_SID,
        ADMINISTRATORS_SID,
        CREATOR_OWNER_SID,
        OWNER_RIGHTS_SID,
        user.as_str(),
        owner_text.as_str(),
    ];
    for index in 0..sizes.AceCount {
        let mut ace: *mut c_void = null_mut();
        // SAFETY: `dacl` is the ACL returned by GetSecurityInfo and `index` is below AceCount.
        let ok = unsafe { GetAce(dacl, index, &raw mut ace) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let header = ace.cast::<u8>();
        // SAFETY: each ACE starts with an ACE_HEADER (type, flags, size) followed by its mask and
        // SID, all inside the descriptor memory that `_descriptor` keeps alive.
        let (ace_type, flags, mask) = unsafe {
            (
                *header,
                *header.add(1),
                header.add(4).cast::<u32>().read_unaligned(),
            )
        };
        if UNSUPPORTED_ALLOW_ACES.contains(&ace_type) {
            return Err(permission_denied());
        }
        if ace_type != ALLOW_ACE && ace_type != ALLOW_CALLBACK_ACE {
            continue;
        }
        let applies_to_self = flags & INHERIT_ONLY_ACE == 0;
        let applies_to_children = is_directory && flags & CHILD_INHERIT_ACES != 0;
        if !(applies_to_self || applies_to_children) || mask & rights == 0 {
            continue;
        }
        // SAFETY: an allow ACE's SID starts at byte offset 8 (after the header and the mask).
        let sid = unsafe { header.add(8).cast::<c_void>() };
        let principal = sid_to_string(sid)?;
        if !trusted.contains(&principal.as_str()) {
            return Err(permission_denied());
        }
    }
    Ok(())
}

/// Ownership and privacy of an opened control or asset: a single-link regular file whose DACL
/// admits only the user, SYSTEM, Administrators and the owner.
pub(crate) fn verify_private_file(file: &File) -> io::Result<()> {
    let info = handle_info(file)?;
    if info.is_directory() || info.is_reparse_point() || info.links != 1 {
        return Err(permission_denied());
    }
    check_private(file, WRITE_RIGHTS | READ_RIGHTS)
}

/// Creates one directory with a protected DACL (user, SYSTEM and Administrators).
pub(crate) fn create_private_directory(path: &str) -> io::Result<()> {
    let normalized = normalize_drive_path(path).map_err(|_| invalid_name())?;
    let descriptor = Descriptor::from_sddl(&private_directory_sddl(&current_user_sid()?))?;
    let attributes = descriptor.attributes();
    let text = wide(&normalized);
    // SAFETY: `text` is NUL-terminated and `attributes` points at a live SECURITY_ATTRIBUTES whose
    // descriptor outlives the call.
    let ok = unsafe { CreateDirectoryW(text.as_ptr(), &raw const attributes) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Whether the volume holding `file` is read-only.
fn volume_is_read_only(file: &File) -> io::Result<bool> {
    let mut flags = 0u32;
    // SAFETY: the handle is live; every optional out pointer is null with a zero length, which the
    // API accepts, and `flags` receives the file system flags.
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle() as _,
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            &raw mut flags,
            null_mut(),
            0,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(flags & FILE_READ_ONLY_VOLUME != 0)
}

/// Whether the process can publish: always true on Windows (create-new and no-replace rename).
pub(crate) fn supported() -> bool {
    true
}

/// Flush of an existing object that was just compared. Only Linux flushes it: a
/// read-only handle cannot be flushed on Windows, so this is a no-op here.
pub(crate) fn sync_existing(_file: &File) -> io::Result<()> {
    Ok(())
}

/// The pinned generated root: every ancestor directory held open.
#[derive(Debug)]
pub(crate) struct Root {
    path: String,
    ancestors: Vec<Pin>,
    leaf: Pin,
}

/// A held directory handle and the identity it was opened with.
#[derive(Debug)]
struct Pin {
    file: File,
    volume: u64,
    index: u64,
}

fn pin(file: File) -> io::Result<Pin> {
    let info = handle_info(&file)?;
    if !info.is_directory() || info.is_reparse_point() {
        return Err(permission_denied());
    }
    Ok(Pin {
        file,
        volume: info.volume,
        index: info.index,
    })
}

/// The drive root and each directory below it, as normalized paths.
fn prefixes(normalized: &str) -> Vec<String> {
    let mut result = vec![normalized[..3].to_owned()];
    let body = normalized[3..].trim_end_matches('\\');
    let mut current = normalized[..3].to_owned();
    for component in body.split('\\').filter(|part| !part.is_empty()) {
        if !current.ends_with('\\') {
            current.push('\\');
        }
        current.push_str(component);
        result.push(current.clone());
    }
    result
}

impl Root {
    /// Pins every directory from the drive root to `path`, each without following reparse points,
    /// and checks that the leaf is private (`_win_open_root`).
    pub(crate) fn open(path: &str) -> io::Result<Self> {
        let normalized = normalize_drive_path(path).map_err(|_| invalid_name())?;
        let mut pins = Vec::new();
        for prefix in prefixes(&normalized) {
            let file = OpenOptions::new()
                .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&prefix)
                .map_err(retryable)?;
            pins.push(pin(file)?);
        }
        let leaf = pins.pop().ok_or_else(invalid_name)?;
        check_private(&leaf.file, WRITE_RIGHTS | READ_RIGHTS)?;
        Ok(Self {
            path: normalized,
            ancestors: pins,
            leaf,
        })
    }

    /// The entries of the pinned directory. Every held handle lacks `FILE_SHARE_DELETE`, so neither
    /// the leaf nor an ancestor can be renamed while the root is pinned: the normalized path still
    /// names the directory that was verified, and the listing is taken after that check.
    pub(crate) fn list_names(&self) -> io::Result<Vec<OsString>> {
        self.verify()?;
        let listing = std::fs::read_dir(&self.path)?;
        listing
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect()
    }

    /// Re-checks every held directory and the final path of the leaf; any drift fails closed.
    pub(crate) fn verify(&self) -> io::Result<()> {
        for held in self.ancestors.iter().chain(std::iter::once(&self.leaf)) {
            let info = handle_info(&held.file)?;
            if (info.volume, info.index) != (held.volume, held.index) || info.is_reparse_point() {
                return Err(permission_denied());
            }
        }
        if !final_path_matches(Path::new(&self.path))? {
            return Err(permission_denied());
        }
        Ok(())
    }

    /// Whether the volume is writable (the control files cannot be written on read-only media).
    pub(crate) fn qualifies_for_controls(&self) -> io::Result<bool> {
        Ok(!volume_is_read_only(&self.leaf.file)?)
    }

    /// Windows has no proc descriptor link to confirm.
    pub(crate) fn require_capability_link(&self) -> io::Result<()> {
        Ok(())
    }

    /// Normalized path of a child of the pinned root.
    fn child_path(&self, name: &str) -> io::Result<String> {
        if name.is_empty() || name.contains(['\\', '/', '\0']) || name == "." || name == ".." {
            return Err(invalid_name());
        }
        normalize_drive_path(&format!("{}\\{name}", self.path.trim_end_matches('\\')))
            .map_err(|_| invalid_name())
    }

    /// Opens an existing child with the requested access, refusing links, non-regular files,
    /// shared or foreign ACLs and multiply linked files.
    pub(crate) fn open_child(&self, name: &str, access: Access) -> io::Result<File> {
        let path = self.child_path(name)?;
        let (rights, share) = match access {
            Access::Read => (GENERIC_READ | READ_CONTROL, FILE_SHARE_READ),
            Access::ReadDelete => (GENERIC_READ | READ_CONTROL | DELETE, FILE_SHARE_READ),
            Access::Exclusive => (GENERIC_READ | GENERIC_WRITE | READ_CONTROL, 0),
        };
        let file = OpenOptions::new()
            .access_mode(rights)
            .share_mode(share)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)
            .map_err(retryable)?;
        verify_private_file(&file)?;
        Ok(file)
    }

    /// Creates a new private child with complete content, flushed before it is returned. A
    /// partial file is deleted so that a retry can create the control again.
    pub(crate) fn create_child_with(&self, name: &str, data: &[u8]) -> io::Result<()> {
        let path = self.child_path(name)?;
        let mut file = create_new_private(&path)?;
        let written = file.write_all(data).and_then(|()| file.sync_all());
        if written.is_err() {
            let _ = delete_on_handle(&file);
        }
        written
    }

    /// Deletes a child left behind by an interrupted initialization. Errors are ignored.
    pub(crate) fn discard_child(&self, name: &str) -> io::Result<()> {
        let path = self.child_path(name)?;
        if let Ok(file) = OpenOptions::new()
            .access_mode(DELETE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)
        {
            let _ = delete_on_handle(&file);
        }
        Ok(())
    }

    /// Publication needs no probe on Windows: create-new and no-replace rename always exist.
    pub(crate) fn probe_publication(&self) -> Result<(), PublishError> {
        Ok(())
    }

    /// Publishes `content` under `name`: a private temporary file is written, flushed, verified
    /// and moved into place with a no-replace rename. The temporary name is removed unless it was
    /// renamed.
    pub(crate) fn publish_anonymous(
        &self,
        name: &str,
        content: &[u8],
    ) -> Result<Publish, PublishError> {
        let target = self.child_path(name).map_err(PublishError::Io)?;
        let temporary = format!("{TEMP_PREFIX}{}", Uuid::new_v4().simple());
        let temporary_path = self.child_path(&temporary).map_err(PublishError::Io)?;
        let mut file = create_new_private(&temporary_path).map_err(PublishError::Io)?;
        let mut renamed = false;
        let outcome = self.finish_publication(&mut file, &target, content, &mut renamed);
        // Anything but a verified creation is removed through the handle this call created, which
        // cannot reach a foreign object: the file was opened with no sharing, so nothing else
        // could have been given the name in between, and the handle always names our object. That
        // covers a mismatch and an I/O error after the rename, which would otherwise leave an
        // orphan under the final name that no rollback knows about.
        if !renamed || !matches!(outcome, Ok(Publish::Created)) {
            let _ = delete_on_handle(&file);
        }
        outcome
    }

    fn finish_publication(
        &self,
        file: &mut File,
        target: &str,
        content: &[u8],
        renamed: &mut bool,
    ) -> Result<Publish, PublishError> {
        file.write_all(content).map_err(PublishError::Io)?;
        file.sync_all().map_err(PublishError::Io)?;
        self.verify().map_err(PublishError::Io)?;
        match rename_no_replace(file, target) {
            Ok(()) => *renamed = true,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Ok(Publish::NameExists);
            }
            Err(error) => return Err(PublishError::Io(error)),
        }
        let info = handle_info(file).map_err(PublishError::Io)?;
        if info.links != 1
            || info.size != content.len() as u64
            || !final_path_matches(Path::new(target)).map_err(PublishError::Io)?
        {
            return Err(PublishError::Mismatch);
        }
        Ok(Publish::Created)
    }

    /// Deletes a child through the handle that was verified for removal.
    pub(crate) fn unlink(&self, name: &str, file: &File) -> io::Result<()> {
        self.child_path(name)?;
        delete_on_handle(file)
    }

    /// Whether no object of that name exists, opened without following a reparse point.
    pub(crate) fn absent(&self, name: &str) -> io::Result<bool> {
        let path = self.child_path(name)?;
        match OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)
        {
            Ok(_) => Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(error),
        }
    }

    /// Windows has no directory flush; each published file is flushed before its rename.
    pub(crate) fn sync(&self) -> io::Result<()> {
        Ok(())
    }

    /// Whether `name` currently resolves to the object `file` holds, with the same final path.
    pub(crate) fn name_refers_to(&self, name: &str, file: &File) -> io::Result<bool> {
        let path = self.child_path(name)?;
        let current = match OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)
        {
            Ok(current) => current,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        if stamp_of(&current)? != stamp_of(file)? {
            return Ok(false);
        }
        final_path_matches(Path::new(&path))
    }

    /// The namespace lock is exclusive by its share mode, so taking it succeeds once opened.
    pub(crate) fn lock_exclusive(&self, file: &File) -> io::Result<()> {
        let _ = file;
        Ok(())
    }

    /// Volume and index of the pinned leaf, for the capability content.
    pub(crate) fn identity(&self) -> (u64, u64) {
        (self.leaf.volume, self.leaf.index)
    }
}

/// The exact capability content of a root published on this platform.
pub(crate) fn capability_bytes(root: &Root, root_id: Uuid) -> io::Result<Vec<u8>> {
    let (volume, index) = root.identity();
    Ok(windows_capability_bytes(root_id, volume, index))
}
