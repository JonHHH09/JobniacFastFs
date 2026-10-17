//! German resume preparation tests: one test per function of the preparation, with the parametrized
//! cases looped inside. Every text is synthetic. The German body expectations use the generic
//! built-in glossary. Cases that need an untyped input (a missing key, a null contact type, a list
//! offset, a surrogate-pair offset) cannot be built against the typed graph; the null contact type
//! is covered by a unit test of the sort in the module.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;

use jobniac_core::errors::{ApplicationError, ErrorCode};
use jobniac_core::generation::content::default_sentence_boundaries;
use jobniac_core::german::content::{
    EducationEntry, ExperienceEntry, ResumeInput, StructuredResumeContent,
};
use jobniac_core::german::resume::{
    PersonalDetails, build_english_resume, parse_include_projects, parse_personal_details,
    prepare_german_resume, render_resume_body, review_resume_content, split_german_bullets,
};
use jobniac_core::german::translation::GermanResumeTranslator;
use jobniac_core::jobs::model::{JobAggregate, JobRecord};
use jobniac_core::profiles::model::{
    ContactRecord, EducationRecord, ExperienceRecord, LanguageRecord, LinkRecord, ProfileGraph,
    ProfileRecord, ProjectRecord, ProjectTechnologyRecord, SkillRecord,
};
use serde_json::{Value, json};
use time::{Date, Month, OffsetDateTime};
use uuid::Uuid;

const LIMIT: &str = "Resume preparation limit exceeded";

fn now() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH
}

fn date(text: &str) -> Date {
    let year = text[0..4].parse().unwrap();
    let month = Month::try_from(text[5..7].parse::<u8>().unwrap()).unwrap();
    Date::from_calendar_date(year, month, text[8..10].parse().unwrap()).unwrap()
}

fn text(value: &str) -> Option<String> {
    Some(value.to_owned())
}

fn contact(contact_type: &str, label: Option<&str>, value: &str) -> ContactRecord {
    ContactRecord {
        id: Uuid::nil(),
        profile_id: Uuid::nil(),
        contact_type: contact_type.to_owned(),
        contact_value: value.to_owned(),
        label: label.map(str::to_owned),
        created_at: now(),
        updated_at: now(),
    }
}

fn skill(name: &str, category: Option<&str>, order: i64) -> SkillRecord {
    SkillRecord {
        id: Uuid::nil(),
        profile_id: Uuid::nil(),
        skill: name.to_owned(),
        normalized_skill: name.to_lowercase(),
        category: category.map(str::to_owned),
        display_order: order,
        created_at: now(),
    }
}

fn experience(
    title: Option<&str>,
    company: Option<&str>,
    order: i64,
    start: Option<&str>,
    end: Option<&str>,
    description: Option<&str>,
) -> ExperienceRecord {
    ExperienceRecord {
        id: Uuid::nil(),
        profile_id: Uuid::nil(),
        company: company.map(str::to_owned),
        title: title.map(str::to_owned),
        location: None,
        start_date: start.map(date),
        end_date: end.map(date),
        description: description.map(str::to_owned),
        display_order: order,
        created_at: now(),
    }
}

fn education(
    degree: Option<&str>,
    institution: Option<&str>,
    end: Option<&str>,
) -> EducationRecord {
    EducationRecord {
        id: Uuid::nil(),
        profile_id: Uuid::nil(),
        institution: institution.map(str::to_owned),
        degree: degree.map(str::to_owned),
        field: None,
        location: None,
        start_date: None,
        end_date: end.map(date),
        relevant_focus: None,
        created_at: now(),
    }
}

fn project(
    name: &str,
    url: Option<&str>,
    description: Option<&str>,
    order: i64,
    technologies: &[&str],
) -> ProjectRecord {
    ProjectRecord {
        id: Uuid::nil(),
        profile_id: Uuid::nil(),
        name: text(name),
        url: url.map(str::to_owned),
        description: description.map(str::to_owned),
        technologies: technologies
            .iter()
            .map(|technology| ProjectTechnologyRecord {
                id: Uuid::nil(),
                project_id: Uuid::nil(),
                technology: (*technology).to_owned(),
                normalized_technology: technology.to_lowercase(),
                display_order: 0,
                created_at: now(),
            })
            .collect(),
        display_order: order,
        created_at: now(),
    }
}

