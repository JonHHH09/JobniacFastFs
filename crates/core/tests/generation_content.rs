//! Replays the resume content golden vectors (`golden/generation/content.json`): master, Canadian
//! English and Canadian French bodies, and the experience bullet rule.
//!
//! Every profile is synthetic. Sentence boundaries of the vectors come from a synthetic finder (a
//! boundary after every dot followed by a space). The extra tests check the built-in UAX #29 finder
//! and the preparation budgets.
//!
//! Note: the production `default_sentence_boundaries` is NOT covered by the vectors above. Its
//! behaviour is covered by `crates/text/tests/sentences_golden.rs`. A pass here is not proof of
//! production sentence-boundary behaviour.

use jobniac_core::errors::ApplicationError;
use jobniac_core::generation::content::{
    PreparedResume, default_sentence_boundaries, experience_bullets, prepare_canadian_resume,
    prepare_french_canadian_resume, prepare_master_resume, validate_pdf_body,
};
use jobniac_core::generation::translation::FrenchTranslator;
use jobniac_core::profiles::model::{
    ContactRecord, EducationRecord, ExperienceRecord, LanguageRecord, LinkRecord, ProfileGraph,
    ProfileRecord, ProjectRecord, ProjectTechnologyRecord, SkillRecord,
};
use serde_json::{Value, json};
use time::{Date, Month, OffsetDateTime};
use uuid::Uuid;

const CONTENT: &str = include_str!("golden/generation/content.json");

type Finder = Box<dyn Fn(&str) -> Option<Vec<usize>>>;

fn vectors() -> Value {
    serde_json::from_str(CONTENT).expect("content vectors parse")
}

fn text(value: &Value) -> &str {
    value.as_str().expect("string")
}

