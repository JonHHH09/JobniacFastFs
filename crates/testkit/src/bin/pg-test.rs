//! `pg-test`: run `cargo test` against one disposable PostgreSQL 18 cluster.
//!
//! See [`jobniac_testkit::runner`] for the behaviour and the exit codes.
#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    jobniac_testkit::runner::run_cli()
}
