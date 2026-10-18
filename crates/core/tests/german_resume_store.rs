//! Literal source mapping, typed canonicalization and aggregate safety admission of the German
//! resume storage records. Synthetic data only.
//!
//! Cases that depend on a dynamic type system (a `bool`, `float`, `str` or naive `datetime` where a
//! typed value belongs), on lone surrogates, on mutating a frozen record or on monkeypatching the
//! normalizer have no Rust counterpart: the typed constructors make them unrepresentable. Each such
//! case is named in the test that covers the rest of its parameter list.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use jobniac_core::errors::{ApplicationError, ErrorCode};
use jobniac_core::german::content::{
    AdditionalEntry, EducationEntry, ExperienceEntry, LanguageEntry, PersonalField, ResumeInput,
    SkillGroup, StructuredResumeContent, raw_strings,
};
use jobniac_core::german::resume_store::{
    BulletFields, DisplayOrder, EntryFields, EntryType, EntryWrite, PairKey, Resume,
    ResumeAggregateWrite, ResumeEntry, ResumeEntryBullet, ResumeFields, ResumeLanguage,
    ResumeSection, ResumeVariant, Revision, SectionFields, SectionType, SectionWrite,
    VariantFields, VariantWrite, to_variant_write, validate_resume_aggregate, validate_revision,
};
use serde_json::Value;
use time::macros::{datetime, offset};
use time::{Date, Month, OffsetDateTime, UtcOffset};
use uuid::Uuid;

const NOW: OffsetDateTime = datetime!(2026-10-02 0:00 UTC);
const INVALID: &str = "Invalid German resume persistence input";
const LIMIT: &str = "Resume preparation limit exceeded";
const GOLDEN: &str = include_str!("golden/german/resume_source_records.json");

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
    let error = result.err().expect("the budget is exceeded");
    assert_eq!(error.message(), LIMIT);
}

fn parent_fields() -> ResumeFields<'static> {
    ResumeFields {
        id: uid(1),
        profile_id: uid(2),
        job_id: uid(3),
        format: "germany",
        profile_revision: NOW,
        job_revision: NOW,
        created_at: NOW,
        updated_at: NOW,
    }
}

fn parent() -> Resume {
    Resume::new(parent_fields()).unwrap()
}

fn variant_fields(language: &str, identity: u128, document: u128) -> VariantFields<'_> {
    VariantFields {
        id: uid(identity),
        resume_id: uid(1),
        language,
        document_id: uid(document),
        file_path: "asset.pdf",
        created_at: NOW,
        updated_at: NOW,
    }
}

fn variant_with(language: &str, identity: u128, document: u128) -> ResumeVariant {
    ResumeVariant::new(variant_fields(language, identity, document)).unwrap()
}

fn variant() -> ResumeVariant {
    variant_with("de", 4, 5)
}

fn entry_fields(
    identity: u128,
    section: u128,
    metadata: Option<&str>,
    position: i64,
) -> EntryFields<'_> {
    EntryFields {
        id: uid(identity),
        section_id: uid(section),
        entry_type: "custom",
        display_order: order(position),
        title: None,
        organization: None,
        location: None,
        start_date: None,
        end_date: None,
        metadata,
    }
}

fn entry(identity: u128, section: u128, metadata: Option<&str>, position: i64) -> ResumeEntry {
    ResumeEntry::new(entry_fields(identity, section, metadata, position)).unwrap()
}

fn bullet(identity: u128, entry_id: u128, position: i64, text: &str) -> ResumeEntryBullet {
    ResumeEntryBullet::new(BulletFields {
        id: uid(identity),
        entry_id: uid(entry_id),
        display_order: order(position),
        text,
    })
    .unwrap()
}

fn section(identity: u128, variant_id: u128, position: i64) -> ResumeSection {
    ResumeSection::new(SectionFields {
        id: uid(identity),
        variant_id: uid(variant_id),
        section_type: "custom",
        title: "Title",
        display_order: order(position),
    })
    .unwrap()
}

fn entry_write(entry_row: ResumeEntry, bullets: Vec<ResumeEntryBullet>) -> EntryWrite {
    EntryWrite::new(entry_row, bullets).unwrap()
}

fn section_write(section_row: ResumeSection, entries: Vec<EntryWrite>) -> SectionWrite {
    SectionWrite::new(section_row, entries).unwrap()
}

fn variant_write(row: ResumeVariant, sections: Vec<SectionWrite>) -> VariantWrite {
    VariantWrite::new(row, sections).unwrap()
}

fn aggregate(row: Resume, variants: Vec<VariantWrite>) -> ResumeAggregateWrite {
    ResumeAggregateWrite::new(row, variants).unwrap()
}

