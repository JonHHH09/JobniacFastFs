//! Arbeitnow scan use case: replays every vector of `golden/arbeitnow-scan.json` (TEST-ONLY keys over
//! synthetic boards) and checks the cases that Rust types can express.
//!
//! Not tested, because Rust types make them unrepresentable: wrong-typed request, job and page
//! values, caller-owned mutable lists, a board that raises something other than a classified
//! `BoardFailure` or returns a wrong type, monkeypatched internals, forged board failures (the
//! failure is a closed enum without text) and lone surrogates.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use jobniac_core::arbeitnow::board::{Page, UpstreamJob};
use jobniac_core::arbeitnow::candidates::{ArbeitnowCandidate, CandidateCodec};
use jobniac_core::arbeitnow::cursor::CursorKey;
use jobniac_core::arbeitnow::failure::{BoardFailure, RetryAfterSeconds, SAFE_MESSAGE};
use jobniac_core::arbeitnow::instant::{ArbeitnowInstant, MAX_EPOCH_SECOND, MIN_EPOCH_SECOND};
use jobniac_core::arbeitnow::scan::{
    ArbeitnowBoard, ArbeitnowScanService, CandidateIssuer, ScanRequest, ScanResult, add_inspected,
};
use jobniac_core::errors::{ApplicationError, ErrorCode};
use serde_json::{Value, json};

const GOLDEN: &str = include_str!("golden/arbeitnow-scan.json");

type Calls = Arc<Mutex<Vec<(u32, Option<bool>)>>>;

fn cursor_key() -> Vec<u8> {
    (64u8..96).collect()
}

fn candidate_key() -> Vec<u8> {
    (0u8..32).collect()
}

fn at(second: i64, nano: u32) -> ArbeitnowInstant {
    ArbeitnowInstant::new(second, nano).expect("in range")
}

/// The board fake: an unknown page is an empty page.
struct FakeBoard {
    pages: HashMap<u32, Page>,
    failure: Option<(u32, BoardFailure)>,
    calls: Calls,
}

impl ArbeitnowBoard for FakeBoard {
    fn fetch(&self, page: u32, visa_sponsorship: Option<bool>) -> Result<Page, BoardFailure> {
        self.calls.lock().unwrap().push((page, visa_sponsorship));
        if let Some((on_page, failure)) = self.failure
            && on_page == page
        {
            return Err(failure);
        }
        Ok(self.pages.get(&page).cloned().unwrap_or(Page {
            jobs: Vec::new(),
            another_page_may_remain: false,
        }))
    }
}

fn job() -> UpstreamJob {
    UpstreamJob {
        slug: "test-job".to_owned(),
        company: "Company".to_owned(),
        title: "Java Engineer".to_owned(),
        html_description: "<p>Description</p>".to_owned(),
        remote: Some(true),
        url: "https://example.com/jobs/1".to_owned(),
        tags: vec!["Java".to_owned()],
        job_types: vec!["Full Time".to_owned()],
        location: Some("Berlin".to_owned()),
        created_at: Some(100),
    }
}

fn page(jobs: Vec<UpstreamJob>, more: bool) -> Page {
    Page {
        jobs,
        another_page_may_remain: more,
    }
}

struct Env {
    service: ArbeitnowScanService<FakeBoard, CandidateCodec>,
    calls: Calls,
    reads: Arc<AtomicUsize>,
}

fn env_with(
    pages: HashMap<u32, Page>,
    failure: Option<(u32, BoardFailure)>,
    source: impl Fn() -> Result<ArbeitnowInstant, ApplicationError> + Send + Sync + 'static,
    cursor: &[u8],
) -> Env {
    let calls: Calls = Arc::default();
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reads);
    let codec = CandidateCodec::with_instant_source(&candidate_key(), move || {
        counter.fetch_add(1, Ordering::SeqCst);
        source()
    })
    .expect("codec");
    let board = FakeBoard {
        pages,
        failure,
        calls: Arc::clone(&calls),
    };
    Env {
        service: ArbeitnowScanService::new(board, CursorKey::new(cursor).expect("key"), codec),
        calls,
        reads,
    }
}

fn env(pages: Vec<(u32, Page)>, second: i64, nano: u32) -> Env {
    env_with(
        pages.into_iter().collect(),
        None,
        move || Ok(at(second, nano)),
        &cursor_key(),
    )
}

