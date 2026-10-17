//! Sentence boundaries for resume bullet splitting.
//!
//! This module uses the default sentence rules of UAX #29 through the `unicode-segmentation`
//! crate, with no clean-room adjustments. The accepted differences against the OpenJDK
//! sentence iterator are pinned in
//! `crates/text/tests/golden/sentences-accepted-differences.json`.
//!
//! Offsets are code-point indices. Converting to UTF-16 units for the UI is a separate step.
//! No normalization and no trimming happen before segmentation.
//!
//! Callers that take text from outside use [`checked_sentence_boundaries`], which keeps the
//! preparation cap of [`MAX_PREPARATION_UNITS`] UTF-16 units per call. The unchecked
//! functions are unbounded and are meant for text that is already within that cap.

use unicode_segmentation::UnicodeSegmentation;

/// The preparation cap, in UTF-16 units per call.
pub const MAX_PREPARATION_UNITS: usize = 1_000_000;

/// Refusal of text that exceeds the preparation cap.
///
/// The message, field and reason are those of the validation error
/// (`Resume preparation limit exceeded`, field `text`, reason `exceeds preparation safety budget`),
/// so a caller can map this value to its own validation error without changing the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SentenceError {
    /// The text is longer than [`MAX_PREPARATION_UNITS`] UTF-16 units.
    PreparationLimitExceeded,
}

impl SentenceError {
    /// The user-facing message.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        "Resume preparation limit exceeded"
    }

    /// The field label of the validation error.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        "text"
    }

    /// The reason of the validation error.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        "exceeds preparation safety budget"
    }
}

impl std::fmt::Display for SentenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for SentenceError {}

/// Returns the sentence boundaries of `text` as code-point offsets, after the cap check.
///
/// Text longer than [`MAX_PREPARATION_UNITS`] UTF-16 units is refused. Exactly the cap
/// is accepted. Otherwise the result is the same as [`sentence_boundaries`].
///
/// # Errors
///
/// Returns [`SentenceError::PreparationLimitExceeded`] when the text has more than
/// [`MAX_PREPARATION_UNITS`] UTF-16 units.
pub fn checked_sentence_boundaries(text: &str) -> Result<Vec<usize>, SentenceError> {
    if text.encode_utf16().count() > MAX_PREPARATION_UNITS {
        return Err(SentenceError::PreparationLimitExceeded);
    }
    Ok(sentence_boundaries(text))
}

/// Returns the sentence boundaries of `text` as code-point offsets, with no size cap.
///
/// The first offset is always 0 and the last equals the number of code points. The offsets are
/// strictly increasing. Empty input gives `[0]`. This function is
/// unbounded; use [`checked_sentence_boundaries`] for text whose size is not already limited.
#[must_use]
pub fn sentence_boundaries(text: &str) -> Vec<usize> {
    let mut wanted = byte_boundaries(text).into_iter().peekable();
    let mut offsets = Vec::new();
    let mut code_points = 0_usize;
    for (byte_index, _) in text.char_indices() {
        while wanted.next_if(|&byte| byte == byte_index).is_some() {
            offsets.push(code_points);
        }
        code_points += 1;
    }
    for _ in wanted {
        offsets.push(code_points);
    }
    offsets
}

/// Returns the sentence segments of `text` in order.
///
/// The segments are the slices between consecutive boundaries, so they concatenate back to
/// `text`. Empty input gives no segments. Callers strip the segments themselves. The boundaries
/// always fall on char boundaries; a violation is a bug, caught by a debug assertion.
#[must_use]
pub fn sentence_spans(text: &str) -> Vec<&str> {
    byte_boundaries(text)
        .windows(2)
        .map(|pair| {
            let segment = text.get(pair[0]..pair[1]);
            debug_assert!(
                segment.is_some(),
                "sentence boundary is not on a char boundary"
            );
            segment.unwrap_or_default()
        })
        .collect()
}

/// Byte offsets of the segment starts, with 0 first and `text.len()` last (one `0` for empty text).
fn byte_boundaries(text: &str) -> Vec<usize> {
    let mut starts: Vec<usize> = text
        .split_sentence_bound_indices()
        .map(|(byte, _)| byte)
        .collect();
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    if starts.last() != Some(&text.len()) {
        starts.push(text.len());
    }
    starts.dedup();
    starts
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_PREPARATION_UNITS, SentenceError, checked_sentence_boundaries, sentence_boundaries,
        sentence_spans,
    };

    #[test]
    fn refusal_carries_the_message_field_and_reason() {
        let error = SentenceError::PreparationLimitExceeded;
        assert_eq!(error.to_string(), "Resume preparation limit exceeded");
        assert_eq!(error.field(), "text");
        assert_eq!(error.reason(), "exceeds preparation safety budget");
    }

    #[test]
    fn checked_refuses_one_unit_over_the_cap_before_segmenting() {
        let text = "a".repeat(MAX_PREPARATION_UNITS + 1);
        assert_eq!(
            checked_sentence_boundaries(&text),
            Err(SentenceError::PreparationLimitExceeded)
        );
    }

    #[test]
    fn checked_matches_the_unchecked_result_below_the_cap() {
        let text = "Led the team. Shipped it.";
        assert_eq!(
            checked_sentence_boundaries(text),
            Ok(sentence_boundaries(text))
        );
    }

    #[test]
    fn empty_text_has_a_single_zero_boundary() {
        assert_eq!(sentence_boundaries(""), vec![0]);
        assert!(sentence_spans("").is_empty());
    }

    #[test]
    fn splits_after_a_terminator_and_space() {
        assert_eq!(
            sentence_boundaries("Led the team. Shipped it."),
            vec![0, 14, 25]
        );
        assert_eq!(
            sentence_spans("Led the team. Shipped it."),
            vec!["Led the team. ", "Shipped it."]
        );
    }

    #[test]
    fn offsets_count_code_points_not_bytes_or_utf16_units() {
        // U+1F680 is one code point, four UTF-8 bytes and two UTF-16 units.
        assert_eq!(
            sentence_boundaries("Shipped the \u{1F680} launch. Users loved it."),
            vec![0, 22, 37]
        );
    }

    #[test]
    fn digits_after_a_period_do_not_split_here() {
        assert_eq!(
            sentence_boundaries("Built the API. 2 new services shipped."),
            vec![0, 38]
        );
    }

    #[test]
    fn exclamation_and_question_marks_split_sentences() {
        assert_eq!(sentence_boundaries("Ready?! Yes."), vec![0, 8, 12]);
    }

    #[test]
    fn segments_concatenate_back_to_the_input() {
        let text = "A\u{1F600}. Bc. Done";
        assert_eq!(sentence_spans(text).concat(), text);
    }
}
