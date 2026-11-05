//! Europass parser: golden vectors for the two sample exports and hostile-input tests.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::europass::{
    EUROPASS_INVALID_MESSAGE, EUROPASS_UNSUPPORTED_MESSAGE, EuropassDocument, EuropassSchema,
    MAX_INPUT_BYTES, detect, parse,
};
use serde_json::Value;

const CV: &[u8] = include_bytes!("fixtures/europass/cv-alex-sample.xml");
const LETTER: &[u8] = include_bytes!("fixtures/europass/cover-letter-alex-sample.xml");
const CV_GOLDEN: &str = include_str!("golden/europass-cv-alex-sample.json");
const LETTER_GOLDEN: &str = include_str!("golden/europass-cover-letter-alex-sample.json");

fn as_json(document: &EuropassDocument) -> Value {
    serde_json::to_value(document).unwrap()
}

fn golden(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn reason_of(error: &AppError) -> (String, String) {
    let envelope = error_envelope(error);
    assert_eq!(envelope.code.code(), "validation_error");
    (
        envelope.message,
        envelope.details.get("reason").cloned().unwrap_or_default(),
    )
}

fn assert_rejected(bytes: &[u8], reason: &str) {
    let error = parse(bytes).unwrap_err();
    assert_eq!(
        reason_of(&error),
        (EUROPASS_INVALID_MESSAGE.to_owned(), reason.to_owned())
    );
    assert_eq!(detect(bytes), None);
}

#[test]
fn cv_sample_matches_the_golden() {
    assert_eq!(detect(CV), Some(EuropassSchema::CandidateV4));
    assert_eq!(as_json(&parse(CV).unwrap()), golden(CV_GOLDEN));
}

#[test]
fn cover_letter_sample_matches_the_golden() {
    assert_eq!(detect(LETTER), Some(EuropassSchema::SkillsPassportV3));
    assert_eq!(as_json(&parse(LETTER).unwrap()), golden(LETTER_GOLDEN));
}

#[test]
fn cv_sample_has_no_unsupported_sections_and_the_expected_shape() {
    let EuropassDocument::Cv(cv) = parse(CV).unwrap() else {
        panic!("expected a CV");
    };
    assert!(cv.unsupported_sections.is_empty());
    assert!(cv.truncated_fields.is_empty());
    assert_eq!(cv.employment.len(), 2);
    assert_eq!(cv.education.len(), 1);
    assert_eq!(cv.languages.len(), 1);
    assert_eq!(cv.skills.len(), 5);
    assert!(cv.invalid_fields.is_empty());
    assert_eq!(cv.person.phones[0].international(), "+1 2025550147");
}

#[test]
fn non_empty_unmapped_sections_are_listed_and_unknown_elements_ignored() {
    let xml = String::from_utf8(CV.to_vec())
        .unwrap()
        .replace(
            "<VoluntaryWorks/>",
            "<VoluntaryWorks><VoluntaryWork>Food bank</VoluntaryWork></VoluntaryWorks>",
        )
        .replace(
            "<Certifications/>",
            "<Certifications><X/></Certifications><Brand-New>1</Brand-New>",
        );
    let EuropassDocument::Cv(cv) = parse(xml.as_bytes()).unwrap() else {
        panic!("expected a CV");
    };
    assert_eq!(
        cv.unsupported_sections,
        ["Certifications", "Brand-New", "VoluntaryWorks"]
    );
    assert_eq!(cv.skills.len(), 5);
}

#[test]
fn invalid_dates_and_overlong_text_are_handled() {
    let bad = String::from_utf8(CV.to_vec())
        .unwrap()
        .replace("2023-03-15", "2023-02-30");
    let EuropassDocument::Cv(cv) = parse(bad.as_bytes()).unwrap() else {
        panic!("expected a CV");
    };
    assert_eq!(cv.employment[0].period.start, None);
    assert_eq!(cv.invalid_fields, ["employment[0].period.start"]);

    let bad_birth = String::from_utf8(CV.to_vec())
        .unwrap()
        .replace("1998-05-14", "14.05.1998")
        .replace("2021-06-30", "2021-06-30+02:00");
    let EuropassDocument::Cv(cv) = parse(bad_birth.as_bytes()).unwrap() else {
        panic!("expected a CV");
    };
    assert_eq!(cv.person.birth_date, None);
    assert_eq!(cv.invalid_fields, ["person.birthDate"]);
    assert_eq!(
        cv.education[0].period.end.unwrap().to_string(),
        "2021-06-30"
    );

    let long = "x".repeat(900);
    let xml = String::from_utf8(CV.to_vec())
        .unwrap()
        .replace("Software QA Engineer", &long);
    let EuropassDocument::Cv(cv) = parse(xml.as_bytes()).unwrap() else {
        panic!("expected a CV");
    };
    assert_eq!(cv.employment[0].position.as_deref().unwrap().len(), 500);
    assert_eq!(cv.truncated_fields, ["employment[0].position"]);
}

fn cv_with(replace: &str, with: &str) -> jobniac_core::europass::EuropassCv {
    let xml = String::from_utf8(CV.to_vec())
        .unwrap()
        .replace(replace, with);
    match parse(xml.as_bytes()).unwrap() {
        EuropassDocument::Cv(cv) => *cv,
        EuropassDocument::CoverLetter(_) => panic!("expected a CV"),
    }
}

#[test]
fn lists_are_capped_in_total_and_every_cap_is_recorded() {
    let employer = "<EmployerHistory><PositionHistory><PositionTitle>T</PositionTitle></PositionHistory><PositionHistory><PositionTitle>U</PositionTitle></PositionHistory></EmployerHistory>";
    let cv = cv_with(
        "</EmploymentHistory>",
        &format!("{}</EmploymentHistory>", employer.repeat(150)),
    );
    assert_eq!(cv.employment.len(), 200);
    assert!(cv.truncated_fields.contains(&"employment".to_owned()));

    let skill = "<PersonCompetency><hr:CompetencyName>S</hr:CompetencyName></PersonCompetency>";
    let cv = cv_with("</Skills>", &format!("{}</Skills>", skill.repeat(300)));
    assert_eq!(cv.skills.len(), 200);
    assert!(cv.truncated_fields.contains(&"skills".to_owned()));

    let degrees = "<EducationDegree><hr:DegreeName>D</hr:DegreeName></EducationDegree>";
    let attendance = format!(
        "<EducationOrganizationAttendance>{}</EducationOrganizationAttendance>",
        degrees.repeat(50)
    );
    let cv = cv_with(
        "</EducationHistory>",
        &format!("{}</EducationHistory>", attendance.repeat(50)),
    );
    assert_eq!(cv.education.len(), 200);
    assert!(cv.truncated_fields.contains(&"education".to_owned()));
}

#[test]
fn education_with_only_a_period_is_kept() {
    let text = String::from_utf8(CV.to_vec()).unwrap();
    let start = text.find("<EducationDegree>").unwrap();
    let end = text.find("</EducationDegree>").unwrap() + "</EducationDegree>".len();
    let without_degree = format!("{}{}", &text[..start], &text[end..]).replace(
        "<hr:OrganizationName>Fictional Technical University</hr:OrganizationName>",
        "",
    );
    let EuropassDocument::Cv(cv) = parse(without_degree.as_bytes()).unwrap() else {
        panic!("expected a CV");
    };
    assert_eq!(cv.education.len(), 1);
    assert_eq!(cv.education[0].organization, None);
    assert_eq!(cv.education[0].degree, None);
    assert_eq!(
        cv.education[0].period.start.unwrap().to_string(),
        "2017-09-01"
    );
}

#[test]
fn unreadable_cefr_levels_are_recorded() {
    let cv = cv_with(
        "<hr:ScoreText>B1</hr:ScoreText>",
        "<hr:ScoreText>native</hr:ScoreText>",
    );
    assert_eq!(cv.languages[0].listening, None);
    assert_eq!(cv.invalid_fields.len(), 5);
    assert_eq!(cv.invalid_fields[0], "languages[0].listening");
}

#[test]
fn rich_text_cut_drops_markup_and_flags_the_field() {
    let long = "word ".repeat(4_000);
    let cv = cv_with(
        "Software QA engineer with sample experience",
        &format!("{long} &amp;"),
    );
    let summary = cv.summary.unwrap();
    assert_eq!(summary.markup, None);
    assert!(cv.truncated_fields.contains(&"summary".to_owned()));
    let text = summary.plain();
    assert!(text.encode_utf16().count() <= 10_000 + 200);
    assert!(!text.contains('<'));

    let intact = cv_with("QA engineer", "QA engineer");
    assert!(intact.summary.unwrap().markup.is_some());
}

#[test]
fn child_elements_inside_rich_text_fields_are_reported() {
    let cv = cv_with(
        "</hr:ExecutiveSummary>",
        "<p>real child</p></hr:ExecutiveSummary>",
    );
    assert_eq!(cv.unsupported_sections, ["ExecutiveSummary/p"]);
}

#[test]
fn control_characters_in_attributes_are_cleaned() {
    let cv = cv_with("name=\"FREE_TEXT\"", "name=\"FREE_TEXT\"");
    assert_eq!(cv.person.primary_language.as_deref(), Some("English"));
    let cv = cv_with(
        "schemeName=\"FREE_TEXT\">Spanish",
        "schemeName=\"FREE&#9;TEXT\">Spanish",
    );
    assert_eq!(cv.languages[0].scheme.as_deref(), Some("FREE TEXT"));
}

#[test]
fn billion_laughs_is_rejected() {
    let xml = r#"<?xml version="1.0"?>
<!DOCTYPE lolz [
 <!ENTITY lol "lol">
 <!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">
 <!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">
]>
<Candidate xmlns="http://www.europass.eu/1.0">&lol3;</Candidate>"#;
    assert_rejected(xml.as_bytes(), "dtd_not_allowed");
}

#[test]
fn external_entities_are_rejected() {
    let xml = r#"<?xml version="1.0"?>
<!DOCTYPE c [ <!ENTITY secret SYSTEM "file:///etc/passwd"> ]>
<Candidate xmlns="http://www.europass.eu/1.0">&secret;</Candidate>"#;
    assert_rejected(xml.as_bytes(), "dtd_not_allowed");
}

#[test]
fn oversized_input_is_rejected() {
    let mut bytes = CV.to_vec();
    bytes.extend(std::iter::repeat_n(b' ', MAX_INPUT_BYTES));
    assert_rejected(&bytes, "too_large");
}

#[test]
fn invalid_utf8_is_rejected() {
    let mut bytes = CV.to_vec();
    bytes.splice(60..60, [0xFF, 0xFE, 0xC3]);
    assert_rejected(&bytes, "not_utf8");
}

#[test]
fn wrong_namespace_or_root_is_not_europass() {
    let wrong_ns = String::from_utf8(CV.to_vec()).unwrap().replacen(
        "xmlns=\"http://www.europass.eu/1.0\"",
        "xmlns=\"http://example.com/other\"",
        1,
    );
    assert_rejected(wrong_ns.as_bytes(), "not_europass");
    assert_rejected(b"<Candidate/>", "not_europass");
    assert_rejected(
        b"<html xmlns=\"http://www.europass.eu/1.0\"/>",
        "not_europass",
    );
    assert_rejected(b"not xml at all", "malformed_xml");
    assert_rejected(b"", "malformed_xml");
}

#[test]
fn deeply_nested_and_huge_trees_are_rejected() {
    let depth = 5_000;
    let mut deep = String::from("<Candidate xmlns=\"http://www.europass.eu/1.0\">");
    deep.push_str(&"<a>".repeat(depth));
    deep.push_str(&"</a>".repeat(depth));
    deep.push_str("</Candidate>");
    assert_rejected(deep.as_bytes(), "too_complex");

    let wide = format!(
        "<Candidate xmlns=\"http://www.europass.eu/1.0\">{}</Candidate>",
        "<a/>".repeat(30_000)
    );
    assert_rejected(wide.as_bytes(), "too_complex");
}

#[test]
fn legacy_cv_is_detected_but_unsupported() {
    let legacy = String::from_utf8(LETTER.to_vec()).unwrap().replace(
        "<DocumentType>ECL</DocumentType>",
        "<DocumentType>ECV</DocumentType>",
    );
    assert_eq!(
        detect(legacy.as_bytes()),
        Some(EuropassSchema::SkillsPassportV3)
    );
    let error = parse(legacy.as_bytes()).unwrap_err();
    assert_eq!(
        reason_of(&error),
        (
            EUROPASS_UNSUPPORTED_MESSAGE.to_owned(),
            "legacy_cv_not_supported".to_owned()
        )
    );
    let other = legacy.replace("ECV", "XYZ");
    assert_eq!(
        reason_of(&parse(other.as_bytes()).unwrap_err()).1,
        "document_type_not_supported"
    );
}

#[test]
fn cover_letter_dates_are_checked() {
    let bad = String::from_utf8(LETTER.to_vec())
        .unwrap()
        .replace("month=\"--11\"", "month=\"11\"");
    let EuropassDocument::CoverLetter(letter) = parse(bad.as_bytes()).unwrap() else {
        panic!("expected a letter");
    };
    assert_eq!(letter.date, None);
    assert_eq!(letter.invalid_fields, ["date"]);
}

fn assert_fast_rejection(xml: &str) {
    let started = std::time::Instant::now();
    assert_rejected(xml.as_bytes(), "too_complex");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "took {:?}",
        started.elapsed()
    );
}

