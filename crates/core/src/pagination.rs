//! Bounded, HMAC-signed opaque keyset cursors and page types.
//!
//! Wire format (preserved): `b64url(payload) + "." + b64url(HMAC-SHA256(key, payload))`, with
//! unpadded URL-safe Base64. The payload is UTF-8 `v1|scope|fingerprint|snapshotAt|createdAt|id`.
//! A cursor is valid only for the scope and filter fingerprint that issued it. The page size is not
//! part of the fingerprint, so a follow-up request may change `limit`.
//!
//! `snapshotAt` is a created-at high-watermark, not a commit-visibility snapshot: a row stamped
//! before page one but committed afterward can become visible on a later page.
//!
//! Decoding is strict: exactly two `.` parts, the URL-safe alphabet only, padding only in the
//! canonical count, the MAC is checked before the payload is parsed, and every payload failure is
//! the same `invalid_cursor` error. Non-canonical trailing bits decode to the same bytes, so they are accepted.

use std::fmt;

use base64::Engine as _;
use base64::alphabet;
use base64::engine::general_purpose::{GeneralPurpose, NO_PAD, URL_SAFE_NO_PAD};
use hmac::{KeyInit, Mac, SimpleHmac};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::clock::{format_instant, parse_instant};
use crate::env::{EnvValue, Environment};
use crate::errors::{AppError, validation_error};
use crate::ids::admit_uuid;

/// Page size used when the caller gives no limit.
pub const DEFAULT_LIMIT: usize = 20;
/// Largest accepted page size.
pub const MAX_LIMIT: usize = 100;
/// Environment variable that configures the cursor key.
pub const CURSOR_SECRET_ENV: &str = "JOBNIAC_CURSOR_HMAC_SECRET";

const VERSION: &str = "v1";
const MAX_CURSOR_LENGTH: usize = 4096;
const MIN_SECRET_LENGTH: usize = 32;
const KEY_LENGTH: usize = 32;
const INVALID_CURSOR_MESSAGE: &str = "Invalid pagination cursor";
const CURSOR_FIELD: &str = "cursor";
const CURSOR_REASON: &str = "malformed or does not match this request";
const INSTANT_MESSAGE: &str = "instant must be an ISO-8601 UTC timestamp";

/// Base64 decoder for the cursor envelope: the URL-safe alphabet without padding, which accepts
/// non-canonical trailing bits.
const TRAILING_BITS_TOLERANT: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    NO_PAD.with_decode_allow_trailing_bits(true),
);

/// The single caller-facing error for every rejected cursor.
pub fn invalid_cursor() -> AppError {
    AppError::from(validation_error(
        INVALID_CURSOR_MESSAGE,
        Some(CURSOR_FIELD),
        Some(CURSOR_REASON),
    ))
}

/// Why the cursor key could not be configured. The messages are fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CursorKeyError {
    /// A configured secret has fewer than 32 characters (counted before any trimming).
    #[error("JOBNIAC_CURSOR_HMAC_SECRET must contain at least 32 characters")]
    TooShort,
    /// The configured value is present but is not valid Unicode.
    #[error("JOBNIAC_CURSOR_HMAC_SECRET must be valid Unicode")]
    NotUnicode,
    /// The operating system could not supply random bytes for the per-process key.
    #[error("the random cursor key could not be generated")]
    Random,
}

/// The HMAC key. Its bytes are the UTF-8 of the configured secret, never base64-decoded.
/// `Debug` output is redacted.
pub struct CursorKey(Vec<u8>);

impl CursorKey {
    /// A key from a configured secret. The length check counts Unicode scalar values on the
    /// untrimmed string; the key bytes are the secret's UTF-8 exactly as configured.
    pub fn from_secret(secret: &str) -> Result<Self, CursorKeyError> {
        if secret.chars().count() < MIN_SECRET_LENGTH {
            return Err(CursorKeyError::TooShort);
        }
        Ok(Self(secret.as_bytes().to_vec()))
    }

    /// A fresh random 32-byte key for this process. Cursors then do not survive a restart.
    pub fn random() -> Result<Self, CursorKeyError> {
        let mut bytes = vec![0_u8; KEY_LENGTH];
        getrandom::fill(&mut bytes).map_err(|_| CursorKeyError::Random)?;
        Ok(Self(bytes))
    }

