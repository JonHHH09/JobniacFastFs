//! German cover-letter content builder and review. Synthetic data only. The layout of the body is
//! covered by the render crate.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use jobniac_core::errors::{ApplicationError, ErrorCode};
use jobniac_core::german::content::{
    CoverLetterInput, GermanCoverLetterContent, PersonalField, raw_strings,
};
use jobniac_core::german::cover_letter::{
    build_german_cover_letter, render_cover_letter_body, review_cover_letter_content,
};
use jobniac_core::jobs::{JobAggregate, JobRecord};
use jobniac_core::profiles::ProfileGraph;
use jobniac_core::profiles::model::{
    ContactRecord, ExperienceRecord, LinkRecord, ProfileRecord, SkillRecord,
};
use jobniac_text::java::utf16_length;
use time::{Date, Month, OffsetDateTime};
use uuid::Uuid;

const LIMIT: &str = "Resume preparation limit exceeded";

const BODY: &str = "Ada Example\nE-Mail: ada@example.test\nPhone: 123\ngithub: https://example.test\n\
Example Hiring\nBerlin\nBewerbung als AI SQL Developer bei Example Hiring\n\
Sehr geehrte Damen und Herren,\n\
Mit großem Interesse bewerbe ich mich als AI SQL Developer bei Example Hiring.\n\n\
In meiner bisherigen Tätigkeit als Developer bei Example Company habe ich praktische \
Erfahrung gesammelt.\n\n\
Für diese Position bringe ich insbesondere Kenntnisse in SQL, AI mit.\n\n\
Gern erläutere ich Ihnen in einem persönlichen Gespräch, wie ich meine Erfahrung in Ihr Team \
einbringen kann.\n\nMit freundlichen Grüßen,\nAda Example";

fn date(year: i32, month: Month, day: u8) -> Date {
    Date::from_calendar_date(year, month, day).unwrap()
}

