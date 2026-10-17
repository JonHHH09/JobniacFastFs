//! Bounded job search: request validation, weighted prefix re-scoring of
//! the hydrated candidates, ordering, and the response envelope.
//!
//! The repository returns at most a bounded sample of candidate aggregates (the PostgreSQL adapter samples
//! at most 500 jobs). This module scores each candidate, drops the candidates with a score of zero or
//! less, orders the rest by score (descending) and identifier (ascending), and truncates to the
//! validated limit. A token of the query matches a token of a field when one is a prefix of the other,
//! so `javascript` matches `java` and `java` matches `javascript`; each query token counts at most once
//! per field, and each skill contributes its own score.

use jobniac_text::java::java_strip;
use jobniac_text::search::{MAX_QUERY_CHARACTERS, MAX_QUERY_TOKENS, tokens};
use serde::Serialize;
use serde_json::Value;

use crate::errors::{AppError, validation_error};
use crate::pagination::SearchCandidates;
use crate::profiles::repository::{READ_ONLY, UnitOfWork};

use super::model::{JobAggregate, JobRecord, JobSkillRecord};
use super::repository::JobSearchRepository;
use super::request::JOB_REQUEST_MESSAGE;

/// Limit used when the request gives none.
pub const DEFAULT_LIMIT: usize = 20;
/// Largest accepted limit.
pub const MAX_LIMIT: usize = 100;

/// Weight of each text field, in the matched-field order of the response.
const WEIGHTED_FIELDS: [(&str, i64); 7] = [
    ("title", 8),
    ("company", 5),
    ("location", 3),
    ("description", 3),
    ("experienceRequirement", 4),
    ("employmentType", 2),
    ("seniority", 2),
];

/// Weight of each skill name.
const SKILL_WEIGHT: i64 = 7;

/// One scored job of the response: `{job, skills, score, matchedFields}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSearchMatch {
    /// The job row.
    pub job: JobRecord,
    /// The skill rows of the job.
    pub skills: Vec<JobSkillRecord>,
    /// The score (positive).
    pub score: i64,
    /// The matched fields: `job.<field>` in the weighted order, then `job.skills`.
    pub matched_fields: Vec<String>,
}

/// The job-search envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSearchResponse {
    /// The query, Java-stripped.
    pub query: String,
    /// The distinct query tokens, in first-occurrence order.
    pub query_tokens: Vec<String>,
    /// The total match count, or null when the candidate sample is partial.
    pub total_matches: Option<i64>,
    /// The repository's match count (a lower bound when `has_more`).
    pub matched_count: i64,
    /// Whether more candidates exist beyond the sample.
    pub has_more: bool,
    /// The number of matches returned.
    pub returned_count: usize,
    /// The returned matches.
    pub jobs: Vec<JobSearchMatch>,
}

/// Validates and runs job searches over a unit of work and a search repository.
#[derive(Debug)]
pub struct JobSearchService<U, R> {
    uow: U,
    repo: R,
}

impl<U, R> JobSearchService<U, R>
where
    U: UnitOfWork,
    R: JobSearchRepository<Tx = U::Tx>,
{
    /// A service over a unit of work and a search repository.
    pub fn new(uow: U, repo: R) -> Self {
        Self { uow, repo }
    }

    /// Validates the request, searches, and formats the envelope.
    ///
    /// # Errors
    ///
    /// `Invalid job request` for the first invalid field (before any repository call), or a
    /// repository error.
    pub async fn search_jobs(&self, request: &Value) -> Result<JobSearchResponse, AppError> {
        let (query, query_tokens, limit) = validate_request(request)?;
        let candidates = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo
                    .search_job_candidates(tx, &query_tokens, limit)
                    .await
            })
            .await?;
        Ok(format_response(query, query_tokens, limit, &candidates))
    }
}