    /// The key for an environment snapshot. An absent, empty or
    /// whitespace-only value gives a random per-process key.
    pub fn from_environment(environment: &Environment) -> Result<Self, CursorKeyError> {
        match environment.lookup(CURSOR_SECRET_ENV) {
            EnvValue::Missing => Self::random(),
            EnvValue::Invalid => Err(CursorKeyError::NotUnicode),
            EnvValue::Valid(value) if is_blank_text(value) => Self::random(),
            EnvValue::Valid(value) => Self::from_secret(value),
        }
    }
}

impl fmt::Debug for CursorKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CursorKey(<redacted>)")
    }
}

/// A decoded cursor position.
#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    /// The scope the cursor was issued for.
    pub scope: String,
    /// Lowercase hexadecimal SHA-256 of the filter identity.
    pub fingerprint: String,
    /// High-watermark of the created-at column when the first page was read.
    pub snapshot_at: OffsetDateTime,
    /// Created-at value of the last item on the previous page.
    pub created_at: OffsetDateTime,
    /// Identifier of the last item on the previous page.
    pub id: Uuid,
}

/// One page of items and the cursor for the next page, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// At most `limit` items, in repository order.
    pub items: Vec<T>,
    /// Present only when more rows exist and at least one item was returned.
    pub next_cursor: Option<String>,
}

/// Bounded search candidates; `matched_count` is -1 when the total is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchCandidates<T> {
    matched_count: i64,
    has_more: bool,
    items: Vec<T>,
}

impl<T> SearchCandidates<T> {
    /// Build candidates, enforcing these invariants: `matched_count` is at least -1, and when
    /// it is non-negative it bounds the number of items.
    pub fn new(matched_count: i64, has_more: bool, items: Vec<T>) -> Result<Self, AppError> {
        if matched_count < -1 {
            return Err(AppError::DomainValue(
                "matchedCount must be -1 or non-negative".to_owned(),
            ));
        }
        if usize::try_from(matched_count).is_ok_and(|total| items.len() > total) {
            return Err(AppError::DomainValue(
                "items must not exceed matchedCount".to_owned(),
            ));
        }
        Ok(Self {
            matched_count,
            has_more,
            items,
        })
    }

    /// The total match count, or -1 when unknown.
    pub fn matched_count(&self) -> i64 {
        self.matched_count
    }

    /// Whether more candidates exist beyond these items.
    pub fn has_more(&self) -> bool {
        self.has_more
    }

    /// The candidate items.
    pub fn items(&self) -> &[T] {
        &self.items
    }
}

/// The validated parameters of one page request. It borrows the codec
/// that validated it, so it can only sign cursors with that key.
#[derive(Debug)]
pub struct PageRequest<'a> {
    limit: usize,
    cursor: Option<Cursor>,
    scope: String,
    fingerprint: String,
    codec: &'a CursorCodec,
}

impl PageRequest<'_> {
    /// The resolved page size, from 1 to [`MAX_LIMIT`].
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// The decoded position of the request, or `None` for the first page.
    pub fn cursor(&self) -> Option<&Cursor> {
        self.cursor.as_ref()
    }

    /// The scope the request was validated for.
    pub fn scope(&self) -> &str {
        &self.scope
    }

    /// The filter fingerprint the request was validated for.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// The cursor for the page that follows an item at this position.
    pub fn next_cursor(
        &self,
        snapshot_at: OffsetDateTime,
        created_at: OffsetDateTime,
        item_id: Uuid,
    ) -> Result<String, AppError> {
        self.codec.next_cursor(
            &self.scope,
            &self.fingerprint,
            snapshot_at,
            created_at,
            item_id,
        )
    }
}

/// Signs and verifies cursors with one key.
pub struct CursorCodec {
    key: CursorKey,
}

impl fmt::Debug for CursorCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CursorCodec")
            .field("key", &self.key)
            .finish()
    }
}

impl CursorCodec {
    /// A codec that signs and verifies with `key`.
    pub fn new(key: CursorKey) -> Self {
        Self { key }
    }