fn contact(contact_type: &str, label: Option<&str>, value: &str) -> ContactRecord {
    ContactRecord {
        id: Uuid::nil(),
        profile_id: Uuid::nil(),
        contact_type: contact_type.to_owned(),
        contact_value: value.to_owned(),
        label: label.map(str::to_owned),
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn link(link_type: &str, label: Option<&str>, url: &str) -> LinkRecord {
    LinkRecord {
        id: Uuid::nil(),
        profile_id: Uuid::nil(),
        link_type: link_type.to_owned(),
        url: url.to_owned(),
        label: label.map(str::to_owned),
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn experience(
    title: Option<&str>,
    company: Option<&str>,
    start: Option<Date>,
    end: Option<Date>,
    display_order: i64,
) -> ExperienceRecord {
    ExperienceRecord {
        id: Uuid::nil(),
        profile_id: Uuid::nil(),
        company: company.map(str::to_owned),
        title: title.map(str::to_owned),
        location: None,
        start_date: start,
        end_date: end,
        description: None,
        display_order,
        created_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn skill(name: &str, category: Option<&str>, display_order: i64) -> SkillRecord {
    SkillRecord {
        id: Uuid::nil(),
        profile_id: Uuid::nil(),
        skill: name.to_owned(),
        normalized_skill: name.to_lowercase(),
        category: category.map(str::to_owned),
        display_order,
        created_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn profile() -> ProfileGraph {
    ProfileGraph {
        profile: ProfileRecord {
            id: Uuid::nil(),
            full_name: "Ada Example".to_owned(),
            email: "ada@example.test".to_owned(),
            summary: None,
            raw_resume_text: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            embedding: None,
            revision: 0,
        },
        contacts: vec![contact("phone", Some("Phone"), "123")],
        links: vec![link("github", None, "https://example.test")],
        skills: vec![
            skill("AI", Some("Backend"), 1),
            skill("SQL", Some("Backend"), 0),
            skill("Unrelated", Some("Architecture"), 0),
        ],
        languages: Vec::new(),
        education: Vec::new(),
        experiences: vec![experience(
            Some("Developer"),
            Some("Example Company"),
            None,
            Some(date(2020, Month::January, 1)),
            0,
        )],
        projects: Vec::new(),
        project_technologies: Vec::new(),
    }
}

fn job() -> JobAggregate {
    JobAggregate {
        job: JobRecord {
            id: Uuid::nil(),
            source_method: "text".to_owned(),
            source_label: None,
            title: "AI SQL Developer".to_owned(),
            company: Some("Example Hiring".to_owned()),
            location: Some("Berlin".to_owned()),
            description: String::new(),
            experience_requirement: None,
            employment_type: None,
            seniority: None,
            posted_at: None,
            canonical_fingerprint: String::new(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            revision: 0,
        },
        skills: Vec::new(),
        link_ingestion: None,
        text_ingestion: None,
    }
}

fn build(graph: &ProfileGraph, posting: &JobAggregate) -> GermanCoverLetterContent {
    build_german_cover_letter(graph, posting).unwrap()
}

fn labels(content: &GermanCoverLetterContent) -> Vec<&str> {
    content
        .personal_fields()
        .iter()
        .map(PersonalField::label)
        .collect()
}

fn paragraphs_of(items: &[&str]) -> Option<Vec<Option<String>>> {
    raw_strings(items.iter().copied())
}

/// Builds the content from the default input after `change` edits it.
fn content_with(change: impl FnOnce(&mut CoverLetterInput)) -> GermanCoverLetterContent {
    let mut input = CoverLetterInput {
        sender_name: Some("N".to_owned()),
        personal_fields: vec![PersonalField::new(Some("Email"), Some("e@x")).unwrap()],
        recipient_company: None,
        recipient_location: None,
        job_title: Some("Role".to_owned()),
        subject: Some("Bewerbung als Role".to_owned()),
        salutation: Some("Hello".to_owned()),
        paragraphs: paragraphs_of(&["a", "b"]),
        closing: Some("Bye".to_owned()),
        signature: Some("N".to_owned()),
    };
    change(&mut input);
    GermanCoverLetterContent::new(input).unwrap()
}

fn defects(error: &ApplicationError) -> &str {
    assert_eq!(error.code(), ErrorCode::ValidationError);
    assert_eq!(error.message(), "German cover-letter content review failed");
    error.details().get("defects").unwrap()
}

fn review_failure(content: &GermanCoverLetterContent) -> ApplicationError {
    review_cover_letter_content(content).unwrap_err()
}

fn limit_message(result: Result<GermanCoverLetterContent, ApplicationError>) -> String {
    result.unwrap_err().message().to_owned()
}

#[test]
fn complete_literal_content_body_and_input_preserved() {
    let data = profile();
    let before = data.clone();
    let content = build(&data, &job());
    review_cover_letter_content(&content).unwrap();
    assert_eq!(render_cover_letter_body(&content).unwrap(), BODY);
    assert_eq!(content.job_title(), "AI SQL Developer");
    assert_eq!(content.recipient_company(), Some("Example Hiring"));
    assert_eq!(content.paragraphs().len(), 4);
    assert_eq!(data, before);
}

#[test]
fn contact_mail_filter_differs_from_resume_and_capacity() {
    let mut data = profile();
    data.contacts = vec![
        contact("mail", None, "postal"),
        contact("phone", Some("Email label retained"), "123"),
        contact("z", None, "hidden@invalid"),
    ];
    let content = build(&data, &job());
    let pairs: Vec<(&str, &str)> = content
        .personal_fields()
        .iter()
        .map(|field| (field.label(), field.value()))
        .collect();
    assert_eq!(
        pairs,
        [
            ("E-Mail", "ada@example.test"),
            ("Email label retained", "123"),
            ("github", "https://example.test"),
        ]
    );
    data.contacts = (0..9)
        .rev()
        .map(|index| contact(&index.to_string(), None, &index.to_string()))
        .collect();
    assert_eq!(
        labels(&build(&data, &job())),
        ["E-Mail", "0", "1", "2", "3", "4"]
    );
}

#[test]
fn latest_experience_stable_input_ties_ignores_display_order() {
    let mut data = profile();
    data.experiences = vec![
        experience(
            Some("First"),
            Some("A"),
            None,
            Some(date(2020, Month::January, 1)),
            100,
        ),
        experience(
            Some("Second"),
            Some("B"),
            Some(date(2020, Month::January, 1)),
            None,
            0,
        ),
    ];
    assert_eq!(
        build(&data, &job()).paragraphs()[1],
        "In meiner bisherigen Tätigkeit als First bei A habe ich praktische Erfahrung gesammelt."
    );
    data.experiences[1].start_date = Some(date(2021, Month::January, 1));
    assert!(build(&data, &job()).paragraphs()[1].contains("als Second bei B"));
}

#[test]
fn education_uses_first_input_not_sorted_and_null_evidence_preserved() {
    use jobniac_core::profiles::model::EducationRecord;
    let education =
        |degree: Option<&str>, institution: Option<&str>, end: Option<Date>| EducationRecord {
            id: Uuid::nil(),
            profile_id: Uuid::nil(),
            institution: institution.map(str::to_owned),
            degree: degree.map(str::to_owned),
            field: None,
            location: None,
            start_date: None,
            end_date: end,
            relevant_focus: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
        };
    let mut data = profile();
    data.experiences = Vec::new();
    data.education = vec![
        education(None, None, None),
        education(
            Some("Later"),
            Some("University"),
            Some(date(2025, Month::January, 1)),
        ),
    ];
    assert_eq!(
        build(&data, &job()).paragraphs()[1],
        "Mein Abschluss null an null bildet eine fachliche Grundlage für diese Aufgabe."
    );
    data.experiences = vec![experience(None, None, None, None, 0)];
    assert_eq!(
        build(&data, &job()).paragraphs()[1],
        "In meiner bisherigen Tätigkeit als null bei null habe ich praktische Erfahrung gesammelt."
    );
}

#[test]
fn without_evidence_and_skills_keeps_two_source_paragraphs() {
    let mut data = profile();
    data.experiences = Vec::new();
    data.education = Vec::new();
    data.skills = Vec::new();
    let mut context = job();
    context.job.company = Some(" ".to_owned());
    context.job.location = Some(" ".to_owned());
    let content = build(&data, &context);
    assert_eq!(content.recipient_company(), None);
    assert_eq!(content.recipient_location(), None);
    assert_eq!(content.subject(), "Bewerbung als AI SQL Developer");
    assert_eq!(content.paragraphs().len(), 2);
    review_cover_letter_content(&content).unwrap();
}

#[test]
fn cover_token_selection_excludes_company_seniority_and_uses_minimum_two() {
    let mut data = profile();
    data.skills = vec![skill("AI", None, 9), skill("Java", None, 0)];
    let mut context = job();
    context.job.title = "AI".to_owned();
    context.job.company = Some("Java".to_owned());
    context.job.seniority = Some("Java".to_owned());
    context.job.experience_requirement = Some("Java".to_owned());
    let content = build(&data, &context);
    assert!(content.paragraphs()[2].contains("Kenntnisse in AI mit."));
    assert!(!content.paragraphs()[2].contains("Java"));
}

#[test]
fn null_description_is_source_literal_token_not_empty() {
    // The typed job holds a required description, so an absent description read as the
    // literal `null` is the description text `null` here.
    let mut data = profile();
    data.skills = vec![skill("null", None, 9), skill("Python", None, 0)];
    let mut context = job();
    context.job.description = "null".to_owned();
    assert!(build(&data, &context).paragraphs()[2].contains("Kenntnisse in null mit."));
}

#[test]
fn skill_fallback_has_no_category_priority_and_max_five() {
    let mut data = profile();
    data.skills = (0..9)
        .map(|index| {
            skill(
                &format!("Skill {index}"),
                (index == 8).then_some("Machine learning"),
                index,
            )
        })
        .collect();
    assert_eq!(
        build(&data, &job()).paragraphs()[2],
        "Für diese Position bringe ich insbesondere Kenntnisse in \
         Skill 0, Skill 1, Skill 2, Skill 3, Skill 4 mit."
    );
}

#[test]
fn source_utf16_contact_and_skill_order() {
    let mut data = profile();
    data.contacts = vec![
        contact("\u{e000}", None, "b"),
        contact("\u{1f600}", None, "s"),
    ];
    data.skills = vec![skill("\u{e000}", None, 0), skill("\u{1f600}", None, 0)];
    let content = build(&data, &job());
    assert_eq!(labels(&content)[1..3], ["\u{1f600}", "\u{e000}"]);
    assert!(content.paragraphs()[2].contains("Kenntnisse in \u{1f600}, \u{e000} mit."));
}

#[test]
fn budget_excludes_unselected_private_text_and_projects() {
    let big = "x".repeat(1_000_001);
    let mut data = profile();
    data.profile.summary = Some(big.clone());
    data.experiences[0].description = Some(big.clone());
    assert_eq!(build(&data, &job()).sender_name(), "Ada Example");
    let mut context = job();
    context.job.description = big;
    assert_eq!(
        limit_message(build_german_cover_letter(&data, &context)),
        LIMIT
    );
    data.links = vec![link("x", None, "x"); 10_001];
    assert_eq!(
        limit_message(build_german_cover_letter(&data, &job())),
        LIMIT
    );
}

#[test]
fn review_all_ordered_defects_are_fixed_and_sanitized() {
    let value = content_with(|input| {
        input.personal_fields = Vec::new();
        input.subject = Some("bad".to_owned());
        input.paragraphs = paragraphs_of(&["a\u{1}"]);
    });
    let error = review_failure(&value);
    assert_eq!(
        defects(&error),
        "contact email is required; at least two ordered paragraphs are required; \
         subject is too short; subject must identify the job title; \
         content must not contain control characters; \
         rendered body is too short to be a usable cover letter"
    );
    assert_eq!(error.details().len(), 1);
}

#[test]
fn review_iso_control_across_selected_fields() {
    let long = "x".repeat(150);
    for character in ['\u{0}', '\u{1f}', '\u{7f}', '\u{85}', '\u{9f}'] {
        let text = format!("a{character}b");
        for field in [
            "sender_name",
            "recipient_company",
            "recipient_location",
            "job_title",
            "subject",
            "salutation",
            "closing",
            "signature",
        ] {
            let value = content_with(|input| {
                let slot = match field {
                    "sender_name" => &mut input.sender_name,
                    "recipient_company" => &mut input.recipient_company,
                    "recipient_location" => &mut input.recipient_location,
                    "job_title" => &mut input.job_title,
                    "subject" => &mut input.subject,
                    "salutation" => &mut input.salutation,
                    "closing" => &mut input.closing,
                    _ => &mut input.signature,
                };
                *slot = Some(text.clone());
                input.paragraphs = paragraphs_of(&[&long, "b"]);
            });
            let error = review_failure(&value);
            assert!(
                defects(&error).contains("content must not contain control characters"),
                "{field} {character:?}"
            );
        }
    }
}

#[test]
fn non_iso_control_and_utf16_body_minimum() {
    for character in ["\u{a0}", "\u{2028}", "\u{1f600}"] {
        let value = content_with(|input| {
            input.paragraphs = paragraphs_of(&[&format!("{}{character}", "a".repeat(150)), "b"]);
        });
        review_cover_letter_content(&value).unwrap();
    }
}

#[test]
fn review_exact_utf16_threshold_and_subject_length() {
    let base = content_with(|_| {});
    let missing = 140 - utf16_length(&render_cover_letter_body(&base).unwrap());
    let sized = |count: usize| {
        content_with(|input| input.paragraphs = paragraphs_of(&[&"a".repeat(count), "b"]))
    };
    review_cover_letter_content(&sized(missing + 1)).unwrap();
    let error = review_failure(&sized(missing));
    assert_eq!(
        defects(&error),
        "rendered body is too short to be a usable cover letter"
    );
    assert_eq!(error.details().len(), 1);
    let emoji = content_with(|input| {
        input.job_title = Some("\u{1f600}".to_owned());
        input.subject = Some("1234567890\u{1f600}".to_owned());
        input.paragraphs = paragraphs_of(&[&"a".repeat(150), "b"]);
    });
    review_cover_letter_content(&emoji).unwrap();
}

#[test]
fn personal_field_and_paragraph_controls_checked() {
    let long = "a".repeat(150);
    let cases = [
        (("E-Mail\u{1}", "e@x"), long.clone(), "b".to_owned()),
        (("E-Mail", "e@x\u{7f}"), long.clone(), "b".to_owned()),
        (("E-Mail", "e@x"), long, "b\u{85}".to_owned()),
    ];
    for ((label, value), first, second) in cases {
        let content = content_with(|input| {
            input.personal_fields = vec![PersonalField::new(Some(label), Some(value)).unwrap()];
            input.paragraphs = paragraphs_of(&[&first, &second]);
        });
        let error = review_failure(&content);
        assert_eq!(
            defects(&error),
            "content must not contain control characters"
        );
    }
}