fn profile() -> ProfileGraph {
    ProfileGraph {
        profile: ProfileRecord {
            id: Uuid::nil(),
            full_name: "Ada Example".to_owned(),
            email: "ada@example.test".to_owned(),
            summary: text("Built systems using Java."),
            raw_resume_text: None,
            created_at: now(),
            updated_at: now(),
            embedding: None,
            revision: 0,
        },
        contacts: vec![contact(
            "address",
            Some("Address"),
            "Quorvex Harbor, Zelandia",
        )],
        links: vec![LinkRecord {
            id: Uuid::nil(),
            profile_id: Uuid::nil(),
            link_type: "github".to_owned(),
            url: "https://example.test".to_owned(),
            label: text("GitHub"),
            created_at: now(),
            updated_at: now(),
        }],
        skills: vec![
            skill("Java", Some("Backend"), 1),
            skill("SQL", Some("Data"), 0),
            skill("Decorative", Some("Architecture"), 0),
        ],
        languages: vec![LanguageRecord {
            id: Uuid::nil(),
            profile_id: Uuid::nil(),
            language: "English".to_owned(),
            normalized_language: "english".to_owned(),
            proficiency: text("Fluent"),
            display_order: 0,
            created_at: now(),
        }],
        education: vec![EducationRecord {
            field: text("Quorvex Studies"),
            location: text("Vystra"),
            relevant_focus: text("Quorvex lattices"),
            ..education(
                Some("B.A. Quorvex Studies"),
                Some("Example University"),
                Some("2019-06-01"),
            )
        }],
        experiences: vec![ExperienceRecord {
            location: text("Quorvex Harbor, Zelandia"),
            ..experience(
                Some("Quorvex Application Developer"),
                Some("Example Company"),
                0,
                Some("2020-01-01"),
                None,
                Some("- Built Java systems.\n* Worked on SQL jobs."),
            )
        }],
        projects: Vec::new(),
        project_technologies: Vec::new(),
    }
}

fn job_with(title: &str) -> JobAggregate {
    JobAggregate {
        job: JobRecord {
            id: Uuid::nil(),
            source_method: "text".to_owned(),
            source_label: None,
            title: title.to_owned(),
            company: None,
            location: None,
            description: String::new(),
            experience_requirement: None,
            employment_type: None,
            seniority: None,
            posted_at: None,
            canonical_fingerprint: String::new(),
            created_at: now(),
            updated_at: now(),
            revision: 0,
        },
        skills: Vec::new(),
        link_ingestion: None,
        text_ingestion: None,
    }
}

fn job() -> JobAggregate {
    job_with("Java SQL Developer")
}

fn finder() -> impl Fn(&str) -> Option<Vec<usize>> {
    default_sentence_boundaries
}

fn english(graph: &ProfileGraph, job: &JobAggregate, projects: bool) -> StructuredResumeContent {
    build_english_resume(graph, job, None, projects, &finder()).unwrap()
}

fn try_english(
    graph: &ProfileGraph,
    job: &JobAggregate,
    details: Option<&PersonalDetails>,
    projects: bool,
) -> Result<StructuredResumeContent, ApplicationError> {
    build_english_resume(graph, job, details, projects, &finder())
}

fn split(text: &str) -> Vec<String> {
    split_german_bullets(Some(text), &finder()).unwrap()
}

fn assert_limit<T>(result: Result<T, ApplicationError>) {
    match result {
        Err(error) => assert_eq!(error.message(), LIMIT),
        Ok(_) => panic!("expected the preparation limit error"),
    }
}

fn detail(error: &ApplicationError, key: &str) -> Option<String> {
    error.details().get(key).cloned()
}