fn tree_parts() -> (EntryWrite, SectionWrite) {
    let entries = entry_write(entry(7, 6, None, 0), vec![bullet(8, 7, 0, "Bullet")]);
    (
        entries.clone(),
        section_write(section(6, 4, 0), vec![entries]),
    )
}

fn tree() -> ResumeAggregateWrite {
    let (_, sections) = tree_parts();
    aggregate(parent(), vec![variant_write(variant(), vec![sections])])
}

fn sequence(start: u128) -> impl FnMut() -> Uuid {
    let mut next = start;
    move || {
        let value = uid(next);
        next += 1;
        value
    }
}

fn text(value: &str) -> Option<&str> {
    Some(value)
}

fn field(label: &str, value: &str) -> PersonalField {
    PersonalField::new(text(label), text(value)).unwrap()
}

fn bare(language: &str) -> StructuredResumeContent {
    StructuredResumeContent::new(ResumeInput {
        full_name: Some("Ada".to_owned()),
        language: Some(language.to_owned()),
        ..ResumeInput::default()
    })
    .unwrap()
}

fn date(year: i32, month: Month, day: u8) -> Date {
    Date::from_calendar_date(year, month, day).unwrap()
}

#[test]
fn sparse_mapper_always_retains_empty_personal_section() {
    let target = variant();
    let mapped = to_variant_write(target.clone(), &bare("de"), &mut || uid(6)).unwrap();
    assert_eq!(mapped.variant(), &target);
    let first = mapped.sections()[0].section();
    assert_eq!(first.section_type(), "personal");
    assert_eq!(first.title(), "Kontakt");
    assert!(mapped.sections()[0].entries().is_empty());
    assert_eq!(mapped.sections().len(), 1);
}

fn full_content(language: &str) -> StructuredResumeContent {
    StructuredResumeContent::new(ResumeInput {
        full_name: Some("Unstored Full Name".to_owned()),
        language: Some(language.to_owned()),
        summary: Some("Summary text".to_owned()),
        personal_fields: vec![field("Email", "ada@example.test"), field("Place", "Berlin")],
        experiences: vec![
            ExperienceEntry::new(
                text("Engineer"),
                text("Company"),
                text("Berlin"),
                text("0001-01-02"),
                None,
                raw_strings(["First", "Second"]),
            )
            .unwrap(),
        ],
        education: vec![
            EducationEntry::new(
                text("Degree"),
                text("Uni"),
                None,
                None,
                text("9999-12-31"),
                raw_strings(["Third"]),
            )
            .unwrap(),
        ],
        skill_groups: vec![
            SkillGroup::new(text("Technology"), raw_strings(["Python", "SQL"])).unwrap(),
        ],
        languages: vec![
            LanguageEntry::new(text("German"), text("C1")).unwrap(),
            LanguageEntry::new(text("English"), None).unwrap(),
        ],
        additional: vec![
            AdditionalEntry::new(
                text("Project"),
                text("https://example.test"),
                raw_strings(["Fourth"]),
            )
            .unwrap(),
        ],
    })
    .unwrap()
}

fn check_full_mapping(language: &str, titles: [&str; 7]) {
    let content = full_content(language);
    let mut factory = sequence(100);
    let mapped = to_variant_write(variant_with(language, 4, 5), &content, &mut factory).unwrap();
    let sections = mapped.sections();
    let section_titles: Vec<&str> = sections.iter().map(|row| row.section().title()).collect();
    assert_eq!(section_titles, titles);
    let kinds: Vec<&str> = sections
        .iter()
        .map(|row| row.section().section_type())
        .collect();
    assert_eq!(
        kinds,
        [
            "personal",
            "summary",
            "experience",
            "additional",
            "education",
            "skills",
            "languages"
        ]
    );
    let positions: Vec<i32> = sections
        .iter()
        .map(|row| row.section().display_order().get())
        .collect();
    assert_eq!(positions, (0..7).collect::<Vec<i32>>());
    let rows: Vec<Vec<&ResumeEntry>> = sections
        .iter()
        .map(|row| row.entries().iter().map(EntryWrite::entry).collect())
        .collect();
    assert_eq!(rows[0][0].title(), Some("Email"));
    assert_eq!(rows[0][0].metadata(), Some("ada@example.test"));
    assert_eq!(rows[1][0].title(), Some(titles[1]));
    assert_eq!(rows[1][0].metadata(), Some("Summary text"));
    assert_eq!(rows[2][0].start_date(), Some(date(1, Month::January, 2)));
    assert_eq!(rows[2][0].end_date(), None);
    assert_eq!(rows[2][0].organization(), Some("Company"));
    assert_eq!(rows[2][0].location(), Some("Berlin"));
    assert_eq!(rows[3][0].entry_type(), "project");
    assert_eq!(rows[3][0].organization(), Some("https://example.test"));
    assert_eq!(rows[4][0].end_date(), Some(date(9999, Month::December, 31)));
    assert_eq!(rows[4][0].metadata(), None);
    assert_eq!(rows[5][0].title(), Some("Technology"));
    assert_eq!(rows[5][0].metadata(), Some("Python, SQL"));
    assert_eq!(rows[6][0].metadata(), Some("C1"));
    assert_eq!(rows[6][1].metadata(), None);
    assert!(
        rows.iter()
            .flatten()
            .all(|record| record.title() != Some(content.full_name()))
    );
    let mut ids = Vec::new();
    for stored in sections {
        ids.push(stored.section().id());
        let positions: Vec<i32> = stored
            .entries()
            .iter()
            .map(|row| row.entry().display_order().get())
            .collect();
        assert_eq!(
            positions,
            (0..stored.entries().len() as i32).collect::<Vec<i32>>()
        );
        for row in stored.entries() {
            assert_eq!(row.entry().section_id(), stored.section().id());
            ids.push(row.entry().id());
            let bullets: Vec<i32> = row
                .bullets()
                .iter()
                .map(|item| item.display_order().get())
                .collect();
            assert_eq!(
                bullets,
                (0..row.bullets().len() as i32).collect::<Vec<i32>>()
            );
            assert!(
                row.bullets()
                    .iter()
                    .all(|item| item.entry_id() == row.entry().id())
            );
            ids.extend(row.bullets().iter().map(ResumeEntryBullet::id));
        }
    }
    let unique: std::collections::HashSet<Uuid> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len());
    validate_resume_aggregate(&aggregate(parent(), vec![mapped])).unwrap();
}

