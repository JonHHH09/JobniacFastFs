//! The system resolver adapter of the core [`AddressLookup`] port.
//!
//! One `lookup_ip` call answers one hop. The answer is converted to the raw TCP records that
//! `getaddrinfo` would return (`SOCK_STREAM`, `IPPROTO_TCP`, `(flow, scope) = (0, 0)` for IPv6),
//! so the core [`PinnedResolver`] and [`admit_records`] decide the whole answer. Nothing here
//! admits or refuses an address.

use std::io;

use async_trait::async_trait;
use hickory_resolver::TokioResolver;
use jobniac_core::outbound::{
    AddressLookup, Endpoint, Family, LookupFailed, RecordProtocol, ResolverRecord, SocketKind,
};

/// The system resolver, configured from the operating system (resolv.conf or the Windows network
/// configuration).
#[derive(Debug)]
pub struct SystemLookup {
    resolver: TokioResolver,
}

impl SystemLookup {
    /// Builds the resolver from the system configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the system configuration cannot be read. The message is fixed.
    pub fn new() -> io::Result<Self> {
        let builder = TokioResolver::builder_tokio()
            .map_err(|_| io::Error::other("system resolver configuration is unavailable"))?;
        let resolver = builder
            .build()
            .map_err(|_| io::Error::other("system resolver configuration is unavailable"))?;
        Ok(Self { resolver })
    }
}

#[async_trait]
impl AddressLookup for SystemLookup {
    async fn lookup(&self, host: &str, port: i64) -> Result<Vec<ResolverRecord>, LookupFailed> {
        let answer = self
            .resolver
            .lookup_ip(host)
            .await
            .map_err(|_| LookupFailed)?;
        let port = u16::try_from(port).map_err(|_| LookupFailed)?;
        Ok(records_for(answer.iter(), port))
    }
}

/// The raw TCP records of a set of addresses, in answer order.
pub fn records_for(
    addresses: impl Iterator<Item = std::net::IpAddr>,
    port: u16,
) -> Vec<ResolverRecord> {
    addresses
        .map(|address| {
            let (family, ipv6_extra) = match address {
                std::net::IpAddr::V4(_) => (Family::Inet, None),
                std::net::IpAddr::V6(_) => (Family::Inet6, Some((0, 0))),
            };
            ResolverRecord {
                family,
                kind: SocketKind::Stream,
                protocol: RecordProtocol::Tcp,
                endpoint: Endpoint {
                    host: address.to_string(),
                    port: i64::from(port),
                    ipv6_extra,
                },
            }
        })
        .collect()
}
