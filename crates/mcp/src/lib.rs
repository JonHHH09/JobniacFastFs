//! MCP tool catalog, tool framework and transports for Jobniac.
//!
//! The server is built on the official `rmcp` SDK and implements `ServerHandler` by hand. Tool
//! metadata lives as JSON data in [`catalog`] (all 40 tools). Each tool module
//! implements [`registry::ToolHandler`] and is registered under its catalog name; the server lists
//! only the tools whose handlers are registered, in catalog order. Arguments are
//! decoded with [`args`], and results take the shapes of [`result`] (text layouts in
//! [`wire_json`]). [`serve_stdio`] writes JSON-RPC only to stdout; diagnostics belong on stderr,
//! which the CLI configures.
#![forbid(unsafe_code)]

pub mod args;
pub mod catalog;
pub mod http;
pub mod registry;
pub mod result;
mod server;
pub mod stdin_admission;
pub mod tools;
pub mod wire_json;

pub use server::{JobniacMcpServer, McpServeError, McpServices, serve_stdio, serve_with};
