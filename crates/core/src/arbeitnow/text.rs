//! Arbeitnow text preparation.
//!
//! One import point for the candidate and scan modules over the HTML 4 engine in
//! [`jobniac_text::html`]. No operation can fail: the types (`&str`, `usize`, slices of strings)
//! make non-string values and non-integer or negative caps unrepresentable, so the validation error
//! [`TEXT_PREPARATION_MESSAGE`] is never produced here. A `None` text is passed as the empty
//! string.

use jobniac_text::html;

/// Message of the validation error for a failed text preparation. Nothing fails here (see the
/// failing case (see the module documentation); the constant is kept for callers that map
/// foreign input to the same message.
pub const TEXT_PREPARATION_MESSAGE: &str = "Invalid Arbeitnow text preparation";

/// Cap of the plain description text, in code points.
pub const PLAIN_CAP: usize = 1000;

/// Cap of each entry of a bounded list, in code points.
pub const LIST_ENTRY_CAP: usize = 128;

/// Most non-blank entries a bounded list keeps.
pub const LIST_CAP: usize = 20;

/// Decodes HTML 4 character references once; malformed references stay literal. A numeric
/// reference to a surrogate becomes U+FFFD.
pub fn html4_unescape(value: &str) -> String {
    html::html4_unescape(Some(value))
}

/// Source preparation of description text: decode, remove `script`/`style` elements and tags,
/// replace controls, no-break spaces and U+FFFD with spaces, collapse whitespace and cap at
/// [`PLAIN_CAP`] code points.
pub fn plain(value: &str) -> String {
    html::plain(Some(value))
}

/// Normalizes controls and ASCII whitespace and caps at `cap` code points, without HTML decoding.
pub fn bound(value: &str, cap: usize) -> String {
    html::bound(Some(value), cap)
}

/// Applies [`plain`] (with its 1,000 code point cap) and then caps at `cap` code points.
pub fn plain_bound(value: &str, cap: usize) -> String {
    html::plain_bound(Some(value), cap)
}

/// Keeps the first [`LIST_CAP`] non-blank entries, each prepared like [`plain`] and capped at
/// [`LIST_ENTRY_CAP`] code points; order and duplicates are kept. `None` gives an empty list.
pub fn bound_list<S: AsRef<str>>(values: Option<&[S]>) -> Vec<String> {
    html::bound_list(values)
}
