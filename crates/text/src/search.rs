//! Search text normalization, tokens and query prefixes.
//!
//! [`normalize`] removes Unicode whitespace from both ends, lower-cases the text,
//! decomposes with NFKD and drops every mark (general category M). [`tokens`] splits the normalized
//! text on every character that is not an ASCII letter, digit, `+`, `#` or `.`, and keeps the first
//! occurrence of each token. The golden vectors live in `tests/golden/search.json`.

use std::collections::HashSet;
use std::sync::Arc;

use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::str_rules::strip_text;

/// Most characters a job search query may hold, counted in code points.
pub const MAX_QUERY_CHARACTERS: usize = 256;

/// Most tokens a job search query may hold.
pub const MAX_QUERY_TOKENS: usize = 16;

/// Normalizes text for search.
///
/// `None`, and text that is empty after stripping Unicode whitespace, give an empty string. Otherwise the
/// stripped text is lower-cased, decomposed with NFKD and stripped of every mark. The strip happens
/// before lower-casing and no whitespace is collapsed afterwards.
pub fn normalize(text: Option<&str>) -> String {
    let Some(text) = text else {
        return String::new();
    };
    let stripped = strip_text(text);
    if stripped.is_empty() {
        return String::new();
    }
    fold_segments(stripped)
}

/// Lower-cases, decomposes and drops the marks of `text` with Unicode 16.0.0 data.
///
/// The Rust standard library and `unicode-normalization` use Unicode 17.0, which gives the code
/// points in [`UNICODE_17_ONLY_CASED`] a lowercase mapping and, for U+A7F1, a compatibility
/// decomposition. Unicode 16.0.0 leaves all of them unchanged. The text is therefore processed in
/// the segments between those code points, and they are copied as they are. Unicode 16.0.0 treats
/// them as unassigned, which is neither cased nor case-ignorable, so final-sigma context does not
/// cross them, and the segment boundaries give the same context.
fn fold_segments(text: &str) -> String {
    apply_segments(text, fold_segment)
}

/// Lower-case (Unicode 16.0.0) of `text`: the segmented lower-case of [`fold_segments`]
/// without the NFKD step and without dropping marks.
///
/// Callers that canonicalize text for identity (the job input-text hash and fingerprint) need the
/// same lower-casing for the code points that Unicode 17.0 added a case mapping for, so they use
/// this instead of `str::to_lowercase`.
pub fn lower_text(text: &str) -> String {
    apply_segments(text, |segment| segment.to_lowercase())
}

/// Applies `step` to the segments of `text` between the [`UNICODE_17_ONLY_CASED`] code points,
/// which are copied unchanged.
fn apply_segments(text: &str, step: impl Fn(&str) -> String) -> String {
    let mut output = String::with_capacity(text.len());
    let mut segment_start = 0;
    for (index, character) in text.char_indices() {
        if UNICODE_17_ONLY_CASED
            .binary_search(&u32::from(character))
            .is_ok()
        {
            output.push_str(&step(&text[segment_start..index]));
            output.push(character);
            segment_start = index + character.len_utf8();
        }
    }
    output.push_str(&step(&text[segment_start..]));
    output
}

/// Applies the normalization steps to one segment: lower-case, NFKD, drop marks.
fn fold_segment(segment: &str) -> String {
    segment
        .to_lowercase()
        .nfkd()
        .filter(|character| !is_mark(*character))
        .collect()
}

/// Code points with a lowercase mapping in Unicode 17.0 and none in Unicode 16.0.0 (unassigned
/// there), sorted for binary search: U+A7CE, U+A7D2, U+A7D4, U+A7F1 and U+16EA0 to U+16EB8.
const UNICODE_17_ONLY_CASED: [u32; 29] = [
    0xA7CE, 0xA7D2, 0xA7D4, 0xA7F1, 0x16EA0, 0x16EA1, 0x16EA2, 0x16EA3, 0x16EA4, 0x16EA5, 0x16EA6,
    0x16EA7, 0x16EA8, 0x16EA9, 0x16EAA, 0x16EAB, 0x16EAC, 0x16EAD, 0x16EAE, 0x16EAF, 0x16EB0,
    0x16EB1, 0x16EB2, 0x16EB3, 0x16EB4, 0x16EB5, 0x16EB6, 0x16EB7, 0x16EB8,
];