const EN_BODY: &str = "Ada Example\nEmail: ada@example.test\nAddress: Quorvex Harbor, Zelandia\nGitHub: https://example.test\n\
Date of birth: 1901-02-03\nNationality: Zelandian\n\nPROFILE\nBuilt systems using Java.\n\n\
PROFESSIONAL EXPERIENCE\nQuorvex Application Developer | Example Company\n\
01/2020 - Present | Quorvex Harbor, Zelandia\n- Built Java systems.\n- Worked on SQL jobs.\n\n\
EDUCATION\nB.A. Quorvex Studies | Example University\nUnknown - 06/2019 | Vystra\n\
- Field of study: Quorvex Studies\n- Quorvex lattices\n\
- International degree program (Vystra).\n\nSKILLS\nData: SQL\nBackend: Java\n\nLANGUAGES\nEnglish (Fluent)";

const DE_BODY: &str = "Ada Example\nE-Mail: ada@example.test\nAdresse: Quorvex Harbor, Zelandia\nGitHub: https://example.test\n\
Geburtsdatum: 1901-02-03\nStaatsangehörigkeit: Zelandian\n\nPROFIL\nErstellte Systeme mit Java.\n\n\
BERUFSERFAHRUNG\nQuorvex Anwendung Developer | Example Company\n\
01/2020 - heute | Quorvex Harbor, Zelandia\n- Erstellte Java systems.\n- Arbeitete on SQL jobs.\n\n\
AUSBILDUNG\nB.A. Quorvex Studies | Example University\nunbekannt - 06/2019 | Vystra\n\
- Studienfach: Quorvex Studies\n- Quorvex lattices\n\
- International Abschluss program (Vystra).\n\nKENNTNISSE\nDaten: SQL\nBackend: Java\n\nSPRACHEN\nEnglish (fließend)";

fn details() -> PersonalDetails {
    PersonalDetails {
        date_of_birth: text("1901-02-03"),
        nationality: text("Zelandian"),
    }
}

#[test]
fn complete_bilingual_source_bodies_and_immutable_input() {
    let data = profile();
    let before = data.clone();
    let (english, german) = prepare_german_resume(
        &data,
        &job(),
        Some(&details()),
        false,
        &finder(),
        &GermanResumeTranslator::builtin(),
    )
    .unwrap();
    assert_eq!(english.language(), "en");
    assert_eq!(german.language(), "de");
    assert_eq!(render_resume_body(&english).unwrap(), EN_BODY);
    assert_eq!(render_resume_body(&german).unwrap(), DE_BODY);
    assert_eq!(data, before);
    assert!(english.additional().is_empty() && german.additional().is_empty());
}

#[test]
fn projects_omitted_without_reading_or_budgeting() {
    let mut data = profile();
    let huge = "x".repeat(1_000_001);
    data.projects = vec![project("Huge", None, Some(huge.as_str()), 0, &[])];
    assert!(english(&data, &job(), false).additional().is_empty());
    assert_limit(try_english(&data, &job(), None, true));
}

#[test]
fn project_selection_order_and_relevance_before_three_bullet_limit() {
    let mut data = profile();
    data.projects = vec![
        project(
            "Java tools",
            Some("https://example.test/java"),
            Some("A\nB\nC\nJava"),
            2,
            &[" SQL ", "Java"],
        ),
        project("SQL tools", None, None, 1, &[]),
        project("Other", None, None, 0, &[]),
    ];
    let content = english(&data, &job(), true);
    let titles: Vec<&str> = content.additional().iter().map(|row| row.title()).collect();
    assert_eq!(titles, ["Java tools", "SQL tools"]);
    assert_eq!(
        content.additional()[0].bullets(),
        ["Java", "A", "B", "Technologies:  SQL , Java"]
    );
    assert_eq!(
        content.additional()[0].organization(),
        Some("https://example.test/java")
    );
    let body = render_resume_body(&content).unwrap();
    assert!(body.find("PROJECTS").unwrap() < body.find("EDUCATION").unwrap());
}

