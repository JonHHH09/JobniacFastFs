//! Admission and source projection for exact-resume German cover-letter persistence. Synthetic data
//! only.
//!
//! Cases that depend on a dynamic type system (a `bool`, `float`, `str` or naive `datetime` where a
//! typed value belongs), on lone surrogates, on mutating or deleting attributes of a frozen record or
//! on monkeypatching the normalizer have no Rust counterpart: rows and writes are immutable behind
//! checked constructors. Each such case is named in the test that covers the rest of its parameter
//! list.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use jobniac_core::errors::{ApplicationError, ErrorCode};
use jobniac_core::german::content::{
    CoverLetterInput, GermanCoverLetterContent, PersonalField, raw_strings,
};
use jobniac_core::german::cover_letter_store::{
    CoverLetter, CoverLetterAggregateWrite, CoverLetterFields, CoverLetterParagraph,
    CoverLetterVariant, DisplayOrder, PairKey, ParagraphFields, ReplaceResult, VariantFields,
    VariantWrite, map_cover_letter_content, validate_cover_letter_aggregate, validate_revision,
};
use time::macros::{datetime, offset};
use time::{OffsetDateTime, UtcOffset};
use uuid::Uuid;

const NOW: OffsetDateTime = datetime!(2026-10-03 0:00 UTC);
const INVALID: &str = "Invalid German cover-letter persistence input";
const LIMIT: &str = "Cover letter persistence limit exceeded";

fn uid(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn order(value: i64) -> DisplayOrder {
    DisplayOrder::new(value).unwrap()
}

fn assert_invalid<T>(result: Result<T, ApplicationError>) {
    let error = result.err().expect("the input is refused");
    assert_eq!(error.code(), ErrorCode::ValidationError);
    assert_eq!(error.message(), INVALID);
    assert!(error.details().is_empty());
}

fn assert_limit<T>(result: Result<T, ApplicationError>) {
    let error = result.err().expect("the limit is exceeded");
    assert_eq!(error.code(), ErrorCode::ValidationError);
    assert_eq!(error.message(), LIMIT);
    assert!(error.details().is_empty());
}

fn parent_fields() -> CoverLetterFields {
    CoverLetterFields {
        id: uid(1),
        profile_id: uid(2),
        job_id: uid(3),
        resume_id: uid(4),
        profile_revision: NOW,
        job_revision: NOW,
        resume_revision: NOW,
        created_at: NOW,
        updated_at: NOW,
    }
}

fn parent() -> CoverLetter {
    CoverLetter::new(parent_fields()).unwrap()
}

fn variant_fields() -> VariantFields<'static> {
    VariantFields {
        id: uid(5),
        cover_letter_id: uid(1),
        format: "germany",
        language: "de",
        document_id: uid(6),
        file_path: "asset.pdf",
        subject: "Subject",
        salutation: "Salutation",
        closing: "Closing",
        signature: "Signature",
        created_at: NOW,
        updated_at: NOW,
    }
}

fn variant() -> CoverLetterVariant {
    CoverLetterVariant::new(variant_fields()).unwrap()
}

fn paragraph_fields(identity: u128, position: i64, text: &str) -> ParagraphFields<'_> {
    ParagraphFields {
        id: uid(identity),
        variant_id: uid(5),
        display_order: order(position),
        text,
    }
}

fn paragraph(identity: u128, position: i64, text: &str) -> CoverLetterParagraph {
    CoverLetterParagraph::new(paragraph_fields(identity, position, text)).unwrap()
}

fn variant_write(rows: Vec<CoverLetterParagraph>) -> VariantWrite {
    VariantWrite::new(variant(), rows).unwrap()
}

fn tree() -> CoverLetterAggregateWrite {
    CoverLetterAggregateWrite::new(parent(), variant_write(vec![paragraph(7, 0, "Text")])).unwrap()
}

fn sequence(start: u128) -> impl FnMut() -> Uuid {
    let mut next = start;
    move || {
        let value = uid(next);
        next += 1;
        value
    }
}

fn input() -> CoverLetterInput {
    CoverLetterInput {
        sender_name: Some("Unstored Sender".to_owned()),
        personal_fields: vec![
            PersonalField::new(Some("Email"), Some("sender@example.test")).unwrap(),
        ],
        recipient_company: Some("Company".to_owned()),
        recipient_location: Some("Berlin".to_owned()),
        job_title: Some("Role".to_owned()),
        subject: Some("Subject".to_owned()),
        salutation: Some("Salutation".to_owned()),
        paragraphs: raw_strings(["First", "Second"]),
        closing: Some("Closing".to_owned()),
        signature: Some("Signature".to_owned()),
    }
}

