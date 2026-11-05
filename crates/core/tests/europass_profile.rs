//! Europass CV to profile write request: a golden vector for the sample export and the edge cases
//! (missing email, long text, ongoing employment, mixed CEFR levels, empty sections).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::europass::{
    Cefr, EuropassCv, EuropassDocument, LanguageSkill, PartialDate, RichBlock, RichText, parse,
};
use jobniac_core::profiles::canonicalize_profile_write;
use jobniac_core::profiles::extract::{EXTRACTION_INPUT_MESSAGE, profile_write_from_europass_cv};
use serde_json::Value;

const CV: &[u8] = include_bytes!("fixtures/europass/cv-alex-sample.xml");
const GOLDEN: &str = include_str!("golden/europass-profile-write-alex-sample.json");

fn sample() -> EuropassCv {
    match parse(CV).unwrap() {
        EuropassDocument::Cv(cv) => *cv,
        EuropassDocument::CoverLetter(_) => panic!("expected a CV"),
    }
}

fn mapped(cv: &EuropassCv) -> Value {
    profile_write_from_europass_cv(cv).unwrap()
}

fn text(value: &str) -> RichText {
    RichText {
        markup: None,
        blocks: vec![RichBlock::Paragraph(value.to_owned())],
    }
}

fn reason_of(error: &AppError) -> (String, String, String) {
    let envelope = error_envelope(error);
    assert_eq!(envelope.code.code(), "validation_error");
    (
        envelope.message,
        envelope.details.get("field").cloned().unwrap_or_default(),
        envelope.details.get("reason").cloned().unwrap_or_default(),
    )
}

fn language(name: &str, levels: [Option<Cefr>; 5]) -> LanguageSkill {
    LanguageSkill {
        name: name.to_owned(),
        scheme: None,
        listening: levels[0],
        reading: levels[1],
        spoken_interaction: levels[2],
        spoken_production: levels[3],
        writing: levels[4],
    }
}

#[test]
fn sample_cv_maps_to_the_golden_profile_write() {
    let value = mapped(&sample());
    assert_eq!(value, serde_json::from_str::<Value>(GOLDEN).unwrap());
    canonicalize_profile_write(&value).expect("the mapping is a valid profile write");
}

#[test]
fn a_cv_without_an_email_is_the_extraction_email_error() {
    let mut cv = sample();
    cv.person.emails.clear();
    let error = profile_write_from_europass_cv(&cv).unwrap_err();
    assert_eq!(
        reason_of(&error),
        (
            EXTRACTION_INPUT_MESSAGE.to_owned(),
            "email".to_owned(),
            "could not be extracted from Europass CV".to_owned()
        )
    );
}

#[test]
fn a_cv_without_a_name_is_the_extraction_name_error() {
    let mut cv = sample();
    cv.person.given_name = None;
    cv.person.family_name = Some("  ".to_owned());
    let error = profile_write_from_europass_cv(&cv).unwrap_err();
    assert_eq!(reason_of(&error).1, "fullName");
}

#[test]
fn long_text_is_cut_to_the_extractor_limits_and_never_fails() {
    let mut cv = sample();
    cv.person.given_name = Some("N".repeat(300));
    cv.summary = Some(text(&"s".repeat(2000)));
    cv.employment[0].description = Some(text(&"e".repeat(2000)));
    cv.education[0].description = Some(text(&"\u{1F600}".repeat(400)));
    let value = mapped(&cv);
    assert_eq!(
        value["fullName"].as_str().unwrap().encode_utf16().count(),
        80
    );
    assert_eq!(value["summary"].as_str().unwrap().len(), 500);
    assert_eq!(
        value["experiences"][0]["description"]
            .as_str()
            .unwrap()
            .len(),
        500
    );
    let focus = value["education"][0]["relevantFocus"].as_str().unwrap();
    assert!(focus.encode_utf16().count() <= 500);
    canonicalize_profile_write(&value).unwrap();
}

#[test]
fn ongoing_employment_has_no_end_date_and_partial_dates_cover_their_period() {
    let mut cv = sample();
    let value = mapped(&cv);
    assert_eq!(value["experiences"][0]["startDate"], "2023-03-15");
    assert_eq!(value["experiences"][0]["endDate"], Value::Null);
    assert_eq!(value["experiences"][1]["endDate"], "2023-02-28");

    cv.employment[0].period.current = Some(false);
    cv.employment[0].period.start = PartialDate::new(2022, None, None);
    cv.employment[0].period.end = PartialDate::new(2022, Some(2), None);
    let value = mapped(&cv);
    assert_eq!(value["experiences"][0]["startDate"], "2022-01-01");
    assert_eq!(value["experiences"][0]["endDate"], "2022-02-28");
    canonicalize_profile_write(&value).unwrap();
}

