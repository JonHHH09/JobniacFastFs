//! The sandboxed outbound fetch: the guarded job-page GET and the fixed-host
//! Arbeitnow GET, both run inside a worker child.
//!
//! The child side is the `fetch-link` and `fetch-arbeitnow` operations ([`link`], [`board`]). Each
//! one runs on `hyper` 1.x over an own connector ([`connect`]):
//!
//! - DNS is answered once per hop through the core pinned resolver and admission ([`resolve`]); the
//!   whole answer is refused when any address is not public;
//! - the socket connects only to the admitted addresses (at most [`MAX_CONNECT_ATTEMPTS`]), while
//!   TLS keeps the original host name for SNI and certificate verification;
//! - the raw header block passes through the core [`RawHeaderGuard`] before `hyper` parses it;
//! - no redirects, no proxy, no decompression, a total deadline, a per-read idle timeout and a
//!   byte-counted body loop.
//!
//! The request carries the URL (or the page and visa fields) and nothing else. The response is a
//! binary frame (see [`wire`]). Test seams (the resolver, the dialer and the trust roots) exist only
//! under `cfg(test)` or the `test-hooks` feature; the production handlers always use
//! [`FetchEnvironment::production`].

pub mod board;
pub mod connect;
pub mod io;
pub mod link;
pub mod resolve;
pub mod target;
pub mod tls;
pub mod wire;

#[cfg(any(test, feature = "test-hooks"))]
pub mod testing;

use std::sync::Arc;
use std::time::Duration;

use jobniac_core::outbound::AddressLookup;

pub use connect::Dialer;

/// Connect, TLS handshake and write timeout of one attempt.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Idle timeout between two reads of the response.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(10);

/// Addresses tried for one hop, in admitted order.
pub const MAX_CONNECT_ATTEMPTS: usize = 4;

/// The collaborators of one fetch: the resolver port, the TCP dialer and the TLS client
/// configuration. Production builds all three from the system ([`FetchEnvironment::production`],
/// the only public constructor); the test seams in [`testing`] replace them. The fields are crate
/// private, so no caller outside this crate can substitute a resolver, dialer or trust root.
#[derive(Clone)]
pub struct FetchEnvironment {
    // Debug is implemented by hand below: the trait objects do not implement it.
    /// The resolver answer port; its records go through the core admission rules.
    pub(crate) lookup: Arc<dyn AddressLookup>,
    /// Opens the TCP connection to one validated socket address.
    pub(crate) dialer: Arc<dyn Dialer>,
    /// The TLS client configuration (ALPN `http/1.1`, system roots).
    pub(crate) tls: Arc<rustls::ClientConfig>,
}

impl std::fmt::Debug for FetchEnvironment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FetchEnvironment")
            .finish_non_exhaustive()
    }
}

impl FetchEnvironment {
    /// The production environment: the system resolver, a plain TCP dialer, and the system trust
    /// store.
    ///
    /// # Errors
    ///
    /// Returns an error when the system resolver configuration cannot be read or when the system
    /// trust store holds no usable certificate (fail closed).
    pub fn production() -> Result<Self, std::io::Error> {
        Ok(Self {
            lookup: Arc::new(resolve::SystemLookup::new()?),
            dialer: Arc::new(connect::TcpDialer),
            tls: tls::client_config(tls::system_roots()?)?,
        })
    }
}