    /// Decode `cursor`, then validate `limit`, then `scope`, then compare the scope and fingerprint
    /// of the decoded cursor. A bad cursor therefore wins over a bad limit. `limit` of `None` means
    /// [`DEFAULT_LIMIT`].
    pub fn page_request(
        &self,
        limit: Option<i64>,
        cursor: Option<&str>,
        scope: &str,
        filter_identity: &str,
    ) -> Result<PageRequest<'_>, AppError> {
        let fingerprint = fingerprint_of(filter_identity);
        let decoded = self.decode(cursor)?;
        let limit = match limit {
            None => DEFAULT_LIMIT,
            Some(value) => usize::try_from(value)
                .ok()
                .filter(|resolved| (1..=MAX_LIMIT).contains(resolved))
                .ok_or_else(|| {
                    AppError::DomainValue(format!("limit must be between 1 and {MAX_LIMIT}"))
                })?,
        };
        require_value(scope, "scope")?;
        if let Some(cursor) = &decoded
            && (cursor.scope != scope || cursor.fingerprint != fingerprint)
        {
            return Err(invalid_cursor());
        }
        Ok(PageRequest {
            limit,
            cursor: decoded,
            scope: scope.to_owned(),
            fingerprint,
            codec: self,
        })
    }

    /// Sign the position of an item. The inputs are not validated, so a
    /// scope that contains `|` produces a cursor that [`CursorCodec::decode`] later refuses.
    pub fn next_cursor(
        &self,
        scope: &str,
        fingerprint: &str,
        snapshot_at: OffsetDateTime,
        created_at: OffsetDateTime,
        item_id: Uuid,
    ) -> Result<String, AppError> {
        check_instant_range(snapshot_at)?;
        check_instant_range(created_at)?;
        let snapshot = format_instant(snapshot_at);
        let created = format_instant(created_at);
        let item = item_id.to_string();
        let fields: [&str; 6] = [VERSION, scope, fingerprint, &snapshot, &created, &item];
        let payload = fields.join("|");
        let signature = self.sign(payload.as_bytes())?;
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(payload.as_bytes()),
            URL_SAFE_NO_PAD.encode(signature)
        ))
    }

    /// Verify and decode a cursor. `None` gives `Ok(None)` (first page). Every failure is the same
    /// [`invalid_cursor`] error.
    pub fn decode(&self, encoded: Option<&str>) -> Result<Option<Cursor>, AppError> {
        let Some(encoded) = encoded else {
            return Ok(None);
        };
        if encoded.chars().count() > MAX_CURSOR_LENGTH {
            return Err(invalid_cursor());
        }
        let mut envelope = encoded.split('.');
        let (Some(payload_part), Some(signature_part), None) =
            (envelope.next(), envelope.next(), envelope.next())
        else {
            return Err(invalid_cursor());
        };
        let payload = decode_base64url(payload_part).ok_or_else(invalid_cursor)?;
        let signature = decode_base64url(signature_part).ok_or_else(invalid_cursor)?;
        if !self.verify(&payload, &signature)? {
            return Err(invalid_cursor());
        }
        parse_payload(&payload).map(Some).ok_or_else(invalid_cursor)
    }

    fn mac(&self, payload: &[u8]) -> Result<SimpleHmac<Sha256>, AppError> {
        let mut mac = <SimpleHmac<Sha256> as KeyInit>::new_from_slice(&self.key.0)
            .map_err(|_| AppError::Internal)?;
        mac.update(payload);
        Ok(mac)
    }

    fn sign(&self, payload: &[u8]) -> Result<Vec<u8>, AppError> {
        Ok(self.mac(payload)?.finalize().into_bytes().to_vec())
    }

    fn verify(&self, payload: &[u8], signature: &[u8]) -> Result<bool, AppError> {
        Ok(self.mac(payload)?.verify_slice(signature).is_ok())
    }
}

/// Build a page from `limit + 1` rows already ordered by the repository's keyset policy.
/// `items` is the first `limit` rows. No next cursor is issued when the rows fit or
/// no item is returned; otherwise the cursor carries the request's snapshot and the last item's
/// created-at and identifier.
pub fn keyset_page<T>(
    request: &PageRequest<'_>,
    rows: Vec<T>,
    snapshot_at: OffsetDateTime,
    created_at_of: impl Fn(&T) -> OffsetDateTime,
    id_of: impl Fn(&T) -> Uuid,
) -> Result<Page<T>, AppError> {
    let total = rows.len();
    let items: Vec<T> = rows.into_iter().take(request.limit()).collect();
    let next_cursor = match items.last() {
        Some(last) if total > request.limit() => {
            Some(request.next_cursor(snapshot_at, created_at_of(last), id_of(last))?)
        }
        _ => None,
    };
    Ok(Page { items, next_cursor })
}

