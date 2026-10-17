//! Offline English-to-French translation of resume text.
//!
//! The translator applies, in order: protected spans (caller terms, then URL and e-mail spans that
//! win at the same start), whole-phrase replacement of the full text, phrase substitution (ASCII
//! case-insensitive, in glossary order), then the word glossary. Protected spans are copied
//! unchanged. The tables come from a [`Glossary`], so the algorithm is the same for the built-in
//! and the extended glossary.
//!
//! Positions are code-point indices. Rust strings cannot hold lone
//! surrogates, so no surrogate normalization is needed.

use std::collections::{BTreeMap, HashMap, HashSet};

use jobniac_text::java::{
    BudgetField, PreparationBudget, PreparationLimitExceeded, java_is_blank, java_strip,
    utf16_length,
};

use super::fold::{
    eq_ascii_insensitive, is_ascii_upper_letter, is_opaque_boundary, preserve_case, unicode_fold,
};
use super::glossary::Glossary;

/// The French translator built from one glossary.
///
/// Build it once per glossary and reuse it; it holds no mutable state.
#[derive(Debug, Clone)]
pub struct FrenchTranslator {
    labels: BTreeMap<String, String>,
    phrase_patterns: Vec<(Vec<char>, String)>,
    full_phrases: Vec<(Vec<char>, String)>,
    words: BTreeMap<String, String>,
}

/// The caller's protected terms, prepared once for many translations of one resume.
///
/// The terms are stripped, de-duplicated by exact text and ordered by decreasing UTF-16 length
/// (stable), so the first term to match at a position is the longest one. Matching is
/// case-insensitive under the translator's simple case fold, except that a term equal to the
/// sharp s (U+00DF) matches only the raw character.
#[derive(Debug, Clone)]
pub struct PreparedProtectedTerms {
    terms: Vec<String>,
    units: usize,
    entries: usize,
    sharp_s_rank: Option<usize>,
    index: HashMap<char, Vec<(usize, Vec<char>)>>,
}

impl PreparedProtectedTerms {
    /// No protected terms.
    fn empty() -> Self {
        Self {
            terms: Vec::new(),
            units: 0,
            entries: 0,
            sharp_s_rank: None,
            index: HashMap::new(),
        }
    }

    /// The ordered, de-duplicated terms.
    #[must_use]
    pub fn terms(&self) -> &[String] {
        &self.terms
    }

    /// The UTF-16 units of the terms as given, before de-duplication.
    #[must_use]
    pub fn units(&self) -> usize {
        self.units
    }

    /// The number of terms as given, before de-duplication.
    #[must_use]
    pub fn entries(&self) -> usize {
        self.entries
    }

    /// For each code-point index of `source`, the end index of the best protected term that
    /// starts there (`None` when no term starts there).
    fn ends(&self, source: &[char]) -> Vec<Option<usize>> {
        let length = source.len();
        let mut ends = vec![None; length];
        if self.terms.is_empty() {
            return ends;
        }
        for (start, slot) in ends.iter_mut().enumerate() {
            let character = source[start];
            let mut best: Option<(usize, usize)> = None;
            if let Some(candidates) = self.index.get(&unicode_fold(character)) {
                for (rank, folded) in candidates {
                    let fits = start + folded.len() <= length;
                    if fits
                        && folded.iter().enumerate().all(|(offset, expected)| {
                            unicode_fold(source[start + offset]) == *expected
                        })
                    {
                        best = Some((*rank, folded.len()));
                        break;
                    }
                }
            }
            if character == '\u{DF}'
                && let Some(rank) = self.sharp_s_rank
                && best.is_none_or(|(current, _)| rank < current)
            {
                best = Some((rank, 1));
            }
            if let Some((_, span)) = best {
                *slot = Some(start + span);
            }
        }
        ends
    }
}

/// The protected spans of `ends`, scanning left to right.
fn protected_spans(ends: &[Option<usize>]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut cursor = 0;
    while cursor < ends.len() {
        match ends[cursor] {
            Some(end) => {
                spans.push((cursor, end));
                cursor = end;
            }
            None => cursor += 1,
        }
    }
    spans
}