fn content_of(input: CoverLetterInput) -> GermanCoverLetterContent {
    GermanCoverLetterContent::new(input).unwrap()
}

fn content() -> GermanCoverLetterContent {
    content_of(input())
}

fn with_paragraphs(items: &[&str]) -> GermanCoverLetterContent {
    content_of(CoverLetterInput {
        paragraphs: raw_strings(items.iter().copied()),
        ..input()
    })
}

#[test]
fn mapper_preserves_supplied_identities_and_paragraph_encounter_order() {
    let source = with_paragraphs(&["Second in lexical order", "First in lexical order"]);
    let mapped =
        map_cover_letter_content(parent(), variant(), &source, &mut sequence(100)).unwrap();
    assert_eq!(mapped.cover_letter(), &parent());
    assert_eq!(mapped.variant().variant(), &variant());
    assert_eq!(
        mapped.variant().paragraphs(),
        [
            paragraph(100, 0, "Second in lexical order"),
            paragraph(101, 1, "First in lexical order"),
        ]
    );
    validate_cover_letter_aggregate(&mapped).unwrap();
}

#[test]
fn mapper_rejects_variant_content_disagreement_before_uuid_allocation() {
    for field in ["subject", "salutation", "closing", "signature"] {
        let mut fields = variant_fields();
        match field {
            "subject" => fields.subject = "Different",
            "salutation" => fields.salutation = "Different",
            "closing" => fields.closing = "Different",
            _ => fields.signature = "Different",
        }
        let mut allocated = false;
        let result = map_cover_letter_content(
            parent(),
            CoverLetterVariant::new(fields).unwrap(),
            &content(),
            &mut || {
                allocated = true;
                uid(0)
            },
        );
        assert_invalid(result);
        assert!(!allocated, "{field} allocated paragraph identities");
    }
}

#[test]
fn mapper_agrees_after_java_strip_and_scalar_conversion() {
    // A surrogate pair is one scalar in Rust, so the paired spelling is this literal.
    let source = content_of(CoverLetterInput {
        subject: Some("  \u{1f600}\t".to_owned()),
        paragraphs: raw_strings(["\u{1f600}"]),
        ..input()
    });
    let supplied = CoverLetterVariant::new(VariantFields {
        subject: "\u{1f600}",
        ..variant_fields()
    })
    .unwrap();
    let mapped = map_cover_letter_content(parent(), supplied, &source, &mut sequence(1)).unwrap();
    assert_eq!(mapped.variant().variant().subject(), "\u{1f600}");
    assert_eq!(mapped.variant().paragraphs()[0].text(), "\u{1f600}");
}

#[test]
fn java_whitespace_is_preserved_and_instants_are_utc() {
    let shifted = NOW.to_offset(offset!(+3));
    let row = CoverLetter::new(CoverLetterFields {
        profile_revision: shifted,
        ..parent_fields()
    })
    .unwrap();
    assert_eq!(row.profile_revision().offset(), UtcOffset::UTC);
    assert_eq!(row.profile_revision(), NOW);
    let shaped = CoverLetterVariant::new(VariantFields {
        format: "\t GeRmAnY\u{2003}",
        language: " DE ",
        subject: "\u{a0}",
        ..variant_fields()
    })
    .unwrap();
    assert_eq!(shaped.format(), "germany");
    assert_eq!(shaped.language(), "de");
    assert_eq!(shaped.subject(), "\u{a0}");
}

#[test]
fn invalid_required_text_has_fixed_content_free_error() {
    for value in ["\0", "a\0b", " \t", "\u{2003}"] {
        assert_invalid(CoverLetterParagraph::new(paragraph_fields(7, 0, value)));
    }
}

