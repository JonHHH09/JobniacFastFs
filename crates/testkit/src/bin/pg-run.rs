//! `pg-run`: run one command against a disposable PostgreSQL 18 cluster.
//!
//! See [`jobniac_testkit::runner::run_command_cli`] for the behaviour and the exit codes.
#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    jobniac_testkit::runner::run_command_cli()
}