/// Returns the distinct search tokens of `text` in first-occurrence order.
///
/// Tokens are maximal runs of ASCII letters, digits, `+`, `#` and `.` in the normalized text, so
/// `c++`, `c#`, `node.js` and `.net` are single tokens. The dotless `i` (U+0131) is a separator.
pub fn tokens(text: Option<&str>) -> Vec<String> {
    let normalized = normalize(text);
    if strip_text(&normalized).is_empty() {
        return Vec::new();
    }
    let mut seen: HashSet<&str> = HashSet::new();
    let mut ordered = Vec::new();
    for token in normalized.split(|character: char| !is_token_character(character)) {
        if !token.is_empty() && seen.insert(token) {
            ordered.push(token.to_owned());
        }
    }
    ordered
}

/// Every code-point prefix of every query token, each paired with the token that owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryPrefixes {
    values: Vec<String>,
    owners: Vec<Arc<str>>,
}

impl QueryPrefixes {
    /// Pairs prefix values with their owners. Returns `None` when the two lists differ in length
    /// (a programming error).
    pub fn new(values: Vec<String>, owners: Vec<Arc<str>>) -> Option<Self> {
        (values.len() == owners.len()).then_some(Self { values, owners })
    }

    /// The prefix values, in token order and then by increasing length.
    pub fn values(&self) -> &[String] {
        &self.values
    }

    /// The token that owns each prefix, aligned with [`QueryPrefixes::values`]. Every prefix of
    /// one token shares a single allocation.
    pub fn owners(&self) -> &[Arc<str>] {
        &self.owners
    }
}

/// Builds every code-point prefix (length 1 to the token's length) of each query token.
///
/// Returns `None` when the list holds more than [`MAX_QUERY_TOKENS`] tokens. The profile search
/// validates the raw query length and the token count before it reaches this function. A token is
/// not bounded here: a normalized token may be longer than [`MAX_QUERY_CHARACTERS`] code points.
pub fn prefixes(query_tokens: &[String]) -> Option<QueryPrefixes> {
    if query_tokens.len() > MAX_QUERY_TOKENS {
        return None;
    }
    let mut values = Vec::new();
    let mut owners = Vec::new();
    for token in query_tokens {
        let owner: Arc<str> = Arc::from(token.as_str());
        for (start, character) in token.char_indices() {
            values.push(token[..start + character.len_utf8()].to_owned());
            owners.push(Arc::clone(&owner));
        }
    }
    Some(QueryPrefixes { values, owners })
}

/// One weighted posting of a token in the search index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchTerm {
    /// The entity the posting belongs to.
    pub entity_id: Uuid,
    /// The searched field of the entity.
    pub field_key: String,
    /// One distinct token of the field text.
    pub term: String,
    /// The weight of the field for this posting.
    pub weight: i64,
}

impl SearchTerm {
    /// Builds one posting per distinct token of `text`, in first-occurrence order.
    pub fn from_text(
        entity_id: Uuid,
        field_key: &str,
        text: Option<&str>,
        weight: i64,
    ) -> Vec<SearchTerm> {
        tokens(text)
            .into_iter()
            .map(|term| SearchTerm {
                entity_id,
                field_key: field_key.to_owned(),
                term,
                weight,
            })
            .collect()
    }
}

/// True for the general categories Mn, Mc and Me, which NFKD normalization leaves in place.
fn is_mark(character: char) -> bool {
    matches!(
        get_general_category(character),
        GeneralCategory::NonspacingMark
            | GeneralCategory::SpacingMark
            | GeneralCategory::EnclosingMark
    )
}

/// True for the characters a token may contain: ASCII letters, digits, `+`, `#` and `.`.
fn is_token_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '+' | '#' | '.')
}