#[test]
fn mapper_source_field_order_types_dates_metadata_and_restarted_positions_german() {
    check_full_mapping(
        "de",
        [
            "Kontakt",
            "Profil",
            "Berufserfahrung",
            "Projekte",
            "Ausbildung",
            "Kenntnisse",
            "Sprachen",
        ],
    );
}

#[test]
fn mapper_source_field_order_types_dates_metadata_and_restarted_positions_english() {
    check_full_mapping(
        "en",
        [
            "Contact",
            "Profile",
            "Professional Experience",
            "Projects",
            "Education",
            "Skills",
            "Languages",
        ],
    );
}

#[test]
fn supplied_variant_language_is_not_overwritten_by_mapper() {
    let mapped = to_variant_write(variant_with("en", 4, 5), &bare("de"), &mut || uid(6)).unwrap();
    assert_eq!(mapped.variant().language(), "en");
    assert_eq!(mapped.sections()[0].section().title(), "Kontakt");
}

#[test]
fn source_canonicalization_and_opaque_metadata() {
    let row = ResumeEntry::new(EntryFields {
        entry_type: "  CUSTOM\u{130}  ",
        title: Some("  Title  "),
        organization: Some("\t"),
        location: Some("\u{a0}"),
        metadata: Some("  {invalid: JSON}  "),
        ..entry_fields(7, 6, None, 0)
    })
    .unwrap();
    assert_eq!(row.entry_type(), "customi\u{307}");
    assert_eq!(row.title(), Some("Title"));
    assert_eq!(row.organization(), None);
    assert_eq!(row.location(), Some("\u{a0}"));
    assert_eq!(row.metadata(), Some("{invalid: JSON}"));
    let stored = "customi\u{307}Title\u{a0}{invalid: JSON}"
        .encode_utf16()
        .count();
    assert!(row.raw_units() > stored);
    let shaped = Resume::new(ResumeFields {
        format: "\t GeRmAnY\u{2003}",
        ..parent_fields()
    })
    .unwrap();
    let relanguaged = ResumeVariant::new(VariantFields {
        language: "  DE\t",
        file_path: "  file.pdf\t",
        ..variant_fields("de", 4, 5)
    })
    .unwrap();
    assert_eq!(shaped.format(), "germany");
    assert_eq!(relanguaged.language(), "de");
    assert_eq!(relanguaged.file_path(), "file.pdf");
}

#[test]
fn paired_and_scalar_unicode_keep_utf16_totals() {
    // A surrogate pair is one scalar in Rust, so the paired spelling is this literal.
    let row = entry(7, 6, Some("\u{1f600}"), 0);
    assert_eq!(row.metadata(), Some("\u{1f600}"));
    assert_eq!(row.raw_units(), "custom".len() + 2);
}

#[test]
fn unsupported_text_has_fixed_error() {
    let refused = ResumeEntry::new(entry_fields(7, 6, Some("\0"), 0));
    assert_invalid(refused);
    assert_invalid(ResumeEntryBullet::new(BulletFields {
        id: uid(8),
        entry_id: uid(7),
        display_order: order(0),
        text: "a\0b",
    }));
    assert_invalid(ResumeEntry::new(EntryFields {
        entry_type: " \t",
        ..entry_fields(7, 6, None, 0)
    }));
}