fn parse_payload(payload: &[u8]) -> Option<Cursor> {
    let text = std::str::from_utf8(payload).ok()?;
    let parts: Vec<&str> = text.split('|').collect();
    let [version, scope, fingerprint, snapshot, created, item] = parts.as_slice() else {
        return None;
    };
    if *version != VERSION {
        return None;
    }
    Some(Cursor {
        scope: require_cursor_value(scope)?.to_owned(),
        fingerprint: require_cursor_value(fingerprint)?.to_owned(),
        snapshot_at: parse_instant(snapshot).ok()?,
        created_at: parse_instant(created).ok()?,
        id: admit_uuid(item).ok()?,
    })
}

/// Strict unpadded URL-safe Base64 decode: non-empty, alphabet only, padding
/// (if any) of the canonical count, and a length that is not one more than a multiple of four.
fn decode_base64url(text: &str) -> Option<Vec<u8>> {
    let unpadded = text.trim_end_matches('=');
    if unpadded.is_empty() || !unpadded.bytes().all(is_url_safe) {
        return None;
    }
    let padding = text.len() - unpadded.len();
    let canonical_padding = (4 - unpadded.len() % 4) % 4;
    if padding != 0 && padding != canonical_padding {
        return None;
    }
    if unpadded.len() % 4 == 1 {
        return None;
    }
    TRAILING_BITS_TOLERANT.decode(unpadded).ok()
}

fn is_url_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