#[test]
fn variant_rejects_invalid_source_fields() {
    let early = NOW - time::Duration::seconds(1);
    assert_invalid(CoverLetterVariant::new(VariantFields {
        format: "austria",
        ..variant_fields()
    }));
    assert_invalid(CoverLetterVariant::new(VariantFields {
        language: "en",
        ..variant_fields()
    }));
    assert_invalid(CoverLetterVariant::new(VariantFields {
        updated_at: early,
        ..variant_fields()
    }));
    assert_invalid(CoverLetterVariant::new(VariantFields {
        file_path: " ",
        ..variant_fields()
    }));
    // A bad document id and a naive `created_at` are unrepresentable: the fields are `Uuid` and
    // `OffsetDateTime`.
}

#[test]
fn parent_chronology_is_source_constructor_invariant() {
    assert_invalid(CoverLetter::new(CoverLetterFields {
        updated_at: NOW - time::Duration::seconds(1),
        ..parent_fields()
    }));
}

#[test]
fn positions_are_exact_postgres_nonnegative_integers() {
    for value in [-1, 2_147_483_648] {
        assert!(DisplayOrder::new(value).is_none(), "{value}");
    }
    assert!(DisplayOrder::new(0).is_some());
    assert!(DisplayOrder::new(2_147_483_647).is_some());
}

fn broken_tree(case: &str) -> CoverLetterAggregateWrite {
    let row = paragraph(7, 0, "Text");
    let write = match case {
        "variant_parent" => VariantWrite::new(
            CoverLetterVariant::new(VariantFields {
                cover_letter_id: uid(20),
                ..variant_fields()
            })
            .unwrap(),
            vec![row],
        ),
        "paragraph_parent" => VariantWrite::new(
            variant(),
            vec![
                CoverLetterParagraph::new(ParagraphFields {
                    variant_id: uid(20),
                    ..paragraph_fields(7, 0, "Text")
                })
                .unwrap(),
            ],
        ),
        "paragraph_id" => VariantWrite::new(variant(), vec![row, paragraph(7, 2, "Text")]),
        "position" => VariantWrite::new(variant(), vec![row, paragraph(20, 0, "Text")]),
        _ => VariantWrite::new(variant(), vec![row]),
    };
    CoverLetterAggregateWrite::new(parent(), write.unwrap()).unwrap()
}

#[test]
fn aggregate_rejects_wrong_parentage_and_duplicate_paragraph_ids_or_positions() {
    for case in [
        "variant_parent",
        "paragraph_parent",
        "paragraph_id",
        "position",
    ] {
        assert_invalid(validate_cover_letter_aggregate(&broken_tree(case)));
    }
    validate_cover_letter_aggregate(&broken_tree("control")).unwrap();
}

#[test]
fn zero_paragraphs_and_sparse_positions_are_valid_storage() {
    let sparse = vec![
        paragraph(7, 17, "Text"),
        paragraph(8, 2_147_483_647, "Text"),
    ];
    for rows in [Vec::new(), sparse] {
        let write = CoverLetterAggregateWrite::new(parent(), variant_write(rows)).unwrap();
        validate_cover_letter_aggregate(&write).unwrap();
    }
}

#[test]
fn uuid_uniqueness_is_per_table() {
    let same = CoverLetterVariant::new(VariantFields {
        id: parent().id(),
        ..variant_fields()
    })
    .unwrap();
    let row = CoverLetterParagraph::new(ParagraphFields {
        id: same.id(),
        variant_id: same.id(),
        display_order: order(0),
        text: "Text",
    })
    .unwrap();
    let write = VariantWrite::new(same, vec![row]).unwrap();
    validate_cover_letter_aggregate(&CoverLetterAggregateWrite::new(parent(), write).unwrap())
        .unwrap();
}

#[test]
fn write_and_result_own_their_collections() {
    // The write takes ownership of the vectors and exposes them by shared slice only.
    let write = variant_write(vec![paragraph(7, 0, "Text")]);
    let result = ReplaceResult {
        cover_letter: parent(),
        variant: variant(),
        previous_variants: vec![variant()],
    };
    assert_eq!(write.paragraphs(), [paragraph(7, 0, "Text")]);
    assert_eq!(result.previous_variants, [variant()]);
}

#[test]
fn raw_text_limit_precedes_normalization() {
    let long = " ".repeat(1_000_001);
    assert_limit(CoverLetterParagraph::new(paragraph_fields(7, 0, &long)));
}

#[test]
fn stripped_raw_units_still_count_against_aggregate_limit() {
    let first_text = format!("{}First", " ".repeat(600_000));
    let second_text = format!("{}Second", " ".repeat(600_000));
    let first = paragraph(7, 0, &first_text);
    let second = paragraph(8, 1, &second_text);
    assert_eq!(first.text(), "First");
    assert_limit(VariantWrite::new(variant(), vec![first, second]));
}

