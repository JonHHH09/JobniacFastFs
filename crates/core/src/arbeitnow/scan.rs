//! Fixed-board scan use case.
//!
//! Validates the request, pages through an injected [`ArbeitnowBoard`], filters matches locally,
//! maps matches to [`ScannedJob`]s with signed candidate tokens and produces HMAC-protected
//! cursors (see [`crate::arbeitnow::cursor`]). No transport and no persistence.
//!
//! Fail closed: every failure is a fresh [`ApplicationError`] with a fixed message and no chained
//! or private detail. Board failures keep the fixed safe message with their classification
//! details; every other failure is an internal error with the default message.
//!
//! Not representable in Rust: wrong-typed request or page values, a board that
//! raises something other than a [`BoardFailure`] (the port is closed), and lone surrogates.

use std::fmt;

use jobniac_text::java::{java_strip, utf16_length};

use crate::arbeitnow::board::{Page, UpstreamJob};
use crate::arbeitnow::candidates::{ArbeitnowCandidate, CandidateCodec};
use crate::arbeitnow::cursor::{
    CursorAuthority, CursorKey, REQUEST_MESSAGE, Scope, normalized, terms,
};
use crate::arbeitnow::failure::BoardFailure;
use crate::arbeitnow::instant::{ArbeitnowInstant, MAX_EPOCH_SECOND, MIN_EPOCH_SECOND};
use crate::arbeitnow::text::{bound_list, plain, plain_bound};
use crate::errors::{ApplicationError, ErrorCode, validation_error};
use jobniac_text::case::root_lower;

/// Message of a refused cursor (malformed, forged, foreign, out of bounds or stale).
pub const CURSOR_MESSAGE: &str = "Invalid Arbeitnow scan cursor";
/// Message of upstream data that cannot be mapped.
pub const UPSTREAM_MESSAGE: &str = "Invalid Arbeitnow upstream data";
/// Message of a candidate that the codec refused.
pub const CANDIDATE_MESSAGE: &str = "Invalid Arbeitnow candidate token";

/// The `source` of every result.
pub const SOURCE: &str = "arbeitnow";
/// The `source_docs_url` of every result.
pub const SOURCE_DOCS_URL: &str = "https://www.arbeitnow.com/api/job-board-api";
/// The `consistency_notice` of every result.
pub const CONSISTENCY_NOTICE: &str = "Upstream results are mutable; this scan is not a snapshot.";

/// Default number of jobs per scan.
pub const DEFAULT_LIMIT: i64 = 25;
/// Default number of upstream pages per scan.
pub const DEFAULT_MAX_PAGES: i64 = 1;
/// Longest query, in UTF-16 units.
pub const MAX_QUERY_UNITS: usize = 256;
/// Most query terms.
pub const MAX_QUERY_TERMS: usize = 16;
/// Longest location filter, in UTF-16 units.
pub const MAX_LOCATION_UNITS: usize = 128;
/// Most entries of a tag or job type filter.
pub const MAX_FILTER_ENTRIES: usize = 10;
/// Longest filter entry, in UTF-16 units.
pub const MAX_FILTER_UNITS: usize = 128;

const CANONICAL_PREFIX: &str = "https://arbeitnow.com/view/";
const CANDIDATE_LIFETIME_SECONDS: i64 = 900;
const FIELD_CAP: usize = 256;

/// The fixed read-only job board the scan pages through.
pub trait ArbeitnowBoard {
    /// Fetch one upstream page by number, with the optional visa sponsorship filter.
    fn fetch(&self, page: u32, visa_sponsorship: Option<bool>) -> Result<Page, BoardFailure>;
}

/// The candidate token authority the scan needs: a clock and an issuer.
pub trait CandidateIssuer {
    /// Read the clock once.
    fn now(&self) -> Result<ArbeitnowInstant, ApplicationError>;
    /// Issue a signed token for `candidate`.
    fn issue(&self, candidate: &ArbeitnowCandidate) -> Result<String, ApplicationError>;
}

impl CandidateIssuer for CandidateCodec {
    fn now(&self) -> Result<ArbeitnowInstant, ApplicationError> {
        CandidateCodec::now(self)
    }

    fn issue(&self, candidate: &ArbeitnowCandidate) -> Result<String, ApplicationError> {
        CandidateCodec::issue(self, candidate)
    }
}

