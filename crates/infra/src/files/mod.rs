//! Document import file access: import root reads, and the Windows ACL and reparse rules.
//!
//! - [`import_root`]: the import root reader, its refusal reasons and the bounded, identity-checked
//!   read of one regular file.
//! - [`no_follow`]: the platform primitives (pinned directory chains and no-follow opens) for Linux
//!   and Windows. Its one FFI call, `GetFileInformationByHandle`, is a function-level `unsafe`
//!   block with a `SAFETY:` comment; `tests/unsafe_confined.rs` lists it as a known violation to
//!   move into a platform module.
//! - `export`: the PDF exporter. Orchestration only; its FFI lives in `export::platform_linux`
//!   (rustix safe API, no `unsafe`) and `export::platform_windows`.
//! - [`windows_path`]: the lexical rules for drive-absolute Windows paths (device names, alternate
//!   data streams, reserved characters). Pure string logic, compiled on every platform.

pub mod export;
pub mod import_root;
pub mod no_follow;
pub mod windows_path;
