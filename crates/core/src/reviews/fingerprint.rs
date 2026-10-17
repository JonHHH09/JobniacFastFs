//! Content-addressed identifiers of reviews and disagreements.
//!
//! The identity strings are fixed byte for byte, because the identifiers are stored and compared.
//! A review identity is length-prefixed: each value is written as its UTF-16
//! code-unit count, a colon and the text. Sorting uses the UTF-16 code-unit order (the UTF-16BE
//! byte order). A disagreement identity is not length-prefixed; it is a `|`-separated string.

use std::collections::BTreeSet;

use uuid::Uuid;

use crate::ids::name_uuid;

use super::model::{EvidenceDefectCode, MatchReview, Reason, outcome_text};

/// The length-prefixed encoding of one value: UTF-16 code units, a colon, the text.
fn encode(text: &str) -> String {
    format!("{}:{}", text.encode_utf16().count(), text)
}

/// The list text of already encoded or plain items (`[a, b]`).
fn list_text(items: &[String]) -> String {
    format!("[{}]", items.join(", "))
}

/// The UTF-16 code units of a text: the sort key.
fn utf16_key(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// The text of a boolean.
const fn bool_text(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

/// The content address of a normalized review.
///
/// Components are sorted by name only, stably, so two components with the same name keep their input
/// order and the identifier depends on it (deliberate). Evidence is sorted by
/// component, status, source, fact and the defect flag. The identifier ignores `id` and `created_at`.
pub fn review_fingerprint(review: &MatchReview) -> Uuid {
    let mut components: Vec<(Vec<u16>, String)> = review
        .components
        .iter()
        .map(|row| {
            let encoded = [
                encode(row.component.as_str()),
                encode(&row.earned_points.to_string()),
                encode(&row.available_points.to_string()),
                encode(row.status.as_str()),
            ]
            .concat();
            (utf16_key(row.component.as_str()), encoded)
        })
        .collect();
    components.sort_by(|left, right| left.0.cmp(&right.0));

    type EvidenceKey = (Vec<u16>, Vec<u16>, Vec<u16>, Vec<u16>, bool);
    let mut evidence: Vec<(EvidenceKey, String)> = review
        .evidence
        .iter()
        .map(|row| {
            let key = (
                utf16_key(row.component.as_str()),
                utf16_key(row.status.as_str()),
                utf16_key(&row.source_type),
                utf16_key(&row.fact),
                row.outcome_changing_defect,
            );
            let encoded = [
                encode(row.component.as_str()),
                encode(row.status.as_str()),
                encode(&row.source_type),
                encode(&row.fact),
                encode(bool_text(row.outcome_changing_defect)),
            ]
            .concat();
            (key, encoded)
        })
        .collect();
    evidence.sort_by(|left, right| left.0.cmp(&right.0));

    let component_rows: Vec<String> = components.into_iter().map(|(_, row)| row).collect();
    let evidence_rows: Vec<String> = evidence.into_iter().map(|(_, row)| row).collect();
    let identity = [
        encode(&review.report_id.to_string()),
        encode(&review.reviewer),
        encode(&review.model),
        encode(&review.review_version),
        encode(&review.overall_score.to_string()),
        encode(outcome_text(review.outcome)),
        encode(bool_text(review.blocker_mismatch)),
    ]
    .concat()
        + &encode(&list_text(&component_rows))
        + &encode(&list_text(&evidence_rows))
        + &encode(review.summary.as_str());
    name_uuid(&identity)
}

/// The dedupe key of a disagreement.
///
/// The reasons are taken in canonical order and without repeats, the defect codes sorted and without
/// repeats. The identifier does not depend on the review, so a second review with the same reasons and
/// codes returns the stored disagreement (deliberate).
pub fn disagreement_fingerprint(
    report_id: Uuid,
    reasons: &[Reason],
    defect_codes: &[EvidenceDefectCode],
    policy_version: &str,
) -> Uuid {
    let ordered: BTreeSet<Reason> = reasons.iter().copied().collect();
    let ordered: Vec<&str> = ordered.into_iter().map(Reason::as_str).collect();
    let codes: BTreeSet<&str> = defect_codes.iter().map(|code| code.as_str()).collect();
    let codes: Vec<&str> = codes.into_iter().collect();
    let identity = format!(
        "{report_id}|{policy_version}|[{}]|[{}]",
        ordered.join(", "),
        codes.join(", ")
    );
    name_uuid(&identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_counts_utf16_code_units() {
        assert_eq!(encode("Straße"), "6:Straße");
        assert_eq!(encode("a😀"), "3:a😀");
        assert_eq!(encode("[]"), "2:[]");
        assert_eq!(list_text(&[]), "[]");
    }

    #[test]
    fn utf16_order_places_surrogates_before_the_full_width_block() {
        assert!(utf16_key("😀") < utf16_key("\u{ff01}"));
        assert!("😀" > "\u{ff01}");
    }
}