fn single(jobs: Vec<UpstreamJob>, more: bool) -> Env {
    env(vec![(1, page(jobs, more))], 100, 0)
}

fn error_of<T: std::fmt::Debug>(result: Result<T, ApplicationError>) -> ApplicationError {
    result.expect_err("an error")
}

fn assert_safe(error: &ApplicationError, code: ErrorCode, message: &str) {
    assert_eq!(error.code(), code);
    assert_eq!(error.message(), message);
    assert!(error.details().is_empty());
}

const REQUEST: &str = "Invalid Arbeitnow scan request";
const CURSOR: &str = "Invalid Arbeitnow scan cursor";
const UPSTREAM: &str = "Invalid Arbeitnow upstream data";
const CANDIDATE: &str = "Invalid Arbeitnow candidate token";
const INTERNAL: &str = "Unexpected application error";

// ----- golden replay -----

fn upstream_of(raw: &Value) -> UpstreamJob {
    let text = |key: &str| raw[key].as_str().map(str::to_owned);
    let list = |key: &str| -> Vec<String> {
        raw[key]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    UpstreamJob {
        slug: text("slug").unwrap(),
        company: text("company").unwrap(),
        title: text("title").unwrap(),
        html_description: text("htmlDescription").unwrap(),
        remote: raw["remote"].as_bool(),
        url: text("url").unwrap(),
        tags: list("tags"),
        job_types: list("jobTypes"),
        location: text("location"),
        created_at: raw["createdAt"].as_i64(),
    }
}

fn failure_of(raw: &Value) -> (u32, BoardFailure) {
    let retryable = raw["retryable"].as_bool().unwrap();
    let failure = match raw["category"].as_str().unwrap() {
        "transport" => BoardFailure::Transport,
        "http_status" => BoardFailure::HttpStatus { retryable },
        "rate_limited" => BoardFailure::RateLimited {
            retry_after: raw["retryAfter"]
                .as_u64()
                .map(|seconds| RetryAfterSeconds::new(u32::try_from(seconds).unwrap()).unwrap()),
        },
        "content_type" => BoardFailure::ContentType,
        "body_size" => BoardFailure::BodySize,
        "json" => BoardFailure::Json,
        "schema" => BoardFailure::Schema,
        "pagination" => BoardFailure::Pagination,
        other => panic!("unknown category {other}"),
    };
    (
        u32::try_from(raw["onPage"].as_u64().unwrap()).unwrap(),
        failure,
    )
}

fn request_of(raw: &Value, cursor: Option<&str>) -> ScanRequest {
    let list = |key: &str| {
        raw[key].as_array().map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        })
    };
    ScanRequest {
        query: raw["query"].as_str().map(str::to_owned),
        location: raw["location"].as_str().map(str::to_owned),
        remote_only: raw["remoteOnly"].as_bool(),
        visa_sponsorship: raw["visaSponsorship"].as_bool(),
        tags: list("tags"),
        job_types: list("jobTypes"),
        limit: raw["limit"].as_i64(),
        max_pages: raw["maxPages"].as_i64(),
        cursor: cursor
            .map(str::to_owned)
            .or_else(|| raw["cursor"].as_str().map(str::to_owned)),
    }
}

fn instant_json(value: Option<ArbeitnowInstant>) -> Value {
    value.map_or(Value::Null, |instant| {
        json!({"epochSecond": instant.epoch_second(), "nanosecond": instant.nanosecond()})
    })
}

fn candidate_json(value: &ArbeitnowCandidate) -> Value {
    json!({
        "issuedAt": instant_json(Some(value.issued_at)),
        "expiresAt": instant_json(Some(value.expires_at)),
        "slug": value.slug,
        "canonicalUrl": value.canonical_url,
        "company": value.company,
        "title": value.title,
        "location": value.location,
        "remote": value.remote,
        "postedAt": instant_json(value.posted_at),
        "tags": value.tags,
        "jobTypes": value.job_types,
        "descriptionExcerpt": value.description_excerpt,
    })
}

