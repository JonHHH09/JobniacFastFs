//! Framing and the child state machine of the worker protocol, over in-memory streams.

use std::error::Error;
use std::io::{self, Cursor, Write};

use jobniac_infra::workers::child::{WorkerFault, serve, write_response};
use jobniac_infra::workers::protocol::{
    FRAME_HEADER_BYTES, GO_BYTE, decode_response, read_frame, write_frame,
};
use jobniac_infra::workers::{WorkerErrorKind, WorkerKind};

type TestResult = Result<(), Box<dyn Error>>;

/// Echoes its request as one response frame.
fn echo(request: &[u8], output: &mut dyn Write) -> Result<(), WorkerFault> {
    write_response(output, request)
}

/// Fails after writing a partial response, which the parent must never use.
fn fail_after_partial(_request: &[u8], output: &mut dyn Write) -> Result<(), WorkerFault> {
    output.write_all(b"partial")?;
    Err(WorkerFault)
}

/// Writes a frame with the given body, as a request for the child.
fn request_bytes(body: &[u8]) -> Result<Vec<u8>, io::Error> {
    let mut bytes = vec![GO_BYTE];
    write_frame(&mut bytes, body)?;
    Ok(bytes)
}

#[test]
fn frame_prefix_is_big_endian_length() -> TestResult {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, b"abc")?;
    assert_eq!(bytes, [0, 0, 0, 3, b'a', b'b', b'c']);
    Ok(())
}

#[test]
fn frame_round_trips_and_respects_its_limit() -> TestResult {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, b"hello")?;
    let body = read_frame(&mut Cursor::new(bytes.clone()), 5)?;
    assert_eq!(body, b"hello");

    let refused = read_frame(&mut Cursor::new(bytes), 4);
    assert_eq!(
        refused.err().map(|error| error.kind()),
        Some(io::ErrorKind::InvalidData)
    );
    Ok(())
}

#[test]
fn truncated_frame_is_an_unexpected_end_of_input() {
    let truncated = read_frame(&mut Cursor::new(vec![0, 0, 0, 9, b'a']), 64);
    assert_eq!(
        truncated.err().map(|error| error.kind()),
        Some(io::ErrorKind::UnexpectedEof)
    );
}

#[test]
fn response_must_be_exactly_one_frame() -> TestResult {
    assert_eq!(decode_response(&[0, 0, 0, 2, b'o', b'k'])?, b"ok");
    assert_eq!(decode_response(&[0, 0, 0, 0])?, b"");

    let cases: [&[u8]; 4] = [
        &[],
        &[0, 0, 0],
        &[0, 0, 0, 2, b'o', b'k', b'!'],
        &[0, 0, 0, 3, b'o', b'k'],
    ];
    for stdout in cases {
        let kind = decode_response(stdout).err().map(|error| error.kind());
        assert_eq!(
            kind,
            Some(WorkerErrorKind::ProtocolViolation),
            "stdout of {} bytes must be refused",
            stdout.len()
        );
    }
    Ok(())
}

#[test]
fn child_answers_one_request_with_one_response_frame() -> TestResult {
    let mut output = Vec::new();
    let request = request_bytes(br#"{"op":"echo"}"#)?;
    serve(
        WorkerKind::Probe,
        echo,
        &mut Cursor::new(request),
        &mut output,
    )
    .map_err(|_| "serve failed")?;
    assert_eq!(decode_response(&output)?, br#"{"op":"echo"}"#);
    Ok(())
}

#[test]
fn child_refuses_input_without_the_go_byte() -> TestResult {
    let mut bytes = vec![b'X'];
    write_frame(&mut bytes, b"{}")?;
    let mut output = Vec::new();
    let result = serve(
        WorkerKind::Probe,
        echo,
        &mut Cursor::new(bytes),
        &mut output,
    );
    assert!(result.is_err());
    assert!(output.is_empty());
    Ok(())
}

#[test]
fn child_refuses_an_oversized_request_before_reading_its_body() -> TestResult {
    let declared = u32::try_from(WorkerKind::Probe.request_limit() + 1)?;
    let mut bytes = vec![GO_BYTE];
    bytes.extend_from_slice(&declared.to_be_bytes());
    let mut output = Vec::new();
    let result = serve(
        WorkerKind::Probe,
        echo,
        &mut Cursor::new(bytes),
        &mut output,
    );
    assert!(result.is_err());
    assert!(output.is_empty());
    Ok(())
}

#[test]
fn child_failure_leaves_partial_output_for_the_parent_to_discard() -> TestResult {
    let mut output = Vec::new();
    let request = request_bytes(b"{}")?;
    let result = serve(
        WorkerKind::Probe,
        fail_after_partial,
        &mut Cursor::new(request),
        &mut output,
    );
    assert!(result.is_err());
    // The partial bytes are on the wire, but they are not a frame, so decoding refuses them.
    let kind = decode_response(&output).err().map(|error| error.kind());
    assert_eq!(kind, Some(WorkerErrorKind::ProtocolViolation));
    Ok(())
}

#[test]
fn prefix_size_is_four_bytes() {
    assert_eq!(FRAME_HEADER_BYTES, 4);
}
