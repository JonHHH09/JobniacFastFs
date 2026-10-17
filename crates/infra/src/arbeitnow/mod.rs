//! Infrastructure of the Arbeitnow provider: the parent-side admission of the worker's raw board
//! answer ([`board`]) and the PostgreSQL adapter of the trusted candidate import ([`repository`]).
//!
//! The byte transport itself runs in the `fetch-arbeitnow` worker child
//! ([`crate::workers::fetch::board`]); nothing here reads a clock or opens a socket.

pub mod board;
pub mod repository;