fn result_json(result: &ScanResult, verifier: &CandidateCodec) -> Value {
    let jobs: Vec<Value> = result
        .jobs
        .iter()
        .map(|item| {
            json!({
                "slug": item.slug,
                "canonicalUrl": item.canonical_url,
                "company": item.company,
                "title": item.title,
                "location": item.location,
                "remote": item.remote,
                "postedAt": instant_json(item.posted_at),
                "tags": item.tags,
                "jobTypes": item.job_types,
                "descriptionExcerpt": item.description_excerpt,
                "candidateToken": item.candidate_token,
            })
        })
        .collect();
    let verified: Vec<Value> = result
        .jobs
        .iter()
        .map(|item| candidate_json(&verifier.verify(&item.candidate_token).expect("verifies")))
        .collect();
    json!({
        "jobs": jobs,
        "verifiedCandidates": verified,
        "pagesScanned": result.pages_scanned,
        "recordsInspected": result.records_inspected,
        "nextCursor": result.next_cursor,
        "source": result.source,
        "sourceDocsUrl": result.source_docs_url,
        "consistencyNotice": result.consistency_notice,
    })
}

fn error_json(error: &ApplicationError) -> Value {
    json!({
        "code": error.code().code(),
        "message": error.message(),
        "details": serde_json::to_value(error.details()).unwrap(),
    })
}

fn replay(vector: &Value) -> Value {
    let board = &vector["board"];
    let mut pages = HashMap::new();
    if let Some(map) = board["pages"].as_object() {
        for (number, raw) in map {
            let one: Vec<UpstreamJob> = raw["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .map(upstream_of)
                .collect();
            let repeat = usize::try_from(raw["repeat"].as_u64().unwrap()).unwrap();
            let jobs: Vec<UpstreamJob> = (0..repeat).flat_map(|_| one.clone()).collect();
            pages.insert(
                number.parse::<u32>().unwrap(),
                page(jobs, raw["more"].as_bool().unwrap()),
            );
        }
    }
    let failure = board.get("failure").map(failure_of);
    let (second, nano) = match vector.get("clock") {
        Some(clock) => (
            clock["second"].as_i64().unwrap(),
            u32::try_from(clock["nano"].as_u64().unwrap()).unwrap(),
        ),
        None => (100, 0),
    };
    let env = env_with(pages, failure, move || Ok(at(second, nano)), &cursor_key());
    let verifier =
        CandidateCodec::with_instant_source(&candidate_key(), move || Ok(at(second, nano)))
            .expect("codec");
    let mut outcome = serde_json::Map::new();
    let mut steps = vec![("", &vector["request"])];
    if let Some(follow) = vector.get("followRequest") {
        steps.push(("follow", follow));
    }
    let mut previous: Option<ScanResult> = None;
    for (name, raw) in steps {
        let cursor = if name.is_empty() {
            None
        } else {
            previous
                .as_ref()
                .and_then(|result| result.next_cursor.clone())
        };
        match env.service.scan(&request_of(raw, cursor.as_deref())) {
            Ok(result) => {
                let key = if name.is_empty() {
                    "result".to_owned()
                } else {
                    format!("{name}Result")
                };
                outcome.insert(key, result_json(&result, &verifier));
                previous = Some(result);
            }
            Err(error) => {
                let key = if name.is_empty() {
                    "error".to_owned()
                } else {
                    format!("{name}Error")
                };
                outcome.insert(key, error_json(&error));
                break;
            }
        }
    }
    let calls: Vec<Value> = env
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(page, visa)| json!({"page": page, "visaSponsorship": visa}))
        .collect();
    outcome.insert("calls".to_owned(), Value::Array(calls));
    outcome.insert(
        "clockReads".to_owned(),
        json!(env.reads.load(Ordering::SeqCst)),
    );
    Value::Object(outcome)
}

#[test]
fn every_golden_vector_replays_identically() {
    let document: Value = serde_json::from_str(GOLDEN).unwrap();
    let vectors = document["vectors"].as_array().unwrap();
    assert!(vectors.len() >= 150, "{} vectors", vectors.len());
    for vector in vectors {
        let id = vector["id"].as_str().unwrap();
        let expected = match vector.get("deviation") {
            Some(deviation) => deviation["expected"].clone(),
            None => vector["expected"].clone(),
        };
        let actual = replay(vector);
        assert_eq!(actual, expected, "vector {id}");
    }
}