/// Validates the request: the object, the query (non-blank, at most 256
/// code points, with searchable tokens, at most 16 of them), and the limit (absent or 1 to 100, not a
/// boolean or a string).
fn validate_request(request: &Value) -> Result<(String, Vec<String>, usize), AppError> {
    let Value::Object(map) = request else {
        return Err(invalid("request", "must not be null"));
    };
    let raw_query = match map.get("query") {
        Some(Value::String(text)) if !java_strip(text).is_empty() => text,
        _ => return Err(invalid("query", "must not be blank")),
    };
    let query = java_strip(raw_query);
    if query.chars().count() > MAX_QUERY_CHARACTERS {
        return Err(invalid(
            "query",
            &format!("must not exceed {MAX_QUERY_CHARACTERS} characters"),
        ));
    }
    let query_tokens = tokens(Some(query));
    if query_tokens.is_empty() {
        return Err(invalid("query", "must contain searchable text"));
    }
    if query_tokens.len() > MAX_QUERY_TOKENS {
        return Err(invalid(
            "query",
            &format!("must contain at most {MAX_QUERY_TOKENS} searchable terms"),
        ));
    }
    let limit = match map.get("limit") {
        None | Some(Value::Null) => DEFAULT_LIMIT,
        Some(Value::Number(number)) => match number.as_i64() {
            Some(found) if (1..=100).contains(&found) => {
                usize::try_from(found).unwrap_or(DEFAULT_LIMIT)
            }
            _ => return Err(limit_error()),
        },
        Some(_) => return Err(limit_error()),
    };
    Ok((query.to_owned(), query_tokens, limit))
}

fn limit_error() -> AppError {
    invalid("limit", &format!("must be between 1 and {MAX_LIMIT}"))
}

fn invalid(field: &str, reason: &str) -> AppError {
    AppError::from(validation_error(
        JOB_REQUEST_MESSAGE,
        Some(field),
        Some(reason),
    ))
}

/// Scores the positive candidates, orders them and builds the envelope.
fn format_response(
    query: String,
    query_tokens: Vec<String>,
    limit: usize,
    candidates: &SearchCandidates<JobAggregate>,
) -> JobSearchResponse {
    // Score on borrowed aggregates; only the rows that survive the limit are cloned.
    let mut matches: Vec<(i64, Vec<String>, &JobAggregate)> = candidates
        .items()
        .iter()
        .map(|aggregate| {
            let (score, matched_fields) = score_candidate(aggregate, &query_tokens);
            (score, matched_fields, aggregate)
        })
        .filter(|(score, _, _)| *score > 0)
        .collect();
    matches.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.2.job.id.cmp(&right.2.job.id))
    });
    let positive = matches.len();
    let has_more = candidates.has_more();
    let matched_count = match candidates.matched_count() {
        known if known < 0 => i64::try_from(positive).unwrap_or(i64::MAX),
        known => known,
    };
    let jobs: Vec<JobSearchMatch> = matches
        .into_iter()
        .take(limit)
        .map(|(score, matched_fields, aggregate)| JobSearchMatch {
            job: aggregate.job.clone(),
            skills: aggregate.skills.clone(),
            score,
            matched_fields,
        })
        .collect();
    JobSearchResponse {
        query,
        query_tokens,
        total_matches: if has_more { None } else { Some(matched_count) },
        matched_count,
        has_more,
        returned_count: jobs.len(),
        jobs,
    }
}

/// Scores one candidate: the score and the matched field names.
fn score_candidate(aggregate: &JobAggregate, query_tokens: &[String]) -> (i64, Vec<String>) {
    let job = &aggregate.job;
    let fields: [Option<&str>; 7] = [
        Some(job.title.as_str()),
        job.company.as_deref(),
        job.location.as_deref(),
        Some(job.description.as_str()),
        job.experience_requirement.as_deref(),
        job.employment_type.as_deref(),
        job.seniority.as_deref(),
    ];
    let mut score = 0;
    let mut matched_fields = Vec::new();
    for ((name, weight), value) in WEIGHTED_FIELDS.iter().zip(fields) {
        let field_score = text_score(query_tokens, value, *weight);
        score += field_score;
        if field_score > 0 {
            matched_fields.push(format!("job.{name}"));
        }
    }
    let skill_score: i64 = aggregate
        .skills
        .iter()
        .map(|skill| text_score(query_tokens, Some(skill.skill.as_str()), SKILL_WEIGHT))
        .sum();
    score += skill_score;
    if skill_score > 0 {
        matched_fields.push("job.skills".to_owned());
    }
    (score, matched_fields)
}

/// `weight` times the number of query tokens that prefix-match some token of `value`.
fn text_score(query_tokens: &[String], value: Option<&str>, weight: i64) -> i64 {
    let text_tokens = tokens(value);
    let matched = query_tokens
        .iter()
        .filter(|query| {
            text_tokens
                .iter()
                .any(|text| text.starts_with(query.as_str()) || query.starts_with(text.as_str()))
        })
        .count();
    weight * i64::try_from(matched).unwrap_or(0)
}