/// A scan request. Every field is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanRequest {
    /// Free-text query: every term must be a substring of the job text.
    pub query: Option<String>,
    /// Location filter: a substring of the job location.
    pub location: Option<String>,
    /// Only remote jobs when `Some(true)`.
    pub remote_only: Option<bool>,
    /// Upstream visa sponsorship filter (not applied locally).
    pub visa_sponsorship: Option<bool>,
    /// Tag filter (OR within the list).
    pub tags: Option<Vec<String>>,
    /// Job type filter (OR within the list).
    pub job_types: Option<Vec<String>>,
    /// Jobs per scan, `1..=100` (default 25).
    pub limit: Option<i64>,
    /// Upstream pages per scan, `1..=5` (default 1).
    pub max_pages: Option<i64>,
    /// Cursor of an earlier scan with the same filters.
    pub cursor: Option<String>,
}

/// One mapped job of a scan result.
#[derive(Clone, PartialEq, Eq)]
pub struct ScannedJob {
    /// Board slug.
    pub slug: String,
    /// Canonical URL (`https://arbeitnow.com/view/<slug>`).
    pub canonical_url: String,
    /// Employer name.
    pub company: String,
    /// Job title.
    pub title: String,
    /// Location (empty when missing).
    pub location: String,
    /// Remote flag.
    pub remote: bool,
    /// Posting time when inside the Java range.
    pub posted_at: Option<ArbeitnowInstant>,
    /// Tags (at most 20).
    pub tags: Vec<String>,
    /// Job types (at most 20).
    pub job_types: Vec<String>,
    /// Description excerpt (at most 1000 code points).
    pub description_excerpt: String,
    /// Signed candidate token for import.
    pub candidate_token: String,
}

impl fmt::Debug for ScannedJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScannedJob")
            .field("slug", &self.slug)
            .finish_non_exhaustive()
    }
}

/// The outcome of a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanResult {
    /// Always `arbeitnow`.
    pub source: &'static str,
    /// Always the board documentation URL.
    pub source_docs_url: &'static str,
    /// The matched and mapped jobs.
    pub jobs: Vec<ScannedJob>,
    /// Upstream pages fetched.
    pub pages_scanned: u32,
    /// Sum of the page job counts with int32 wraparound.
    pub records_inspected: i32,
    /// Cursor of the next scan, when more may remain.
    pub next_cursor: Option<String>,
    /// Always the fixed consistency notice.
    pub consistency_notice: &'static str,
}

/// Java `int` addition of a page job count to the running total.
pub fn add_inspected(total: i32, count: usize) -> i32 {
    let wrapped = u64::try_from(count).unwrap_or(u64::MAX) & 0xFFFF_FFFF;
    let wrapped = u32::try_from(wrapped).unwrap_or(0);
    total.wrapping_add(i32::from_ne_bytes(wrapped.to_ne_bytes()))
}

fn failure(message: &str) -> ApplicationError {
    validation_error(message, None, None)
}

fn internal() -> ApplicationError {
    ApplicationError::of(ErrorCode::InternalError)
}

/// The code survives, the message is the code default, except the fixed
/// candidate message of a validation error from the codec. Details and causes are dropped.
fn safe_failure(error: &ApplicationError, candidate: bool) -> ApplicationError {
    if candidate
        && error.code() == ErrorCode::ValidationError
        && error.message() == CANDIDATE_MESSAGE
    {
        failure(CANDIDATE_MESSAGE)
    } else {
        ApplicationError::of(error.code())
    }
}

