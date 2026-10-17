//! Jobniac console binary: MCP, migrations, maintenance and workers.
//!
//! The mode is a required positional argument; a missing or
//! unknown mode is a usage error and exits 2 (clap). Every other failure exits 1 with a sanitized
//! message.
#![forbid(unsafe_code)]

mod arbeitnow_ports;
mod args;
mod artifact_ports;
mod console;
mod deletion_ports;
mod document_ports;
mod generation_ports;
mod http_mode;
mod job_link_ports;
mod job_ports;
mod match_ports;
mod modes;
mod profile_ports;

use std::ffi::OsStr;
use std::process::ExitCode;

use clap::Parser;
use jobniac_runtime::worker;

use crate::args::{Cli, Mode};

fn main() -> ExitCode {
    // The hidden worker mode is recognized before clap, so it stays out of the grammar and help.
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if worker::selects_worker_mode(arguments.first().map(AsRef::as_ref)) {
        let rest: Vec<&OsStr> = arguments[1..].iter().map(AsRef::as_ref).collect();
        return worker::run(&rest);
    }
    let cli = Cli::parse();
    match cli.mode {
        Mode::Mcp => modes::run_mcp(),
        Mode::Migrate => modes::run_migrate(),
        Mode::Cleanup => modes::run_cleanup(),
        Mode::SetupGeneratedRoot => modes::run_setup_generated_root(),
        Mode::Http => http_mode::run_http(),
    }
}
