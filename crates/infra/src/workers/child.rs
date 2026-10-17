//! The child side of a worker: the entry point of the hidden `__worker <kind>` mode.
//!
//! Order of work in a child, which the supervisor depends on:
//!
//! 1. the internal marker must carry the value of this kind, otherwise nothing is read or run;
//! 2. the child applies its own limits (Linux `setrlimit`; nothing on Windows);
//! 3. the child reads the go byte, which the parent writes only after attaching the child to its
//!    containment;
//! 4. the child reads one bounded request frame and runs the kind's operation, which writes its
//!    response to stdout;
//! 5. the child exits. Any failure exits with status 1 and writes no diagnostics.
//!
//! Self-confinement (Linux Landlock and seccomp) is applied by each operation, because only
//! the operation knows when it has loaded what it needs: the PDF operations after PDFium is loaded
//! and before the document is parsed, the fetch operations after the system resolver and trust
//! store are read and before the first socket. A failed confinement fails the child.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Write};

use crate::workers::kinds::{WorkerHandler, WorkerKind};
use crate::workers::platform;
use crate::workers::protocol::{GO_BYTE, MARKER_VARIABLE, read_frame, write_frame};
use crate::workers::sandbox;

/// A failure of a worker child. It carries no detail by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerFault;

impl fmt::Display for WorkerFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("worker failed")
    }
}

impl std::error::Error for WorkerFault {}

impl From<io::Error> for WorkerFault {
    fn from(_: io::Error) -> Self {
        Self
    }
}

/// Whether the internal marker value identifies the given kind.
#[must_use]
pub fn marker_matches(kind: WorkerKind, marker: Option<&OsString>) -> bool {
    marker.is_some_and(|value| value.to_str() == Some(kind.marker_value().as_str()))
}

/// Writes one response frame to the output of an operation.
///
/// Operations use this for their normal response; each caller enforces its own size first.
///
/// # Errors
///
/// Returns [`WorkerFault`] when the frame cannot be written.
pub fn write_response(output: &mut dyn Write, body: &[u8]) -> Result<(), WorkerFault> {
    write_frame(output, body)?;
    Ok(())
}

/// Runs the worker operation of `kind` in this process, over the process's stdin and stdout.
///
/// Called by the hidden `__worker <kind>` mode. The marker check comes first, so a process started
/// without the marker never reads its input.
///
/// # Errors
///
/// Returns [`WorkerFault`] when the marker is missing, the kind has no operation in this build,
/// a limit cannot be applied, or the operation fails.
pub fn run_from_process(kind: WorkerKind) -> Result<(), WorkerFault> {
    let handler = kind.handler().ok_or(WorkerFault)?;
    run_from_process_with(kind, handler)
}

/// Runs `handler` as the operation of `kind` in this process, with the same marker check, limits
/// and stream handling as [`run_from_process`].
///
/// Used by the CLI for kinds whose operation needs a dependency the infrastructure crate does not
/// link (the PDFium operations).
///
/// # Errors
///
/// Returns [`WorkerFault`] when the marker is missing or a limit cannot be applied, or when the
/// operation fails.
pub fn run_from_process_with(kind: WorkerKind, handler: WorkerHandler) -> Result<(), WorkerFault> {
    let marker = std::env::var_os(MARKER_VARIABLE);
    if !marker_matches(kind, marker.as_ref()) {
        return Err(WorkerFault);
    }
    platform::child_apply_limits(&kind.limits())?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    serve(kind, handler, &mut input, &mut output)
}

/// Reads the go byte and one bounded request, runs `handler`, and flushes its output.
///
/// The request is refused before its body is read when its declared length exceeds the limit of
/// the kind. The handler validates the body again.
///
/// # Errors
///
/// Returns [`WorkerFault`] for a missing go byte, an oversized or truncated request, or a failed
/// handler.
pub fn serve(
    kind: WorkerKind,
    handler: WorkerHandler,
    input: &mut dyn Read,
    output: &mut dyn Write,
) -> Result<(), WorkerFault> {
    let mut go = [0_u8; 1];
    input.read_exact(&mut go)?;
    if go[0] != GO_BYTE {
        return Err(WorkerFault);
    }
    let request = read_frame(input, kind.request_limit())?;
    handler(&request, output)?;
    // Tripwire: an operation of a kind with a profile must have confined itself by now. A child that
    // fails here exits 1, and the supervisor discards whatever it wrote.
    if kind.sandbox_profile().is_some() {
        sandbox::verify_confined()?;
    }
    output.flush()?;
    Ok(())
}
