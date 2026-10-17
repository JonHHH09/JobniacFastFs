//! Arbeitnow scan cursors and fingerprints: replays the fingerprint and cursor vectors of
//! `golden/arbeitnow-scan.json` (TEST-ONLY keys, synthetic scopes), recomputes fingerprints with
//! an independent HMAC, and checks tamper, alias, bounds and key separation.
//!
//! The decoder accepts alias spellings of a cursor (correct `=` padding, non-zero trailing bits,
//! leading zeros, sign, Unicode decimal digits); each still needs a genuine HMAC. Emitted cursors
//! are canonical.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{KeyInit, Mac, SimpleHmac};
use jobniac_core::arbeitnow::cursor::{
    CursorAuthority, CursorKey, MIN_KEY_BYTES, Scope, normalized, terms,
};
use jobniac_text::java::utf16_length;
use serde_json::Value;
use sha2::Sha256;

const GOLDEN: &str = include_str!("golden/arbeitnow-scan.json");

type Hmac = SimpleHmac<Sha256>;

fn cursor_key() -> Vec<u8> {
    (64u8..96).collect()
}

fn candidate_key() -> Vec<u8> {
    (0u8..32).collect()
}

fn authority(key: &[u8]) -> CursorAuthority {
    CursorAuthority::new(CursorKey::new(key).unwrap())
}

fn mac(key: &[u8], payload: &str) -> Vec<u8> {
    let mut mac = <Hmac as KeyInit>::new_from_slice(key).unwrap();
    mac.update(payload.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct Owned {
    query: Option<String>,
    location: Option<String>,
    remote_only: Option<bool>,
    visa_sponsorship: Option<bool>,
    tags: Option<Vec<String>>,
    job_types: Option<Vec<String>>,
}

impl Owned {
    fn none() -> Self {
        Self {
            query: None,
            location: None,
            remote_only: None,
            visa_sponsorship: None,
            tags: None,
            job_types: None,
        }
    }

    fn from_json(raw: &Value) -> Self {
        let list = |key: &str| {
            raw[key].as_array().map(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
            })
        };
        Self {
            query: raw["query"].as_str().map(str::to_owned),
            location: raw["location"].as_str().map(str::to_owned),
            remote_only: raw["remoteOnly"].as_bool(),
            visa_sponsorship: raw["visaSponsorship"].as_bool(),
            tags: list("tags"),
            job_types: list("jobTypes"),
        }
    }

    fn scope(&self) -> Scope<'_> {
        Scope {
            query: self.query.as_deref(),
            location: self.location.as_deref(),
            remote_only: self.remote_only,
            visa_sponsorship: self.visa_sponsorship,
            tags: self.tags.as_deref(),
            job_types: self.job_types.as_deref(),
        }
    }

    fn fingerprint(&self, authority: &CursorAuthority) -> String {
        authority.fingerprint(&self.scope()).unwrap()
    }
}

/// The documented fingerprint, computed here without the code under test: UTF-16 length prefixes
/// and UTF-16 code unit order for the filter lists.
fn reference_fingerprint(key: &[u8], fields: [&str; 6]) -> String {
    let text: String = fields
        .iter()
        .map(|value| format!("{}:{value}", utf16_length(value)))
        .collect();
    hex(&mac(key, &text))
}

fn list_field(values: &[&str]) -> String {
    let mut sorted: Vec<String> = values.iter().map(|value| value.to_lowercase()).collect();
    sorted.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
    sorted
        .iter()
        .map(|value| format!("{}:{value}", utf16_length(value)))
        .collect()
}

#[test]
fn golden_fingerprints_replay() {
    let document: Value = serde_json::from_str(GOLDEN).unwrap();
    let authority = authority(&cursor_key());
    let vectors = document["fingerprints"].as_array().unwrap();
    assert!(vectors.len() >= 30);
    for vector in vectors {
        let owned = Owned::from_json(&vector["request"]);
        assert_eq!(
            owned.fingerprint(&authority),
            vector["fingerprint"].as_str().unwrap(),
            "fingerprint {}",
            vector["id"]
        );
    }
}

