//! The request checks in front of the MCP service, in wire order.
//!
//! The first failing check wins and its status code is part of the contract. Checks run on the parsed request parts, in the
//! `HeaderMap` iteration order. That order is not the wire order: all values of a repeated name
//! are yielded together. So a repeated critical header is refused (400) even where wire order would
//! reach an oversize field first (431). This is the one named difference of the golden vectors
//! (see `tests/http_guard_golden.rs`, `EXPECTED_DIFFERENCES`).

use std::net::{Ipv4Addr, SocketAddr};

use http::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HOST, ORIGIN, TRANSFER_ENCODING};
use http::request::Parts;
use http::{HeaderName, HeaderValue, Method, StatusCode};

use super::config::{LoopbackHttpConfig, sha256};

/// Largest request body the listener reads: 4 MiB.
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
/// Largest header block, counted as `len(name) + len(value) + 4` per field: 16 KiB.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;

/// Headers that must not repeat: a duplicate gives 400.
const CRITICAL_HEADERS: [&str; 8] = [
    "host",
    "origin",
    "authorization",
    "content-length",
    "content-type",
    "accept",
    "transfer-encoding",
    "last-event-id",
];
/// Headers that prove a reverse proxy sits in front of the listener: refused with 400.
const PROXY_HEADERS: [&str; 5] = [
    "forwarded",
    "x-real-ip",
    "via",
    "x-original-url",
    "x-rewrite-url",
];
/// The one path served.
const MCP_PATH: &str = "/mcp";
/// The `Bearer ` scheme prefix, matched case-insensitively.
const BEARER_PREFIX: &[u8] = b"Bearer ";

/// The per-listener values the checks compare against.
#[derive(Debug)]
pub struct Guard {
    port: u16,
    token_digest: [u8; 32],
}

impl Guard {
    /// Checks for a listener on `port` that accepts the token whose digest is `token_digest`.
    pub const fn new(port: u16, token_digest: [u8; 32]) -> Self {
        Self { port, token_digest }
    }

