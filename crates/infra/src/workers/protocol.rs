//! The framed protocol between the worker supervisor and a worker child.
//!
//! The parent writes one go byte and then one request frame to the child's stdin. The child
//! applies its own resource limits before it reads the go byte, so nothing it does can happen
//! before the parent has attached the child to its containment. The child writes exactly one
//! response frame to stdout and exits with status 0. A frame is a `u32` byte length in big-endian
//! order followed by that many bytes. Stdout must hold exactly one frame and nothing after it.

use std::io::{self, Read, Write};

use crate::workers::error::{WorkerError, WorkerErrorKind};

/// The byte the parent writes after the child is attached to its containment.
pub const GO_BYTE: u8 = b'G';

/// The internal environment variable that every worker child requires. The child refuses to run
/// when it is missing or does not carry the value of its own kind.
pub const MARKER_VARIABLE: &str = "JOBNIAC_INTERNAL_WORKER";

/// Bytes of the length prefix of a frame.
pub const FRAME_HEADER_BYTES: usize = 4;

/// Write one frame: the big-endian length prefix, then the body.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`] when the body does not fit the `u32` length prefix,
/// and any error of the writer.
pub fn write_frame(out: &mut dyn Write, body: &[u8]) -> io::Result<()> {
    let length = u32::try_from(body.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame body is too large"))?;
    out.write_all(&length.to_be_bytes())?;
    out.write_all(body)
}

/// Read one frame whose body may not exceed `max_body` bytes.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidData`] when the declared length exceeds `max_body`, and any
/// error of the reader (including an unexpected end of input).
pub fn read_frame(input: &mut dyn Read, max_body: usize) -> io::Result<Vec<u8>> {
    let mut header = [0_u8; FRAME_HEADER_BYTES];
    input.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    if length > max_body {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame body exceeds the limit",
        ));
    }
    let mut body = vec![0_u8; length];
    input.read_exact(&mut body)?;
    Ok(body)
}

/// Decode the complete stdout of a worker child into its response body.
///
/// The stdout must be exactly one frame: the declared length must equal the number of bytes that
/// follow the prefix. Anything shorter, longer or otherwise malformed is a protocol violation.
///
/// # Errors
///
/// Returns a [`WorkerErrorKind::ProtocolViolation`] error for any framing mismatch.
pub fn decode_response(stdout: &[u8]) -> Result<Vec<u8>, WorkerError> {
    let violation = WorkerError::new(WorkerErrorKind::ProtocolViolation);
    let Some((header, body)) = stdout.split_at_checked(FRAME_HEADER_BYTES) else {
        return Err(violation);
    };
    let mut prefix = [0_u8; FRAME_HEADER_BYTES];
    prefix.copy_from_slice(header);
    if u32::from_be_bytes(prefix) as usize != body.len() {
        return Err(violation);
    }
    Ok(body.to_vec())
}
