//! Signed Arbeitnow candidate tokens.
//!
//! A token is `base64url(payload) "." base64url(HMAC-SHA256(key, payload))`, both unpadded and
//! canonical. The payload is the compact JSON of the candidate with the exact key order and
//! minimal string escaping (`"`, `\` and C0 controls only), so equal candidates yield equal bytes.
//! Verification authenticates the bounded original bytes before any JSON parser
//! runs, then validates the fields and the clock; every failure is the same
//! [`INVALID_MESSAGE`] validation error with no cause and no details.
//!
//! Rust strings cannot hold a lone surrogate, and the
//! strict JSON admission refuses an escaped one, so a signed token carrying `\ud800` is refused.

use std::fmt;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{KeyInit, Mac, SimpleHmac};
use jobniac_text::json::{JsonValue, StrictLimits, parse_strict};
use sha2::Sha256;

use crate::arbeitnow::instant::ArbeitnowInstant;
use crate::clock::Clock;
use crate::errors::{ApplicationError, ErrorCode, validation_error};

/// Largest signed payload, in bytes.
pub const MAX_PAYLOAD_BYTES: usize = 32 * 1024;
/// Largest token, in characters.
pub const MAX_TOKEN_CHARS: usize = 44_000;
/// Shortest accepted key, in bytes.
pub const MIN_KEY_BYTES: usize = 32;
/// Safe message of every token failure.
pub const INVALID_MESSAGE: &str = "Invalid Arbeitnow candidate token";
/// Safe message of a refused codec configuration.
pub const INVALID_CONFIGURATION_MESSAGE: &str = "Invalid Arbeitnow candidate codec configuration";

const PROVIDER: &str = "arbeitnow";
const CANONICAL_PREFIX: &str = "https://arbeitnow.com/view/";
const MAX_LIFETIME_SECONDS: i64 = 900;
const FUTURE_SKEW_SECONDS: i64 = 60;
const MAX_LIST_ENTRIES: usize = 20;
const FIELDS: [&str; 14] = [
    "v",
    "provider",
    "issuedAt",
    "expiresAt",
    "slug",
    "canonicalUrl",
    "company",
    "title",
    "location",
    "remote",
    "postedAt",
    "tags",
    "jobTypes",
    "descriptionExcerpt",
];

type Hmac = SimpleHmac<Sha256>;
type InstantSource = dyn Fn() -> Result<ArbeitnowInstant, ApplicationError> + Send + Sync;

/// A candidate job of the Arbeitnow board; the provider is always `arbeitnow`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArbeitnowCandidate {
    /// When the token was issued.
    pub issued_at: ArbeitnowInstant,
    /// When the token stops being valid (at most 900 seconds after issue).
    pub expires_at: ArbeitnowInstant,
    /// Board slug: ASCII alphanumeric segments joined by single hyphens.
    pub slug: String,
    /// Always `https://arbeitnow.com/view/` followed by the slug.
    pub canonical_url: String,
    /// Employer name (at most 256 code points).
    pub company: String,
    /// Job title (at most 256 code points).
    pub title: String,
    /// Location (at most 256 code points).
    pub location: String,
    /// Remote flag.
    pub remote: bool,
    /// Posting time, when the board gave one.
    pub posted_at: Option<ArbeitnowInstant>,
    /// Tags (at most 20 entries of at most 128 code points).
    pub tags: Vec<String>,
    /// Job types (at most 20 entries of at most 128 code points).
    pub job_types: Vec<String>,
    /// Description excerpt (at most 1000 code points).
    pub description_excerpt: String,
}

/// Issues and verifies candidate tokens with an owned key and an owned clock.
pub struct CandidateCodec {
    key: Vec<u8>,
    clock: Arc<InstantSource>,
}

impl fmt::Debug for CandidateCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CandidateCodec").finish_non_exhaustive()
    }
}

/// Why a candidate or token was refused, before it becomes a public error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Invalid,
    /// Instant arithmetic left the Java range (an internal error on issue).
    Overflow,
}

impl CandidateCodec {
    /// Build a codec over a system-style clock. The key must hold at least 32 bytes.
    pub fn new(key: &[u8], clock: Arc<dyn Clock>) -> Result<Self, ApplicationError> {
        Self::with_instant_source(key, move || {
            let now = clock.now();
            ArbeitnowInstant::new(now.unix_timestamp(), now.nanosecond())
                .map_err(|_| ApplicationError::of(ErrorCode::InternalError))
        })
    }

    /// Build a codec over a nanosecond instant source (the full Java range). The key must hold at
    /// least 32 bytes.
    pub fn with_instant_source<F>(key: &[u8], source: F) -> Result<Self, ApplicationError>
    where
        F: Fn() -> Result<ArbeitnowInstant, ApplicationError> + Send + Sync + 'static,
    {
        if key.len() < MIN_KEY_BYTES {
            return Err(validation_error(INVALID_CONFIGURATION_MESSAGE, None, None));
        }
        Ok(Self {
            key: key.to_vec(),
            clock: Arc::new(source),
        })
    }

