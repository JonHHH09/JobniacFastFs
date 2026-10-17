//! Worker isolation: the supervisor that runs sandboxed children, and the child side that the hidden
//! `jobniac-cli __worker <kind>` mode calls.
//!
//! - [`kinds`]: the worker kinds and their compiled limit profiles;
//! - [`env`]: the environment allowlist of a child;
//! - [`protocol`]: the go byte, the marker variable and the length-prefixed frames;
//! - [`supervisor`]: the parent side (deadline, cancellation, output cap, tree kill, semaphores);
//! - [`child`]: the child side (marker check, its own limits, bounded request, operation);
//! - [`sandbox`]: the self-confinement a child applies before it handles untrusted input;
//! - [`platform`]: Job Objects on Windows and rlimits plus process groups on Linux.
//!
//! The only unsafe code is in the platform modules, each block with a `// SAFETY:` comment.

pub mod child;
pub mod env;
pub mod error;
pub mod fetch;
pub mod kinds;
pub mod pdf;
pub mod probe;
pub mod protocol;
pub mod sandbox;
pub mod supervisor;

mod platform;

pub use child::{WorkerFault, run_from_process, run_from_process_with, serve};
pub use error::{ExitClass, WorkerError, WorkerErrorKind};
pub use kinds::{WorkerHandler, WorkerKind, WorkerLimits};
pub use supervisor::WorkerSupervisor;