#[test]
fn aware_instants_are_normalized_to_utc() {
    let shifted = NOW.to_offset(offset!(+3));
    assert_eq!(shifted.offset(), offset!(+3));
    let row = Resume::new(ResumeFields {
        profile_revision: shifted,
        ..parent_fields()
    })
    .unwrap();
    assert_eq!(row.profile_revision(), NOW);
    assert_eq!(row.profile_revision().offset(), UtcOffset::UTC);
    // A naive datetime and a datetime where a date belongs are unrepresentable: the fields are
    // `OffsetDateTime` and `time::Date`.
}

#[test]
fn positions_are_exact_nonnegative_integers() {
    for value in [-1, 2_147_483_648, i64::MIN, i64::MAX] {
        assert!(DisplayOrder::new(value).is_none(), "{value}");
    }
    assert_eq!(DisplayOrder::new(0).map(DisplayOrder::get), Some(0));
    assert_eq!(
        DisplayOrder::new(2_147_483_647).map(DisplayOrder::get),
        Some(i32::MAX)
    );
}

#[test]
fn duplicate_entry_and_bullet_positions_are_source_compatible() {
    let max = section(6, 4, 2_147_483_647);
    let first = entry_write(
        entry(7, 6, None, 3),
        vec![bullet(8, 7, 4, "One"), bullet(9, 7, 4, "Two")],
    );
    let second = entry_write(entry(10, 6, None, 3), Vec::new());
    let write = aggregate(
        parent(),
        vec![variant_write(
            variant(),
            vec![section_write(max, vec![first, second])],
        )],
    );
    validate_resume_aggregate(&write).unwrap();
}

fn second_variant(case: &str, base: &ResumeVariant) -> VariantWrite {
    let identity = if case == "variant_id" {
        base.id().as_u128()
    } else {
        20
    };
    let document = if case == "document_id" {
        base.document_id().as_u128()
    } else {
        21
    };
    let language = if case == "language" {
        base.language()
    } else {
        "en"
    };
    variant_write(
        ResumeVariant::new(VariantFields {
            id: uid(identity),
            document_id: uid(document),
            language,
            ..variant_fields("de", 4, 5)
        })
        .unwrap(),
        Vec::new(),
    )
}

fn broken_tree(case: &str) -> ResumeAggregateWrite {
    let (ew, sw) = tree_parts();
    let rebuilt_variant = |sections: Vec<SectionWrite>| variant_write(variant(), sections);
    let second_section =
        |id: u128, position: i64| section_write(section(id, 4, position), vec![ew.clone()]);
    let vw = match case {
        "variant_parent" => variant_write(
            ResumeVariant::new(VariantFields {
                resume_id: uid(20),
                ..variant_fields("de", 4, 5)
            })
            .unwrap(),
            vec![sw],
        ),
        "section_parent" => {
            rebuilt_variant(vec![section_write(section(6, 20, 0), vec![ew.clone()])])
        }
        "entry_parent" => rebuilt_variant(vec![section_write(
            section(6, 4, 0),
            vec![entry_write(
                entry(7, 20, None, 0),
                vec![bullet(8, 7, 0, "Bullet")],
            )],
        )]),
        "bullet_parent" => rebuilt_variant(vec![section_write(
            section(6, 4, 0),
            vec![entry_write(
                entry(7, 6, None, 0),
                vec![bullet(8, 20, 0, "Bullet")],
            )],
        )]),
        "section_id" => rebuilt_variant(vec![sw, second_section(6, 1)]),
        "entry_id" => rebuilt_variant(vec![section_write(
            section(6, 4, 0),
            vec![ew.clone(), ew.clone()],
        )]),
        "bullet_id" => rebuilt_variant(vec![section_write(
            section(6, 4, 0),
            vec![entry_write(
                entry(7, 6, None, 0),
                vec![bullet(8, 7, 0, "Bullet"), bullet(8, 7, 0, "Bullet")],
            )],
        )]),
        "section_position" => rebuilt_variant(vec![sw, second_section(20, 0)]),
        _ => rebuilt_variant(vec![sw]),
    };
    if matches!(case, "variant_id" | "document_id" | "language") {
        let second = second_variant(case, vw.variant());
        aggregate(parent(), vec![vw, second])
    } else {
        aggregate(parent(), vec![vw])
    }
}

#[test]
fn aggregate_rejects_wrong_parentage_and_per_table_duplicates() {
    for case in [
        "variant_parent",
        "section_parent",
        "entry_parent",
        "bullet_parent",
        "variant_id",
        "section_id",
        "entry_id",
        "bullet_id",
        "document_id",
        "language",
        "section_position",
    ] {
        assert_invalid(validate_resume_aggregate(&broken_tree(case)));
    }
    validate_resume_aggregate(&broken_tree("control")).unwrap();
}

