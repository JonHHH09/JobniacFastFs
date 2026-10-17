//! IP address parsing and the public-only predicate.
//!
//! The parser accepts strict dotted-quad IPv4 (no leading zeros, ASCII digits only), IPv6 with at
//! most one `::`, one to four hexadecimal digits per group and an optional dotted IPv4 suffix. Zone
//! identifiers are not parsed; the callers refuse `%` before they reach here. The predicate uses the
//! special-purpose registry tables written out below (the stable-Rust replacement for the unstable `Ipv6Addr::is_global`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// An IPv4 network as a masked base address and a prefix length.
#[derive(Clone, Copy)]
struct Net4 {
    base: u32,
    len: u32,
}

impl Net4 {
    const fn new(octets: [u8; 4], len: u32) -> Self {
        Self {
            base: u32::from_be_bytes(octets),
            len,
        }
    }

    fn contains(self, address: u32) -> bool {
        let mask = u32::MAX.checked_shl(32 - self.len).unwrap_or(0);
        address & mask == self.base & mask
    }
}

/// An IPv6 network as a masked base address and a prefix length.
#[derive(Clone, Copy)]
struct Net6 {
    base: u128,
    len: u32,
}

impl Net6 {
    const fn new(groups: [u16; 8], len: u32) -> Self {
        let mut base: u128 = 0;
        let mut index = 0;
        while index < 8 {
            base = (base << 16) | groups[index] as u128;
            index += 1;
        }
        Self { base, len }
    }

    fn contains(self, address: u128) -> bool {
        let mask = u128::MAX.checked_shl(128 - self.len).unwrap_or(0);
        address & mask == self.base & mask
    }
}

/// Blocked IPv4 ranges. Together with the global-address rule (which excludes the same ranges plus
/// 100.64.0.0/10) this is the complete IPv4 policy.
const V4_BLOCKED: [Net4; 15] = [
    Net4::new([0, 0, 0, 0], 8),
    Net4::new([10, 0, 0, 0], 8),
    Net4::new([100, 64, 0, 0], 10),
    Net4::new([127, 0, 0, 0], 8),
    Net4::new([169, 254, 0, 0], 16),
    Net4::new([172, 16, 0, 0], 12),
    Net4::new([192, 0, 0, 0], 24),
    Net4::new([192, 0, 2, 0], 24),
    Net4::new([192, 88, 99, 0], 24),
    Net4::new([192, 168, 0, 0], 16),
    Net4::new([198, 18, 0, 0], 15),
    Net4::new([198, 51, 100, 0], 24),
    Net4::new([203, 0, 113, 0], 24),
    Net4::new([224, 0, 0, 0], 4),
    Net4::new([240, 0, 0, 0], 4),
];

