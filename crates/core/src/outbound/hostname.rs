//! Host name normalization for outbound destinations.
//!
//! Non-ASCII names are converted with UTS 46 (`idna` crate, strict profile) rather than
//! the IDNA 2003 codec. The differences between the two are listed in
//! `crates/core/tests/outbound_idna.rs`. ASCII names skip the conversion.

use super::policy::{OutboundPolicyError, PolicyFailure};

/// Longest host name after normalization, in octets.
pub const MAX_HOSTNAME_LENGTH: usize = 253;
/// Longest DNS label, in octets.
pub const MAX_LABEL_LENGTH: usize = 63;

/// Last labels that never designate a public job board.
const RESERVED_LAST_LABELS: [&str; 13] = [
    "localhost",
    "local",
    "localdomain",
    "internal",
    "invalid",
    "test",
    "example",
    "onion",
    "lan",
    "intranet",
    "corp",
    "home",
    "arpa",
];

fn refuse() -> OutboundPolicyError {
    OutboundPolicyError::new(PolicyFailure::Hostname)
}

/// True when the label matches `[a-z0-9](?:[a-z0-9-]*[a-z0-9])?` on an already lowercased label.
fn is_ldh_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    let is_alnum = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    match (bytes.first(), bytes.last()) {
        (Some(&first), Some(&last)) => {
            is_alnum(first)
                && is_alnum(last)
                && bytes.iter().all(|&byte| is_alnum(byte) || byte == b'-')
        }
        _ => false,
    }
}

/// True when the label matches `[0-9]+|0x[0-9a-f]*`.
fn is_numeric_label(label: &str) -> bool {
    if !label.is_empty() && label.bytes().all(|byte| byte.is_ascii_digit()) {
        return true;
    }
    match label.strip_prefix("0x") {
        Some(rest) => rest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        None => false,
    }
}

/// Checks one `xn--` label: the punycode must decode and
/// convert back to the same label.
fn punycode_round_trips(label: &str) -> bool {
    let Some(encoded) = label.strip_prefix("xn--") else {
        return true;
    };
    let Some(decoded) = idna::punycode::decode_to_string(encoded) else {
        return false;
    };
    match idna::domain_to_ascii(&decoded) {
        Ok(ascii) => ascii == label,
        Err(_) => false,
    }
}

/// Returns the lowercase ASCII (IDNA) form of a public-looking DNS name, or refuses it.
///
/// One trailing root dot is dropped. IP literals, numeric forms such as `2130706433`,
/// `0x7f000001` or `0177.0.0.1`, single-label and reserved names are refused.
pub fn normalize_hostname(host: &str) -> Result<String, OutboundPolicyError> {
    if host.is_empty() {
        return Err(refuse());
    }
    let mut text = if host.is_ascii() {
        host.to_owned()
    } else {
        // The IDNA 2003 codec splits on the ideographic and fullwidth full stops too, and keeps a root
        // dot until the trailing-dot strip below. Both steps happen here before the UTS 46 mapping.
        let unified: String = host
            .chars()
            .map(|character| match character {
                '\u{3002}' | '\u{FF0E}' | '\u{FF61}' => '.',
                other => other,
            })
            .collect();
        let unrooted = unified.strip_suffix('.').unwrap_or(&unified);
        idna::domain_to_ascii_strict(unrooted).map_err(|_| refuse())?
    };
    if let Some(stripped) = text.strip_suffix('.') {
        text = stripped.to_owned();
    }
    let text = text.to_ascii_lowercase();
    if text.is_empty() || text.len() > MAX_HOSTNAME_LENGTH {
        return Err(refuse());
    }
    let labels: Vec<&str> = text.split('.').collect();
    if labels.len() < 2 {
        return Err(refuse());
    }
    for label in &labels {
        if label.len() > MAX_LABEL_LENGTH || !is_ldh_label(label) {
            return Err(refuse());
        }
    }
    let last = labels[labels.len() - 1];
    if is_numeric_label(last) || RESERVED_LAST_LABELS.contains(&last) {
        return Err(refuse());
    }
    if !labels.iter().all(|label| punycode_round_trips(label)) {
        return Err(refuse());
    }
    Ok(text)
}
