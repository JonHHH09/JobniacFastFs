//! Literal source normalization, immutable copies and selected-input bounds. Synthetic data only.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use jobniac_core::errors::{ApplicationError, ErrorCode};
use jobniac_core::german::content::{
    AdditionalEntry, CoverLetterInput, EducationEntry, ExperienceEntry, GermanCoverLetterContent,
    LanguageEntry, PersonalField, RawStrings, ResumeInput, SkillGroup, StructuredResumeContent,
    budget_cover_letter_content, budget_resume_content, raw_strings,
};

const LIMIT: &str = "Resume preparation limit exceeded";

fn s(value: &str) -> Option<&str> {
    Some(value)
}

fn strings(items: &[Option<&str>]) -> RawStrings {
    Some(items.iter().map(|item| item.map(str::to_owned)).collect())
}

fn owned(value: &str) -> Option<String> {
    Some(value.to_owned())
}

fn field(label: &str, value: &str) -> PersonalField {
    PersonalField::new(s(label), s(value)).unwrap()
}

fn resume(full_name: &str, language: &str) -> ResumeInput {
    ResumeInput {
        full_name: owned(full_name),
        language: owned(language),
        ..ResumeInput::default()
    }
}

fn cover_input() -> CoverLetterInput {
    CoverLetterInput {
        sender_name: owned(" Sender "),
        personal_fields: Vec::new(),
        recipient_company: owned(" "),
        recipient_location: owned("\u{a0}"),
        job_title: owned(" Engineer "),
        subject: owned(" Subject "),
        salutation: owned(" Hello "),
        paragraphs: strings(&[s(" Paragraph ")]),
        closing: owned(" Regards "),
        signature: owned(" Sender "),
    }
}

fn assert_limit<T>(result: Result<T, ApplicationError>) {
    match result {
        Err(error) => assert_eq!(error.message(), LIMIT),
        Ok(_) => panic!("expected the preparation limit error"),
    }
}

fn assert_safe_invalid<T>(result: Result<T, ApplicationError>) {
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("expected the fixed invalid error"),
    };
    let envelope = error.to_envelope();
    assert_eq!(envelope.code, ErrorCode::ValidationError);
    assert_eq!(envelope.message, "Invalid German content input");
    assert!(envelope.details.is_empty());
}

#[test]
fn complete_source_record_normalization_and_copy() {
    let bullets = || strings(&[None, s(" "), s("  Built tools  "), s("\u{a0}")]);
    let content = StructuredResumeContent::new(ResumeInput {
        full_name: owned(" Full Name "),
        language: owned(" EN "),
        summary: owned(" Summary "),
        personal_fields: vec![field(" Email ", " person@example.test ")],
        experiences: vec![
            ExperienceEntry::new(
                s(" "),
                s(" Company "),
                s(" "),
                s("2024-01-01"),
                None,
                bullets(),
            )
            .unwrap(),
        ],
        education: vec![
            EducationEntry::new(
                s(" "),
                s(" University "),
                s("\u{a0}"),
                None,
                None,
                bullets(),
            )
            .unwrap(),
        ],
        skill_groups: vec![
            SkillGroup::new(s(" Backend "), strings(&[s(" Java "), None, s(" ")])).unwrap(),
        ],
        languages: vec![LanguageEntry::new(s(" "), s(" ")).unwrap()],
        additional: vec![AdditionalEntry::new(s(" "), s(" URL "), bullets()).unwrap()],
    })
    .unwrap();
    assert_eq!(content.full_name(), "Full Name");
    assert_eq!(content.language(), "en");
    assert_eq!(content.summary(), Some("Summary"));
    assert_eq!(
        content.personal_fields(),
        &[field("Email", "person@example.test")]
    );
    assert_eq!(content.experiences()[0].title(), "");
    assert_eq!(content.experiences()[0].location(), None);
    assert_eq!(
        content.experiences()[0].bullets(),
        ["Built tools", "\u{a0}"]
    );
    assert_eq!(content.education()[0].degree(), "");
    assert_eq!(content.education()[0].location(), Some("\u{a0}"));
    assert_eq!(content.skill_groups()[0].skills(), ["Java"]);
    assert_eq!(
        content.languages()[0],
        LanguageEntry::new(s(""), None).unwrap()
    );
    assert_eq!(content.additional()[0].organization(), Some("URL"));
    budget_resume_content(&content).unwrap();
    let layout = content.to_layout_resume();
    assert_eq!(layout.full_name, "Full Name");
    assert_eq!(layout.experiences[0].bullets, ["Built tools", "\u{a0}"]);
    assert_eq!(layout.skill_groups[0].skills, ["Java"]);
}

#[test]
fn top_level_null_collections_empty_and_null_members_invalid() {
    // Rust inputs are typed: an absent collection is the empty vector, and a null member cannot
    // be built. The null-member refusals remain at the string-list level (below).
    let content = StructuredResumeContent::new(resume("Name", "en")).unwrap();
    assert!(content.personal_fields().is_empty());
    assert!(content.experiences().is_empty());
    assert!(content.education().is_empty());
    assert!(content.skill_groups().is_empty());
    assert!(content.languages().is_empty());
    assert!(content.additional().is_empty());
    assert_safe_invalid(SkillGroup::new(s("Backend"), None).map(|_| ()));
    assert!(
        AdditionalEntry::new(s("T"), None, None)
            .unwrap()
            .bullets()
            .is_empty()
    );
}

