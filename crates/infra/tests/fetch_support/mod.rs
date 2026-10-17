//! Loopback fixtures of the fetch tests: a test certificate authority (rcgen), a TLS server on
//! 127.0.0.1 that answers one scripted reply, and the test environment that maps validated public
//! addresses onto that server. No test reaches the network.

#![allow(dead_code, clippy::expect_used)]

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use jobniac_infra::workers::fetch::FetchEnvironment;
use jobniac_infra::workers::fetch::testing::{
    FixedLookup, MappingDialer, environment, roots_config,
};
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// A test authority and one leaf certificate issued by it.
pub struct Pki {
    /// The DER of the authority, to be trusted by the client.
    pub authority: Vec<u8>,
    leaf: CertificateDer<'static>,
    key: Vec<u8>,
}

/// Issues a leaf for `names`. With `expired`, the leaf's validity ended a year ago.
pub fn pki(names: &[&str], expired: bool) -> Pki {
    let ca_key = KeyPair::generate().expect("authority key");
    let mut ca_params = CertificateParams::new(vec!["jobniac test authority".to_owned()])
        .expect("authority parameters");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = CertifiedIssuer::self_signed(ca_params, ca_key).expect("authority");

    let mut leaf_params = CertificateParams::new(
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>(),
    )
    .expect("leaf parameters");
    if expired {
        let now = time::OffsetDateTime::now_utc();
        leaf_params.not_before = now - time::Duration::days(730);
        leaf_params.not_after = now - time::Duration::days(365);
    }
    let leaf_key = KeyPair::generate().expect("leaf key");
    let leaf = leaf_params
        .signed_by(&leaf_key, &*ca)
        .expect("leaf certificate");
    Pki {
        authority: ca.der().to_vec(),
        leaf: leaf.der().clone(),
        key: leaf_key.serialize_der(),
    }
}

/// What the server does with an accepted, handshaken connection.
#[derive(Clone)]
pub enum Reply {
    /// Reads the request head, then writes these bytes and closes.
    Bytes(Vec<u8>),
    /// Reads the request head, then echoes it back as the body of a 200 response.
    EchoHead,
    /// Completes the handshake and never writes (slow peer).
    Stall,
}

/// Starts a TLS server on 127.0.0.1 that serves the first connection with `reply`. Returns its
/// address; the task is aborted when the runtime ends.
pub async fn serve(pki: &Pki, reply: Reply) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_no_client_auth()
        .with_single_cert(
            vec![pki.leaf.clone()],
            PrivatePkcs8KeyDer::from(pki.key.clone()).into(),
        )
        .expect("server certificate");
    let acceptor = TlsAcceptor::from(Arc::new(config));
    tokio::spawn(async move {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let Ok(mut tls) = acceptor.accept(tcp).await else {
            return;
        };
        match reply {
            Reply::Stall => {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            Reply::Bytes(bytes) => {
                let _ = read_head(&mut tls).await;
                let _ = tls.write_all(&bytes).await;
                let _ = tls.shutdown().await;
            }
            Reply::EchoHead => {
                let head = read_head(&mut tls).await;
                let mut response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    head.len()
                )
                .into_bytes();
                response.extend_from_slice(&head);
                let _ = tls.write_all(&response).await;
                let _ = tls.shutdown().await;
            }
        }
    });
    address
}

/// Reads the request head (up to the blank line) and returns it.
async fn read_head<S: AsyncReadExt + Unpin>(stream: &mut S) -> Vec<u8> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") && head.len() < 8192 {
        match stream.read(&mut byte).await {
            Ok(1) => head.push(byte[0]),
            _ => break,
        }
    }
    head
}

/// The test environment for a host name answered with `public` and mapped to `server`.
pub fn env_for_name(
    pki: &Pki,
    host: &str,
    public: IpAddr,
    server: SocketAddr,
) -> (FetchEnvironment, Arc<FixedLookup>, Arc<MappingDialer>) {
    let mut answers = BTreeMap::new();
    answers.insert(host.to_owned(), vec![public]);
    let lookup = Arc::new(FixedLookup {
        answers,
        calls: Default::default(),
    });
    let mut map = HashMap::new();
    map.insert(SocketAddr::new(public, 443), server);
    let dialer = Arc::new(MappingDialer {
        map,
        attempts: Default::default(),
    });
    let tls = roots_config(&pki.authority).expect("test roots");
    let env = environment(lookup.clone(), dialer.clone(), tls);
    (env, lookup, dialer)
}

/// A 200 response that declares `declared` bytes of body and writes `body` (which may be shorter,
/// for a refusal that is decided by the header alone).
pub fn declared_response(declared: usize, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n"
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

/// A 200 response with a complete HTML body.
pub fn html_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .into_bytes()
}
