//! Command-line grammar: the required positional mode and the `--version` flag.
//!
//! Usage errors, including a missing mode, are reported by clap and exit 2. The desktop application is not a mode of this binary; the
//! `jobniac` GUI binary serves it.

use clap::{ArgAction, Parser, ValueEnum};

/// The modes of the console binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    /// Serve the diagnostic MCP server over STDIO.
    Mcp,
    /// Serve the loopback HTTP MCP listener (JOBNIAC_HTTP_PORT and JOBNIAC_HTTP_BEARER_TOKEN required).
    Http,
    /// Apply pending schema migrations.
    Migrate,
    /// Run one bounded generated-root cleanup pass; prints metadata JSON.
    Cleanup,
    /// Initialize and register the owned generated root; idempotent.
    SetupGeneratedRoot,
}

/// Jobniac local-first desktop and MCP workspace.
#[derive(Debug, Parser)]
#[command(
    name = "jobniac-cli",
    display_name = "Jobniac",
    version = env!("CARGO_PKG_VERSION"),
    disable_version_flag = true,
    about = "Jobniac local-first desktop and MCP workspace"
)]
pub struct Cli {
    /// Print the version and exit.
    #[arg(long, action = ArgAction::Version)]
    version: (),

    /// Mode to run: mcp, http, migrate, cleanup or setup-generated-root.
    #[arg(value_enum)]
    pub mode: Mode,
}