/// Marks URL and e-mail spans in `ends`.
///
/// The source is cut into chunks at ASCII whitespace and `<`, `>`, `(` and `)`. In a chunk, a
/// scheme (letters, digits, `+`, `.`, `-` before a colon that has text after it) or a `www.`
/// prefix starts a span that runs to the chunk end. A run of characters before an `@` that has a
/// non-empty right segment starts an e-mail span that ends before the next `@` or the chunk end.
/// An opaque span overrides a protected term at the same start.
fn add_opaque_ends(source: &[char], ends: &mut [Option<usize>]) {
    let size = source.len();
    let mut url_starts = vec![false; size];
    let mut chunk_start = 0;
    while chunk_start < size {
        if is_opaque_boundary(source[chunk_start]) {
            chunk_start += 1;
            continue;
        }
        let mut chunk_end = chunk_start;
        while chunk_end < size && !is_opaque_boundary(source[chunk_end]) {
            chunk_end += 1;
        }
        let mut scheme_run_start = chunk_start;
        let mut segment_start = chunk_start;
        for index in chunk_start..chunk_end {
            let character = source[index];
            let folded = unicode_fold(character);
            let is_letter = is_ascii_upper_letter(character);
            if !(is_letter || character.is_ascii_digit() || matches!(character, '+' | '.' | '-')) {
                if character == ':' && index + 1 < chunk_end {
                    for start in scheme_run_start..index {
                        if is_ascii_upper_letter(source[start]) {
                            ends[start] = Some(chunk_end);
                            url_starts[start] = true;
                        }
                    }
                }
                scheme_run_start = index + 1;
            }
            if index + 4 < chunk_end
                && folded == 'w'
                && unicode_fold(source[index + 1]) == 'w'
                && unicode_fold(source[index + 2]) == 'w'
                && source[index + 3] == '.'
            {
                ends[index] = Some(chunk_end);
                url_starts[index] = true;
            }
            if character == '@' {
                let mut right_end = index + 1;
                while right_end < chunk_end && source[right_end] != '@' {
                    right_end += 1;
                }
                if segment_start < index && index + 1 < right_end {
                    for start in segment_start..index {
                        if !url_starts[start] {
                            ends[start] = Some(right_end);
                        }
                    }
                }
                segment_start = index + 1;
            }
        }
        chunk_start = chunk_end;
    }
}

/// Replaces every non-overlapping ASCII case-insensitive match of `pattern`, left to right.
fn replace_ascii_insensitive(text: &[char], pattern: &[char], replacement: &str) -> Vec<char> {
    let length = pattern.len();
    if length == 0 {
        return text.to_vec();
    }
    let mut output = Vec::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let matches = index + length <= text.len()
            && pattern
                .iter()
                .enumerate()
                .all(|(offset, expected)| eq_ascii_insensitive(text[index + offset], *expected));
        if matches {
            output.extend(replacement.chars());
            index += length;
        } else {
            output.push(text[index]);
            index += 1;
        }
    }
    output
}

impl FrenchTranslator {
    /// A translator for `glossary`.
    #[must_use]
    pub fn new(glossary: &Glossary) -> Self {
        Self {
            labels: glossary.labels.clone(),
            phrase_patterns: glossary
                .phrases
                .iter()
                .map(|(source, target)| (source.chars().collect(), target.clone()))
                .collect(),
            full_phrases: glossary
                .phrases
                .iter()
                .map(|(source, target)| {
                    (source.chars().map(unicode_fold).collect(), target.clone())
                })
                .collect(),
            words: glossary.words.clone(),
        }
    }

    /// A translator for the generic built-in glossary.
    #[must_use]
    pub fn builtin() -> Self {
        Self::new(&Glossary::builtin())
    }

