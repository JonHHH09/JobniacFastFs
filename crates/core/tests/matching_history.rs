//! Staleness of stored reports and the snapshot retry rule.
//!
//! `golden/matching-staleness.json` records the staleness flag for stored rows whose current source
//! revisions are equal, changed or missing. The Rust code keeps the same rule in `is_stale`; the SQL
//! that reads the current revisions belongs to the adapter.

mod matching_support;

use jobniac_core::clock::parse_instant;
use jobniac_core::matching::{
    ALGORITHM_VERSION, MatchReport, Outcome, is_stale, retries_serialization_failure,
};
use matching_support::{array, parse, text, uuid_of};
use time::OffsetDateTime;

const STALENESS: &str = include_str!("golden/matching-staleness.json");

fn instant(text: &str) -> OffsetDateTime {
    match parse_instant(text) {
        Ok(value) => value,
        Err(error) => panic!("vector instant is refused: {error}"),
    }
}

/// A stored report with the vector's revisions. The other fields do not take part in staleness.
fn stored(profile_revision: &str, job_revision: &str) -> MatchReport {
    MatchReport {
        id: uuid_of("9e119fdc-6f1d-3ce4-9385-d19a52d2fc26"),
        profile_id: uuid_of("11111111-1111-4111-8111-111111111111"),
        job_id: uuid_of("22222222-2222-4222-8222-222222222222"),
        profile_revision: instant(profile_revision),
        job_revision: instant(job_revision),
        algorithm_version: ALGORITHM_VERSION.to_owned(),
        overall_score: 100,
        confidence: 40,
        outcome: Outcome::StrongMatch,
        blocker_mismatch: false,
        components: Vec::new(),
        evidence: Vec::new(),
        created_at: instant("2026-10-02T12:00:00.123456Z"),
    }
}

#[test]
fn staleness_matches_the_golden_view_for_every_vector() {
    let document = parse(STALENESS);
    let cases = array(&document, "cases");
    assert_eq!(cases.len(), 7, "the staleness vector count drifted");
    for case in cases {
        let report = stored(text(case, "profileRevision"), text(case, "jobRevision"));
        let current_profile = case["currentProfileRevision"].as_str().map(instant);
        let current_job = case["currentJobRevision"].as_str().map(instant);
        let expected = case["stale"].as_bool();
        assert_eq!(
            Some(is_stale(&report, current_profile, current_job)),
            expected,
            "{}",
            text(case, "name")
        );
    }
}

#[test]
fn snapshot_reads_retry_serialization_failures_twice_and_nothing_else() {
    assert!(retries_serialization_failure(0, true));
    assert!(retries_serialization_failure(1, true));
    assert!(!retries_serialization_failure(2, true));
    assert!(!retries_serialization_failure(0, false));
    assert!(!retries_serialization_failure(u32::MAX, true));
}