    /// Run the checks of steps 1 to 10 and return the first refusal status.
    ///
    /// Admission (step 11), the body (steps 12 to 13) and the service (step 14) are the
    /// listener's job.
    pub fn check(
        &self,
        parts: &Parts,
        peer: SocketAddr,
        local: SocketAddr,
    ) -> Result<(), StatusCode> {
        // Step 1: the peer is IPv4 loopback and the listener's own address is 127.0.0.1:<PORT>.
        let loopback = Ipv4Addr::LOCALHOST;
        let peer_is_loopback = matches!(peer, SocketAddr::V4(address) if *address.ip() == loopback);
        if !peer_is_loopback || local != SocketAddr::from((loopback, self.port)) {
            return Err(StatusCode::FORBIDDEN);
        }
        // Step 2: exactly /mcp, with no query string.
        let query_empty = parts.uri.query().is_none_or(str::is_empty);
        if parts.uri.path() != MCP_PATH || !query_empty {
            return Err(StatusCode::NOT_FOUND);
        }
        // Step 3: the methods the SDK serves; checked before authentication.
        if !matches!(parts.method, Method::GET | Method::POST | Method::DELETE) {
            return Err(StatusCode::METHOD_NOT_ALLOWED);
        }
        // Step 4: header bytes, names, values, proxies and duplicates.
        let mut size = 0_usize;
        for (name, value) in &parts.headers {
            size = size.saturating_add(name.as_str().len() + value.as_bytes().len() + 4);
            if size > MAX_HEADER_BYTES {
                return Err(StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);
            }
            check_field(name, value)?;
            if parts.headers.get_all(name).iter().nth(1).is_some()
                && (CRITICAL_HEADERS.contains(&name.as_str()) || name.as_str().starts_with("mcp-"))
            {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
        // Step 5: Host is exactly 127.0.0.1:<PORT>.
        let host = format!("127.0.0.1:{}", self.port);
        if parts.headers.get(HOST).map(HeaderValue::as_bytes) != Some(host.as_bytes()) {
            return Err(StatusCode::MISDIRECTED_REQUEST);
        }
        // Step 6: Origin, when present, is exactly http://127.0.0.1:<PORT>.
        if let Some(origin) = parts.headers.get(ORIGIN) {
            let expected = format!("http://{host}");
            if origin.as_bytes() != expected.as_bytes() {
                return Err(StatusCode::FORBIDDEN);
            }
        }
        // Step 7: Bearer authentication, digests compared in constant time.
        let authorization = parts
            .headers
            .get(AUTHORIZATION)
            .map_or(&[][..], HeaderValue::as_bytes);
        let bearer = authorization
            .get(..BEARER_PREFIX.len())
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case(BEARER_PREFIX));
        let candidate = authorization
            .get(BEARER_PREFIX.len()..)
            .filter(|_| bearer)
            .unwrap_or_default();
        let matches = constant_time_eq(&sha256(candidate), &self.token_digest);
        if !bearer || !matches {
            return Err(StatusCode::UNAUTHORIZED);
        }
        // Step 8: Transfer-Encoding only as chunked on a POST without Content-Length.
        let content_length = parts.headers.get(CONTENT_LENGTH);
        if let Some(encoding) = parts.headers.get(TRANSFER_ENCODING) {
            let chunked = encoding
                .as_bytes()
                .trim_ascii()
                .eq_ignore_ascii_case(b"chunked");
            if parts.method != Method::POST || content_length.is_some() || !chunked {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
        // Step 9: Content-Length is digits only, at most 10 characters, within 4 MiB, and zero
        // on a non-POST.
        if let Some(length) = content_length {
            let digits = length.as_bytes();
            if digits.is_empty() || digits.len() > 10 || !digits.iter().all(u8::is_ascii_digit) {
                return Err(StatusCode::BAD_REQUEST);
            }
            let declared = parse_length(digits);
            if declared > MAX_BODY_BYTES {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            if parts.method != Method::POST && declared != 0 {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
        // Step 10: a POST is application/json, optionally with charset=utf-8.
        if parts.method == Method::POST && !json_content_type(parts.headers.get(CONTENT_TYPE)) {
            return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        }
        Ok(())
    }
}

/// Run the pre-body checks (steps 1 to 10) for one parsed request.
///
/// This is the guard's decision function: the listener calls it for every request, and the
/// golden vectors of `tests/golden/http-guard.json` replay it without a socket.
pub fn check_request(
    config: &LoopbackHttpConfig,
    parts: &Parts,
    peer: SocketAddr,
    local: SocketAddr,
) -> Result<(), StatusCode> {
    Guard::new(config.port(), config.token_digest()).check(parts, peer, local)
}

/// Step 4 for one field: the name is a token, the value is printable ASCII, and no proxy header.
fn check_field(name: &HeaderName, value: &HeaderValue) -> Result<(), StatusCode> {
    let key = name.as_str();
    let printable = value
        .as_bytes()
        .iter()
        .all(|byte| (32..=126).contains(byte));
    let proxied = PROXY_HEADERS.contains(&key) || key.starts_with("x-forwarded-");
    if !is_token(key.as_bytes()) || !printable || proxied {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

/// The RFC 7230 token alphabet (the first-line name rule of step 4b).
fn is_token(name: &[u8]) -> bool {
    !name.is_empty()
        && name.iter().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// Decimal digits of at most 10 characters, already validated, as a number.
fn parse_length(digits: &[u8]) -> usize {
    digits.iter().fold(0_usize, |total, digit| {
        total
            .saturating_mul(10)
            .saturating_add(usize::from(digit.saturating_sub(b'0')))
    })
}

/// Step 10: the first segment is `application/json`; an optional second is `charset=utf-8`.
fn json_content_type(value: Option<&HeaderValue>) -> bool {
    let raw = value
        .map_or(&[][..], HeaderValue::as_bytes)
        .to_ascii_lowercase();
    let segments: Vec<&[u8]> = raw.split(|byte| *byte == b';').collect();
    let media = segments.first().copied().unwrap_or_default().trim_ascii();
    if media != b"application/json" || segments.len() > 2 {
        return false;
    }
    segments
        .get(1)
        .is_none_or(|parameter| is_utf8_charset(parameter))
}

/// The regex ` *charset *= *(?:utf-8|"utf-8") *` matched in full.
fn is_utf8_charset(parameter: &[u8]) -> bool {
    let text = trim_spaces(parameter);
    let Some(rest) = text.strip_prefix(b"charset") else {
        return false;
    };
    let Some(rest) = trim_start_spaces(rest).strip_prefix(b"=") else {
        return false;
    };
    matches!(trim_start_spaces(rest), b"utf-8" | b"\"utf-8\"")
}

/// Remove leading and trailing spaces (only the space byte, as the regex does).
fn trim_spaces(bytes: &[u8]) -> &[u8] {
    let text = trim_start_spaces(bytes);
    let end = text
        .iter()
        .rposition(|byte| *byte != b' ')
        .map_or(0, |last| last + 1);
    text.get(..end).unwrap_or_default()
}

/// Remove leading spaces.
fn trim_start_spaces(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| *byte != b' ')
        .unwrap_or(bytes.len());
    bytes.get(start..).unwrap_or_default()
}

/// Compare two digests without an early exit on the first differing byte.
pub fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_content_type_accepts_the_two_documented_forms() {
        for value in [
            "application/json",
            "Application/JSON",
            "application/json; charset=utf-8",
            "application/json;charset=\"UTF-8\"",
            "application/json ;  charset = utf-8 ",
        ] {
            let header = HeaderValue::from_static(value);
            assert!(json_content_type(Some(&header)), "{value} must be accepted");
        }
    }

    #[test]
    fn json_content_type_refuses_everything_else() {
        for value in [
            "",
            "text/plain",
            "application/jsonx",
            "application/json; charset=latin-1",
            "application/json; charset=utf-8; x=1",
            "application/json; foo=bar",
            "application/json; charset=\"utf-8",
            "application/json;\tcharset=utf-8",
        ] {
            let header = HeaderValue::from_str(value).ok();
            assert!(
                !json_content_type(header.as_ref()),
                "{value} must be refused"
            );
        }
        assert!(!json_content_type(None));
    }

    #[test]
    fn token_alphabet_follows_rfc_7230() {
        assert!(is_token(b"x-custom.name"));
        assert!(!is_token(b""));
        assert!(!is_token(b"bad name"));
        assert!(!is_token(b"bad:name"));
    }

    #[test]
    fn digests_compare_without_short_circuit() {
        let left = [7_u8; 32];
        let mut right = left;
        assert!(constant_time_eq(&left, &right));
        right[31] ^= 1;
        assert!(!constant_time_eq(&left, &right));
    }
}