fn slug_ok(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= FIELD_CAP
        && value
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

/// Validates the request.
fn validate(request: &ScanRequest) -> Result<(), ApplicationError> {
    let bad = || failure(REQUEST_MESSAGE);
    if request
        .limit
        .is_some_and(|limit| !(1..=100).contains(&limit))
        || request
            .max_pages
            .is_some_and(|pages| !(1..=5).contains(&pages))
        || request.query.as_deref().is_some_and(|query| {
            utf16_length(query) > MAX_QUERY_UNITS || terms(Some(query)).len() > MAX_QUERY_TERMS
        })
        || request
            .location
            .as_deref()
            .is_some_and(|location| utf16_length(location) > MAX_LOCATION_UNITS)
    {
        return Err(bad());
    }
    for values in [&request.tags, &request.job_types].into_iter().flatten() {
        if values.len() > MAX_FILTER_ENTRIES
            || values
                .iter()
                .any(|value| java_strip(value).is_empty() || utf16_length(value) > MAX_FILTER_UNITS)
        {
            return Err(bad());
        }
    }
    Ok(())
}

/// The request filters in their normal form, computed once per scan.
struct Filters {
    terms: Vec<String>,
    location: Option<String>,
    remote_only: bool,
    tags: Vec<String>,
    job_types: Vec<String>,
}

impl Filters {
    fn new(request: &ScanRequest) -> Self {
        let normal = |values: &Option<Vec<String>>| -> Vec<String> {
            values
                .iter()
                .flatten()
                .map(|value| normalized(value))
                .collect()
        };
        Self {
            terms: terms(request.query.as_deref()),
            location: request.location.as_deref().map(normalized),
            remote_only: request.remote_only == Some(true),
            tags: normal(&request.tags),
            job_types: normal(&request.job_types),
        }
    }

    /// Whether the job matches the request filters.
    fn matches(&self, job: &UpstreamJob) -> bool {
        // Every check is a pure predicate that must all hold, so the cheap ones run first and the
        // HTML-to-text search text is built only when the query has terms.
        if self.remote_only && job.remote != Some(true) {
            return false;
        }
        if let Some(wanted) = &self.location
            && !root_lower(job.location.as_deref().unwrap_or("")).contains(wanted.as_str())
        {
            return false;
        }
        if !(any_equal(&job.tags, &self.tags) && any_equal(&job.job_types, &self.job_types)) {
            return false;
        }
        if self.terms.is_empty() {
            return true;
        }
        let searchable = root_lower(
            &[
                job.title.as_str(),
                job.company.as_str(),
                job.location.as_deref().unwrap_or(""),
                &plain(&job.html_description),
                &job.tags.join(" "),
                &job.job_types.join(" "),
            ]
            .join(" "),
        );
        self.terms
            .iter()
            .all(|term| searchable.contains(term.as_str()))
    }
}

/// An empty filter accepts everything; otherwise some value must equal some filter value.
fn any_equal(actual: &[String], wanted: &[String]) -> bool {
    wanted.is_empty()
        || actual.iter().any(|value| {
            let lowered = root_lower(value);
            wanted.contains(&lowered)
        })
}

/// The scan use case over a board, a cursor key and a candidate authority.
pub struct ArbeitnowScanService<B, C = CandidateCodec> {
    board: B,
    cursors: CursorAuthority,
    candidates: C,
}

impl<B, C> fmt::Debug for ArbeitnowScanService<B, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArbeitnowScanService")
            .finish_non_exhaustive()
    }
}

impl<B: ArbeitnowBoard, C: CandidateIssuer> ArbeitnowScanService<B, C> {
    /// Build the service. The cursor key is owned and independent of the candidate authority.
    pub fn new(board: B, cursor_key: CursorKey, candidates: C) -> Self {
        Self {
            board,
            cursors: CursorAuthority::new(cursor_key),
            candidates,
        }
    }

    /// Run a scan. Request bounds fail before any board or clock effect.
    pub fn scan(&self, request: &ScanRequest) -> Result<ScanResult, ApplicationError> {
        validate(request)?;
        self.run(request)
    }

    fn fetch(&self, page: u32, visa: Option<bool>) -> Result<Page, ApplicationError> {
        self.board
            .fetch(page, visa)
            .map_err(|error| error.to_application_error())
    }

