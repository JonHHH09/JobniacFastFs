//! macOS self-confinement of worker children with a Seatbelt profile.
//!
//! The child calls [`apply`] once, after it has loaded what it needs and before it handles
//! untrusted input (the same moment as the Linux Landlock and seccomp layers). The profile is a
//! custom SBPL string handed to `sandbox_init`, the call that Chromium's macOS sandbox uses. Every
//! profile starts with `(deny default)`, so the file system, the network, process creation, signals
//! to other processes and Mach lookups are all refused unless a rule below allows them. The
//! built-in `kSBXProfile*` names are not used: recent SDKs terminate a process that selects one.
//!
//! The confinement cannot be undone. Descriptors that the child already holds (stdin, stdout, the
//! loaded PDFium library) keep working, as with Landlock.

#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_int};
use std::io;

use crate::workers::sandbox::{SandboxLevel, SandboxProfile};

unsafe extern "C" {
    /// `sandbox_init(3)`: confines the calling process with the SBPL string `profile` (flags 0).
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
    /// Frees the message that a failed `sandbox_init` returned.
    fn sandbox_free_error(errorbuf: *mut c_char);
    /// Asks the sandbox about a process. With no operation and the filter type 0 it answers `1`
    /// for a process that is confined, and `0` for one that is not.
    fn sandbox_check(pid: libc::pid_t, operation: *const c_char, filter_type: c_int, ...) -> c_int;
}

/// The start of every profile: nothing is allowed, and a denial is not reported to the system log
/// (a confined child that probes a forbidden path would otherwise flood it).
const BASE: &str = "(version 1)\n(deny default (with no-report))\n";

/// PDF text extraction: the base alone. The child reads its request from stdin, writes to stdout,
/// allocates memory and starts threads, none of which needs a rule once the library is loaded.
const PDF_EXTRACT: &str = "";

/// PDF page rendering: read-only access to the system font directories that PDFium searches for
/// fonts a document does not embed. Never the home directory (`~/Library/Fonts`).
const PDF_RENDER: &str = "(allow file-read* (subpath \"/System/Library/Fonts\") \
(subpath \"/Library/Fonts\"))\n";

/// Outbound fetch: TCP to any address (the guard in the child admits public addresses only before
/// it connects) and UDP to port 53 for the resolver, whose servers were read from the system
/// configuration before the sandbox. The resolver binds its UDP socket to an ephemeral local port
/// and waits for the answer, so UDP may bind and receive; no TCP socket may bind (no listener) and
/// no other UDP port may be reached. The system trust store is read before the sandbox too, so no
/// file or Mach rule is needed (a lookup through `mDNSResponder` is not used: the resolver speaks
/// DNS itself).
const FETCH: &str = "(allow network-outbound (remote tcp) (remote udp \"*:53\"))\n\
(allow network-bind (local udp \"*:*\"))\n\
(allow network-inbound (local udp \"*:*\"))\n";

/// The SBPL text of `profile`.
fn profile_text(profile: SandboxProfile) -> String {
    let rules = match profile {
        SandboxProfile::PdfExtract => PDF_EXTRACT,
        SandboxProfile::PdfRender => PDF_RENDER,
        SandboxProfile::Fetch => FETCH,
    };
    format!("{BASE}{rules}")
}

/// Confines the calling process with the Seatbelt profile of `profile`.
///
/// # Errors
///
/// Returns an error when the sandbox refuses the profile, or when the process is not confined
/// afterwards. The caller must then not run unconfined.
pub fn apply(profile: SandboxProfile) -> io::Result<SandboxLevel> {
    let text = CString::new(profile_text(profile)).map_err(io::Error::other)?;
    let mut error: *mut c_char = std::ptr::null_mut();
    // SAFETY: `text` is a valid NUL-terminated string that outlives the call, `error` is a valid
    // location for the error message pointer, and flags 0 selects an SBPL string profile.
    let status = unsafe { sandbox_init(text.as_ptr(), 0, &raw mut error) };
    if status != 0 {
        let message = if error.is_null() {
            String::from("sandbox_init failed")
        } else {
            // SAFETY: on failure `sandbox_init` stores a NUL-terminated message in `error`.
            let message = unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned();
            // SAFETY: `error` came from `sandbox_init` and is freed exactly once.
            unsafe { sandbox_free_error(error) };
            message
        };
        return Err(io::Error::other(message));
    }
    if !confined() {
        return Err(io::Error::other("the process is not confined"));
    }
    Ok(SandboxLevel::Seatbelt)
}

/// Whether the calling process is confined by a Seatbelt profile.
///
/// It cannot tell a profile that this process applied itself from one that it inherited (a
/// sandboxed parent), or one that the system applied. The tripwire therefore proves that some
/// confinement is in force, and [`apply`] is what puts the profile of the kind in place.
#[must_use]
pub fn confined() -> bool {
    // SAFETY: `sandbox_check` reads only its integer and null arguments; with a null operation and
    // the filter type 0 (`SANDBOX_FILTER_NONE`) no variadic argument is read.
    unsafe { sandbox_check(libc::getpid(), std::ptr::null(), 0) == 1 }
}
