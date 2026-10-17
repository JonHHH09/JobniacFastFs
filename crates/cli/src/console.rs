//! Console output for the CLI.
//!
//! The messages written here are the CLI's user-facing contract, and clap's own output is the
//! only other text on the console. The print lints are allowed in this module alone so that the
//! rest of the crate has to go through these two helpers.

/// Write one line to stdout.
#[allow(clippy::print_stdout)]
pub fn stdout_line(message: &str) {
    println!("{message}");
}

/// Write one line to stderr.
#[allow(clippy::print_stderr)]
pub fn stderr_line(message: &str) {
    eprintln!("{message}");
}