    fn map(&self, job: &UpstreamJob) -> Result<ScannedJob, ApplicationError> {
        if !slug_ok(&job.slug) {
            return Err(failure(UPSTREAM_MESSAGE));
        }
        let url = format!("{CANONICAL_PREFIX}{}", job.slug);
        let company = plain_bound(&job.company, FIELD_CAP);
        let title = plain_bound(&job.title, FIELD_CAP);
        let location = plain_bound(job.location.as_deref().unwrap_or(""), FIELD_CAP);
        let remote = job.remote == Some(true);
        let posted = job
            .created_at
            .filter(|second| (MIN_EPOCH_SECOND..=MAX_EPOCH_SECOND).contains(second))
            .and_then(|second| ArbeitnowInstant::new(second, 0).ok());
        let tags = bound_list(Some(job.tags.as_slice()));
        let job_types = bound_list(Some(job.job_types.as_slice()));
        let description = plain(&job.html_description);
        if java_strip(&title).is_empty() || java_strip(&description).is_empty() {
            return Err(failure(UPSTREAM_MESSAGE));
        }
        let issued = self.candidates.now().map_err(|_| internal())?;
        let expires = issued
            .checked_add_seconds(CANDIDATE_LIFETIME_SECONDS)
            .ok_or_else(internal)?;
        let candidate = ArbeitnowCandidate {
            issued_at: issued,
            expires_at: expires,
            slug: job.slug.clone(),
            canonical_url: url.clone(),
            company: company.clone(),
            title: title.clone(),
            location: location.clone(),
            remote,
            posted_at: posted,
            tags: tags.clone(),
            job_types: job_types.clone(),
            description_excerpt: description.clone(),
        };
        let token = self
            .candidates
            .issue(&candidate)
            .map_err(|error| safe_failure(&error, true))?;
        Ok(ScannedJob {
            slug: job.slug.clone(),
            canonical_url: url,
            company,
            title,
            location,
            remote,
            posted_at: posted,
            tags,
            job_types,
            description_excerpt: description,
            candidate_token: token,
        })
    }

    fn cursor(
        &self,
        page: u32,
        offset: usize,
        fingerprint: &str,
    ) -> Result<String, ApplicationError> {
        self.cursors
            .encode(page, offset, fingerprint)
            .ok_or_else(internal)
    }

    /// Runs the scan.
    fn run(&self, request: &ScanRequest) -> Result<ScanResult, ApplicationError> {
        let scope = Scope {
            query: request.query.as_deref(),
            location: request.location.as_deref(),
            remote_only: request.remote_only,
            visa_sponsorship: request.visa_sponsorship,
            tags: request.tags.as_deref(),
            job_types: request.job_types.as_deref(),
        };
        let fingerprint = self.cursors.fingerprint(&scope).ok_or_else(internal)?;
        let (mut page_number, mut offset) = match request.cursor.as_deref() {
            None => (1, 0),
            Some(token) => self
                .cursors
                .decode(token, &fingerprint)
                .ok_or_else(|| failure(CURSOR_MESSAGE))?,
        };
        let limit =
            usize::try_from(request.limit.unwrap_or(DEFAULT_LIMIT)).map_err(|_| internal())?;
        let maximum = u32::try_from(request.max_pages.unwrap_or(DEFAULT_MAX_PAGES))
            .map_err(|_| internal())?;
        let filters = Filters::new(request);
        let mut scanned = 0u32;
        let mut inspected = 0i32;
        let mut jobs: Vec<ScannedJob> = Vec::new();
        let next_cursor = loop {
            let page = self.fetch(page_number, request.visa_sponsorship)?;
            scanned += 1;
            inspected = add_inspected(inspected, page.jobs.len());
            let matches: Vec<&UpstreamJob> = page
                .jobs
                .iter()
                .filter(|job| filters.matches(job))
                .collect();
            if offset > matches.len() {
                return Err(failure(CURSOR_MESSAGE));
            }
            let take = (limit - jobs.len()).min(matches.len() - offset);
            for job in &matches[offset..offset + take] {
                jobs.push(self.map(job)?);
            }
            offset += take;
            if jobs.len() == limit {
                break if offset < matches.len() {
                    Some(self.cursor(page_number, offset, &fingerprint)?)
                } else if page.another_page_may_remain {
                    Some(self.cursor(page_number + 1, 0, &fingerprint)?)
                } else {
                    None
                };
            }
            if !page.another_page_may_remain {
                break None;
            }
            if scanned == maximum {
                break Some(self.cursor(page_number + 1, 0, &fingerprint)?);
            }
            page_number += 1;
            offset = 0;
        };
        Ok(ScanResult {
            source: SOURCE,
            source_docs_url: SOURCE_DOCS_URL,
            jobs,
            pages_scanned: scanned,
            records_inspected: inspected,
            next_cursor,
            consistency_notice: CONSISTENCY_NOTICE,
        })
    }
}