#[test]
fn uuid_uniqueness_is_per_table_not_global() {
    let same = 1;
    let write = aggregate(
        parent(),
        vec![variant_write(
            ResumeVariant::new(VariantFields {
                id: uid(same),
                resume_id: uid(1),
                language: "de",
                document_id: uid(same),
                file_path: "file.pdf",
                created_at: NOW,
                updated_at: NOW,
            })
            .unwrap(),
            vec![section_write(
                section(same, same, 0),
                vec![entry_write(
                    entry(same, same, None, 0),
                    vec![bullet(same, same, 0, "Text")],
                )],
            )],
        )],
    );
    validate_resume_aggregate(&write).unwrap();
}

#[test]
fn zero_one_two_variants_allowed_and_three_rejected() {
    let english = || variant_write(variant_with("en", 20, 21), Vec::new());
    for variants in [
        Vec::new(),
        vec![variant_write(variant(), Vec::new())],
        vec![variant_write(variant(), Vec::new()), english()],
    ] {
        validate_resume_aggregate(&aggregate(parent(), variants)).unwrap();
    }
    let three = vec![variant_write(variant(), Vec::new()); 3];
    assert_invalid(ResumeAggregateWrite::new(parent(), three));
}

#[test]
fn timestamp_chronology_checked_at_aggregate_boundary() {
    let early = NOW - time::Duration::seconds(1);
    let regressed = Resume::new(ResumeFields {
        updated_at: early,
        ..parent_fields()
    })
    .unwrap();
    let (_, sections) = tree_parts();
    let first = aggregate(regressed, vec![variant_write(variant(), vec![sections])]);
    let late_variant = ResumeVariant::new(VariantFields {
        updated_at: early,
        ..variant_fields("de", 4, 5)
    })
    .unwrap();
    let second = aggregate(parent(), vec![variant_write(late_variant, Vec::new())]);
    assert_invalid(validate_resume_aggregate(&first));
    assert_invalid(validate_resume_aggregate(&second));
}

#[test]
fn nested_collections_are_admitted_then_owned() {
    // The write takes ownership of the vector and exposes it by shared slice only.
    let write = entry_write(entry(7, 6, None, 0), vec![bullet(8, 7, 0, "Text")]);
    assert_eq!(write.bullets().len(), 1);
}

#[test]
fn oversized_raw_text_is_rejected_by_the_budget() {
    let long = " ".repeat(1_000_001);
    assert_limit(ResumeEntry::new(entry_fields(7, 6, Some(&long), 0)));
}

#[test]
fn stripped_raw_text_still_counts_against_aggregate_budget() {
    let pad = " ".repeat(600_000);
    let first = entry_write(entry(7, 6, Some(&pad), 0), Vec::new());
    let second = entry_write(entry(9, 6, Some(&pad), 0), Vec::new());
    assert_eq!(first.entry().metadata(), None);
    assert_limit(SectionWrite::new(section(6, 4, 0), vec![first, second]));
}

#[test]
fn lowercase_expansion_is_readmitted_before_the_write_wrapper() {
    // U+0130 expands into two UTF-16 units under ROOT lowercase. Raw input fits, but the
    // canonicalized open type exceeds the storage admission limit.
    let source = "\u{130}".repeat(600_000);
    let expanded = ResumeEntry::new(EntryFields {
        entry_type: &source,
        ..entry_fields(7, 6, None, 0)
    })
    .unwrap();
    assert_eq!(expanded.raw_units(), 600_000);
    assert_eq!(expanded.entry_type(), "i\u{307}".repeat(600_000));
    assert_limit(EntryWrite::new(expanded, Vec::new()));
}

#[test]
fn budget_counts_10000_physical_rows_and_not_write_wrappers() {
    // Parent + variant + section + entry + 9,996 bullets = 10,000 rows.
    let bullets: Vec<ResumeEntryBullet> = (0..9996_i64)
        .map(|index| bullet(100 + index as u128, 7, index, "Text"))
        .collect();
    let build = |rows: Vec<ResumeEntryBullet>| {
        let section_row = section(6, 4, 0);
        let entries = EntryWrite::new(entry(7, 6, None, 0), rows)?;
        let sections = SectionWrite::new(section_row, vec![entries])?;
        let variants = VariantWrite::new(variant(), vec![sections])?;
        ResumeAggregateWrite::new(parent(), vec![variants])
    };
    let write = build(bullets.clone()).unwrap();
    assert_eq!(write.raw_entries(), 10_000);
    validate_resume_aggregate(&write).unwrap();
    let mut extra = bullets;
    extra.push(bullet(20_000, 7, 0, "Text"));
    assert_limit(build(extra));
}

