//! TLS client configuration of the fetch children.
//!
//! The trust store is the system store (`rustls-native-certs`). The provider is `ring`. ALPN is
//! `http/1.1` only, because the children speak HTTP/1.1. A system store without any usable certificate fails closed.

use std::io;
use std::sync::Arc;

use rustls::{ClientConfig, RootCertStore};

/// The ALPN identifier of HTTP/1.1.
const ALPN_HTTP_1_1: &[u8] = b"http/1.1";

/// The root certificates of the operating system.
///
/// # Errors
///
/// Returns an error when no usable root certificate can be loaded. The message is fixed.
pub fn system_roots() -> io::Result<RootCertStore> {
    let loaded = rustls_native_certs::load_native_certs();
    let mut roots = RootCertStore::empty();
    let (added, _ignored) = roots.add_parsable_certificates(loaded.certs);
    if added == 0 {
        return Err(io::Error::other(
            "no system trust store certificate is usable",
        ));
    }
    Ok(roots)
}

/// The client configuration that verifies servers against `roots` with the `ring` provider.
///
/// # Errors
///
/// Returns an error when the provider cannot provide the default protocol versions.
pub fn client_config(roots: RootCertStore) -> io::Result<Arc<ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| io::Error::other("TLS protocol versions are unavailable"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![ALPN_HTTP_1_1.to_vec()];
    Ok(Arc::new(config))
}