#[test]
fn invalid_direct_records_have_fixed_safe_error() {
    assert_safe_invalid(PersonalField::new(None, s("x")));
    assert_safe_invalid(PersonalField::new(s("x"), s(" ")));
    assert_safe_invalid(SkillGroup::new(s(" "), raw_strings(["Java"])));
    assert_safe_invalid(SkillGroup::new(s("Backend"), strings(&[None, s(" ")])));
    assert_safe_invalid(ExperienceEntry::new(
        None,
        s("C"),
        None,
        None,
        None,
        raw_strings::<_, &str>([]),
    ));
    assert_safe_invalid(LanguageEntry::new(None, None));
    assert_safe_invalid(AdditionalEntry::new(None, None, raw_strings::<_, &str>([])));
    assert_safe_invalid(StructuredResumeContent::new(resume(" ", "en")));
    assert_safe_invalid(StructuredResumeContent::new(resume("N", "fr")));
}

#[test]
fn dates_accept_only_normalized_positive_year_iso() {
    for date in [
        "2024-02-30",
        "0000-01-01",
        "2024-1-01",
        "20240101",
        "private",
    ] {
        assert_safe_invalid(ExperienceEntry::new(
            s("T"),
            s("C"),
            None,
            s(date),
            None,
            None,
        ));
        assert_safe_invalid(EducationEntry::new(
            s("T"),
            s("C"),
            None,
            None,
            s(date),
            None,
        ));
    }
}

#[test]
fn cover_null_distinctions_and_normalization() {
    let content = GermanCoverLetterContent::new(cover_input()).unwrap();
    assert!(content.personal_fields().is_empty());
    assert_eq!(content.recipient_company(), None);
    assert_eq!(content.recipient_location(), Some("\u{a0}"));
    assert_eq!(content.paragraphs(), ["Paragraph"]);
    assert_eq!(content.sender_name(), "Sender");
    assert_eq!(content.job_title(), "Engineer");
    let empty = GermanCoverLetterContent::new(CoverLetterInput {
        paragraphs: strings(&[]),
        ..cover_input()
    })
    .unwrap();
    assert!(empty.paragraphs().is_empty());
    for value in [None, strings(&[None]), strings(&[s(" ")])] {
        assert_safe_invalid(GermanCoverLetterContent::new(CoverLetterInput {
            paragraphs: value,
            ..cover_input()
        }));
    }
    budget_cover_letter_content(&content).unwrap();
    let layout = content.to_layout_cover_letter();
    assert_eq!(layout.subject, "Subject");
    assert_eq!(layout.paragraphs, ["Paragraph"]);
    assert_eq!(layout.recipient_company, None);
}

#[test]
fn budget_counts_raw_null_and_blank_entries_before_filtering() {
    assert_limit(AdditionalEntry::new(s("T"), None, Some(vec![None; 10_001])));
    let blank = " ".repeat(600_000);
    assert_limit(AdditionalEntry::new(
        s("T"),
        None,
        raw_strings([&blank, &blank]),
    ));
}

#[test]
fn aggregate_budget_counts_repeated_record_occurrences() {
    let entry = field("Email", &"x".repeat(600_000));
    assert_limit(StructuredResumeContent::new(ResumeInput {
        personal_fields: vec![entry.clone(), entry.clone()],
        ..resume("Name", "en")
    }));
    assert_limit(GermanCoverLetterContent::new(CoverLetterInput {
        personal_fields: vec![entry.clone(), entry],
        ..cover_input()
    }));
}

#[test]
fn budget_boundaries_and_utf16_units() {
    assert!(StructuredResumeContent::new(resume(&"x".repeat(999_998), "en")).is_ok());
    assert!(StructuredResumeContent::new(resume(&"\u{1F600}".repeat(499_999), "en")).is_ok());
    assert_limit(StructuredResumeContent::new(resume(
        &"\u{1F600}".repeat(500_000),
        "en",
    )));
}

#[test]
fn raw_nested_totals_survive_normalization_and_repeat_per_occurrence() {
    let blank = " ".repeat(600_000);
    let entry = AdditionalEntry::new(s("Title"), None, raw_strings([&blank])).unwrap();
    assert!(entry.bullets().is_empty());
    assert_limit(StructuredResumeContent::new(ResumeInput {
        additional: vec![entry.clone(), entry],
        ..resume("Name", "en")
    }));
}

#[test]
fn deep_records_cannot_be_built_to_bypass_admission() {
    // No subclassing in Rust: fields are private and every record comes from an admitting
    // constructor. A clone keeps its raw totals, so it cannot shrink the budget either.
    let blank = " ".repeat(600_000);
    let entry = AdditionalEntry::new(s("Title"), None, raw_strings([&blank])).unwrap();
    let cloned = entry.clone();
    assert_eq!(cloned, entry);
    assert_limit(StructuredResumeContent::new(ResumeInput {
        additional: vec![entry, cloned],
        ..resume("Name", "en")
    }));
}

#[test]
fn normalized_positive_year_date_edges() {
    let entry =
        ExperienceEntry::new(s("T"), s("C"), None, s("0001-01-01"), s("9999-12-31"), None).unwrap();
    assert_eq!(entry.start_date(), Some("0001-01-01"));
    assert_eq!(entry.end_date(), Some("9999-12-31"));
    assert!(entry.bullets().is_empty());
}