#[test]
fn mapper_output_budget_counts_physical_rows_when_many_skills_become_metadata() {
    let content = StructuredResumeContent::new(ResumeInput {
        full_name: Some("Ada".to_owned()),
        language: Some("de".to_owned()),
        skill_groups: vec![SkillGroup::new(text("Tools"), raw_strings(vec!["X"; 7000])).unwrap()],
        ..ResumeInput::default()
    })
    .unwrap();
    let mapped = to_variant_write(variant(), &content, &mut sequence(100)).unwrap();
    let write = aggregate(parent(), vec![mapped.clone()]);
    assert_eq!(mapped.raw_entries(), 4);
    assert_eq!(write.raw_entries(), 5);
    validate_resume_aggregate(&write).unwrap();
}

#[test]
fn mapper_output_over_budget_is_refused() {
    let content = StructuredResumeContent::new(ResumeInput {
        full_name: Some("Ada".to_owned()),
        language: Some("de".to_owned()),
        summary: Some("x".repeat(999_900)),
        ..ResumeInput::default()
    })
    .unwrap();
    let long_path = "p".repeat(200);
    let crowded = ResumeVariant::new(VariantFields {
        file_path: &long_path,
        ..variant_fields("de", 4, 5)
    })
    .unwrap();
    assert_limit(to_variant_write(crowded, &content, &mut sequence(100)));
}

#[test]
fn numeric_owner_revision_preconditions_are_exact_and_bounded() {
    for value in [-1, i128::from(i64::MAX) + 1, i128::MIN, i128::MAX] {
        assert_invalid(validate_revision(value));
    }
    assert!(Revision::new(-1).is_none());
}

#[test]
fn numeric_owner_revision_preconditions_include_both_boundaries() {
    assert_eq!(validate_revision(0).unwrap().get(), 0);
    assert_eq!(
        validate_revision(i128::from(i64::MAX)).unwrap().get(),
        i64::MAX
    );
}

#[test]
fn stored_vocabulary_is_exact() {
    let kinds: Vec<&str> = SectionType::ALL.iter().map(|kind| kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "personal",
            "summary",
            "experience",
            "additional",
            "education",
            "skills",
            "languages"
        ]
    );
    let entries: Vec<&str> = EntryType::ALL.iter().map(|kind| kind.as_str()).collect();
    assert_eq!(
        entries,
        [
            "personal_field",
            "summary",
            "experience",
            "project",
            "education",
            "skill_group",
            "language"
        ]
    );
    let titles = |language| -> Vec<&'static str> {
        SectionType::ALL
            .iter()
            .map(|kind| kind.title(language))
            .collect()
    };
    assert_eq!(
        titles(ResumeLanguage::English),
        [
            "Contact",
            "Profile",
            "Professional Experience",
            "Projects",
            "Education",
            "Skills",
            "Languages"
        ]
    );
    assert_eq!(
        titles(ResumeLanguage::German),
        [
            "Kontakt",
            "Profil",
            "Berufserfahrung",
            "Projekte",
            "Ausbildung",
            "Kenntnisse",
            "Sprachen"
        ]
    );
    assert_eq!(ResumeLanguage::parse("en"), Some(ResumeLanguage::English));
    assert_eq!(ResumeLanguage::parse("fr"), None);
}

#[test]
fn pair_key_names_the_shared_advisory_lock() {
    let key = tree().pair_key();
    assert_eq!(
        key,
        PairKey {
            profile_id: uid(2),
            job_id: uid(3)
        }
    );
    assert_eq!(
        key.lock_name(),
        format!(
            "jobniac-germany-resume:{}:{}",
            "00000000-0000-0000-0000-000000000002", "00000000-0000-0000-0000-000000000003"
        )
    );
}

// ---- golden vectors (row canonicalization and mapper over synthetic inputs) ----

fn decode(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(|encoded| String::from_utf8(STANDARD.decode(encoded).unwrap()).unwrap())
}

fn expect_text(actual: Option<&str>, expected: &Value) {
    assert_eq!(actual.map(str::to_owned), decode(expected));
}

