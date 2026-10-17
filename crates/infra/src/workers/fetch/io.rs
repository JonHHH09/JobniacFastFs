//! The guarded byte stream under `hyper`.
//!
//! Every read first asks the core [`RawHeaderGuard`] how much it may take, so the raw header block
//! is bounded before the HTTP parser sees it. The bytes that were read are then admitted by the
//! guard; a refusal turns into an I/O error that the caller reports as a generic failure. After the
//! final header block the guard only bounds the read size (64 KiB). Each read also has an idle
//! timeout, which is re-armed by every successful read.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use http_body_util::BodyExt;
use hyper::body::Incoming;
use jobniac_core::job_links::html::{NETWORK_READ_BYTES, RawHeaderGuard};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep};

/// A connection with the raw header guard and an idle read timeout. `Debug` is hand-written: the
/// guard holds the raw response bytes, which are fetched text and must not reach a log.
pub struct GuardedIo<S> {
    inner: S,
    guard: RawHeaderGuard,
    idle: Pin<Box<Sleep>>,
    idle_for: Duration,
}

impl<S> std::fmt::Debug for GuardedIo<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GuardedIo")
            .field("idle_for", &self.idle_for)
            .finish_non_exhaustive()
    }
}

impl<S> GuardedIo<S> {
    /// Wraps `inner`; the idle timer starts now. `body_limit` is the largest declared body length
    /// the raw header guard admits (2 MiB for the job page, 4 MiB for the board).
    pub fn new(inner: S, idle_for: Duration, body_limit: u64) -> Self {
        Self {
            inner,
            guard: RawHeaderGuard::with_body_limit(body_limit),
            idle: Box::pin(tokio::time::sleep(idle_for)),
            idle_for,
        }
    }

    fn rearm(&mut self) {
        self.idle.as_mut().reset(Instant::now() + self.idle_for);
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for GuardedIo<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        let limit = this
            .guard
            .read_limit(buf.remaining())
            .min(NETWORK_READ_BYTES)
            .min(buf.remaining());
        if limit == 0 {
            return Poll::Ready(Err(refused()));
        }
        let start = buf.filled().len();
        let mut sub = ReadBuf::new(buf.initialize_unfilled_to(limit));
        match Pin::new(&mut this.inner).poll_read(cx, &mut sub) {
            Poll::Ready(Ok(())) => {
                let read = sub.filled().len();
                buf.advance(read);
                if read == 0 {
                    return Poll::Ready(Ok(()));
                }
                if this.guard.check(&buf.filled()[start..]).is_err() {
                    return Poll::Ready(Err(refused()));
                }
                this.rearm();
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => {
                if this.idle.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "idle timeout",
                    )));
                }
                Poll::Pending
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for GuardedIo<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// The error of a refused raw header block. It carries no bytes and no detail.
fn refused() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "response header block refused")
}

/// Why a response body was not read to the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyError {
    /// The data bytes alone are over the budget.
    Oversize,
    /// The framing failed: a frame error, the deadline, or trailers that do not fit the budget.
    Framing,
}

/// Reads every data frame and every trailer frame of `body` before `deadline`. Data and trailer
/// bytes share one budget of `limit` bytes; the trailer bytes are counted the way the header block
/// is (name, value and four separator bytes per field), so a trailer section is bounded like the
/// body is instead of only by the transport buffer. The trailers are then discarded: they are
/// never exposed to a caller.
///
/// # Errors
///
/// Returns [`BodyError::Oversize`] when the data is over `limit`, and [`BodyError::Framing`] for
/// a frame error, a trailer section that does not fit the remaining budget, or the deadline.
pub async fn read_body(
    mut body: Incoming,
    limit: usize,
    deadline: Instant,
) -> Result<Vec<u8>, BodyError> {
    let mut bytes: Vec<u8> = Vec::new();
    let mut trailer_bytes: usize = 0;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| BodyError::Framing)?;
        match frame.into_data() {
            Ok(data) => {
                if data.len() > limit - bytes.len() - trailer_bytes {
                    return Err(BodyError::Oversize);
                }
                bytes.extend_from_slice(&data);
            }
            Err(frame) => {
                if let Ok(trailers) = frame.into_trailers() {
                    trailer_bytes += trailers
                        .iter()
                        .map(|(name, value)| name.as_str().len() + value.len() + 4)
                        .sum::<usize>();
                    if trailer_bytes > limit - bytes.len() {
                        return Err(BodyError::Framing);
                    }
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(BodyError::Framing);
        }
    }
    Ok(bytes)
}
