//! Validated settings of the loopback HTTP listener.
//!
//! The bearer token is held as a [`SecretString`] and is never printed: the type has no `Display`,
//! and its `Debug` output is redacted.

use std::fmt;

use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};

/// The lowest port the listener accepts (`1024`); lower ports are refused.
pub const MIN_PORT: u16 = 1024;

/// The sanitized refusal of an invalid listener configuration. It names no setting and no value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid loopback HTTP configuration")]
pub struct HttpConfigError;

/// Explicit single-operator loopback configuration: a port and a bearer token.
pub struct LoopbackHttpConfig {
    port: u16,
    bearer_token: SecretString,
}

impl LoopbackHttpConfig {
    /// Validate a port and a bearer token.
    ///
    /// The port must be in `1024..=65535`. The token must be 43 to 128 characters drawn from
    /// `[A-Za-z0-9_-]`, with nothing before or after.
    pub fn new(port: u16, bearer_token: &str) -> Result<Self, HttpConfigError> {
        if port < MIN_PORT || !is_valid_token(bearer_token) {
            return Err(HttpConfigError);
        }
        Ok(Self {
            port,
            bearer_token: SecretString::from(bearer_token.to_owned()),
        })
    }

    /// The loopback port the listener binds.
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// The SHA-256 digest of the bearer token. Only digests are compared, never the token.
    pub(crate) fn token_digest(&self) -> [u8; 32] {
        sha256(self.bearer_token.expose_secret().as_bytes())
    }
}

impl fmt::Debug for LoopbackHttpConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoopbackHttpConfig")
            .field("port", &self.port)
            .field("bearer_token", &"<redacted>")
            .finish()
    }
}

/// Whether `token` is 43 to 128 characters of `[A-Za-z0-9_-]`.
fn is_valid_token(token: &str) -> bool {
    (43..=128).contains(&token.len())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// SHA-256 of `bytes` as a fixed-size array.
pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut out = [0_u8; 32];
    out.copy_from_slice(digest.as_slice());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "synthetic_http_token_0123456789abcdefghijklmnop";

    #[test]
    fn accepts_port_and_token_at_their_bounds() {
        let low = "a".repeat(43);
        let high = "Z-_9".repeat(32);
        assert!(LoopbackHttpConfig::new(1024, &low).is_ok());
        assert!(LoopbackHttpConfig::new(65535, &high).is_ok());
        assert!(LoopbackHttpConfig::new(8765, TOKEN).is_ok());
    }

    #[test]
    fn refuses_ports_outside_the_range() {
        for port in [0_u16, 1, 80, 1023] {
            assert_eq!(
                LoopbackHttpConfig::new(port, TOKEN).err(),
                Some(HttpConfigError)
            );
        }
    }

    #[test]
    fn refuses_tokens_that_are_not_exact() {
        let bad = [
            String::new(),
            "A".repeat(42),
            "A".repeat(129),
            "é".repeat(43),
            format!("{}+", "A".repeat(42)),
            format!("{} ", "A".repeat(43)),
            format!(" {}", "A".repeat(43)),
            format!("{}\n", "A".repeat(43)),
            "A".repeat(43) + "/",
        ];
        for token in bad {
            assert_eq!(
                LoopbackHttpConfig::new(8765, &token).err(),
                Some(HttpConfigError),
                "token of length {} must be refused",
                token.len()
            );
        }
    }

    #[test]
    fn debug_output_never_contains_the_token() {
        let config = LoopbackHttpConfig::new(8765, TOKEN).unwrap_or_else(|_| unreachable!());
        let text = format!("{config:?}");
        assert!(!text.contains(TOKEN));
        assert!(text.contains("8765"));
    }

    #[test]
    fn refusal_text_is_fixed() {
        assert_eq!(
            HttpConfigError.to_string(),
            "invalid loopback HTTP configuration"
        );
    }
}