fn check_row(kind: &str, input: &Value, expect: &Value) {
    let get = |name: &str| decode(&input[name]);
    let refused = expect.as_str() == Some("INVALID");
    match kind {
        "parent" => {
            let format = get("format").unwrap();
            let result = Resume::new(ResumeFields {
                format: &format,
                ..parent_fields()
            });
            if refused {
                assert_invalid(result);
            } else {
                assert_eq!(result.unwrap().format(), expect["format"].as_str().unwrap());
            }
        }
        "variant" => {
            let language = get("language").unwrap();
            let path = get("file_path").unwrap();
            let result = ResumeVariant::new(VariantFields {
                file_path: &path,
                ..variant_fields(&language, 4, 5)
            });
            if refused {
                assert_invalid(result);
            } else {
                let row = result.unwrap();
                assert_eq!(row.language(), expect["language"].as_str().unwrap());
                expect_text(Some(row.file_path()), &expect["file_path"]);
            }
        }
        "section" => {
            let (section_type, title) = (get("section_type").unwrap(), get("title").unwrap());
            let result = ResumeSection::new(SectionFields {
                id: uid(6),
                variant_id: uid(4),
                section_type: &section_type,
                title: &title,
                display_order: order(0),
            });
            if refused {
                assert_invalid(result);
            } else {
                let row = result.unwrap();
                assert_eq!(row.section_type(), expect["section_type"].as_str().unwrap());
                expect_text(Some(row.title()), &expect["title"]);
            }
        }
        "bullet" => {
            let body = get("text").unwrap();
            let result = ResumeEntryBullet::new(BulletFields {
                id: uid(8),
                entry_id: uid(7),
                display_order: order(0),
                text: &body,
            });
            if refused {
                assert_invalid(result);
            } else {
                expect_text(Some(result.unwrap().text()), &expect["text"]);
            }
        }
        _ => {
            let (entry_type, title) = (get("entry_type").unwrap(), get("title"));
            let (organization, location, metadata) =
                (get("organization"), get("location"), get("metadata"));
            let result = ResumeEntry::new(EntryFields {
                entry_type: &entry_type,
                title: title.as_deref(),
                organization: organization.as_deref(),
                location: location.as_deref(),
                metadata: metadata.as_deref(),
                ..entry_fields(7, 6, None, 0)
            });
            if refused {
                assert_invalid(result);
            } else {
                let row = result.unwrap();
                expect_text(Some(row.entry_type()), &expect["entry_type"]);
                expect_text(row.title(), &expect["title"]);
                expect_text(row.organization(), &expect["organization"]);
                expect_text(row.location(), &expect["location"]);
                expect_text(row.metadata(), &expect["metadata"]);
                assert_eq!(
                    row.raw_units() as u64,
                    expect["raw_units"].as_u64().unwrap()
                );
            }
        }
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| decode(item).unwrap())
        .collect()
}

fn opt_text(row: &Value, index: usize) -> Option<String> {
    decode(&row[index])
}

fn golden_content(language: &str, spec: &Value) -> StructuredResumeContent {
    let list = |name: &str| spec[name].as_array().cloned().unwrap_or_default();
    let owned = |value: &str| Some(value.to_owned());
    let personal = list("personal")
        .iter()
        .map(|row| {
            PersonalField::new(opt_text(row, 0).as_deref(), opt_text(row, 1).as_deref()).unwrap()
        })
        .collect();
    let experiences = list("experiences")
        .iter()
        .map(|row| {
            ExperienceEntry::new(
                opt_text(row, 0).as_deref(),
                opt_text(row, 1).as_deref(),
                opt_text(row, 2).as_deref(),
                opt_text(row, 3).as_deref(),
                opt_text(row, 4).as_deref(),
                raw_strings(strings(&row[5])),
            )
            .unwrap()
        })
        .collect();
    let education = list("education")
        .iter()
        .map(|row| {
            EducationEntry::new(
                opt_text(row, 0).as_deref(),
                opt_text(row, 1).as_deref(),
                opt_text(row, 2).as_deref(),
                opt_text(row, 3).as_deref(),
                opt_text(row, 4).as_deref(),
                raw_strings(strings(&row[5])),
            )
            .unwrap()
        })
        .collect();
    let skill_groups = list("skills")
        .iter()
        .map(|row| {
            SkillGroup::new(opt_text(row, 0).as_deref(), raw_strings(strings(&row[1]))).unwrap()
        })
        .collect();
    let languages = list("languages")
        .iter()
        .map(|row| {
            LanguageEntry::new(opt_text(row, 0).as_deref(), opt_text(row, 1).as_deref()).unwrap()
        })
        .collect();
    let additional = list("additional")
        .iter()
        .map(|row| {
            AdditionalEntry::new(
                opt_text(row, 0).as_deref(),
                opt_text(row, 1).as_deref(),
                raw_strings(strings(&row[2])),
            )
            .unwrap()
        })
        .collect();
    StructuredResumeContent::new(ResumeInput {
        full_name: owned(&decode(&spec["full_name"]).unwrap()),
        language: owned(language),
        summary: decode(&spec["summary"]),
        personal_fields: personal,
        experiences,
        education,
        skill_groups,
        languages,
        additional,
    })
    .unwrap()
}

fn iso(value: Option<Date>) -> Option<String> {
    value.map(|day| {
        format!(
            "{:04}-{:02}-{:02}",
            day.year(),
            u8::from(day.month()),
            day.day()
        )
    })
}