fn optional(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

fn date(value: &Value) -> Option<Date> {
    let raw = value.as_str()?;
    let mut parts = raw.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u8 = parts.next()?.parse().ok()?;
    let day: u8 = parts.next()?.parse().ok()?;
    Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()
}

/// Builds the typed graph of a synthetic profile. Ids are sequential; instants fixed.
fn graph(value: &Value) -> ProfileGraph {
    let mut next = 0_u128;
    let mut id = move || {
        next += 1;
        Uuid::from_u128(next)
    };
    let at = OffsetDateTime::UNIX_EPOCH;
    let profile_id = id();
    let profile_value = &value["profile"];
    let summary = match &profile_value["summary"] {
        Value::Object(marker) => {
            Some(text(&marker["repeat"]).repeat(marker["count"].as_u64().expect("count") as usize))
        }
        other => optional(other),
    };
    let profile = ProfileRecord {
        id: profile_id,
        full_name: text(&profile_value["fullName"]).to_owned(),
        email: text(&profile_value["email"]).to_owned(),
        summary,
        raw_resume_text: None,
        created_at: at,
        updated_at: at,
        embedding: None,
        revision: 1,
    };
    let rows = |key: &str| -> Vec<Value> { value[key].as_array().cloned().unwrap_or_default() };
    let contacts = rows("contacts")
        .iter()
        .map(|row| ContactRecord {
            id: id(),
            profile_id,
            contact_type: text(&row["contactType"]).to_owned(),
            contact_value: text(&row["contactValue"]).to_owned(),
            label: optional(&row["label"]),
            created_at: at,
            updated_at: at,
        })
        .collect();
    let links = rows("links")
        .iter()
        .map(|row| LinkRecord {
            id: id(),
            profile_id,
            link_type: text(&row["linkType"]).to_owned(),
            url: text(&row["url"]).to_owned(),
            label: optional(&row["label"]),
            created_at: at,
            updated_at: at,
        })
        .collect();
    let skills = rows("skills")
        .iter()
        .map(|row| SkillRecord {
            id: id(),
            profile_id,
            skill: text(&row["skill"]).to_owned(),
            normalized_skill: text(&row["skill"]).to_lowercase(),
            category: optional(&row["category"]),
            display_order: row["displayOrder"].as_i64().expect("order"),
            created_at: at,
        })
        .collect();
    let languages = rows("languages")
        .iter()
        .map(|row| LanguageRecord {
            id: id(),
            profile_id,
            language: text(&row["language"]).to_owned(),
            normalized_language: text(&row["language"]).to_lowercase(),
            proficiency: optional(&row["proficiency"]),
            display_order: row["displayOrder"].as_i64().expect("order"),
            created_at: at,
        })
        .collect();
    let education = rows("education")
        .iter()
        .map(|row| EducationRecord {
            id: id(),
            profile_id,
            institution: optional(&row["institution"]),
            degree: optional(&row["degree"]),
            field: optional(&row["field"]),
            location: optional(&row["location"]),
            start_date: date(&row["startDate"]),
            end_date: date(&row["endDate"]),
            relevant_focus: optional(&row["relevantFocus"]),
            created_at: at,
        })
        .collect();
    let experiences = rows("experiences")
        .iter()
        .map(|row| ExperienceRecord {
            id: id(),
            profile_id,
            company: optional(&row["company"]),
            title: optional(&row["title"]),
            location: optional(&row["location"]),
            start_date: date(&row["startDate"]),
            end_date: date(&row["endDate"]),
            description: optional(&row["description"]),
            display_order: row["displayOrder"].as_i64().expect("order"),
            created_at: at,
        })
        .collect();
    let mut projects = Vec::new();
    for row in rows("projects") {
        let project_id = id();
        let technologies = row["technologies"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|technology| ProjectTechnologyRecord {
                id: id(),
                project_id,
                technology: text(&technology["technology"]).to_owned(),
                normalized_technology: text(&technology["technology"]).to_lowercase(),
                display_order: technology["displayOrder"].as_i64().expect("order"),
                created_at: at,
            })
            .collect();
        projects.push(ProjectRecord {
            id: project_id,
            profile_id,
            name: optional(&row["name"]),
            url: optional(&row["url"]),
            description: optional(&row["description"]),
            technologies,
            display_order: row["displayOrder"].as_i64().expect("order"),
            created_at: at,
        });
    }
    let project_technologies = rows("projectTechnologies")
        .iter()
        .map(|row| ProjectTechnologyRecord {
            id: id(),
            project_id: profile_id,
            technology: text(&row["technology"]).to_owned(),
            normalized_technology: text(&row["technology"]).to_lowercase(),
            display_order: row["displayOrder"].as_i64().expect("order"),
            created_at: at,
        })
        .collect();
    ProfileGraph {
        profile,
        contacts,
        links,
        skills,
        languages,
        education,
        experiences,
        projects,
        project_technologies,
    }
}

/// The finder named by a vector: the synthetic splitter, or one of the two broken backends.
fn finder(name: &str) -> Finder {
    match name {
        "synthetic" => Box::new(synthetic_sentences),
        "raise" => Box::new(|_: &str| None),
        "invalid" => Box::new(|_: &str| Some(vec![0, 1])),
        other => panic!("unknown finder {other}"),
    }
}

/// A boundary after every dot followed by a space, as the synthetic finder of the vectors.
fn synthetic_sentences(text: &str) -> Option<Vec<usize>> {
    let characters: Vec<char> = text.chars().collect();
    let mut offsets = vec![0];
    for index in 0..characters.len().saturating_sub(1) {
        if characters[index] == '.' && characters[index + 1] == ' ' {
            offsets.push(index + 2);
        }
    }
    offsets.push(characters.len());
    let mut result: Vec<usize> = Vec::new();
    for offset in offsets {
        if result.last().is_none_or(|last| offset > *last) {
            result.push(offset);
        }
    }
    Some(result)
}

fn outcome_value(resume: &PreparedResume) -> Value {
    json!({
        "title": resume.title,
        "body": resume.body,
        "resumeType": resume.resume_type.as_str(),
    })
}

fn envelope_of(error: &ApplicationError) -> Value {
    serde_json::to_value(error.to_envelope()).expect("envelope serializes")
}

fn run(case: &Value) -> Value {
    let kind = text(&case["kind"]);
    let sentences = finder(text(&case["finder"]));
    let result: Result<Value, ApplicationError> = match kind {
        "bullets" => {
            let description = optional(&case["description"]);
            experience_bullets(description.as_deref(), &*sentences).map(|bullets| json!(bullets))
        }
        "master" => {
            prepare_master_resume(&graph(&case["graph"])).map(|resume| outcome_value(&resume))
        }
        "canadian" => prepare_canadian_resume(&graph(&case["graph"]), &*sentences)
            .map(|resume| outcome_value(&resume)),
        "canadian_fr" => {
            let translator = FrenchTranslator::builtin();
            prepare_french_canadian_resume(&graph(&case["graph"]), &translator, &*sentences)
                .map(|resume| outcome_value(&resume))
        }
        other => panic!("unknown kind {other}"),
    };
    match result {
        Ok(value) => json!({"ok": true, "value": value}),
        Err(error) => json!({"ok": false, "error": envelope_of(&error)}),
    }
}

#[test]
fn content_cases_match_the_golden_vectors() {
    let root = vectors();
    let cases = root["cases"].as_array().expect("cases");
    assert!(cases.len() >= 20, "{} cases", cases.len());
    let mut failures = Vec::new();
    for case in cases {
        let id = text(&case["id"]);
        let actual = run(case);
        if actual != case["expected"] {
            failures.push(format!(
                "{id}\n rust:   {actual}\n recorded: {}",
                case["expected"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn default_finder_splits_a_single_line_into_sentences() {
    let bullets = experience_bullets(
        Some("Built the service. Shipped the release."),
        &default_sentence_boundaries,
    )
    .expect("bullets");
    assert_eq!(bullets, vec!["Built the service.", "Shipped the release."]);
}

#[test]
fn default_finder_keeps_at_most_three_bullets() {
    let bullets = experience_bullets(
        Some("One. Two. Three. Four. Five."),
        &default_sentence_boundaries,
    )
    .expect("bullets");
    assert_eq!(bullets, vec!["One.", "Two.", "Three."]);
}

#[test]
fn body_gate_refuses_blank_and_long_bodies() {
    assert_eq!(validate_pdf_body(Some("  x  ")).expect("body"), "x");
    let blank = validate_pdf_body(None).expect_err("missing body is blank");
    assert_eq!(
        blank.details().get("reason").map(String::as_str),
        Some("must not be blank")
    );
    let long = "a".repeat(50_001);
    let error = validate_pdf_body(Some(&long)).expect_err("too long");
    assert_eq!(
        error.details().get("reason").map(String::as_str),
        Some("must be less than or equal to 50000 characters")
    );
}

#[test]
fn profile_over_the_preparation_budget_is_refused_by_entries() {
    let mut value = vectors()["cases"][0]["graph"].clone();
    let skills: Vec<Value> = (0..10_001)
        .map(|n| json!({"skill": format!("Skill {n}"), "category": null, "displayOrder": n}))
        .collect();
    value["skills"] = Value::Array(skills);
    let error = prepare_master_resume(&graph(&value)).expect_err("entry budget");
    let envelope = envelope_of(&error);
    assert_eq!(envelope["details"]["field"], json!("profile"));
    assert_eq!(
        envelope["message"],
        json!("Resume preparation limit exceeded")
    );
}

#[test]
fn canadian_french_resume_keeps_protected_terms_and_translates_labels() {
    let root = vectors();
    let case = root["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| text(&case["id"]) == "canadian-fr-base")
        .expect("case");
    let actual = run(case);
    let body = text(&actual["value"]["body"]);
    assert!(body.contains("Example Corp"), "protected company kept");
    assert!(
        body.starts_with("Coordonnées: "),
        "contact label translated: {body}"
    );
    assert!(body.contains("COMPÉTENCES TECHNIQUES"));
}
