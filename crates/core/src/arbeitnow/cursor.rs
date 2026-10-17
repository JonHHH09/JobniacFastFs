//! Scan cursors and their fingerprint.
//!
//! A cursor is `b64url_nopad("{page}:{offset}:{fingerprint}:" + b64url_nopad(HMAC))`. The
//! fingerprint is 64 lowercase hex digits of HMAC-SHA256 over the length-prefixed scan fields
//! (`"{utf16_length}:{value}"`, UTF-16 units as in Java). The HMAC covers `"{page}:{offset}:{fingerprint}"`.
//! The key is a [`CursorKey`], a separate authority that is never shared with the candidate key.
//!
//! The decoder accepts several aliases of the canonical form: correct `=` padding, non-zero
//! trailing
//! bits, leading zeros, a sign and
//! the Unicode decimal digits of the pinned table. All of them still need the genuine HMAC over the
//! decoded text, so an alias never widens what a key holder could not already sign; the pinned
//! golden vectors (`cursor_outer_padded`, `cursor_inner_alias`, `cursor_outer_alias`,
//! `cursor_leading_zeros`, `cursor_unicode_decimal`) record the accepted forms. Cursors this build
//! emits are always canonical.

use std::fmt;

use base64::Engine as _;
use base64::alphabet;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, URL_SAFE_NO_PAD};
use hmac::{KeyInit, Mac, SimpleHmac};
use jobniac_text::case::root_lower;
use jobniac_text::java::{java_string_key, java_strip, utf16_length};
use sha2::Sha256;

use crate::errors::{ApplicationError, validation_error};

/// Shortest accepted cursor key, in bytes.
pub const MIN_KEY_BYTES: usize = 32;

/// Message of every refused scan request or configuration.
pub const REQUEST_MESSAGE: &str = "Invalid Arbeitnow scan request";

/// Largest page number a cursor may carry.
pub const MAX_CURSOR_PAGE: i64 = 1_000_000;

/// Largest offset a cursor may carry.
pub const MAX_CURSOR_OFFSET: i64 = 10_000;

type Hmac = SimpleHmac<Sha256>;

/// Start code points of the pinned decimal digit blocks (value of a digit is its distance from
/// the block start). Digit values 0 to 9.
const DIGIT_BLOCKS: [u16; 37] = [
    0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
    0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
    0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0, 0xff10,
];

/// The HMAC key of scan cursors: at least 32 bytes, owned, and independent of the candidate key.
#[derive(Clone)]
pub struct CursorKey(Vec<u8>);

impl CursorKey {
    /// Build a key; fewer than 32 bytes is `Invalid Arbeitnow scan request`.
    pub fn new(key: &[u8]) -> Result<Self, ApplicationError> {
        if key.len() < MIN_KEY_BYTES {
            return Err(validation_error(REQUEST_MESSAGE, None, None));
        }
        Ok(Self(key.to_vec()))
    }
}

impl fmt::Debug for CursorKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CursorKey").finish_non_exhaustive()
    }
}

/// The raw filter fields that a cursor is bound to. `limit` and `max_pages` are not part of it, so
/// a cursor works with another page size (quirk l).
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    /// The free-text query.
    pub query: Option<&'a str>,
    /// The location filter.
    pub location: Option<&'a str>,
    /// The remote-only flag.
    pub remote_only: Option<bool>,
    /// The visa sponsorship flag.
    pub visa_sponsorship: Option<bool>,
    /// The tag filter values.
    pub tags: Option<&'a [String]>,
    /// The job type filter values.
    pub job_types: Option<&'a [String]>,
}

/// The ASCII `\s` class.
const fn is_ascii_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r' | '\u{c}' | '\u{b}')
}

/// One piece between runs of ASCII whitespace.
fn split_ascii_space(text: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut in_space = false;
    for character in text.chars() {
        if is_ascii_space(character) {
            if !in_space {
                pieces.push(std::mem::take(&mut current));
                in_space = true;
            }
        } else {
            in_space = false;
            current.push(character);
        }
    }
    pieces.push(current);
    pieces
}

/// The lowercased, Java-stripped query split on ASCII whitespace, with trailing
/// empty pieces removed. A missing or blank query has no terms.
pub fn terms(query: Option<&str>) -> Vec<String> {
    let Some(value) = query else {
        return Vec::new();
    };
    let stripped = java_strip(value);
    if stripped.is_empty() {
        return Vec::new();
    }
    let mut pieces = split_ascii_space(&root_lower(stripped));
    while pieces.last().is_some_and(String::is_empty) {
        pieces.pop();
    }
    pieces
}

/// The normal form of a filter value: lowercased after the Java strip.
pub fn normalized(value: &str) -> String {
    root_lower(java_strip(value))
}

fn field(value: &str) -> String {
    format!("{}:{value}", utf16_length(value))
}

/// Normalized values sorted by UTF-16BE bytes, each length-prefixed.
fn filter_list(values: Option<&[String]>) -> String {
    let mut sorted: Vec<String> = values
        .unwrap_or_default()
        .iter()
        .map(|value| normalized(value))
        .collect();
    sorted.sort_by_cached_key(|value| java_string_key(value));
    sorted.iter().map(|value| field(value)).collect()
}