#[test]
fn include_projects_exact_bool() {
    for option in [Value::Null, json!(0), json!(1), json!("true")] {
        let error = parse_include_projects(&option).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ValidationError);
        assert_eq!(error.message(), "Invalid German resume preparation input");
        assert_eq!(detail(&error, "field").as_deref(), Some("includeProjects"));
        assert_eq!(
            detail(&error, "reason").as_deref(),
            Some("must be a boolean")
        );
        assert_eq!(error.details().len(), 2);
    }
    assert!(parse_include_projects(&json!(true)).unwrap());
    assert!(!parse_include_projects(&json!(false)).unwrap());
}

#[test]
fn bullets_rank_all_candidates_before_limit_stable_ties() {
    let mut data = profile();
    data.experiences[0].description = text("A\nB\nC\nD\nE\nSQL\nJava");
    let content = english(&data, &job(), false);
    assert_eq!(
        content.experiences()[0].bullets(),
        ["SQL", "Java", "A", "B", "C"]
    );
}

#[test]
fn source_multiline_and_ascii_prefix_semantics() {
    let cases: [(&str, &[&str]); 4] = [
        ("a\r\nb\rc\n", &["a", "b", "c"]),
        ("- a\n* b\n• c\n2.) d", &["a", "b", "c", "2.) d"]),
        ("1. a\n2)\tb\n3.\u{a0}c", &["a", "b", "3.\u{a0}c"]),
        ("a\u{2028}b\nc\u{85}d", &["a\u{2028}b", "c\u{85}d"]),
    ];
    for (input, expected) in cases {
        assert_eq!(split(input), expected, "{input:?}");
    }
}

#[test]
fn actual_jdk_25_english_boundary_fixtures() {
    // Exact Locale.ENGLISH source BreakIterator probe; offsets converted to code points.
    let cases: [(&str, &[usize], &[&str]); 4] = [
        (
            "Dr. Smith built Java. SQL is next.",
            &[0, 4, 22, 34],
            &["Dr.", "Smith built Java.", "SQL is next."],
        ),
        (
            "First sentence. lower continuation. Next.",
            &[0, 36, 41],
            &["First sentence. lower continuation.", "Next."],
        ),
        (
            "Version 2.5 works. \"Next\" sentence!",
            &[0, 19, 35],
            &["Version 2.5 works.", "\"Next\" sentence!"],
        ),
        (
            "Built 😀 software. Tested it.",
            &[0, 18, 28],
            &["Built 😀 software.", "Tested it."],
        ),
    ];
    for (input, offsets, expected) in cases {
        let result = split_german_bullets(Some(input), &|_| Some(offsets.to_vec())).unwrap();
        assert_eq!(result, expected, "{input:?}");
    }
}

#[test]
fn one_survivor_uses_backend_even_after_multiple_raw_lines() {
    let seen = RefCell::new(Vec::new());
    let backend = |text: &str| {
        seen.borrow_mut().push(text.to_owned());
        Some(vec![0, text.chars().count()])
    };
    let result = split_german_bullets(Some(" \n\n* One sentence."), &backend).unwrap();
    assert_eq!(result, ["One sentence."]);
    assert_eq!(*seen.borrow(), ["One sentence."]);
}

#[test]
fn bare_markers_survive_java_strip_before_prefix_matching() {
    assert_eq!(split("1. \n2) "), ["1.", "2)"]);
    assert_eq!(split("- \n* "), ["-", "*"]);
}

#[test]
fn invalid_backend_offsets_fail_safely() {
    let cases: [&[usize]; 5] = [&[], &[0, 0, 1], &[0, 2], &[1], &[0, 1, 2, 3]];
    for offsets in cases {
        let error = split_german_bullets(Some("a"), &|_| Some(offsets.to_vec())).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InternalError, "{offsets:?}");
        assert_eq!(
            error.message(),
            "German sentence boundary backend returned invalid offsets"
        );
    }
}

#[test]
fn default_backend_and_injected_failure_sanitized() {
    assert_eq!(split("PRIVATE narrative."), ["PRIVATE narrative."]);
    let error = split_german_bullets(Some("PRIVATE narrative."), &|_| None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InternalError);
    assert_eq!(error.message(), "German sentence boundary backend failed");
    assert!(error.details().is_empty());
}

