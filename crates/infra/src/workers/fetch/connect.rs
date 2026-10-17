//! The own connector of the fetch children.
//!
//! One hop works as follows:
//!
//! 1. an IP-literal host is checked for public use (the URL check already did, the connector checks
//!    again, so no path reaches a socket unchecked);
//! 2. a host name is resolved once through the core [`PinnedResolver`]; the whole answer is refused
//!    when any record is not public;
//! 3. the dialer connects to the admitted addresses in order, at most [`MAX_CONNECT_ATTEMPTS`];
//! 4. for `https` the TLS handshake uses the original host name (SNI and certificate name) even
//!    though the socket is numeric;
//! 5. the stream is wrapped in the guarded I/O, and `hyper` speaks HTTP/1.1 over it.
//!
//! The connector never follows redirects, never reads proxy settings and never decompresses.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::task::{Context, Poll};

use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::Empty;
use hyper::body::Incoming;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use jobniac_core::outbound::{
    PinnedResolver, PolicyFailure, is_public_address, normalize_hostname, parse_ip,
};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use super::io::GuardedIo;
use super::target::Target;
use super::{CONNECT_TIMEOUT, FetchEnvironment, IDLE_TIMEOUT, MAX_CONNECT_ATTEMPTS};

/// Why a hop could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectError {
    /// A resolved or literal address is not public (the address policy refused it).
    Unsafe,
    /// Resolution, dialing or the TLS handshake failed. No cause is kept.
    Unavailable,
}

/// Opens one TCP connection to a validated socket address. Production uses [`TcpDialer`]; tests
/// may map addresses to a loopback server.
#[async_trait]
pub trait Dialer: Send + Sync {
    /// Connects to `address`.
    async fn dial(&self, address: SocketAddr) -> io::Result<TcpStream>;
}

/// The production dialer: a plain TCP connection to the given address.
#[derive(Debug, Clone, Copy)]
pub struct TcpDialer;

#[async_trait]
impl Dialer for TcpDialer {
    async fn dial(&self, address: SocketAddr) -> io::Result<TcpStream> {
        TcpStream::connect(address).await
    }
}

/// A plain or TLS connection, both with the same byte interface.
#[derive(Debug)]
pub enum Connection {
    /// `http`: no TLS.
    Plain(TcpStream),
    /// `https`: TLS over TCP.
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for Connection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Opens the hop for `target` and returns the guarded connection. `body_limit` is the largest
/// declared body length the header guard admits for this request.
///
/// # Errors
///
/// Returns [`ConnectError::Unsafe`] for a non-public address and [`ConnectError::Unavailable`] for
/// every other failure.
pub async fn open(
    env: &FetchEnvironment,
    target: &Target,
    body_limit: u64,
) -> Result<GuardedIo<Connection>, ConnectError> {
    let addresses = admitted_addresses(env, target).await?;
    let tcp = dial_first(env, &addresses, target.port).await?;
    let connection = if target.https {
        let name = match target.literal {
            Some(ip) => ServerName::IpAddress(ip.into()),
            None => {
                ServerName::try_from(target.host.clone()).map_err(|_| ConnectError::Unavailable)?
            }
        };
        let handshake = TlsConnector::from(env.tls.clone()).connect(name, tcp);
        match tokio::time::timeout(CONNECT_TIMEOUT, handshake).await {
            Ok(Ok(tls)) => Connection::Tls(Box::new(tls)),
            _ => return Err(ConnectError::Unavailable),
        }
    } else {
        Connection::Plain(tcp)
    };
    Ok(GuardedIo::new(connection, IDLE_TIMEOUT, body_limit))
}

/// The public addresses of the target, in admitted order. A literal is checked, a name is resolved
/// once and the whole answer is admitted by the core rules.
async fn admitted_addresses(
    env: &FetchEnvironment,
    target: &Target,
) -> Result<Vec<IpAddr>, ConnectError> {
    if let Some(literal) = target.literal {
        return if is_public_address(literal) {
            Ok(vec![literal])
        } else {
            Err(ConnectError::Unsafe)
        };
    }
    if normalize_hostname(&target.host).ok().as_deref() != Some(target.host.as_str()) {
        return Err(ConnectError::Unavailable);
    }
    let mut pinned = PinnedResolver::new();
    let texts = pinned
        .resolve(env.lookup.as_ref(), &target.host, i64::from(target.port))
        .await
        .map_err(|error| match error.failure() {
            PolicyFailure::Address => ConnectError::Unsafe,
            PolicyFailure::Hostname | PolicyFailure::Resolution => ConnectError::Unavailable,
        })?;
    texts
        .iter()
        .map(|text| parse_ip(text).ok_or(ConnectError::Unavailable))
        .collect()
}

/// Connects to the first address that accepts, trying at most [`MAX_CONNECT_ATTEMPTS`].
async fn dial_first(
    env: &FetchEnvironment,
    addresses: &[IpAddr],
    port: u16,
) -> Result<TcpStream, ConnectError> {
    for address in addresses.iter().take(MAX_CONNECT_ATTEMPTS) {
        let attempt = env.dialer.dial(SocketAddr::new(*address, port));
        if let Ok(Ok(stream)) = tokio::time::timeout(CONNECT_TIMEOUT, attempt).await {
            return Ok(stream);
        }
    }
    Err(ConnectError::Unavailable)
}

/// Sends one request over an opened hop and returns the response head. The body is still
/// unread; the connection task keeps running until the body is dropped or consumed.
///
/// # Errors
///
/// Returns an I/O error when the HTTP exchange fails. The message carries no detail.
pub async fn send(
    stream: GuardedIo<Connection>,
    request: Request<Empty<Bytes>>,
) -> io::Result<Response<Incoming>> {
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(io::Error::other)?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    sender.send_request(request).await.map_err(io::Error::other)
}
