//! Sanitized failure categories of the worker supervisor.
//!
//! No category carries child output, paths, parser text or environment values. The exit status
//! of a child is kept only as a class (a status code or a signal number), so callers can map a
//! failure to their own fixed message.

use std::fmt;

/// How a worker child ended, without any of its output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitClass {
    /// The child exited with this status code.
    Code(i32),
    /// The child was terminated by this signal (Linux only).
    Signal(i32),
    /// The status carries neither a code nor a signal.
    Unknown,
}

/// The category of a failed worker operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerErrorKind {
    /// The request does not fit the limit of its worker kind. Nothing was started.
    InvalidRequest,
    /// The containment or the limits could not be prepared, so nothing ran (fail closed).
    SetupFailed,
    /// The child process could not be started.
    SpawnFailed,
    /// The wall-clock limit expired and the child tree was killed.
    TimedOut,
    /// The caller cancelled the operation and the child tree was killed.
    Cancelled,
    /// The child wrote more than its output limit and the child tree was killed.
    OutputLimitExceeded,
    /// The child exited unsuccessfully, or was killed by a limit or a signal.
    ChildFailed(ExitClass),
    /// The child's stdout was not exactly one well-formed response frame.
    ProtocolViolation,
}

/// A failed worker operation. Its message is fixed and never includes child data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerError {
    kind: WorkerErrorKind,
}

impl WorkerError {
    /// A failure of the given category.
    #[must_use]
    pub const fn new(kind: WorkerErrorKind) -> Self {
        Self { kind }
    }

    /// The category of the failure.
    #[must_use]
    pub const fn kind(&self) -> WorkerErrorKind {
        self.kind
    }
}

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            WorkerErrorKind::InvalidRequest => "worker request is not accepted",
            WorkerErrorKind::SetupFailed => "worker containment could not be prepared",
            WorkerErrorKind::SpawnFailed => "worker could not be started",
            WorkerErrorKind::TimedOut => "worker timed out",
            WorkerErrorKind::Cancelled => "worker was cancelled",
            WorkerErrorKind::OutputLimitExceeded => "worker output limit was exceeded",
            WorkerErrorKind::ChildFailed(_) => "worker failed",
            WorkerErrorKind::ProtocolViolation => "worker response is malformed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for WorkerError {}