fn flag(value: Option<bool>) -> &'static str {
    match value {
        None => "null",
        Some(true) => "true",
        Some(false) => "false",
    }
}

/// Base64 with optional correct `=` padding, canonical or not.
fn lenient_base64(value: &str) -> Option<Vec<u8>> {
    let body = value.trim_end_matches('=');
    let padding = value.len() - body.len();
    if padding > 2
        || !body
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return None;
    }
    let remainder = body.len() % 4;
    if remainder == 1
        || (padding > 0
            && (remainder == 0 || padding != 4 - remainder || !value.len().is_multiple_of(4)))
    {
        return None;
    }
    let engine = GeneralPurpose::new(
        &alphabet::URL_SAFE,
        GeneralPurposeConfig::new()
            .with_encode_padding(false)
            .with_decode_padding_mode(DecodePaddingMode::RequireNone)
            .with_decode_allow_trailing_bits(true),
    );
    engine.decode(body).ok()
}

/// The value of a pinned decimal digit (ASCII, fullwidth and the other Unicode blocks).
fn digit(character: char) -> Option<i64> {
    let code = u32::from(character);
    DIGIT_BLOCKS.iter().find_map(|&start| {
        let start = u32::from(start);
        (start..start + 10)
            .contains(&code)
            .then(|| i64::from(code - start))
    })
}

/// Optional sign, pinned decimal digits, at most `+/-2^31`.
fn integer(value: &str) -> Option<i64> {
    let negative = value.starts_with('-');
    let digits = value.strip_prefix(['-', '+']).unwrap_or(value);
    if digits.is_empty() {
        return None;
    }
    let maximum: i64 = if negative { 1 << 31 } else { (1 << 31) - 1 };
    let mut number: i64 = 0;
    for character in digits.chars() {
        number = number * 10 + digit(character)?;
        if number > maximum {
            return None;
        }
    }
    Some(if negative { -number } else { number })
}

/// Reads and writes scan cursors for one [`CursorKey`].
pub struct CursorAuthority {
    key: CursorKey,
}

impl fmt::Debug for CursorAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CursorAuthority").finish_non_exhaustive()
    }
}

impl CursorAuthority {
    /// An authority over `key`.
    pub fn new(key: CursorKey) -> Self {
        Self { key }
    }

    fn mac(&self, payload: &[u8]) -> Option<Hmac> {
        let mut mac = <Hmac as KeyInit>::new_from_slice(&self.key.0).ok()?;
        mac.update(payload);
        Some(mac)
    }

    /// HMAC-SHA256 of `payload`; `None` only if the HMAC could not be built (never for a valid key).
    fn digest(&self, payload: &str) -> Option<Vec<u8>> {
        Some(
            self.mac(payload.as_bytes())?
                .finalize()
                .into_bytes()
                .to_vec(),
        )
    }

    /// The 64 lowercase hex digit fingerprint of a scan scope.
    pub fn fingerprint(&self, scope: &Scope<'_>) -> Option<String> {
        let fields = [
            terms(scope.query).join(" "),
            scope.location.map(normalized).unwrap_or_default(),
            flag(scope.remote_only).to_owned(),
            flag(scope.visa_sponsorship).to_owned(),
            filter_list(scope.tags),
            filter_list(scope.job_types),
        ];
        let text: String = fields.iter().map(|value| field(value)).collect();
        let digest = self.digest(&text)?;
        Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    /// Encode a cursor for `page` and `offset` under `fingerprint`.
    pub fn encode(&self, page: u32, offset: usize, fingerprint: &str) -> Option<String> {
        let payload = format!("{page}:{offset}:{fingerprint}");
        let signature = URL_SAFE_NO_PAD.encode(self.digest(&payload)?);
        Some(URL_SAFE_NO_PAD.encode(format!("{payload}:{signature}")))
    }

    /// Decode a cursor: `(page, offset)` when it is well formed, authentic (constant-time HMAC
    /// compare), in bounds and bound to `fingerprint`; `None` for every failure.
    pub fn decode(&self, token: &str, fingerprint: &str) -> Option<(u32, usize)> {
        let bytes = lenient_base64(token)?;
        let text = String::from_utf8_lossy(&bytes);
        let parts: Vec<&str> = text.split(':').collect();
        if parts.len() != 4 {
            return None;
        }
        let page = integer(parts[0])?;
        let offset = integer(parts[1])?;
        let supplied = lenient_base64(parts[3])?;
        let signed = parts[..3].join(":");
        let verified = self.mac(signed.as_bytes())?.verify_slice(&supplied).is_ok();
        if !(1..=MAX_CURSOR_PAGE).contains(&page)
            || !(0..=MAX_CURSOR_OFFSET).contains(&offset)
            || !verified
            || parts[2] != fingerprint
        {
            return None;
        }
        Some((u32::try_from(page).ok()?, usize::try_from(offset).ok()?))
    }
}