#[test]
fn language_levels_are_one_level_when_equal_and_a_compact_form_otherwise() {
    let mut cv = sample();
    let b1 = Some(Cefr::B1);
    let b2 = Some(Cefr::B2);
    cv.person.primary_language = Some("English".to_owned());
    cv.languages = vec![
        language("Spanish", [b1; 5]),
        language("German", [b2, b2, b1, b1, b1]),
        language("French", [None, b2, None, None, None]),
        language("Latin", [None; 5]),
        language("spanish", [b2; 5]),
    ];
    let value = mapped(&cv);
    let languages = value["languages"].as_array().unwrap();
    let proficiency: Vec<(&str, &Value)> = languages
        .iter()
        .map(|entry| (entry["language"].as_str().unwrap(), &entry["proficiency"]))
        .collect();
    assert_eq!(
        proficiency,
        vec![
            ("English", &Value::from("native")),
            ("Spanish", &Value::from("B1")),
            ("German", &Value::from("L B2, R B2, SI B1, SP B1, W B1")),
            ("French", &Value::from("R B2")),
            ("Latin", &Value::Null),
        ],
        "a duplicate language name is kept once"
    );
    assert_eq!(languages[4]["displayOrder"], 4);
    canonicalize_profile_write(&value).unwrap();
}

#[test]
fn empty_sections_give_empty_lists_and_a_null_summary() {
    let mut cv = sample();
    cv.summary = None;
    cv.employment.clear();
    cv.education.clear();
    cv.languages.clear();
    cv.skills.clear();
    cv.person.websites.clear();
    cv.person.phones.clear();
    cv.person.primary_language = None;
    let value = mapped(&cv);
    assert_eq!(value["summary"], Value::Null);
    for key in [
        "links",
        "skills",
        "languages",
        "education",
        "experiences",
        "projects",
    ] {
        assert_eq!(value[key], Value::Array(Vec::new()), "{key}");
    }
    assert_eq!(value["contacts"].as_array().unwrap().len(), 1);
    canonicalize_profile_write(&value).unwrap();
}

#[test]
fn repeated_emails_skills_and_websites_are_kept_once() {
    let mut cv = sample();
    cv.person.emails.push(cv.person.emails[0].clone());
    cv.person.websites.push(cv.person.websites[0].clone());
    cv.person
        .websites
        .push("https://www.linkedin.com/in/alex-sample/".to_owned());
    cv.skills.push(cv.skills[0].clone());
    let value = mapped(&cv);
    assert_eq!(value["contacts"].as_array().unwrap().len(), 2);
    assert_eq!(value["skills"].as_array().unwrap().len(), 5);
    let links = value["links"].as_array().unwrap();
    assert_eq!(links.len(), 2);
    assert_eq!(links[1]["linkType"], "linkedin");
    canonicalize_profile_write(&value).unwrap();
}

#[test]
fn link_variants_of_one_url_are_kept_once_and_the_write_stays_valid() {
    let mut cv = sample();
    cv.person.websites = vec![
        "http://a.example.com".to_owned(),
        "https://a.example.com".to_owned(),
        "https://A.example.com:443/".to_owned(),
        "https://a.example.com/?utm=1".to_owned(),
    ];
    let value = mapped(&cv);
    assert_eq!(value["links"].as_array().unwrap().len(), 1);
    canonicalize_profile_write(&value).unwrap();
}

#[test]
fn links_are_classified_on_the_host_not_on_the_text() {
    let mut cv = sample();
    cv.person.websites = vec![
        "https://example.com/?r=linkedin.com/in/x".to_owned(),
        "https://notgithub.com/x".to_owned(),
        "https://www.linkedin.com/in/alex".to_owned(),
        "github.com/alex".to_owned(),
    ];
    let value = mapped(&cv);
    let kinds: Vec<&str> = value["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|link| link["linkType"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["website", "website", "linkedin", "github"]);
}

#[test]
fn duplicate_entries_are_dropped_and_an_end_before_the_start_is_cleared() {
    let mut cv = sample();
    let first = cv.employment[0].clone();
    cv.employment.insert(1, first);
    cv.employment[2].period.start = PartialDate::new(2023, Some(5), None);
    cv.employment[2].period.end = PartialDate::new(2023, Some(1), None);
    cv.employment[2].period.current = Some(false);
    let education = cv.education[0].clone();
    cv.education.push(education);
    let value = mapped(&cv);
    let experiences = value["experiences"].as_array().unwrap();
    assert_eq!(experiences.len(), 2);
    assert_eq!(experiences[1]["displayOrder"], 1);
    assert_eq!(experiences[1]["endDate"], Value::Null);
    assert_eq!(value["education"].as_array().unwrap().len(), 1);
    canonicalize_profile_write(&value).unwrap();
}

#[test]
fn a_primary_language_code_is_named_and_merged_with_its_assessment() {
    let mut cv = sample();
    let b2 = Some(Cefr::B2);
    cv.person.primary_language = Some("en".to_owned());
    cv.languages = vec![language("English", [b2; 5]), language("xx", [b2; 5])];
    let value = mapped(&cv);
    let names: Vec<&str> = value["languages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["language"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["English", "xx"]);
    assert_eq!(value["languages"][0]["proficiency"], "native");

    cv.person.primary_language = Some("English".to_owned());
    cv.languages = vec![language("SQ", [b2; 5])];
    let value = mapped(&cv);
    assert_eq!(value["languages"][1]["language"], "Albanian");
}