fn check_map(observation: &Value) {
    let language = observation["language"].as_str().unwrap();
    let supplied = observation["variant_language"].as_str().unwrap();
    let spec = &observation["content"];
    let content = golden_content(language, spec);
    let expect = &observation["expect"];
    let mut allocated = Vec::new();
    let mut next = 100_u128;
    let mapped = to_variant_write(variant_with(supplied, 4, 5), &content, &mut || {
        allocated.push(next as u64);
        let value = uid(next);
        next += 1;
        value
    })
    .unwrap();
    assert_eq!(
        mapped.variant().language(),
        expect["variant_language"].as_str().unwrap()
    );
    assert_eq!(
        allocated,
        expect["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_u64().unwrap())
            .collect::<Vec<u64>>()
    );
    assert_eq!(
        mapped.raw_entries() as u64,
        expect["raw_entries"].as_u64().unwrap()
    );
    // The content keeps its raw total privately, so a padded input counts its stripped text only.
    let raw_units = expect["raw_units"].as_u64().unwrap();
    let padded = decode(&spec["full_name"]).is_some_and(|name| name.starts_with(' '));
    if padded {
        assert!((mapped.raw_units() as u64) <= raw_units);
    } else {
        assert_eq!(mapped.raw_units() as u64, raw_units);
    }
    let sections = expect["sections"].as_array().unwrap();
    assert_eq!(mapped.sections().len(), sections.len());
    for (actual, wanted) in mapped.sections().iter().zip(sections) {
        let row = actual.section();
        assert_eq!(row.section_type(), wanted["section_type"].as_str().unwrap());
        expect_text(Some(row.title()), &wanted["title"]);
        assert_eq!(
            i64::from(row.display_order().get()),
            wanted["display_order"].as_i64().unwrap()
        );
        let entries = wanted["entries"].as_array().unwrap();
        assert_eq!(actual.entries().len(), entries.len());
        for (stored, expected) in actual.entries().iter().zip(entries) {
            let item = stored.entry();
            assert_eq!(item.entry_type(), expected["entry_type"].as_str().unwrap());
            assert_eq!(
                i64::from(item.display_order().get()),
                expected["display_order"].as_i64().unwrap()
            );
            expect_text(item.title(), &expected["title"]);
            expect_text(item.organization(), &expected["organization"]);
            expect_text(item.location(), &expected["location"]);
            assert_eq!(
                iso(item.start_date()),
                expected["start_date"].as_str().map(str::to_owned)
            );
            assert_eq!(
                iso(item.end_date()),
                expected["end_date"].as_str().map(str::to_owned)
            );
            expect_text(item.metadata(), &expected["metadata"]);
            let bullets = expected["bullets"].as_array().unwrap();
            assert_eq!(stored.bullets().len(), bullets.len());
            for (bullet_row, wanted_bullet) in stored.bullets().iter().zip(bullets) {
                assert_eq!(
                    i64::from(bullet_row.display_order().get()),
                    wanted_bullet["display_order"].as_i64().unwrap()
                );
                expect_text(Some(bullet_row.text()), &wanted_bullet["text"]);
            }
        }
    }
}

#[test]
fn golden_source_records_match_the_golden_mapper() {
    let vectors: Value = serde_json::from_str(GOLDEN).unwrap();
    let observations = vectors["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 27);
    let constants = &vectors["constants"];
    let names = |key: &str| -> Vec<String> {
        constants[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item.as_str().unwrap().to_owned())
            .collect()
    };
    let kinds: Vec<String> = SectionType::ALL
        .iter()
        .map(|kind| kind.as_str().to_owned())
        .collect();
    assert_eq!(kinds, names("section_types"));
    let entry_kinds: Vec<String> = EntryType::ALL
        .iter()
        .map(|kind| kind.as_str().to_owned())
        .collect();
    assert_eq!(entry_kinds, names("entry_types"));
    for (key, language) in [
        ("titles_en", ResumeLanguage::English),
        ("titles_de", ResumeLanguage::German),
    ] {
        let titles: Vec<String> = SectionType::ALL
            .iter()
            .map(|kind| kind.title(language).to_owned())
            .collect();
        assert_eq!(titles, names(key));
    }
    assert_eq!(constants["max_variants"].as_u64(), Some(2));
    assert_eq!(
        constants["max_display_order"].as_i64(),
        Some(i64::from(i32::MAX))
    );
    assert_eq!(constants["max_revision"].as_i64(), Some(i64::MAX));
    for observation in observations {
        match observation["kind"].as_str().unwrap() {
            "map" => check_map(observation),
            kind => check_row(kind, &observation["input"], &observation["expect"]),
        }
    }
}