#[test]
fn golden_cursors_replay_and_round_trip() {
    let document: Value = serde_json::from_str(GOLDEN).unwrap();
    let authority = authority(&cursor_key());
    for vector in document["cursors"].as_array().unwrap() {
        let owned = Owned::from_json(&vector["request"]);
        let fingerprint = owned.fingerprint(&authority);
        let page = u32::try_from(vector["page"].as_u64().unwrap()).unwrap();
        let offset = usize::try_from(vector["offset"].as_u64().unwrap()).unwrap();
        let expected = vector["cursor"].as_str().unwrap();
        let cursor = authority.encode(page, offset, &fingerprint).unwrap();
        assert_eq!(cursor, expected, "cursor {}", vector["id"]);
        let in_bounds = (1..=1_000_000).contains(&page) && offset <= 10_000;
        let decoded = authority.decode(&cursor, &fingerprint);
        assert_eq!(
            decoded,
            in_bounds.then_some((page, offset)),
            "{}",
            vector["id"]
        );
    }
}

#[test]
fn fingerprint_matches_the_documented_construction() {
    let authority = authority(&cursor_key());
    let key = cursor_key();
    let owned = Owned {
        query: Some("  Rust   ENGINEER ".to_owned()),
        location: Some(" Berlin ".to_owned()),
        remote_only: Some(true),
        visa_sponsorship: Some(false),
        tags: Some(vec!["B".to_owned(), " a ".to_owned()]),
        job_types: Some(vec!["Full Time".to_owned()]),
    };
    let expected = reference_fingerprint(
        &key,
        [
            "rust engineer",
            "berlin",
            "true",
            "false",
            &list_field(&["b", "a"]),
            &list_field(&["full time"]),
        ],
    );
    assert_eq!(owned.fingerprint(&authority), expected);
    let empty = reference_fingerprint(&key, ["", "", "null", "null", "", ""]);
    assert_eq!(Owned::none().fingerprint(&authority), empty);
}

#[test]
fn fingerprint_sorts_filter_values_by_utf16_not_code_point() {
    // U+E000 sorts before U+1F600 by code point but after it by UTF-16 code units (D83D < E000).
    let authority = authority(&cursor_key());
    let key = cursor_key();
    for order in [["\u{E000}", "\u{1F600}"], ["\u{1F600}", "\u{E000}"]] {
        let owned = Owned {
            tags: Some(order.iter().map(|value| (*value).to_owned()).collect()),
            ..Owned::none()
        };
        let by_units = format!("{}{}", "2:\u{1F600}", "1:\u{E000}");
        let expected = reference_fingerprint(&key, ["", "", "null", "null", &by_units, ""]);
        assert_eq!(owned.fingerprint(&authority), expected);
        let by_code_point = format!("{}{}", "1:\u{E000}", "2:\u{1F600}");
        let wrong = reference_fingerprint(&key, ["", "", "null", "null", &by_code_point, ""]);
        assert_ne!(owned.fingerprint(&authority), wrong);
    }
}