#[test]
fn attribute_flooding_is_rejected_quickly() {
    let mut xml = String::from("<Candidate xmlns=\"http://www.europass.eu/1.0\" ");
    for n in 0..60_000 {
        xml.push_str(&format!("a{n}=\"x\" "));
    }
    xml.push_str("/>");
    assert!(xml.len() < MAX_INPUT_BYTES);
    assert_fast_rejection(&xml);
    // Spaces around `=` do not hide attributes.
    let spaced = format!(
        "<Candidate xmlns=\"http://www.europass.eu/1.0\" {}/>",
        (0..5_000)
            .map(|n| format!("b{n} = 'y' "))
            .collect::<String>()
    );
    assert_fast_rejection(&spaced);
}

#[test]
fn namespace_declaration_flooding_is_rejected_quickly() {
    let mut xml = String::from("<Candidate xmlns=\"http://www.europass.eu/1.0\"");
    for n in 0..30_000 {
        xml.push_str(&format!(" xmlns:a{n}=\"u{n}\""));
    }
    xml.push_str("/>");
    assert_fast_rejection(&xml);
}

#[test]
fn nested_namespace_declarations_are_rejected_quickly() {
    let depth = 3_000;
    let mut xml = String::from("<Candidate xmlns=\"http://www.europass.eu/1.0\">");
    for n in 0..depth {
        xml.push_str(&format!("<a xmlns:p{n}=\"u{n}\">"));
    }
    for _ in 0..depth {
        xml.push_str("</a>");
    }
    xml.push_str("</Candidate>");
    assert_fast_rejection(&xml);

    let mut deep = String::from("<Candidate xmlns=\"http://www.europass.eu/1.0\">");
    deep.push_str(&"<a>".repeat(10_000));
    deep.push_str(&"</a>".repeat(10_000));
    deep.push_str("</Candidate>");
    assert_fast_rejection(&deep);
}

#[test]
fn comments_and_cdata_do_not_count_as_nesting() {
    let xml = format!(
        "<?xml version=\"1.0\"?><Candidate xmlns=\"http://www.europass.eu/1.0\"><!-- {} --><![CDATA[{}]]></Candidate>",
        "<a><a><a>".repeat(100),
        "<b>".repeat(100)
    );
    assert_eq!(detect(xml.as_bytes()), Some(EuropassSchema::CandidateV4));
}