    /// Prepares the caller's protected terms.
    ///
    /// Every term is counted against a fresh `text` budget: one entry and its UTF-16 length.
    ///
    /// # Errors
    ///
    /// Returns [`PreparationLimitExceeded`] when the terms exceed the preparation budget.
    pub fn prepare_protected_terms(
        &self,
        terms: &[&str],
    ) -> Result<PreparedProtectedTerms, PreparationLimitExceeded> {
        let mut budget = PreparationBudget::new(BudgetField::Text);
        let mut units = 0;
        let mut seen = HashSet::new();
        let mut unique: Vec<String> = Vec::new();
        for term in terms {
            budget.add_entries(1)?;
            budget.add_text(Some(*term))?;
            units += utf16_length(term);
            if !java_is_blank(term) {
                let stripped = java_strip(term);
                if seen.insert(stripped.to_owned()) {
                    unique.push(stripped.to_owned());
                }
            }
        }
        unique.sort_by_key(|term| std::cmp::Reverse(utf16_length(term)));
        let mut sharp_s_rank = None;
        let mut index: HashMap<char, Vec<(usize, Vec<char>)>> = HashMap::new();
        for (rank, term) in unique.iter().enumerate() {
            if term == "\u{DF}" {
                sharp_s_rank = Some(rank);
                continue;
            }
            let folded: Vec<char> = term.chars().map(unicode_fold).collect();
            if let Some(first) = folded.first().copied() {
                index.entry(first).or_default().push((rank, folded));
            }
        }
        Ok(PreparedProtectedTerms {
            terms: unique,
            units,
            entries: terms.len(),
            sharp_s_rank,
            index,
        })
    }

    /// Translates `value` with the protected terms prepared by
    /// [`prepare_protected_terms`](Self::prepare_protected_terms).
    ///
    /// A blank value is returned unchanged. The budget counts the value's UTF-16 length and the
    /// prepared totals.
    ///
    /// # Errors
    ///
    /// Returns [`PreparationLimitExceeded`] when the budget is exceeded.
    pub fn translate_narrative(
        &self,
        value: &str,
        protected: &PreparedProtectedTerms,
    ) -> Result<String, PreparationLimitExceeded> {
        let mut budget = PreparationBudget::new(BudgetField::Text);
        budget.add_text(Some(value))?;
        budget.add_units(protected.units)?;
        budget.add_entries(protected.entries)?;
        Ok(self.translate_value(value, protected))
    }

    /// Translates `value` with no protected terms.
    ///
    /// # Errors
    ///
    /// Returns [`PreparationLimitExceeded`] when the value exceeds the preparation budget.
    pub fn translate_narrative_unprotected(
        &self,
        value: &str,
    ) -> Result<String, PreparationLimitExceeded> {
        let mut budget = PreparationBudget::new(BudgetField::Text);
        budget.add_text(Some(value))?;
        Ok(self.translate_value(value, &PreparedProtectedTerms::empty()))
    }

    /// Translates a field label.
    ///
    /// The stripped, lower-cased value is looked up in the label map first. Otherwise the value is
    /// translated as narrative text with no protected terms. A blank value is returned unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`PreparationLimitExceeded`] when the value exceeds the preparation budget.
    pub fn translate_label(&self, value: &str) -> Result<String, PreparationLimitExceeded> {
        let mut budget = PreparationBudget::new(BudgetField::Text);
        budget.add_text(Some(value))?;
        if java_is_blank(value) {
            return Ok(value.to_owned());
        }
        let key = java_strip(value).to_lowercase();
        if let Some(label) = self.labels.get(&key) {
            return Ok(label.clone());
        }
        Ok(self.translate_value(value, &PreparedProtectedTerms::empty()))
    }

    /// Returns an opaque value (URL, e-mail address) unchanged, counting it against the budget.
    ///
    /// # Errors
    ///
    /// Returns [`PreparationLimitExceeded`] when the value exceeds the preparation budget.
    pub fn translate_opaque_value(&self, value: &str) -> Result<String, PreparationLimitExceeded> {
        let mut budget = PreparationBudget::new(BudgetField::Text);
        budget.add_text(Some(value))?;
        Ok(value.to_owned())
    }

    /// Returns a technology list unchanged, counting it against the budget.
    ///
    /// # Errors
    ///
    /// Returns [`PreparationLimitExceeded`] when the value exceeds the preparation budget.
    pub fn translate_technology_list(
        &self,
        value: &str,
    ) -> Result<String, PreparationLimitExceeded> {
        let mut budget = PreparationBudget::new(BudgetField::Text);
        budget.add_text(Some(value))?;
        Ok(value.to_owned())
    }