#[test]
fn fingerprint_depends_on_each_field_but_not_on_order_or_case() {
    let authority = authority(&cursor_key());
    let base = Owned::none().fingerprint(&authority);
    let variants = [
        Owned {
            query: Some("a".to_owned()),
            ..Owned::none()
        },
        Owned {
            location: Some("a".to_owned()),
            ..Owned::none()
        },
        Owned {
            remote_only: Some(true),
            ..Owned::none()
        },
        Owned {
            remote_only: Some(false),
            ..Owned::none()
        },
        Owned {
            visa_sponsorship: Some(true),
            ..Owned::none()
        },
        Owned {
            visa_sponsorship: Some(false),
            ..Owned::none()
        },
        Owned {
            tags: Some(vec!["a".to_owned()]),
            ..Owned::none()
        },
        Owned {
            job_types: Some(vec!["a".to_owned()]),
            ..Owned::none()
        },
    ];
    let mut seen = vec![base.clone()];
    for variant in &variants {
        let value = variant.fingerprint(&authority);
        assert_eq!(value.len(), 64);
        assert!(
            value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert!(!seen.contains(&value));
        seen.push(value);
    }
    let ab = Owned {
        tags: Some(vec!["a".to_owned(), "b".to_owned()]),
        ..Owned::none()
    };
    let ba = Owned {
        tags: Some(vec![" B ".to_owned(), "A".to_owned()]),
        ..Owned::none()
    };
    assert_eq!(ab.fingerprint(&authority), ba.fingerprint(&authority));
    // A blank location counts like an empty one; a missing one counts as empty too.
    let blank = Owned {
        location: Some("   ".to_owned()),
        ..Owned::none()
    };
    assert_eq!(blank.fingerprint(&authority), base);
    // The length prefix keeps neighbouring fields apart.
    let left = Owned {
        query: Some("1:a".to_owned()),
        location: Some("b".to_owned()),
        ..Owned::none()
    };
    let right = Owned {
        query: Some("1".to_owned()),
        location: Some("a:b".to_owned()),
        ..Owned::none()
    };
    assert_ne!(left.fingerprint(&authority), right.fingerprint(&authority));
}

#[test]
fn terms_follow_the_documented_split() {
    assert_eq!(terms(None), Vec::<String>::new());
    assert_eq!(terms(Some("   ")), Vec::<String>::new());
    assert_eq!(terms(Some("  Rust   Go\t")), vec!["rust", "go"]);
    assert_eq!(terms(Some("a\u{a0}b")), vec!["a\u{a0}b"]);
    assert_eq!(terms(Some("A\u{b}b\u{c}c")), vec!["a", "b", "c"]);
    assert_eq!(normalized("  ABC "), "abc");
}

fn signed(fingerprint: &str, page: &str, offset: &str, key: &[u8]) -> (String, String) {
    let payload = format!("{page}:{offset}:{fingerprint}");
    let signature = URL_SAFE_NO_PAD.encode(mac(key, &payload));
    (payload, signature)
}

fn outer(payload: &str, signature: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{payload}:{signature}"))
}

#[test]
fn cursor_round_trip_and_canonical_emission() {
    let authority = authority(&cursor_key());
    let fingerprint = Owned::none().fingerprint(&authority);
    let cursor = authority.encode(3, 17, &fingerprint).unwrap();
    assert!(!cursor.contains('=') && !cursor.contains('+') && !cursor.contains('/'));
    let inner = String::from_utf8(URL_SAFE_NO_PAD.decode(&cursor).unwrap()).unwrap();
    let parts: Vec<&str> = inner.split(':').collect();
    assert_eq!(&parts[..3], ["3", "17", fingerprint.as_str()]);
    let (_, signature) = signed(&fingerprint, "3", "17", &cursor_key());
    assert_eq!(parts[3], signature);
    assert_eq!(authority.decode(&cursor, &fingerprint), Some((3, 17)));
    assert_eq!(authority.encode(3, 17, &fingerprint).unwrap(), cursor);
}

#[test]
fn cursor_bounds() {
    let authority = authority(&cursor_key());
    let fingerprint = Owned::none().fingerprint(&authority);
    let key = cursor_key();
    let decode = |page: &str, offset: &str| {
        let (payload, signature) = signed(&fingerprint, page, offset, &key);
        authority.decode(&outer(&payload, &signature), &fingerprint)
    };
    assert_eq!(decode("1", "0"), Some((1, 0)));
    assert_eq!(decode("1000000", "10000"), Some((1_000_000, 10_000)));
    for (page, offset) in [
        ("0", "0"),
        ("-1", "0"),
        ("1000001", "0"),
        ("1", "10001"),
        ("1", "-1"),
        ("2147483648", "0"),
        ("1", "2147483648"),
        ("", "0"),
        ("1", ""),
        ("x", "0"),
        ("1.0", "0"),
        ("1e3", "0"),
        (" 1", "0"),
    ] {
        assert_eq!(decode(page, offset), None, "{page}:{offset}");
    }
}

#[test]
fn cursor_aliases_follow_the_golden_vectors() {
    let authority = authority(&cursor_key());
    let fingerprint = Owned::none().fingerprint(&authority);
    let key = cursor_key();
    let decode = |page: &str, offset: &str| {
        let (payload, signature) = signed(&fingerprint, page, offset, &key);
        authority.decode(&outer(&payload, &signature), &fingerprint)
    };
    assert_eq!(decode("0001", "0000"), Some((1, 0)));
    assert_eq!(decode("+1", "+1"), Some((1, 1)));
    assert_eq!(decode("1", "-0"), Some((1, 0)));
    assert_eq!(decode("\u{661}", "\u{660}"), Some((1, 0)));
    assert_eq!(decode("\u{ff11}", "\u{ff10}"), Some((1, 0)));
    assert_eq!(
        decode(&format!("{}1", "0".repeat(100_000)), "0"),
        Some((1, 0))
    );

    // Correct padding of either part is accepted, wrong padding is not.
    let (payload, signature) = signed(&fingerprint, "1", "1", &key);
    let pad = |text: &str| format!("{text}{}", "=".repeat((4 - text.len() % 4) % 4));
    let padded_signature = pad(&signature);
    let both = URL_SAFE_NO_PAD.encode(format!("{payload}:{padded_signature}"));
    assert_eq!(authority.decode(&both, &fingerprint), Some((1, 1)));
    let padded_outer = pad(&outer(&payload, &signature));
    assert_eq!(authority.decode(&padded_outer, &fingerprint), Some((1, 1)));
    let canonical = outer(&payload, &signature);
    let needed = (4 - canonical.len() % 4) % 4;
    for count in 1..=3usize {
        let decoded = authority.decode(&format!("{canonical}{}", "=".repeat(count)), &fingerprint);
        assert_eq!(
            decoded.is_some(),
            count == needed,
            "{count} padding characters"
        );
    }
    // Non-zero trailing bits in the outer token decode to the same bytes and are accepted.
    let mut zeros = 0;
    let canonical = loop {
        let (payload, signature) =
            signed(&fingerprint, &format!("{}1", "0".repeat(zeros)), "0", &key);
        let token = outer(&payload, &signature);
        if token.len() % 4 >= 2 {
            break token;
        }
        zeros += 1;
    };
    let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let last = canonical.chars().last().unwrap();
    let alias_last = alphabet
        .chars()
        .nth(alphabet.find(last).unwrap() ^ 1)
        .unwrap();
    let alias = format!("{}{alias_last}", &canonical[..canonical.len() - 1]);
    assert_ne!(alias, canonical);
    assert_eq!(authority.decode(&canonical, &fingerprint), Some((1, 0)));
    assert_eq!(authority.decode(&alias, &fingerprint), Some((1, 0)));
}

#[test]
fn cursor_rejects_malformed_input() {
    let authority = authority(&cursor_key());
    let fingerprint = Owned::none().fingerprint(&authority);
    let good = authority.encode(1, 1, &fingerprint).unwrap();
    let inner = String::from_utf8(URL_SAFE_NO_PAD.decode(&good).unwrap()).unwrap();
    let parts: Vec<&str> = inner.split(':').collect();
    for bad in [
        String::new(),
        "!!!".to_owned(),
        "A".to_owned(),
        format!("+{good}"),
        format!("{good}!"),
        format!(" {good}"),
        format!("{good}\n"),
        URL_SAFE_NO_PAD.encode(parts[..3].join(":")),
        URL_SAFE_NO_PAD.encode(format!("{inner}:x")),
        URL_SAFE_NO_PAD.encode(b"\xff\xfe:1:2:3"),
        URL_SAFE_NO_PAD.encode(format!("{}:{}:{}:", parts[0], parts[1], parts[2])),
    ] {
        assert_eq!(authority.decode(&bad, &fingerprint), None, "{bad:?}");
    }
    // A cursor for another scope fails the fingerprint check even with a valid HMAC.
    let other = Owned {
        query: Some("rust".to_owned()),
        ..Owned::none()
    };
    let foreign_scope = other.fingerprint(&authority);
    assert_eq!(authority.decode(&good, &foreign_scope), None);
    let wrong = format!("1:1:{}", "0".repeat(64));
    let signature = URL_SAFE_NO_PAD.encode(mac(&cursor_key(), &wrong));
    assert_eq!(
        authority.decode(&outer(&wrong, &signature), &fingerprint),
        None
    );
}

#[test]
fn flipping_any_byte_of_a_cursor_is_refused() {
    let authority = authority(&cursor_key());
    let fingerprint = Owned::none().fingerprint(&authority);
    let good = authority.encode(1, 1, &fingerprint).unwrap();
    let bytes = URL_SAFE_NO_PAD.decode(&good).unwrap();
    // The last character of the inner signature has unused bits: changing them is the documented
    // trailing-bits alias and may still verify, so that byte only has to be refused or
    // decode to the genuine position.
    let alias_index = bytes.len() - 1;
    for index in 0..bytes.len() {
        for mask in [0x01u8, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80] {
            let mut tampered = bytes.clone();
            tampered[index] ^= mask;
            let token = URL_SAFE_NO_PAD.encode(&tampered);
            let decoded = authority.decode(&token, &fingerprint);
            if index == alias_index {
                assert!(decoded.is_none() || decoded == Some((1, 1)), "mask {mask}");
            } else {
                assert_eq!(decoded, None, "byte {index} mask {mask}");
            }
        }
    }
    // Truncation and extension are refused too.
    for length in 0..bytes.len() - 1 {
        assert_eq!(
            authority.decode(&URL_SAFE_NO_PAD.encode(&bytes[..length]), &fingerprint),
            None,
            "length {length}"
        );
    }
}

#[test]
fn cursor_key_separation_candidate_key_never_verifies_a_cursor() {
    assert!(cursor_key().len() >= MIN_KEY_BYTES);
    let cursor_side = authority(&cursor_key());
    let candidate_side = authority(&candidate_key());
    let fingerprint = Owned::none().fingerprint(&cursor_side);
    // A cursor made by the cursor key is refused by an authority over the candidate key.
    let cursor = cursor_side.encode(1, 0, &fingerprint).unwrap();
    assert_eq!(candidate_side.decode(&cursor, &fingerprint), None);
    // The fingerprint differs per key, so a cursor of the other authority cannot match either.
    let candidate_fingerprint = Owned::none().fingerprint(&candidate_side);
    assert_ne!(candidate_fingerprint, fingerprint);
    let candidate_cursor = candidate_side.encode(1, 0, &candidate_fingerprint).unwrap();
    assert_eq!(cursor_side.decode(&candidate_cursor, &fingerprint), None);
    assert_eq!(
        cursor_side.decode(&candidate_cursor, &candidate_fingerprint),
        None
    );
    // A cursor whose parts are all right but signed with the candidate key is refused.
    let (payload, signature) = signed(&fingerprint, "1", "0", &candidate_key());
    assert_eq!(
        cursor_side.decode(&outer(&payload, &signature), &fingerprint),
        None
    );
    // Another key of the same length is refused as well.
    let third = authority(&[9u8; 32]);
    assert_eq!(third.decode(&cursor, &fingerprint), None);
}

#[test]
fn key_checks() {
    assert!(CursorKey::new(&[]).is_err());
    assert!(CursorKey::new(&[1u8; 31]).is_err());
    assert!(CursorKey::new(&[1u8; 32]).is_ok());
    assert_eq!(
        format!("{:?}", CursorKey::new(&[1u8; 32]).unwrap()),
        "CursorKey { .. }"
    );
}
