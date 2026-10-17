//! Privacy rules of skills, labels and evidence.
//!
//! The hand-written matchers of `privacy.rs` must agree with the recorded regular-expression results
//! on every synthetic string of `golden/matching-privacy.json`. The explicit cases are the refusal
//! lists.

mod matching_support;

use jobniac_core::errors::AppError;
use jobniac_core::matching::privacy::{
    has_sensitive_skill_text, has_sensitive_text, is_safe_label, is_safe_skill,
};
use jobniac_core::matching::{JobGraph, ProfileGraph, score_match};
use matching_support::{array, parse, text};
use serde_json::Value;

const PRIVACY: &str = include_str!("golden/matching-privacy.json");

fn flag(case: &Value, key: &str) -> bool {
    match case[key].as_bool() {
        Some(value) => value,
        None => panic!("missing boolean {key} in {case}"),
    }
}

#[test]
fn the_pattern_matchers_agree_with_golden_re_on_every_vector() {
    let document = parse(PRIVACY);
    let cases = array(&document, "cases");
    assert!(
        cases.len() > 400,
        "the privacy vector count drifted: {}",
        cases.len()
    );
    let mut sensitive_hits = 0_usize;
    for case in cases {
        let value = text(case, "value");
        assert_eq!(
            is_safe_skill(value),
            flag(case, "safeSkill"),
            "safe skill {value:?}"
        );
        assert_eq!(
            has_sensitive_skill_text(value),
            flag(case, "sensitiveSkill"),
            "sensitive skill {value:?}"
        );
        assert_eq!(
            is_safe_label(value),
            flag(case, "safeLabel"),
            "safe label {value:?}"
        );
        assert_eq!(
            has_sensitive_text(value),
            flag(case, "sensitiveText"),
            "sensitive text {value:?}"
        );
        if flag(case, "sensitiveText") {
            sensitive_hits += 1;
        }
    }
    assert!(
        sensitive_hits > 50,
        "too few sensitive samples: {sensitive_hits}"
    );
}

fn refused(skill: &str) -> Result<jobniac_core::matching::MatchReport, AppError> {
    let profile = ProfileGraph {
        id: "11111111-1111-4111-8111-111111111111".to_owned(),
        updated_at: "2025-01-02T03:04:05.123456Z".to_owned(),
        skills: Vec::new(),
        has_experience: false,
    };
    let job = JobGraph {
        id: "22222222-2222-4222-8222-222222222222".to_owned(),
        updated_at: "2025-02-03T04:05:06.654321Z".to_owned(),
        experience_requirement: None,
        seniority: None,
        skills: vec![skill.to_owned()],
    };
    let now = match jobniac_core::clock::parse_instant("2026-10-02T12:00:00.123456Z") {
        Ok(instant) => instant,
        Err(error) => panic!("test instant is refused: {error}"),
    };
    score_match(&profile, &job, now)
}

#[test]
fn unsafe_skills_are_refused_without_echoing_the_input() {
    let long = "a".repeat(81);
    let unsafe_skills = [
        "email@example.test",
        "http://example.invalid",
        "localhost",
        "127.0.0.1",
        "10.2.3.4",
        "192.168.2.1",
        "172.16.0.1",
        "prompt",
        "secret",
        "API key",
        "access-token",
        "bearer",
        "contact",
        "resume",
        "é",
        "\u{a0}java\u{a0}",
        long.as_str(),
    ];
    for unsafe_skill in unsafe_skills {
        assert_eq!(
            refused(unsafe_skill),
            Err(AppError::DomainValue(
                "fact must be a normalized privacy-safe skill".to_owned()
            )),
            "{unsafe_skill:?}"
        );
    }
}

#[test]
fn an_eighty_character_skill_is_accepted_and_eighty_one_is_refused() {
    assert!(is_safe_skill(&"a".repeat(80)));
    assert!(!is_safe_skill(&"a".repeat(81)));
}

#[test]
fn private_addresses_hosts_and_secrets_are_sensitive_text() {
    for value in [
        "localhost",
        "127.0.0.1",
        "10.0.0.1",
        "192.168.0.1",
        "172.16.0.1",
        "example.internal",
        "+355 123456789",
        "reasoning trace",
        "password secret",
    ] {
        assert!(has_sensitive_text(value), "{value}");
    }
}