    /// Read the clock once; a clock failure is never exposed, only an internal error.
    pub fn now(&self) -> Result<ArbeitnowInstant, ApplicationError> {
        (self.clock)().map_err(|_| ApplicationError::of(ErrorCode::InternalError))
    }

    /// Issue a signed token. A candidate that breaks the rules is a validation error; a failing
    /// clock or instant overflow is an internal error.
    pub fn issue(&self, candidate: &ArbeitnowCandidate) -> Result<String, ApplicationError> {
        match self.check_for_issue(candidate) {
            Ok(()) => {}
            Err(Refusal::Invalid) => return Err(invalid()),
            Err(Refusal::Overflow) => return Err(ApplicationError::of(ErrorCode::InternalError)),
        }
        let payload = write_payload(candidate);
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err(invalid());
        }
        let signature = self.sign(&payload).ok_or_else(invalid)?;
        let token = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(&payload),
            URL_SAFE_NO_PAD.encode(signature)
        );
        if token.len() > MAX_TOKEN_CHARS {
            return Err(invalid());
        }
        Ok(token)
    }

    /// Verify a token: bounds, base64, MAC, then strict JSON, field rules and the clock. Every
    /// failure is the same validation error.
    pub fn verify(&self, token: &str) -> Result<ArbeitnowCandidate, ApplicationError> {
        self.verify_inner(token).ok_or_else(invalid)
    }

    fn check_for_issue(&self, candidate: &ArbeitnowCandidate) -> Result<(), Refusal> {
        validate(candidate)?;
        let now = self.now().map_err(|_| Refusal::Overflow)?;
        let latest = now
            .checked_add_seconds(FUTURE_SKEW_SECONDS)
            .ok_or(Refusal::Overflow)?;
        if candidate.issued_at > latest {
            return Err(Refusal::Invalid);
        }
        Ok(())
    }

    fn mac(&self, payload: &[u8]) -> Option<Hmac> {
        let mut mac = <Hmac as KeyInit>::new_from_slice(&self.key).ok()?;
        mac.update(payload);
        Some(mac)
    }

    fn sign(&self, payload: &[u8]) -> Option<Vec<u8>> {
        Some(self.mac(payload)?.finalize().into_bytes().to_vec())
    }

    fn verify_inner(&self, token: &str) -> Option<ArbeitnowCandidate> {
        if token.chars().count() > MAX_TOKEN_CHARS || token.matches('.').count() != 1 {
            return None;
        }
        let (payload_part, signature_part) = token.split_once('.')?;
        let payload = decode_segment(payload_part, MAX_PAYLOAD_BYTES)?;
        let signature = decode_segment(signature_part, 64)?;
        if signature.len() != 32 || self.mac(&payload)?.verify_slice(&signature).is_err() {
            return None;
        }
        // Only authenticated bytes reach the parser.
        let checked = read_candidate(&payload)?;
        validate(&checked).ok()?;
        let now = self.now().ok()?;
        let latest = now.checked_add_seconds(FUTURE_SKEW_SECONDS)?;
        if checked.issued_at > latest || checked.expires_at < now {
            return None;
        }
        Some(checked)
    }
}

fn invalid() -> ApplicationError {
    validation_error(INVALID_MESSAGE, None, None)
}

/// No ISO control (C0, DEL, C1) and at most `maximum` code points.
fn text_ok(value: &str, maximum: usize) -> bool {
    let mut count = 0usize;
    for character in value.chars() {
        let code = u32::from(character);
        if code <= 0x1F || (0x7F..=0x9F).contains(&code) {
            return false;
        }
        count += 1;
        if count > maximum {
            return false;
        }
    }
    true
}

