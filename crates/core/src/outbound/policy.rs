//! Admission of resolver answers and the sanitized
//! refusal type shared by the outbound modules.

use std::fmt;

use super::address::{compressed_ipv6, is_public_address, parse_ip};

/// Most records one resolver answer may carry.
pub const MAX_RECORDS: usize = 64;

/// Why an outbound destination was refused. The variants carry no address, name or cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyFailure {
    /// The host name is not a public-looking DNS name.
    Hostname,
    /// A resolver record is malformed or not a public address.
    Address,
    /// The resolver failed.
    Resolution,
}

impl PolicyFailure {
    /// The reason string (`hostname`, `address` or `resolution`).
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Hostname => "hostname",
            Self::Address => "address",
            Self::Resolution => "resolution",
        }
    }
}

/// A sanitized outbound refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundPolicyError {
    failure: PolicyFailure,
}

impl OutboundPolicyError {
    /// Wraps a refusal reason.
    pub const fn new(failure: PolicyFailure) -> Self {
        Self { failure }
    }

    /// The refusal reason.
    pub const fn failure(self) -> PolicyFailure {
        self.failure
    }

    /// The reason string.
    pub const fn reason(self) -> &'static str {
        self.failure.reason()
    }
}

impl fmt::Display for OutboundPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.reason())
    }
}

impl std::error::Error for OutboundPolicyError {}

/// The socket type of a resolver record: `SOCK_STREAM` or anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketKind {
    /// `SOCK_STREAM`.
    Stream,
    /// Any other socket type (for example datagram).
    Other,
}

/// The protocol of a resolver record: `IPPROTO_TCP` or anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordProtocol {
    /// `IPPROTO_TCP`.
    Tcp,
    /// Any other protocol.
    Other,
}

/// The address family of a resolver record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// `AF_INET`.
    Inet,
    /// `AF_INET6`.
    Inet6,
    /// Any other family.
    Other,
}

/// The socket address of a resolver record. IPv6 carries the flow and scope fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The numeric address text as the resolver returned it.
    pub host: String,
    /// The port number.
    pub port: i64,
    /// `(flow_info, scope_id)` for IPv6 endpoints; `None` for IPv4 endpoints.
    pub ipv6_extra: Option<(i64, i64)>,
}

/// One raw TCP record of a resolver answer (the five fields of `getaddrinfo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverRecord {
    /// Address family.
    pub family: Family,
    /// Socket type.
    pub kind: SocketKind,
    /// Protocol.
    pub protocol: RecordProtocol,
    /// Socket address.
    pub endpoint: Endpoint,
}

/// Validates raw TCP records. Any malformed or non-public record rejects the whole answer.
/// Returns the admitted numeric addresses, deduplicated in order.
pub fn admit_records(
    records: &[ResolverRecord],
    port: i64,
) -> Result<Vec<String>, OutboundPolicyError> {
    let refuse = || OutboundPolicyError::new(PolicyFailure::Address);
    if records.is_empty() || records.len() > MAX_RECORDS {
        return Err(refuse());
    }
    let mut addresses: Vec<String> = Vec::new();
    for record in records {
        let family_ok = matches!(record.family, Family::Inet | Family::Inet6);
        if !family_ok
            || record.kind != SocketKind::Stream
            || record.protocol != RecordProtocol::Tcp
            || record.endpoint.port != port
            || record.endpoint.host.contains('%')
        {
            return Err(refuse());
        }
        let extra_ok = matches!(
            (record.family, record.endpoint.ipv6_extra),
            (Family::Inet, None) | (Family::Inet6, Some((0, 0)))
        );
        if !extra_ok {
            return Err(refuse());
        }
        let address = parse_ip(&record.endpoint.host).ok_or_else(refuse)?;
        let version_matches = matches!(
            (record.family, address),
            (Family::Inet, std::net::IpAddr::V4(_)) | (Family::Inet6, std::net::IpAddr::V6(_))
        );
        if !version_matches || !is_public_address(address) {
            return Err(refuse());
        }
        let text = match address {
            std::net::IpAddr::V4(address) => address.to_string(),
            std::net::IpAddr::V6(address) => compressed_ipv6(u128::from(address)),
        };
        if !addresses.contains(&text) {
            addresses.push(text);
        }
    }
    Ok(addresses)
}
