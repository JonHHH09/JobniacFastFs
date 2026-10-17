//! Outbound destination policy: public-only addresses, host name normalization and pinned
//! one-shot resolution.
//!
//! Nothing here opens a socket or resolves a name. The infrastructure layer supplies the
//! [`AddressLookup`] adapter. Errors are sanitized: they carry a fixed [`PolicyFailure`] and never
//! an address, name or cause.

pub mod address;
pub mod hostname;
pub mod policy;
pub mod resolver;

pub use address::{is_public_address, parse_ip, parse_ipv4, parse_ipv6};
pub use hostname::normalize_hostname;
pub use policy::{
    Endpoint, Family, MAX_RECORDS, OutboundPolicyError, PolicyFailure, RecordProtocol,
    ResolverRecord, SocketKind, admit_records,
};
pub use resolver::{AddressLookup, LookupFailed, PinnedResolver};

/// The User-Agent every outbound client sends.
pub const USER_AGENT: &str = "Jobniac (+https://github.com/JonHHH09/JobniacFast)";