#[test]
fn distinct_resume_email_filter_and_java_type_order() {
    let mut data = profile();
    data.contacts = vec![
        contact("mail", None, "postal"),
        contact("phone", Some("Email"), "123"),
        contact("phone", None, "example@invalid"),
        contact("email secondary", None, "hidden"),
        contact("\u{e000}", Some("BMP"), "b"),
        contact("😀", Some("Supplementary"), "s"),
    ];
    let content = english(&data, &job(), false);
    let fields: Vec<(&str, &str)> = content
        .personal_fields()
        .iter()
        .map(|field| (field.label(), field.value()))
        .collect();
    assert_eq!(
        fields,
        [
            ("Email", "ada@example.test"),
            ("mail", "postal"),
            ("Email", "123"),
            ("Supplementary", "s"),
            ("BMP", "b"),
            ("GitHub", "https://example.test"),
        ]
    );
}

#[test]
fn source_experience_and_education_ties_and_fallbacks() {
    let mut data = profile();
    data.experiences = vec![
        experience(
            Some("Later display"),
            Some("Z"),
            3,
            Some("2020-01-01"),
            None,
            None,
        ),
        experience(
            Some("Earlier display"),
            Some("A"),
            1,
            None,
            Some("2020-01-01"),
            None,
        ),
        experience(None, Some(" "), 0, None, None, None),
    ];
    data.education = vec![
        education(Some("Z"), Some("First"), Some("2020-01-01")),
        education(Some("A"), Some("Second"), Some("2020-01-01")),
        education(None, None, None),
    ];
    let content = english(&data, &job(), false);
    let titles: Vec<&str> = content
        .experiences()
        .iter()
        .map(|row| row.title())
        .collect();
    assert_eq!(titles, ["Earlier display", "Later display", "Role"]);
    let degrees: Vec<&str> = content.education().iter().map(|row| row.degree()).collect();
    assert_eq!(degrees, ["Z", "A", "Degree"]);
    assert_eq!(content.experiences()[2].company(), "Company");
    assert!(render_resume_body(&content).unwrap().contains("Unknown\n"));
}

#[test]
fn skill_fallback_category_priority_relevance_and_cap() {
    let mut data = profile();
    data.skills = vec![
        skill("Java", Some("Backend"), 0),
        skill("Docker", Some("Cloud / Ops"), 0),
        skill("MLOps", Some("Machine learning"), 0),
    ];
    let categories = |content: &StructuredResumeContent| -> Vec<String> {
        content
            .skill_groups()
            .iter()
            .map(|group| group.category().to_owned())
            .collect()
    };
    assert_eq!(
        categories(&english(&data, &job_with("zzz"), false)),
        ["Machine learning", "Cloud / Ops", "Backend"]
    );
    data.skills = (0..30)
        .map(|index| skill(&format!("Java Tool {index}"), None, index))
        .collect();
    let content = english(&data, &job(), false);
    let groups = content.skill_groups();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].category(), "Technical");
    assert_eq!(groups[0].skills().len(), 24);
    assert_eq!(groups[0].skills()[23], "Java Tool 23");
}

#[test]
fn two_character_tokens_do_not_match_resume_but_company_and_requirements_do() {
    let mut data = profile();
    data.skills = vec![
        skill("AI", Some("Architecture"), 0),
        skill("Java", Some("Backend"), 1),
        skill("Python", None, 2),
    ];
    let mut context = job_with("AI");
    context.job.company = text("Java");
    context.job.experience_requirement = text("Python");
    let content = english(&data, &context, false);
    let skills: Vec<&String> = content
        .skill_groups()
        .iter()
        .flat_map(|group| group.skills())
        .collect();
    assert_eq!(skills, ["Java", "Python"]);
}

#[test]
fn budget_selected_job_and_profile_not_unrelated_private_metadata() {
    let mut data = profile();
    data.profile.raw_resume_text = text(&"x".repeat(1_000_001));
    assert_eq!(english(&data, &job(), false).full_name(), "Ada Example");
    let mut context = job();
    context.job.description = "x".repeat(1_000_001);
    assert_limit(try_english(&data, &context, None, false));
    data.contacts = vec![contact("phone", Some("P"), "1"); 10_001];
    assert_limit(try_english(&data, &job(), None, false));
}

