//! Canadian experience bullets: source line normalization, line
//! selection, the sentence split of a single line and the three-bullet limit.

use jobniac_text::java::{BudgetField, PreparationBudget, java_is_blank, java_lines, java_strip};
use jobniac_text::sentences::checked_sentence_boundaries;

use super::budget::budget_error;
use crate::errors::{ApplicationError, ErrorCode};

/// The most bullets one experience description yields.
pub const MAX_BULLETS: usize = 3;

/// The default sentence finder: UAX #29 boundaries as code-point offsets.
///
/// Returns `None` when the text exceeds the preparation cap, which the caller reports as a
/// backend failure.
#[must_use]
pub fn default_sentence_boundaries(text: &str) -> Option<Vec<usize>> {
    checked_sentence_boundaries(text).ok()
}

/// Strips a numbered prefix `^\d+[.)]\s+` (ASCII digits and ASCII whitespace) and returns the rest
/// of the line.
fn strip_numbered_prefix(line: &str) -> &str {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return line;
    }
    let after_digits = &line[digits..];
    let mut characters = after_digits.chars();
    if !matches!(characters.next(), Some('.' | ')')) {
        return line;
    }
    let rest = characters.as_str();
    let spaces = rest
        .find(|c: char| !matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{0B}' | '\u{0C}'))
        .unwrap_or(rest.len());
    if spaces == 0 {
        return line;
    }
    &rest[spaces..]
}

/// Yields the bullets of one experience description.
///
/// Each non-blank line is stripped; a leading `- `, `* ` or `• ` is removed, else a numbered
/// prefix. When exactly one line remains, the sentence finder splits it and the sentences are the
/// bullets. Otherwise the first three lines are the bullets. At most three non-blank bullets are
/// returned.
///
/// `sentences` returns the code-point boundary offsets of a text. The offsets must start at 0,
/// end at the text's code-point length and strictly increase.
///
/// # Errors
///
/// Returns a `validation_error` for a description over the preparation budget, and an
/// `internal_error` when the sentence finder fails or returns invalid offsets.
pub fn experience_bullets(
    description: Option<&str>,
    sentences: &dyn Fn(&str) -> Option<Vec<usize>>,
) -> Result<Vec<String>, ApplicationError> {
    let mut budget = PreparationBudget::new(BudgetField::Profile);
    budget.add_text(description).map_err(budget_error)?;
    let Some(description) = description.filter(|text| !java_is_blank(text)) else {
        return Ok(Vec::new());
    };
    let mut candidates: Vec<String> = Vec::new();
    for line in java_lines(java_strip(description)) {
        let line = java_strip(line);
        if java_is_blank(line) {
            continue;
        }
        let bullet = match line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("• "))
        {
            Some(rest) => java_strip(rest),
            None => java_strip(strip_numbered_prefix(line)),
        };
        if !java_is_blank(bullet) {
            candidates.push(bullet.to_owned());
        }
    }
    if candidates.len() != 1 {
        candidates.truncate(MAX_BULLETS);
        return Ok(candidates);
    }
    let text = candidates[0].as_str();
    let offsets = sentences(text).ok_or_else(|| {
        ApplicationError::new(
            ErrorCode::InternalError,
            Some("Canadian sentence boundary backend failed"),
            std::iter::empty::<(String, String)>(),
        )
    })?;
    let code_points = text.chars().count();
    let valid = offsets.len() >= 2
        && offsets.len() <= code_points + 1
        && offsets.first() == Some(&0)
        && offsets.last() == Some(&code_points)
        && offsets.windows(2).all(|pair| pair[0] < pair[1]);
    if !valid {
        return Err(ApplicationError::new(
            ErrorCode::InternalError,
            Some("Canadian sentence boundary backend returned invalid offsets"),
            std::iter::empty::<(String, String)>(),
        ));
    }
    let byte_offsets: Vec<usize> = text
        .char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut selected = Vec::new();
    for pair in offsets.windows(2) {
        let sentence = &text[byte_offsets[pair[0]]..byte_offsets[pair[1]]];
        let bullet = java_strip(sentence);
        if !java_is_blank(bullet) {
            selected.push(bullet.to_owned());
            if selected.len() == MAX_BULLETS {
                break;
            }
        }
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_prefix_requires_a_separator_after_the_marker() {
        assert_eq!(strip_numbered_prefix("1. Built it"), "Built it");
        assert_eq!(strip_numbered_prefix("12)\tBuilt it"), "Built it");
        assert_eq!(strip_numbered_prefix("1.Built"), "1.Built");
        assert_eq!(strip_numbered_prefix("x. Built"), "x. Built");
    }

    #[test]
    fn three_lines_give_three_bullets_and_no_sentence_split() {
        let text = "- One\n* Two\n3. Three\nFour";
        let bullets = experience_bullets(Some(text), &|_| None).expect("bullets");
        assert_eq!(bullets, vec!["One", "Two", "Three"]);
    }

    #[test]
    fn a_single_line_is_split_by_the_sentence_finder() {
        let finder = |text: &str| {
            let mut offsets = vec![0];
            for (index, pair) in text.char_indices().zip(text.chars().skip(1)) {
                if index.1 == '.' && pair == ' ' {
                    offsets.push(text[..index.0 + 2].chars().count());
                }
            }
            offsets.push(text.chars().count());
            Some(offsets)
        };
        let bullets = experience_bullets(Some("Built A. Shipped B."), &finder).expect("bullets");
        assert_eq!(bullets, vec!["Built A.", "Shipped B."]);
    }

    #[test]
    fn invalid_offsets_are_an_internal_error() {
        let finder = |_: &str| Some(vec![0, 1]);
        let error = experience_bullets(Some("Only one line"), &finder).expect_err("invalid");
        assert_eq!(error.code(), ErrorCode::InternalError);
        assert_eq!(
            error.message(),
            "Canadian sentence boundary backend returned invalid offsets"
        );
    }

    #[test]
    fn default_finder_splits_two_sentences() {
        let bullets = experience_bullets(
            Some("Built the service. Shipped the release."),
            &default_sentence_boundaries,
        )
        .expect("bullets");
        assert_eq!(bullets, vec!["Built the service.", "Shipped the release."]);
    }
}
