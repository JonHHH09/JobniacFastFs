//! Test-only seams of the fetch workers. Compiled only under `cfg(test)` or the `test-hooks`
//! feature, and never reachable from [`FetchEnvironment::production`].
//!
//! The seams replace the three external collaborators and nothing else: the resolver answer (still
//! admitted by the real core rules), the TCP dial (a validated public address is mapped to a
//! loopback server; an unmapped address is refused, so no test can reach the network), and the
//! trust roots (a test certificate authority). Every other step is the production code.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jobniac_core::outbound::{AddressLookup, LookupFailed, ResolverRecord};
use rustls::{ClientConfig, RootCertStore};
use tokio::net::TcpStream;

use super::FetchEnvironment;
use super::connect::Dialer;
use super::resolve::records_for;

/// A resolver answer table. Every call is recorded, so tests can assert one lookup per hop.
#[derive(Debug, Default)]
pub struct FixedLookup {
    /// The addresses per normalized host name.
    pub answers: BTreeMap<String, Vec<IpAddr>>,
    /// The `(host, port)` of every call.
    pub calls: Mutex<Vec<(String, i64)>>,
}

#[async_trait]
impl AddressLookup for FixedLookup {
    async fn lookup(&self, host: &str, port: i64) -> Result<Vec<ResolverRecord>, LookupFailed> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push((host.to_owned(), port));
        }
        let addresses = self.answers.get(host).ok_or(LookupFailed)?;
        let port = u16::try_from(port).map_err(|_| LookupFailed)?;
        Ok(records_for(addresses.iter().copied(), port))
    }
}

/// A dialer that maps validated public addresses to loopback servers. Every attempted address is
/// recorded; an address without a mapping is refused.
#[derive(Debug, Default)]
pub struct MappingDialer {
    /// The validated address to the loopback server that stands in for it.
    pub map: HashMap<SocketAddr, SocketAddr>,
    /// Every address the connector tried, in order.
    pub attempts: Mutex<Vec<SocketAddr>>,
}

#[async_trait]
impl Dialer for MappingDialer {
    async fn dial(&self, address: SocketAddr) -> io::Result<TcpStream> {
        if let Ok(mut attempts) = self.attempts.lock() {
            attempts.push(address);
        }
        let target = self
            .map
            .get(&address)
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::ConnectionRefused, "unmapped"))?;
        TcpStream::connect(target).await
    }
}

/// A TLS client configuration that trusts exactly the given DER certificate authority.
///
/// # Errors
///
/// Returns an error when the certificate is not accepted as a trust anchor.
pub fn roots_config(authority_der: &[u8]) -> io::Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(
            authority_der.to_vec(),
        ))
        .map_err(|_| io::Error::other("test authority is not accepted"))?;
    super::tls::client_config(roots)
}

/// The test environment of one fetch.
#[must_use]
pub fn environment(
    lookup: Arc<dyn AddressLookup>,
    dialer: Arc<dyn Dialer>,
    tls: Arc<ClientConfig>,
) -> FetchEnvironment {
    FetchEnvironment {
        lookup,
        dialer,
        tls,
    }
}
