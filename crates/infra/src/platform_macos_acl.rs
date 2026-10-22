//! Extended-ACL probe shared by the macOS file backends.
//!
//! Mode bits do not show an extended ACL: `chmod +a` or Finder sharing can grant other users
//! access to an object whose mode is `0600` or `0700`. A backend that treats an object as private
//! asks [`has_extended_acl`] and refuses the object when it carries any entry. The two `acl_*`
//! calls have no safe equivalent in std or `libc`, so they are declared by hand and are `unsafe`
//! below, each with a SAFETY comment.

#![allow(unsafe_code)]

use std::ffi::{c_int, c_uint, c_void};
use std::io;
use std::os::unix::io::RawFd;

/// `ACL_TYPE_EXTENDED` of `acl_get_fd_np` (`<sys/acl.h>` in the macOS SDK: the `acl_type_t`
/// enumerator `ACL_TYPE_EXTENDED = 0x00000100`).
const ACL_TYPE_EXTENDED: c_uint = 0x0000_0100;

unsafe extern "C" {
    /// `acl_t acl_get_fd_np(int fd, acl_type_t type)` of `<sys/acl.h>`; `acl_t` is an opaque
    /// pointer and `acl_type_t` an enumeration of `unsigned int` size. Returns `NULL` with
    /// `ENOENT` when the object has no ACL.
    fn acl_get_fd_np(fd: c_int, acl_type: c_uint) -> *mut c_void;
    /// `int acl_free(void *obj_p)` of `<sys/acl.h>`.
    fn acl_free(object: *mut c_void) -> c_int;
}

/// Whether the object behind the open descriptor `fd` carries an extended ACL (any entry).
///
/// The caller keeps `fd` open for the duration of the call. A filesystem without ACL support
/// cannot hold an ACL that grants access, so `ENOENT`, `ENOTSUP` and `EOPNOTSUPP` (45 and 102 on
/// Darwin, `<sys/errno.h>`) report `false`; any other failure is an error, and the caller refuses.
pub(crate) fn has_extended_acl(fd: RawFd) -> io::Result<bool> {
    // SAFETY: the caller holds `fd` open; the call returns a newly allocated ACL or NULL with
    // errno set, and reads no caller memory.
    let acl = unsafe { acl_get_fd_np(fd, ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ENOENT | libc::ENOTSUP | libc::EOPNOTSUPP) => Ok(false),
            _ => Err(error),
        };
    }
    // SAFETY: `acl` was returned by acl_get_fd_np and is released exactly once.
    unsafe { acl_free(acl) };
    Ok(true)
}