/// Blocked IPv6 ranges.
const V6_BLOCKED: [Net6; 14] = [
    Net6::new([0, 0, 0, 0, 0, 0, 0, 0], 96),
    Net6::new([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48),
    Net6::new([0x100, 0, 0, 0, 0, 0, 0, 0], 64),
    Net6::new([0x2001, 0, 0, 0, 0, 0, 0, 0], 32),
    Net6::new([0x2001, 2, 0, 0, 0, 0, 0, 0], 48),
    Net6::new([0x2001, 0x10, 0, 0, 0, 0, 0, 0], 28),
    Net6::new([0x2001, 0x20, 0, 0, 0, 0, 0, 0], 28),
    Net6::new([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32),
    Net6::new([0x3fff, 0, 0, 0, 0, 0, 0, 0], 20),
    Net6::new([0x5f00, 0, 0, 0, 0, 0, 0, 0], 16),
    Net6::new([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7),
    Net6::new([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10),
    Net6::new([0xfec0, 0, 0, 0, 0, 0, 0, 0], 10),
    Net6::new([0xff00, 0, 0, 0, 0, 0, 0, 0], 8),
];

/// IPv6 private networks. Addresses in them are not global.
const V6_PRIVATE: [Net6; 11] = [
    Net6::new([0, 0, 0, 0, 0, 0, 0, 1], 128),
    Net6::new([0, 0, 0, 0, 0, 0, 0, 0], 128),
    Net6::new([0, 0, 0, 0, 0, 0xffff, 0, 0], 96),
    Net6::new([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48),
    Net6::new([0x100, 0, 0, 0, 0, 0, 0, 0], 64),
    Net6::new([0x2001, 0, 0, 0, 0, 0, 0, 0], 23),
    Net6::new([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32),
    Net6::new([0x2002, 0, 0, 0, 0, 0, 0, 0], 16),
    Net6::new([0x3fff, 0, 0, 0, 0, 0, 0, 0], 20),
    Net6::new([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7),
    Net6::new([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10),
];

/// Exceptions to the IPv6 private networks.
const V6_PRIVATE_EXCEPTIONS: [Net6; 6] = [
    Net6::new([0x2001, 1, 0, 0, 0, 0, 0, 1], 128),
    Net6::new([0x2001, 1, 0, 0, 0, 0, 0, 2], 128),
    Net6::new([0x2001, 3, 0, 0, 0, 0, 0, 0], 32),
    Net6::new([0x2001, 0x4, 0x112, 0, 0, 0, 0, 0], 48),
    Net6::new([0x2001, 0x20, 0, 0, 0, 0, 0, 0], 28),
    Net6::new([0x2001, 0x30, 0, 0, 0, 0, 0, 0], 28),
];

/// IPv6 reserved networks.
const V6_RESERVED: [Net6; 15] = [
    Net6::new([0, 0, 0, 0, 0, 0, 0, 0], 8),
    Net6::new([0x100, 0, 0, 0, 0, 0, 0, 0], 8),
    Net6::new([0x200, 0, 0, 0, 0, 0, 0, 0], 7),
    Net6::new([0x400, 0, 0, 0, 0, 0, 0, 0], 6),
    Net6::new([0x800, 0, 0, 0, 0, 0, 0, 0], 5),
    Net6::new([0x1000, 0, 0, 0, 0, 0, 0, 0], 4),
    Net6::new([0x4000, 0, 0, 0, 0, 0, 0, 0], 3),
    Net6::new([0x6000, 0, 0, 0, 0, 0, 0, 0], 3),
    Net6::new([0x8000, 0, 0, 0, 0, 0, 0, 0], 3),
    Net6::new([0xa000, 0, 0, 0, 0, 0, 0, 0], 3),
    Net6::new([0xc000, 0, 0, 0, 0, 0, 0, 0], 3),
    Net6::new([0xe000, 0, 0, 0, 0, 0, 0, 0], 4),
    Net6::new([0xf000, 0, 0, 0, 0, 0, 0, 0], 5),
    Net6::new([0xf800, 0, 0, 0, 0, 0, 0, 0], 6),
    Net6::new([0xfe00, 0, 0, 0, 0, 0, 0, 0], 9),
];

/// The IPv6 multicast network.
const V6_MULTICAST: Net6 = Net6::new([0xff00, 0, 0, 0, 0, 0, 0, 0], 8);
/// IPv4-mapped `::ffff:0:0/96`.
const V6_MAPPED: Net6 = Net6::new([0, 0, 0, 0, 0, 0xffff, 0, 0], 96);
/// NAT64 well-known prefix `64:ff9b::/96`.
const V6_NAT64: Net6 = Net6::new([0x64, 0xff9b, 0, 0, 0, 0, 0, 0], 96);
/// 6to4 `2002::/16`.
const V6_SIX_TO_FOUR: Net6 = Net6::new([0x2002, 0, 0, 0, 0, 0, 0, 0], 16);

/// Parses an IPv4 dotted quad (strict, no leading zeros).
pub fn parse_ipv4(text: &str) -> Option<u32> {
    if text.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    let mut count = 0;
    for octet in text.split('.') {
        count += 1;
        if count > 4 {
            return None;
        }
        value = (value << 8) | parse_octet(octet)? as u32;
    }
    if count == 4 { Some(value) } else { None }
}

fn parse_octet(octet: &str) -> Option<u8> {
    if octet.is_empty() || !octet.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if octet.len() > 3 || (octet != "0" && octet.starts_with('0')) {
        return None;
    }
    let mut value: u32 = 0;
    for byte in octet.bytes() {
        value = value * 10 + u32::from(byte - b'0');
    }
    u8::try_from(value).ok()
}

fn parse_hextet(hextet: &str) -> Option<u128> {
    if hextet.is_empty() || hextet.len() > 4 || !hextet.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    u128::from_str_radix(hextet, 16).ok()
}

/// Parses an IPv6 address without a zone identifier.
pub fn parse_ipv6(text: &str) -> Option<u128> {
    if text.is_empty() || text.chars().count() > 45 {
        return None;
    }
    let mut parts: Vec<String> = text.splitn(10, ':').map(str::to_owned).collect();
    if parts.len() < 3 {
        return None;
    }
    if parts.last().is_some_and(|last| last.contains('.')) {
        let suffix = parts.pop()?;
        let ipv4 = parse_ipv4(&suffix)?;
        parts.push(format!("{:x}", (ipv4 >> 16) & 0xFFFF));
        parts.push(format!("{:x}", ipv4 & 0xFFFF));
    }
    // Nine parts are allowed (the extra one comes from a leading or trailing "::").
    if parts.len() > 9 {
        return None;
    }
    let last = parts.len() - 1;
    let mut skip: Option<usize> = None;
    for (index, part) in parts.iter().enumerate().take(last).skip(1) {
        if part.is_empty() {
            if skip.is_some() {
                return None;
            }
            skip = Some(index);
        }
    }
    let (parts_hi, parts_lo, skipped) = match skip {
        Some(index) => {
            let mut hi = index as isize;
            let mut lo = (parts.len() - index - 1) as isize;
            if parts[0].is_empty() {
                hi -= 1;
                if hi != 0 {
                    return None;
                }
            }
            if parts[last].is_empty() {
                lo -= 1;
                if lo != 0 {
                    return None;
                }
            }
            let skipped = 8 - (hi + lo);
            if skipped < 1 {
                return None;
            }
            (hi as usize, lo as usize, skipped as u32)
        }
        None => {
            if parts.len() != 8 || parts[0].is_empty() || parts[last].is_empty() {
                return None;
            }
            (8, 0, 0)
        }
    };
    let mut value: u128 = 0;
    for part in parts.iter().take(parts_hi) {
        value = (value << 16) | parse_hextet(part)?;
    }
    // A bare "::" skips all eight groups; the value is still zero, so the shift saturates to zero.
    value = value.checked_shl(16 * skipped).unwrap_or(0);
    for part in parts.iter().skip(parts.len() - parts_lo) {
        value = (value << 16) | parse_hextet(part)?;
    }
    Some(value)
}

/// Parses an IP literal: strict IPv4 first, then IPv6.
pub fn parse_ip(text: &str) -> Option<IpAddr> {
    if let Some(value) = parse_ipv4(text) {
        return Some(IpAddr::V4(Ipv4Addr::from(value)));
    }
    parse_ipv6(text).map(|value| IpAddr::V6(Ipv6Addr::from(value)))
}

/// The compressed IPv6 text: lowercase hextets, the longest run of zero groups
/// (two or more, first on ties) replaced by `::`, and no dotted IPv4 suffix.
pub fn compressed_ipv6(value: u128) -> String {
    // An IPv4-mapped address prints as `::ffff:` followed by the dotted IPv4 address.
    if value >> 32 == 0xFFFF {
        return format!("::ffff:{}", Ipv4Addr::from(value as u32));
    }
    let hextets: Vec<String> = (0..8)
        .map(|index| format!("{:x}", (value >> (16 * (7 - index))) & 0xFFFF))
        .collect();
    let mut best_start: usize = 0;
    let mut best_len: usize = 0;
    let mut run_start: usize = 0;
    let mut run_len: usize = 0;
    for (index, hextet) in hextets.iter().enumerate() {
        if hextet == "0" {
            if run_len == 0 {
                run_start = index;
            }
            run_len += 1;
            if run_len > best_len {
                best_len = run_len;
                best_start = run_start;
            }
        } else {
            run_len = 0;
        }
    }
    let mut parts: Vec<String> = hextets.clone();
    if best_len > 1 {
        let end = best_start + best_len;
        if end == parts.len() {
            parts.push(String::new());
        }
        parts.splice(best_start..end, [String::new()]);
        if best_start == 0 {
            parts.insert(0, String::new());
        }
    }
    parts.join(":")
}

fn in_any(networks: &[Net6], address: u128) -> bool {
    networks.iter().any(|network| network.contains(address))
}

/// True when the IPv4 address is global and outside the blocked table.
fn public_v4(address: u32) -> bool {
    !V4_BLOCKED.iter().any(|network| network.contains(address))
}

/// True when an IPv6 address that is not IPv4-embedded is globally routable.
fn public_global_v6(address: u128) -> bool {
    if V6_MULTICAST.contains(address)
        || in_any(&V6_RESERVED, address)
        || (in_any(&V6_PRIVATE, address) && !in_any(&V6_PRIVATE_EXCEPTIONS, address))
    {
        return false;
    }
    !V6_BLOCKED.iter().any(|network| network.contains(address))
}

/// True only for globally routable unicast addresses, including the IPv4 embedded in mapped,
/// NAT64, 6to4 and ISATAP IPv6 addresses.
pub fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => public_v4(u32::from(address)),
        IpAddr::V6(address) => {
            let bits = u128::from(address);
            let embedded_v4 = bits as u32;
            if V6_MAPPED.contains(bits) || V6_NAT64.contains(bits) {
                return public_v4(embedded_v4);
            }
            if V6_SIX_TO_FOUR.contains(bits) {
                return public_v4(((bits >> 80) & 0xFFFF_FFFF) as u32);
            }
            let octets = address.octets();
            // ISATAP interface identifier: 0000:5efe or 0200:5efe followed by the IPv4 address.
            let isatap_prefix = octets[8..10] == [0, 0] || octets[8..10] == [0x02, 0];
            if isatap_prefix && octets[10..12] == [0x5e, 0xfe] {
                return public_v4(embedded_v4) && public_global_v6(bits);
            }
            public_global_v6(bits)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_form_follows_the_standard_rules() {
        assert_eq!(compressed_ipv6(0), "::");
        assert_eq!(compressed_ipv6(1), "::1");
        assert_eq!(
            compressed_ipv6(0x2606_4700_4700_0000_0000_0000_0000_1111),
            "2606:4700:4700::1111"
        );
        assert_eq!(
            compressed_ipv6(0x0001_0000_0000_0001_0000_0000_0000_0001),
            "1:0:0:1::1"
        );
    }

    #[test]
    fn strict_ipv4_refuses_non_canonical_spellings() {
        assert_eq!(parse_ipv4("8.8.8.8"), Some(0x0808_0808));
        assert_eq!(parse_ipv4("08.8.8.8"), None);
        assert_eq!(parse_ipv4("256.1.1.1"), None);
        assert_eq!(parse_ipv4("1.2.3"), None);
        assert_eq!(parse_ipv4("0x7f.0.0.1"), None);
    }
}