/// ASCII alphanumeric runs joined by single hyphens.
fn slug_ok(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

/// Validates the fields, in short-circuit order.
fn validate(candidate: &ArbeitnowCandidate) -> Result<(), Refusal> {
    if candidate.expires_at < candidate.issued_at {
        return Err(Refusal::Invalid);
    }
    let earliest = candidate
        .expires_at
        .checked_add_seconds(-MAX_LIFETIME_SECONDS)
        .ok_or(Refusal::Overflow)?;
    if earliest > candidate.issued_at {
        return Err(Refusal::Invalid);
    }
    let texts_ok = text_ok(&candidate.slug, 256)
        && slug_ok(&candidate.slug)
        && candidate.canonical_url.strip_prefix(CANONICAL_PREFIX) == Some(&candidate.slug)
        && text_ok(&candidate.company, 256)
        && text_ok(&candidate.title, 256)
        && text_ok(&candidate.location, 256)
        && text_ok(&candidate.description_excerpt, 1000);
    if !texts_ok {
        return Err(Refusal::Invalid);
    }
    for values in [&candidate.tags, &candidate.job_types] {
        if values.len() > MAX_LIST_ENTRIES || values.iter().any(|value| !text_ok(value, 128)) {
            return Err(Refusal::Invalid);
        }
    }
    Ok(())
}

/// Non-empty unpadded base64url, bounded, canonical (re-encodes identically).
fn decode_segment(value: &str, maximum: usize) -> Option<Vec<u8>> {
    if value.is_empty()
        || value.len() > (maximum * 4).div_ceil(3)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(value).ok()?;
    if decoded.len() > maximum || URL_SAFE_NO_PAD.encode(&decoded) != value {
        return None;
    }
    Some(decoded)
}

fn instant_of(value: &JsonValue) -> Option<ArbeitnowInstant> {
    ArbeitnowInstant::new(value.as_i64()?, 0).ok()
}

fn strings_of(value: &JsonValue) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|entry| entry.as_str().map(str::to_owned))
        .collect()
}

/// Strict UTF-8 (optional BOM), strict JSON, exact field set and types.
fn read_candidate(payload: &[u8]) -> Option<ArbeitnowCandidate> {
    let bytes = payload.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(payload);
    let text = std::str::from_utf8(bytes).ok()?;
    let document = parse_strict(text, StrictLimits::default()).ok()??;
    let members = document.as_object()?;
    if members.len() != FIELDS.len() || FIELDS.iter().any(|field| document.get(field).is_none()) {
        return None;
    }
    let field = |name: &str| document.get(name);
    if field("v")?.as_i64()? != 1 || field("provider")?.as_str()? != PROVIDER {
        return None;
    }
    let posted = field("postedAt")?;
    Some(ArbeitnowCandidate {
        issued_at: instant_of(field("issuedAt")?)?,
        expires_at: instant_of(field("expiresAt")?)?,
        slug: field("slug")?.as_str()?.to_owned(),
        canonical_url: field("canonicalUrl")?.as_str()?.to_owned(),
        company: field("company")?.as_str()?.to_owned(),
        title: field("title")?.as_str()?.to_owned(),
        location: field("location")?.as_str()?.to_owned(),
        remote: field("remote")?.as_bool()?,
        posted_at: if posted.is_null() {
            None
        } else {
            Some(instant_of(posted)?)
        },
        tags: strings_of(field("tags")?)?,
        job_types: strings_of(field("jobTypes")?)?,
        description_excerpt: field("descriptionExcerpt")?.as_str()?.to_owned(),
    })
}

/// String escaping: `"`, `\` and C0 controls only.
fn write_string(out: &mut Vec<u8>, value: &str) {
    out.push(b'"');
    let mut buffer = [0u8; 4];
    for character in value.chars() {
        match character {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            control if u32::from(control) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", u32::from(control)).as_bytes());
            }
            other => out.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes()),
        }
    }
    out.push(b'"');
}

fn write_list(out: &mut Vec<u8>, values: &[String]) {
    out.push(b'[');
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        write_string(out, value);
    }
    out.push(b']');
}

/// The compact payload in the fixed key order.
fn write_payload(candidate: &ArbeitnowCandidate) -> Vec<u8> {
    let mut out = Vec::with_capacity(512);
    out.extend_from_slice(b"{\"v\":1,\"provider\":\"arbeitnow\",\"issuedAt\":");
    out.extend_from_slice(candidate.issued_at.epoch_second().to_string().as_bytes());
    out.extend_from_slice(b",\"expiresAt\":");
    out.extend_from_slice(candidate.expires_at.epoch_second().to_string().as_bytes());
    for (key, value) in [
        ("slug", &candidate.slug),
        ("canonicalUrl", &candidate.canonical_url),
        ("company", &candidate.company),
        ("title", &candidate.title),
        ("location", &candidate.location),
    ] {
        out.extend_from_slice(format!(",\"{key}\":").as_bytes());
        write_string(&mut out, value);
    }
    out.extend_from_slice(if candidate.remote {
        b",\"remote\":true,\"postedAt\":"
    } else {
        b",\"remote\":false,\"postedAt\":"
    });
    match candidate.posted_at {
        Some(posted) => out.extend_from_slice(posted.epoch_second().to_string().as_bytes()),
        None => out.extend_from_slice(b"null"),
    }
    out.extend_from_slice(b",\"tags\":");
    write_list(&mut out, &candidate.tags);
    out.extend_from_slice(b",\"jobTypes\":");
    write_list(&mut out, &candidate.job_types);
    out.extend_from_slice(b",\"descriptionExcerpt\":");
    write_string(&mut out, &candidate.description_excerpt);
    out.push(b'}');
    out
}