#[test]
fn rows_10000_are_admitted_and_10001_rejected() {
    let rows: Vec<CoverLetterParagraph> = (0..9998_i64)
        .map(|index| paragraph(100 + index as u128, index, "Text"))
        .collect();
    let write = CoverLetterAggregateWrite::new(parent(), variant_write(rows.clone())).unwrap();
    assert_eq!(write.raw_entries(), 10_000);
    validate_cover_letter_aggregate(&write).unwrap();
    let mut extra = rows;
    extra.push(paragraph(20_000, 9998, "Text"));
    // The variant write alone still fits 10,000 rows; the parent makes it 10,001.
    let variant_only = VariantWrite::new(variant(), extra.clone()).unwrap();
    assert_eq!(variant_only.raw_entries(), 10_000);
    assert_limit(CoverLetterAggregateWrite::new(parent(), variant_only));
}

#[test]
fn oversized_collection_is_rejected_by_the_row_count() {
    let rows = vec![paragraph(7, 0, "Text"); 10_000];
    assert_limit(VariantWrite::new(variant(), rows));
}

#[test]
fn mapper_content_budget_and_total_output_budget_are_refused_before_uuid_allocation() {
    // The raw-total half cannot be tested by mutating a private counter; the output half holds.
    let long = "x".repeat(999_900);
    let source = with_paragraphs(&[long.as_str()]);
    let crowded_path = "x".repeat(200);
    let crowded = CoverLetterVariant::new(VariantFields {
        file_path: &crowded_path,
        ..variant_fields()
    })
    .unwrap();
    let mut allocated = false;
    let result = map_cover_letter_content(parent(), crowded, &source, &mut || {
        allocated = true;
        uid(0)
    });
    assert_limit(result);
    assert!(!allocated);
}

#[test]
fn mapper_retains_content_admission_even_for_unstored_metadata() {
    let padded = format!("{}Sender", " ".repeat(100_000));
    let source = content_of(CoverLetterInput {
        sender_name: Some(padded),
        ..input()
    });
    let mapped =
        map_cover_letter_content(parent(), variant(), &source, &mut sequence(100)).unwrap();
    // The content keeps its raw total privately; the mapper counts at least the stored text.
    assert!(mapped.raw_units() >= "Sender".len());
    validate_cover_letter_aggregate(&mapped).unwrap();
}

#[test]
fn mapper_counts_unstored_sender_units() {
    let source = content_of(CoverLetterInput {
        sender_name: Some("S".repeat(100_000)),
        ..input()
    });
    let mapped =
        map_cover_letter_content(parent(), variant(), &source, &mut sequence(100)).unwrap();
    assert!(mapped.raw_units() >= 100_000);
}

#[test]
fn mapper_rejects_duplicate_factory_ids() {
    assert_invalid(map_cover_letter_content(
        parent(),
        variant(),
        &content(),
        &mut || uid(7),
    ));
}

#[test]
fn mapper_rejects_nul_text_before_uuid_allocation() {
    let source = content_of(CoverLetterInput {
        paragraphs: raw_strings(["a\0b"]),
        ..input()
    });
    let mut allocated = false;
    let result = map_cover_letter_content(parent(), variant(), &source, &mut || {
        allocated = true;
        uid(0)
    });
    assert_invalid(result);
    assert!(!allocated);
}

#[test]
fn numeric_revision_preconditions_are_exact_and_bounded() {
    for value in [-1, i128::from(i64::MAX) + 1, i128::MIN, i128::MAX] {
        assert_invalid(validate_revision(value));
    }
}

#[test]
fn numeric_revision_includes_both_boundaries() {
    assert_eq!(validate_revision(0).unwrap().get(), 0);
    assert_eq!(
        validate_revision(i128::from(i64::MAX)).unwrap().get(),
        i64::MAX
    );
}

#[test]
fn aggregate_carries_the_resume_reference_and_pair_key() {
    let write = tree();
    assert_eq!(write.cover_letter().resume_id(), uid(4));
    assert_eq!(write.cover_letter().resume_revision(), NOW);
    assert_eq!(
        write.pair_key(),
        PairKey {
            profile_id: uid(2),
            job_id: uid(3)
        }
    );
}