#[test]
fn golden_covers_the_required_scenarios() {
    let document: Value = serde_json::from_str(GOLDEN).unwrap();
    let ids: Vec<&str> = document["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|vector| vector["id"].as_str().unwrap())
        .collect();
    for needed in [
        "defaults",
        "page_1000000_emits_1000001",
        "mutable_offset_too_large",
        "cursor_foreign_key",
        "cursor_outer_trailing_bits_alias",
        "unselected_invalid_slug_tail",
        "selected_invalid_slug",
        "board_failure_rate_limited_5",
        "clock_max_overflow",
        "max_pages_5_more",
    ] {
        assert!(ids.contains(&needed), "missing {needed}");
    }
}

// ----- request bounds -----

fn refused(request: &ScanRequest) {
    let environment = single(vec![job()], false);
    let error = error_of(environment.service.scan(request));
    assert_safe(&error, ErrorCode::ValidationError, REQUEST);
    assert!(environment.calls.lock().unwrap().is_empty());
    assert_eq!(environment.reads.load(Ordering::SeqCst), 0);
}

fn accepted(request: &ScanRequest) {
    single(vec![job()], false)
        .service
        .scan(request)
        .expect("accepted");
}

fn repeated(text: &str, count: usize) -> String {
    text.repeat(count)
}

#[test]
fn request_bounds_fail_before_any_board_or_clock_effect() {
    let cases = [
        ScanRequest {
            limit: Some(0),
            ..ScanRequest::default()
        },
        ScanRequest {
            limit: Some(101),
            ..ScanRequest::default()
        },
        ScanRequest {
            limit: Some(-1),
            ..ScanRequest::default()
        },
        ScanRequest {
            limit: Some(i64::MAX),
            ..ScanRequest::default()
        },
        ScanRequest {
            max_pages: Some(0),
            ..ScanRequest::default()
        },
        ScanRequest {
            max_pages: Some(6),
            ..ScanRequest::default()
        },
        ScanRequest {
            query: Some(repeated("x", 257)),
            ..ScanRequest::default()
        },
        ScanRequest {
            query: Some(repeated("\u{1F600}", 129)),
            ..ScanRequest::default()
        },
        ScanRequest {
            query: Some(vec!["x"; 17].join(" ")),
            ..ScanRequest::default()
        },
        ScanRequest {
            location: Some(repeated("x", 129)),
            ..ScanRequest::default()
        },
        ScanRequest {
            tags: Some(vec!["x".to_owned(); 11]),
            ..ScanRequest::default()
        },
        ScanRequest {
            tags: Some(vec![" ".to_owned()]),
            ..ScanRequest::default()
        },
        ScanRequest {
            tags: Some(vec![repeated("x", 129)]),
            ..ScanRequest::default()
        },
        ScanRequest {
            job_types: Some(vec!["x".to_owned(); 11]),
            ..ScanRequest::default()
        },
        ScanRequest {
            job_types: Some(vec!["\u{3000}".to_owned()]),
            ..ScanRequest::default()
        },
        ScanRequest {
            job_types: Some(vec![repeated("\u{1F600}", 65)]),
            ..ScanRequest::default()
        },
    ];
    for request in &cases {
        refused(request);
    }
}

#[test]
fn request_boundaries_are_inclusive() {
    for limit in [1, 100] {
        accepted(&ScanRequest {
            limit: Some(limit),
            ..ScanRequest::default()
        });
    }
    for pages in [1, 5] {
        accepted(&ScanRequest {
            max_pages: Some(pages),
            ..ScanRequest::default()
        });
    }
    accepted(&ScanRequest {
        query: Some(vec!["x"; 16].join(" ")),
        ..ScanRequest::default()
    });
    accepted(&ScanRequest {
        query: Some(repeated("x", 256)),
        ..ScanRequest::default()
    });
    accepted(&ScanRequest {
        query: Some(repeated("\u{1F600}", 128)),
        ..ScanRequest::default()
    });
    accepted(&ScanRequest {
        location: Some(repeated("x", 128)),
        ..ScanRequest::default()
    });
    accepted(&ScanRequest {
        tags: Some(vec!["x".to_owned(); 10]),
        ..ScanRequest::default()
    });
    accepted(&ScanRequest {
        job_types: Some(vec![repeated("x", 128)]),
        ..ScanRequest::default()
    });
    accepted(&ScanRequest {
        query: Some("  ".to_owned()),
        ..ScanRequest::default()
    });
}

#[test]
fn cursor_authority_requires_an_owned_key_of_at_least_32_bytes() {
    for length in [0usize, 1, 31] {
        let error = error_of(CursorKey::new(&vec![7u8; length]));
        assert_safe(&error, ErrorCode::ValidationError, REQUEST);
    }
    assert!(CursorKey::new(&[7u8; 32]).is_ok());
    assert!(CursorKey::new(&[7u8; 100]).is_ok());
}

#[test]
fn debug_output_never_shows_keys_tokens_or_cursors() {
    let environment = env_with(
        HashMap::from([(1, page(vec![job()], true))]),
        None,
        || Ok(at(100, 0)),
        &[0xC3; 32],
    );
    let text = format!("{:?}", environment.service);
    assert_eq!(text, "ArbeitnowScanService { .. }");
    let result = environment
        .service
        .scan(&ScanRequest {
            limit: Some(1),
            ..ScanRequest::default()
        })
        .unwrap();
    let shown = format!("{:?}", result.jobs[0]);
    assert!(!shown.contains(&result.jobs[0].candidate_token));
}

// ----- behaviour -----

#[test]
fn defensive_defaults_and_fixed_result_values() {
    let result = single(vec![job()], false)
        .service
        .scan(&ScanRequest::default())
        .unwrap();
    assert_eq!(result.source, "arbeitnow");
    assert_eq!(
        result.source_docs_url,
        "https://www.arbeitnow.com/api/job-board-api"
    );
    assert_eq!(
        result.consistency_notice,
        "Upstream results are mutable; this scan is not a snapshot."
    );
    assert_eq!((result.pages_scanned, result.records_inspected), (1, 1));
    assert_eq!(
        result.jobs[0].canonical_url,
        "https://arbeitnow.com/view/test-job"
    );
}

#[test]
fn board_failures_are_the_fixed_safe_envelope_with_classification_details() {
    let cases = [
        (
            BoardFailure::Transport,
            ErrorCode::UpstreamUnavailable,
            "transport",
            "true",
            None,
        ),
        (
            BoardFailure::HttpStatus { retryable: false },
            ErrorCode::UpstreamUnavailable,
            "http_status",
            "false",
            None,
        ),
        (
            BoardFailure::HttpStatus { retryable: true },
            ErrorCode::UpstreamUnavailable,
            "http_status",
            "true",
            None,
        ),
        (
            BoardFailure::RateLimited {
                retry_after: RetryAfterSeconds::new(5),
            },
            ErrorCode::UpstreamRateLimited,
            "rate_limited",
            "true",
            Some("5"),
        ),
        (
            BoardFailure::Json,
            ErrorCode::UpstreamInvalidResponse,
            "json",
            "false",
            None,
        ),
        (
            BoardFailure::Pagination,
            ErrorCode::UpstreamInvalidResponse,
            "pagination",
            "false",
            None,
        ),
        (
            BoardFailure::Schema,
            ErrorCode::UpstreamInvalidResponse,
            "schema",
            "false",
            None,
        ),
    ];
    for (failure, code, category, retryable, retry) in cases {
        let environment = env_with(
            HashMap::new(),
            Some((1, failure)),
            || Ok(at(100, 0)),
            &cursor_key(),
        );
        let error = error_of(environment.service.scan(&ScanRequest::default()));
        assert_eq!(error.code(), code);
        assert_eq!(error.message(), SAFE_MESSAGE);
        let mut expected = vec![
            ("provider", "arbeitnow"),
            ("failureCategory", category),
            ("retryable", retryable),
        ];
        if let Some(seconds) = retry {
            expected.push(("retryAfterSeconds", seconds));
        }
        let actual: Vec<(&str, &str)> = error
            .details()
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        assert_eq!(actual, expected);
    }
}

#[test]
fn each_selected_job_reads_two_precise_clocks_and_metadata_failure_reads_none() {
    let instants = Arc::new(Mutex::new(VecDeque::from([
        at(100, 123),
        at(100, 124),
        at(101, 999),
        at(102, 0),
    ])));
    let queue = Arc::clone(&instants);
    let second = UpstreamJob {
        slug: "second".to_owned(),
        ..job()
    };
    let environment = env_with(
        HashMap::from([(1, page(vec![job(), second], false))]),
        None,
        move || Ok(queue.lock().unwrap().pop_front().expect("a fifth read")),
        &cursor_key(),
    );
    let result = environment.service.scan(&ScanRequest::default()).unwrap();
    assert_eq!(environment.reads.load(Ordering::SeqCst), 4);
    let verifier =
        CandidateCodec::with_instant_source(&candidate_key(), || Ok(at(102, 0))).unwrap();
    let verified: Vec<ArbeitnowCandidate> = result
        .jobs
        .iter()
        .map(|item| verifier.verify(&item.candidate_token).unwrap())
        .collect();
    assert_eq!(verified[0].issued_at, at(100, 0));
    assert_eq!(verified[1].issued_at, at(101, 0));
    assert_eq!(verified[0].expires_at, at(1000, 0));
    assert_eq!(verified[1].expires_at, at(1001, 0));

    let blank = UpstreamJob {
        title: String::new(),
        ..job()
    };
    let environment = single(vec![blank], false);
    let error = error_of(environment.service.scan(&ScanRequest::default()));
    assert_safe(&error, ErrorCode::ValidationError, UPSTREAM);
    assert_eq!(environment.reads.load(Ordering::SeqCst), 0);
}

#[test]
fn clock_advance_and_full_range_issue_boundaries() {
    let reads = Arc::new(Mutex::new(VecDeque::from([
        at(100, 1),
        at(39, 999_999_999),
    ])));
    let queue = Arc::clone(&reads);
    let environment = env_with(
        HashMap::from([(1, page(vec![job()], false))]),
        None,
        move || Ok(queue.lock().unwrap().pop_front().expect("two reads")),
        &cursor_key(),
    );
    let error = error_of(environment.service.scan(&ScanRequest::default()));
    assert_safe(&error, ErrorCode::ValidationError, CANDIDATE);

    for second in [MIN_EPOCH_SECOND, MAX_EPOCH_SECOND - 900] {
        let posted = UpstreamJob {
            created_at: Some(second),
            ..job()
        };
        let environment = env(vec![(1, page(vec![posted], false))], second, 0);
        let result = environment.service.scan(&ScanRequest::default()).unwrap();
        let verifier =
            CandidateCodec::with_instant_source(&candidate_key(), move || Ok(at(second, 0)))
                .unwrap();
        let candidate = verifier.verify(&result.jobs[0].candidate_token).unwrap();
        assert_eq!(candidate.posted_at, Some(at(second, 0)));
        assert_eq!(candidate.expires_at, at(second + 900, 0));
    }
    let environment = env(
        vec![(1, page(vec![job()], false))],
        MAX_EPOCH_SECOND - 899,
        0,
    );
    let error = error_of(environment.service.scan(&ScanRequest::default()));
    assert_safe(&error, ErrorCode::InternalError, INTERNAL);
}

#[test]
fn failing_clock_is_a_generic_internal_error_for_every_claimed_code() {
    for code in [
        ErrorCode::ValidationError,
        ErrorCode::AuthorizationError,
        ErrorCode::NotFound,
        ErrorCode::Conflict,
        ErrorCode::UpstreamRateLimited,
        ErrorCode::UpstreamInvalidResponse,
        ErrorCode::UpstreamUnavailable,
        ErrorCode::InternalError,
    ] {
        let clock = env_with(
            HashMap::from([(1, page(vec![job()], false))]),
            None,
            move || {
                Err(ApplicationError::new(
                    code,
                    Some("PRIVATE-CLOCK"),
                    [("key", "PRIVATE-KEY")],
                ))
            },
            &cursor_key(),
        );
        let error = error_of(clock.service.scan(&ScanRequest::default()));
        assert_safe(&error, ErrorCode::InternalError, INTERNAL);
    }
}

/// A candidate authority that fails as scripted.
struct Scripted {
    now: Result<ArbeitnowInstant, ApplicationError>,
    issue: Result<String, ApplicationError>,
}

impl CandidateIssuer for Scripted {
    fn now(&self) -> Result<ArbeitnowInstant, ApplicationError> {
        self.now.clone()
    }

    fn issue(&self, _: &ArbeitnowCandidate) -> Result<String, ApplicationError> {
        self.issue.clone()
    }
}

fn scripted_scan(issue: Result<String, ApplicationError>) -> Result<ScanResult, ApplicationError> {
    let board = FakeBoard {
        pages: HashMap::from([(1, page(vec![job()], false))]),
        failure: None,
        calls: Arc::default(),
    };
    let scripted = Scripted {
        now: Ok(at(100, 0)),
        issue,
    };
    ArbeitnowScanService::new(board, CursorKey::new(&cursor_key()).unwrap(), scripted)
        .scan(&ScanRequest::default())
}

#[test]
fn only_the_fixed_candidate_failure_survives_the_codec_stage() {
    let failing = |code, message: &str| {
        Err(ApplicationError::new(
            code,
            Some(message),
            [("private", "KEY")],
        ))
    };
    let error = error_of(scripted_scan(failing(
        ErrorCode::ValidationError,
        CANDIDATE,
    )));
    assert_safe(&error, ErrorCode::ValidationError, CANDIDATE);
    let error = error_of(scripted_scan(failing(
        ErrorCode::ValidationError,
        "PRIVATE-CODEC",
    )));
    assert_safe(
        &error,
        ErrorCode::ValidationError,
        ErrorCode::ValidationError.default_message(),
    );
    let error = error_of(scripted_scan(failing(ErrorCode::ValidationError, CURSOR)));
    assert_safe(
        &error,
        ErrorCode::ValidationError,
        ErrorCode::ValidationError.default_message(),
    );
    let error = error_of(scripted_scan(failing(ErrorCode::NotFound, CANDIDATE)));
    assert_safe(
        &error,
        ErrorCode::NotFound,
        ErrorCode::NotFound.default_message(),
    );
    let error = error_of(scripted_scan(failing(ErrorCode::InternalError, "PRIVATE")));
    assert_safe(&error, ErrorCode::InternalError, INTERNAL);
    assert_eq!(
        scripted_scan(Ok("token".to_owned())).unwrap().jobs[0].candidate_token,
        "token"
    );
}

#[test]
fn raw_list_tail_search_and_lazy_selected_mapping() {
    let mut tags = vec!["irrelevant".to_owned(); 20];
    tags.push("Needle".to_owned());
    let valid = UpstreamJob { tags, ..job() };
    let invalid = UpstreamJob {
        slug: "bad!".to_owned(),
        tags: vec!["Needle".to_owned()],
        ..job()
    };
    let environment = single(vec![valid, invalid], false);
    let result = environment
        .service
        .scan(&ScanRequest {
            query: Some("needle".to_owned()),
            tags: Some(vec!["needle".to_owned()]),
            limit: Some(1),
            ..ScanRequest::default()
        })
        .unwrap();
    assert_eq!(result.jobs.len(), 1);
    assert_eq!(result.jobs[0].tags, vec!["irrelevant"; 20]);
    assert_eq!(result.records_inspected, 2);
    let error = error_of(environment.service.scan(&ScanRequest {
        query: Some("needle".to_owned()),
        limit: Some(2),
        ..ScanRequest::default()
    }));
    assert_safe(&error, ErrorCode::ValidationError, UPSTREAM);
}

#[test]
fn large_raw_inputs_are_capped_in_the_output_only() {
    let huge = UpstreamJob {
        title: format!("{}needle", "x".repeat(100_000)),
        html_description: format!("{}tail", "x".repeat(100_000)),
        ..job()
    };
    let result = single(vec![huge], false)
        .service
        .scan(&ScanRequest {
            query: Some("needle".to_owned()),
            limit: Some(1),
            ..ScanRequest::default()
        })
        .unwrap();
    assert_eq!(result.jobs[0].title, "x".repeat(256));
    assert_eq!(result.jobs[0].description_excerpt, "x".repeat(1000));
}

#[test]
fn cursor_and_candidate_authorities_are_independent() {
    let pages = HashMap::from([(1, page(vec![job()], true))]);
    let first = env_with(pages.clone(), None, || Ok(at(100, 0)), &[b'C'; 32]);
    let result = first
        .service
        .scan(&ScanRequest {
            limit: Some(1),
            ..ScanRequest::default()
        })
        .unwrap();
    let verifier =
        CandidateCodec::with_instant_source(&candidate_key(), || Ok(at(100, 0))).unwrap();
    assert_eq!(
        verifier
            .verify(&result.jobs[0].candidate_token)
            .unwrap()
            .slug,
        "test-job"
    );
    let cursor = result.next_cursor.expect("next");
    // The same cursor key accepts it; another cursor key, even the candidate key, does not.
    first
        .service
        .scan(&ScanRequest {
            limit: Some(1),
            cursor: Some(cursor.clone()),
            ..ScanRequest::default()
        })
        .unwrap();
    for other in [candidate_key(), cursor_key()] {
        let second = env_with(pages.clone(), None, || Ok(at(100, 0)), &other);
        let error = error_of(second.service.scan(&ScanRequest {
            limit: Some(1),
            cursor: Some(cursor.clone()),
            ..ScanRequest::default()
        }));
        assert_safe(&error, ErrorCode::ValidationError, CURSOR);
        assert!(second.calls.lock().unwrap().is_empty());
    }
}

#[test]
fn page_1000000_emits_1000001_and_the_follow_up_is_refused() {
    let pages = HashMap::from([(1, page(vec![job()], true))]);
    let environment = env_with(pages, None, || Ok(at(100, 0)), &cursor_key());
    let request = ScanRequest {
        limit: Some(1),
        ..ScanRequest::default()
    };
    let result = environment.service.scan(&request).unwrap();
    let cursor = result.next_cursor.unwrap();
    // Decode through the public authority to read the emitted page.
    let authority = jobniac_core::arbeitnow::cursor::CursorAuthority::new(
        CursorKey::new(&cursor_key()).unwrap(),
    );
    let scope = jobniac_core::arbeitnow::cursor::Scope {
        query: None,
        location: None,
        remote_only: None,
        visa_sponsorship: None,
        tags: None,
        job_types: None,
    };
    let fingerprint = authority.fingerprint(&scope).unwrap();
    let emitted = authority.encode(1_000_001, 0, &fingerprint).unwrap();
    assert_eq!(authority.decode(&emitted, &fingerprint), None);
    assert_eq!(authority.decode(&cursor, &fingerprint), Some((2, 0)));
    let last = authority.encode(1_000_000, 0, &fingerprint).unwrap();
    assert_eq!(authority.decode(&last, &fingerprint), Some((1_000_000, 0)));
}

#[test]
fn records_inspected_wraps_like_a_java_int() {
    assert_eq!(add_inspected(0, 0), 0);
    assert_eq!(add_inspected(5, 7), 12);
    assert_eq!(add_inspected(i32::MAX, 1), i32::MIN);
    assert_eq!(add_inspected(0, 1usize << 31), i32::MIN);
    assert_eq!(add_inspected(0, (1usize << 32) - 1), -1);
    assert_eq!(add_inspected(1, 1usize << 32), 1);
    assert_eq!(add_inspected(i32::MIN, 1), i32::MIN + 1);
}

#[test]
fn page_counts_stay_exact_across_pages() {
    let pages = (1u32..=5)
        .map(|number| (number, page(vec![job(), job()], true)))
        .collect();
    let environment = env_with(pages, None, || Ok(at(100, 0)), &cursor_key());
    let result = environment
        .service
        .scan(&ScanRequest {
            query: Some("zzz".to_owned()),
            max_pages: Some(5),
            ..ScanRequest::default()
        })
        .unwrap();
    assert_eq!((result.pages_scanned, result.records_inspected), (5, 10));
    assert!(result.jobs.is_empty() && result.next_cursor.is_some());
    assert_eq!(environment.calls.lock().unwrap().len(), 5);
}

#[test]
fn board_failure_on_a_later_page_keeps_earlier_work_private() {
    let environment = env_with(
        HashMap::from([(1, page(vec![job()], true))]),
        Some((2, BoardFailure::Transport)),
        || Ok(at(100, 0)),
        &cursor_key(),
    );
    let error = error_of(environment.service.scan(&ScanRequest {
        query: Some("zzz".to_owned()),
        max_pages: Some(2),
        ..ScanRequest::default()
    }));
    assert_eq!(error.code(), ErrorCode::UpstreamUnavailable);
    assert_eq!(error.message(), SAFE_MESSAGE);
}