    /// The `_translate_narrative` algorithm, without budget accounting.
    fn translate_value(&self, value: &str, protected: &PreparedProtectedTerms) -> String {
        if java_is_blank(value) {
            return value.to_owned();
        }
        let source = java_strip(value);
        let characters: Vec<char> = source.chars().collect();
        let length = characters.len();
        let mut ends = protected.ends(&characters);
        let whole_term = ends[0] == Some(length);
        add_opaque_ends(&characters, &mut ends);
        if whole_term || ends[0] == Some(length) {
            return source.to_owned();
        }
        let folded_source: Vec<char> = characters.iter().map(|c| unicode_fold(*c)).collect();
        for (original, translated) in &self.full_phrases {
            if folded_source == *original {
                let translated = preserve_case(source, translated);
                let lowered = translated.to_lowercase();
                let keeps_spans = protected_spans(&ends).into_iter().all(|(start, end)| {
                    let piece: String = characters[start..end].iter().collect();
                    lowered.contains(&piece.to_lowercase())
                });
                if keeps_spans {
                    return translated;
                }
            }
        }
        let mut output = String::with_capacity(source.len());
        let mut cursor = 0;
        for (start, end) in protected_spans(&ends) {
            if start > cursor {
                output.push_str(&self.translate_unprotected(&characters[cursor..start]));
            }
            output.extend(&characters[start..end]);
            cursor = end;
        }
        if cursor < length {
            output.push_str(&self.translate_unprotected(&characters[cursor..]));
        }
        java_strip(&output).to_owned()
    }

    /// Phrase substitution, then the word glossary, on text outside every protected span.
    fn translate_unprotected(&self, text: &[char]) -> String {
        if text.is_empty() {
            return String::new();
        }
        let mut current = text.to_vec();
        for (pattern, replacement) in &self.phrase_patterns {
            current = replace_ascii_insensitive(&current, pattern, replacement);
        }
        let mut output = String::with_capacity(current.len());
        let mut index = 0;
        while index < current.len() {
            if current[index].is_ascii_alphabetic() {
                let mut end = index + 1;
                while end < current.len()
                    && (current[end].is_ascii_alphabetic()
                        || matches!(current[end], '+' | '.' | '/' | '-'))
                {
                    end += 1;
                }
                let token: String = current[index..end].iter().collect();
                match self.words.get(&token.to_ascii_lowercase()) {
                    Some(word) => output.push_str(&preserve_case(&token, word)),
                    None => output.push_str(&token),
                }
                index = end;
            } else {
                output.push(current[index]);
                index += 1;
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_term_is_kept_and_the_rest_translated() {
        let translator = FrenchTranslator::builtin();
        let protected = translator
            .prepare_protected_terms(&["Ada Example"])
            .expect("terms");
        let output = translator
            .translate_narrative("Software Developer at Ada Example", &protected)
            .expect("translate");
        assert_eq!(output, "Développeur logiciel at Ada Example");
    }

    #[test]
    fn whole_protected_text_is_returned_stripped() {
        let translator = FrenchTranslator::builtin();
        let protected = translator
            .prepare_protected_terms(&["Example Corp"])
            .expect("terms");
        assert_eq!(
            translator
                .translate_narrative("  Example Corp ", &protected)
                .expect("translate"),
            "Example Corp"
        );
    }

    #[test]
    fn url_and_email_spans_are_kept() {
        let translator = FrenchTranslator::builtin();
        let output = translator
            .translate_narrative_unprotected(
                "Remote, see https://example.com/Built and ada@example.com",
            )
            .expect("translate");
        assert_eq!(
            output,
            "À distance, see https://example.com/Built and ada@example.com"
        );
    }

    #[test]
    fn label_map_wins_over_narrative_translation() {
        let translator = FrenchTranslator::builtin();
        assert_eq!(
            translator
                .translate_label(" Frontend / Apps ")
                .expect("label"),
            "Développement frontend / Applications"
        );
        assert_eq!(translator.translate_label("  ").expect("blank"), "  ");
    }

    #[test]
    fn sharp_s_term_matches_only_the_raw_character() {
        let translator = FrenchTranslator::builtin();
        let protected = translator
            .prepare_protected_terms(&["\u{DF}"])
            .expect("terms");
        assert_eq!(
            translator
                .translate_narrative("stra\u{DF}e", &protected)
                .expect("translate")
                .as_str(),
            "stra\u{DF}e"
        );
    }

    #[test]
    fn budget_counts_protected_entries() {
        let translator = FrenchTranslator::builtin();
        let terms: Vec<&str> = vec!["x"; 10_001];
        assert!(translator.prepare_protected_terms(&terms).is_err());
    }
}