fn fingerprint_of(identity: &str) -> String {
    Sha256::digest(identity.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Unix seconds of `0001-01-01T00:00:00Z`, the first representable cursor instant.
const MIN_INSTANT_UNIX_SECONDS: i64 = -62_135_596_800;
/// Unix seconds of `9999-12-31T23:59:59Z`, the last representable cursor instant.
const MAX_INSTANT_UNIX_SECONDS: i64 = 253_402_300_799;

/// Refuse an instant outside years 1 to 9999 (UTC): the decoder would refuse the cursor it signs. The
/// check uses whole UTC seconds, so no offset conversion can overflow the `time` range.
fn check_instant_range(instant: OffsetDateTime) -> Result<(), AppError> {
    let seconds = instant.unix_timestamp();
    if (MIN_INSTANT_UNIX_SECONDS..=MAX_INSTANT_UNIX_SECONDS).contains(&seconds) {
        Ok(())
    } else {
        Err(AppError::DomainValue(INSTANT_MESSAGE.to_owned()))
    }
}

/// A non-blank value without `|`, returned unchanged.
fn require_value(value: &str, name: &str) -> Result<(), AppError> {
    require_cursor_value(value).map(|_| ()).ok_or_else(|| {
        AppError::DomainValue(format!("{name} must be a non-blank cursor-safe value"))
    })
}

/// The same rule as [`require_value`] for a decoded payload field, where failure is `None`.
fn require_cursor_value(value: &str) -> Option<&str> {
    (!is_blank_text(value) && !value.contains('|')).then_some(value)
}

/// True when the text is empty after trimming Unicode whitespace.
fn is_blank_text(text: &str) -> bool {
    text.trim_matches(is_unicode_whitespace).is_empty()
}

fn is_unicode_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{09}'..='\u{0D}'
            | '\u{1C}'..='\u{20}'
            | '\u{85}'
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn codec() -> CursorCodec {
        let key = CursorKey::from_secret("unit-test-cursor-key-0123456789abcdef");
        CursorCodec::new(key.unwrap_or_else(|_| CursorKey(Vec::new())))
    }

    #[test]
    fn round_trips_and_binds_scope_and_fingerprint() {
        let codec = codec();
        let id = Uuid::from_u128(0x0f0e_0d0c_0b0a_4908_8706_0504_0302_0100);
        let encoded = codec
            .next_cursor(
                "jobs",
                "abc",
                datetime!(2026-09-22 19:26:28.717138 UTC),
                datetime!(2026-09-21 10:00 UTC),
                id,
            )
            .unwrap_or_default();
        let decoded = codec.decode(Some(&encoded)).ok().flatten();
        assert_eq!(decoded.as_ref().map(|c| c.scope.as_str()), Some("jobs"));
        assert_eq!(decoded.map(|c| c.id), Some(id));
        let other = codec.page_request(None, Some(&encoded), "profiles", "filters");
        assert_eq!(other.err(), Some(invalid_cursor()));
    }

    #[test]
    fn limit_and_scope_messages_are_fixed() {
        let codec = codec();
        let limit = codec.page_request(Some(101), None, "jobs", "f").err();
        assert_eq!(
            limit,
            Some(AppError::DomainValue(
                "limit must be between 1 and 100".into()
            ))
        );
        let scope = codec.page_request(None, None, " \u{3000} ", "f").err();
        assert_eq!(
            scope,
            Some(AppError::DomainValue(
                "scope must be a non-blank cursor-safe value".into()
            ))
        );
    }

    #[test]
    fn key_rules_are_enforced() {
        assert_eq!(
            CursorKey::from_secret(&"a".repeat(31)).err(),
            Some(CursorKeyError::TooShort)
        );
        assert!(CursorKey::from_secret(&"é".repeat(32)).is_ok());
        assert!(CursorKey::from_secret(&"é".repeat(31)).is_err());
        let blank = Environment::from_pairs([(CURSOR_SECRET_ENV, "   ")]);
        assert!(CursorKey::from_environment(&blank).is_ok());
        let invalid = Environment::default().with_invalid(CURSOR_SECRET_ENV);
        assert_eq!(
            CursorKey::from_environment(&invalid).err(),
            Some(CursorKeyError::NotUnicode)
        );
    }

    #[test]
    fn search_candidates_enforce_their_invariants() {
        assert!(SearchCandidates::new(-2, false, vec![1]).is_err());
        assert!(SearchCandidates::new(1, false, vec![1, 2]).is_err());
        assert!(SearchCandidates::new(-1, true, vec![1, 2, 3]).is_ok());
    }

    #[test]
    fn instants_outside_the_supported_range_are_refused_when_signing() {
        let codec = codec();
        let id = Uuid::from_u128(1);
        let refused = codec.next_cursor(
            "jobs",
            "abc",
            datetime!(0000-01-01 00:00 UTC),
            datetime!(2026-01-01 00:00 UTC),
            id,
        );
        assert_eq!(
            refused,
            Err(AppError::DomainValue(
                "instant must be an ISO-8601 UTC timestamp".into()
            ))
        );
        let just_before = codec.next_cursor(
            "jobs",
            "abc",
            datetime!(2026-01-01 00:00 UTC),
            datetime!(0000-12-31 23:59:59.5 UTC),
            id,
        );
        assert!(just_before.is_err());
        let overflow_offset = codec.next_cursor(
            "jobs",
            "abc",
            datetime!(9999-12-31 23:59:59 -01:00),
            datetime!(2026-01-01 00:00 UTC),
            id,
        );
        assert!(overflow_offset.is_err());
        let request = codec.page_request(Some(1), None, "jobs", "f");
        assert!(request.is_ok());
        if let Ok(request) = request {
            let keyset = keyset_page(
                &request,
                vec![id, id],
                datetime!(2026-01-01 00:00 UTC),
                |_| datetime!(0000-01-01 00:00 UTC),
                |row| *row,
            );
            assert!(keyset.is_err());
        }
    }

    #[test]
    fn the_supported_range_edges_round_trip() {
        let codec = codec();
        let id = Uuid::from_u128(2);
        for instant in [
            datetime!(0001-01-01 00:00 UTC),
            datetime!(9999-12-31 23:59:59.999999 UTC),
        ] {
            let encoded = codec
                .next_cursor("jobs", "abc", instant, instant, id)
                .unwrap_or_default();
            let decoded = codec.decode(Some(&encoded)).ok().flatten();
            assert_eq!(decoded.map(|cursor| cursor.snapshot_at), Some(instant));
        }
    }

    #[test]
    fn strict_base64_rejects_bad_padding_and_lengths() {
        assert!(decode_base64url("AAAA").is_some());
        assert!(decode_base64url("AA==").is_some());
        assert!(decode_base64url("AA=").is_none());
        assert!(decode_base64url("AAAAA").is_none());
        assert!(decode_base64url("AA+A").is_none());
        assert!(decode_base64url("").is_none());
    }
}