#[test]
fn content_review_defect_order_and_translation_failure() {
    let content = StructuredResumeContent::new(ResumeInput {
        full_name: text("N"),
        language: text("en"),
        experiences: vec![
            ExperienceEntry::new(Some(""), Some(""), None, None, None, None).unwrap(),
        ],
        education: vec![EducationEntry::new(Some(""), Some(""), None, None, None, None).unwrap()],
        ..ResumeInput::default()
    })
    .unwrap();
    let error = review_resume_content(&content).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ValidationError);
    assert_eq!(
        error.message(),
        "German tailored resume content review failed"
    );
    assert_eq!(error.details().len(), 1);
    assert_eq!(
        detail(&error, "defects").as_deref(),
        Some(
            "contact email is required; experience title and company must not be blank; \
             education degree and institution must not be blank"
        )
    );
    let mut data = profile();
    data.experiences[0].title = text("the");
    let error = prepare_german_resume(
        &data,
        &job(),
        None,
        false,
        &finder(),
        &GermanResumeTranslator::builtin(),
    )
    .unwrap_err();
    assert!(error.message().contains("content review failed"));
}

#[test]
fn rendering_over_50000_is_not_generic_pdf_gate() {
    let mut data = profile();
    data.profile.summary = text(&"x".repeat(50_001));
    let content = english(&data, &job(), false);
    review_resume_content(&content).unwrap();
    assert!(render_resume_body(&content).unwrap().chars().count() > 50_000);
}

#[test]
fn injected_personal_details_root_rejects_fixed_safely() {
    for value in [json!("PRIVATE details"), json!(1), json!([])] {
        let error = parse_personal_details(&value).unwrap_err();
        let envelope = error.to_envelope();
        assert_eq!(envelope.code, ErrorCode::ValidationError);
        assert_eq!(envelope.message, "Invalid personal details");
        assert_eq!(envelope.details.len(), 2);
        assert_eq!(envelope.details["field"], "personalDetails");
        assert_eq!(envelope.details["reason"], "must be an object");
    }
    assert_eq!(parse_personal_details(&Value::Null).unwrap(), None);
    assert_eq!(
        parse_personal_details(&json!({"dateOfBirth": "1901-02-03", "nationality": null})).unwrap(),
        Some(PersonalDetails {
            date_of_birth: text("1901-02-03"),
            nationality: None,
        })
    );
}

#[test]
fn injected_noncanonical_date_rejects_without_echo() {
    for value in ["2020-1-1", "20200101", "PRIVATE date", "2020-02-31"] {
        let details = PersonalDetails {
            date_of_birth: text(value),
            nationality: None,
        };
        let error = try_english(&profile(), &job(), Some(&details), false).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ValidationError);
        assert_eq!(error.message(), "Invalid personal details");
        assert_eq!(error.details().len(), 1);
        assert_eq!(detail(&error, "field").as_deref(), Some("dateOfBirth"));
    }
}

#[test]
fn to_german_translates_structured_content_and_keeps_names() {
    let translator = GermanResumeTranslator::builtin();
    let content = english(&profile(), &job(), false);
    let german = translator.to_german(&content).unwrap();
    assert_eq!(german.language(), "de");
    assert_eq!(german.full_name(), "Ada Example");
    assert_eq!(german.summary(), Some("Erstellte Systeme mit Java."));
    let labels: Vec<&str> = german
        .personal_fields()
        .iter()
        .map(|field| field.label())
        .collect();
    assert_eq!(labels, ["E-Mail", "Adresse", "GitHub"]);
    assert_eq!(german.experiences()[0].company(), "Example Company");
    assert_eq!(german.education()[0].institution(), "Example University");
    assert_eq!(german.languages()[0].proficiency(), Some("fließend"));
    assert_eq!(german.skill_groups()[0].skills(), ["SQL"]);
}

#[test]
fn single_value_and_aggregate_limits_are_safe() {
    let translator = GermanResumeTranslator::builtin();
    assert_limit(translator.translate_text(Some(&"x".repeat(1_000_001))));
}
