//! Report identity and revision formatting.
//!
//! The identity string is `deterministic-v1|<profileId>|<profileRevision>|<jobId>|<jobRevision>`, and
//! the report ID is the MD5 name-based UUIDv3 of that string. The vectors record the identity
//! and ID for identifier spellings (canonical, uppercase, braced, `urn:`, compact) and for revision
//! edge values.

mod matching_support;

use jobniac_core::clock::{format_instant, parse_instant};
use jobniac_core::matching::{ALGORITHM_VERSION, report_id, report_identity, score_match};
use matching_support::{array, job_graph, parse, profile_graph, text, uuid_of};

const IDENTITY: &str = include_str!("golden/matching-identity.json");

fn instant(text: &str) -> time::OffsetDateTime {
    match parse_instant(text) {
        Ok(value) => value,
        Err(error) => panic!("vector instant is refused: {error}"),
    }
}

#[test]
fn report_ids_equal_the_golden_ids_byte_for_byte() {
    let document = parse(IDENTITY);
    let cases = array(&document, "cases");
    let mut identities = 0_usize;
    for case in cases {
        if case.get("identity").is_none() {
            continue;
        }
        identities += 1;
        let identity = report_identity(
            uuid_of(text(case, "profileId")),
            instant(text(case, "profileUpdatedAt")),
            uuid_of(text(case, "jobId")),
            instant(text(case, "jobUpdatedAt")),
        );
        assert_eq!(identity, text(case, "identity"));
        assert_eq!(
            report_id(&identity).to_string(),
            text(case, "reportId"),
            "{identity}"
        );
        assert!(identity.starts_with(ALGORITHM_VERSION));
    }
    assert_eq!(identities, 7, "the identity vector count drifted");
}

#[test]
fn revisions_format_like_the_golden_vectors() {
    let document = parse(IDENTITY);
    let checks = array(&document, "formatCheck");
    assert_eq!(checks.len(), 7, "the revision format vector count drifted");
    for check in checks {
        assert_eq!(
            format_instant(instant(text(check, "instant"))),
            text(check, "formatted"),
            "{}",
            text(check, "instant")
        );
    }
}

#[test]
fn the_clock_moves_created_at_but_not_the_identity() {
    let document = parse(IDENTITY);
    let cases = array(&document, "cases");
    let clock_cases: Vec<_> = cases
        .iter()
        .filter(|case| case.get("clockCase").is_some())
        .collect();
    assert_eq!(clock_cases.len(), 2);
    let profile = profile_graph(&serde_json::json!({
        "id": "11111111-1111-4111-8111-111111111111",
        "updatedAt": "2025-01-02T03:04:05.123456Z",
        "skills": ["java"],
        "experiences": [],
    }));
    let job = job_graph(&serde_json::json!({
        "id": "22222222-2222-4222-8222-222222222222",
        "updatedAt": "2025-02-03T04:05:06.654321Z",
        "experienceRequirement": null,
        "seniority": null,
        "skills": ["java"],
    }));
    for case in clock_cases {
        let report = match score_match(&profile, &job, instant(text(case, "clockCase"))) {
            Ok(report) => report,
            Err(error) => panic!("clock case refused: {error}"),
        };
        assert_eq!(report.id.to_string(), text(case, "reportId"));
        assert_eq!(format_instant(report.created_at), text(case, "createdAt"));
    }
}
