//! Environment configuration and composition root for Jobniac.
//!
//! [`process_environment`] takes the one immutable snapshot of the process environment. The
//! loaders in [`arbeitnow`] and [`services`] are pure functions of a snapshot, so tests never
//! touch the real environment. Opt-in features stay off unless their flag is exactly `1`.
#![forbid(unsafe_code)]

pub mod app_settings;
pub mod arbeitnow;
pub mod artifacts;
pub mod blocking;
pub mod database;
pub mod deletion;
pub mod diagnostics;
pub mod document_ports;
pub mod generation;
pub mod job_links;
pub mod mcp_server;
pub mod pdfium_location;
pub mod services;
pub mod smoke;
pub mod worker;
pub mod worker_fetch;
pub mod worker_pdf;

use jobniac_core::env::Environment;

/// The process environment, read once at startup. Names are case-insensitive on Windows.
pub fn process_environment() -> Environment {
    Environment::from_os_pairs(std::env::vars_os(), cfg!(windows))
}
