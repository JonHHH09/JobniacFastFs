//! The request target and authority of a wire URL.
//!
//! The wire URL comes from [`jobniac_core::job_links::urls::wire_url`], which has already applied
//! the pre-fetch checks: the scheme is `http` or `https`, the authority is numeric or a normalized
//! host name, and the fragment is gone. This module only splits it. Dot segments are never
//! normalized, because the request line must carry the path as the user wrote it.

use std::net::IpAddr;

/// The safe characters of the request target. Letters, digits,
/// `_.-~` are always kept.
const TARGET_SAFE: &[u8] = b"/?!$&'()*+,;=:@%-._~";

/// One split wire URL, ready for the connector. `Debug` is hand-written: the host, the request
/// target and the `Host` header are not printed, because the target may carry credentials in its
/// query and the host is part of the request.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    /// True for `https`.
    pub https: bool,
    /// The host as the URL writes it, without brackets. A name stays a name, so TLS verifies it.
    pub host: String,
    /// The numeric host, when the authority is an IP literal.
    pub literal: Option<IpAddr>,
    /// The port, explicit or the scheme default.
    pub port: u16,
    /// The origin-form request target (path and query), quoted with the safe set.
    pub request_target: String,
    /// The `Host` header value: the host, with the port unless it is the scheme default.
    pub host_header: String,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Target")
            .field("https", &self.https)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

/// Splits a wire URL. Returns `None` for any text that is not a wire URL; callers only pass
/// output of `wire_url`, so `None` is an internal error.
#[must_use]
pub fn split(wire: &str) -> Option<Target> {
    let (scheme, after) = wire.split_once("://")?;
    let https = match scheme {
        "https" => true,
        "http" => false,
        _ => return None,
    };
    let authority_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (authority, rest) = after.split_at(authority_end);
    let (host, port_text) = if let Some(inner) = authority.strip_prefix('[') {
        let end = inner.find(']')?;
        let suffix = &inner[end + 1..];
        let port = match suffix {
            "" => None,
            _ => Some(suffix.strip_prefix(':')?),
        };
        (&inner[..end], port)
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let default_port = if https { 443 } else { 80 };
    let port = match port_text {
        Some(text) => text.parse::<u16>().ok()?,
        None => default_port,
    };
    let literal = host.parse::<IpAddr>().ok();
    let host_header = {
        let bracketed = if literal.is_some_and(|ip| ip.is_ipv6()) {
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        if port == default_port {
            bracketed
        } else {
            format!("{bracketed}:{port}")
        }
    };
    Some(Target {
        https,
        host: host.to_owned(),
        literal,
        port,
        request_target: request_target(rest),
        host_header,
    })
}

/// The origin-form target: `/` when empty, a leading slash added, and the quoting of
/// the characters outside the safe set.
fn request_target(rest: &str) -> String {
    let target = if rest.is_empty() {
        "/".to_owned()
    } else if rest.starts_with('/') {
        rest.to_owned()
    } else {
        format!("/{rest}")
    };
    let mut quoted = String::with_capacity(target.len());
    for byte in target.bytes() {
        if byte.is_ascii_alphanumeric() || TARGET_SAFE.contains(&byte) {
            quoted.push(char::from(byte));
        } else {
            quoted.push_str(&format!("%{byte:02X}"));
        }
    }
    quoted
}
