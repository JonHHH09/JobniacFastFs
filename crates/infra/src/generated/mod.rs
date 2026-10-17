//! Owned generated-artifact root and its asset files.
//!
//! - [`asset_files`]: the namespace protocol for `<asset-id>.pdf` files: initialization,
//!   qualification, fenced removal and create-only materialization, under the namespace lock.
//! - [`setup`]: the filesystem side of `setup-generated-root`, which only proposes identities.
//! - [`root`]: private creation of a missing default directory and plain-directory checks.
//! - `platform`: the Linux, Windows and fail-closed primitives behind the protocol. Pure path and
//!   layout rules live in `jobniac_core::generated_root`.

pub mod asset_files;
pub mod root;
pub mod setup;

mod platform;
