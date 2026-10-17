//! Bounded profile search.
//!
//! The repository returns bounded candidates with a score and the matched posting keys. This
//! module validates the query, keeps the positive scores, orders them by score then identifier,
//! and formats the response: a null total when the candidate set is bounded, and the
//! matched fields grouped in aggregate order.

use jobniac_text::search::{MAX_QUERY_CHARACTERS, MAX_QUERY_TOKENS, tokens};
use jobniac_text::str_rules::strip_text;
use serde::Serialize;
use serde_json::Value;

use crate::errors::{AppError, validation_error};
use crate::pagination::SearchCandidates;

use super::model::ProfileIdentity;
use super::repository::{ProfileSearchRepository, READ_ONLY, UnitOfWork};

/// Page size used when the request gives no limit.
pub const DEFAULT_LIMIT: usize = 20;
/// Largest accepted limit.
pub const MAX_LIMIT: usize = 100;
/// Message of every profile search validation error.
pub const SEARCH_MESSAGE: &str = "Invalid profile search request";

/// The matched-field order of the response.
const MATCHED_FIELD_ORDER: [&str; 15] = [
    "profile.fullName",
    "profile.email",
    "profile.summary",
    "contacts",
    "links",
    "skills",
    "skills.category",
    "languages",
    "education",
    "experience.company",
    "experience.title",
    "experience.location",
    "experience.description",
    "projects",
    "projectTechnologies",
];

/// One repository candidate: its identity, its score and the posting keys that matched.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileSearchCandidate {
    /// The identity of the profile.
    pub profile: ProfileIdentity,
    /// The score; candidates with a score of 0 or less are not matches.
    pub score: i64,
    /// The matched posting keys, in aggregate traversal order.
    pub field_keys: Vec<String>,
}

/// One matched profile of the response.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSearchMatch {
    /// The profile identity.
    pub profile: ProfileIdentity,
    /// The score.
    pub score: i64,
    /// The matched field groups, in aggregate order.
    pub matched_fields: Vec<String>,
}

/// The profile-search response envelope.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSearchResponse {
    /// The stripped query.
    pub query: String,
    /// The distinct query tokens.
    pub query_tokens: Vec<String>,
    /// The total match count, or null when the candidate set is bounded.
    pub total_matches: Option<i64>,
    /// The repository's match count (a lower bound when `has_more`).
    pub matched_count: i64,
    /// Whether more candidates exist beyond the bounded set.
    pub has_more: bool,
    /// The number of matches returned.
    pub returned_count: usize,
    /// The returned matches.
    pub profiles: Vec<ProfileSearchMatch>,
}

/// Searches indexed profiles through a repository.
#[derive(Debug)]
pub struct ProfileSearchService<U, R> {
    uow: U,
    repo: R,
}

impl<U, R> ProfileSearchService<U, R>
where
    U: UnitOfWork,
    R: ProfileSearchRepository<Tx = U::Tx>,
{
    /// A service over a unit of work and a search repository.
    pub fn new(uow: U, repo: R) -> Self {
        Self { uow, repo }
    }

    /// Validates the request, searches, and formats the envelope.
    ///
    /// # Errors
    ///
    /// The `Invalid profile search request` error of the first invalid field, before any read.
    pub async fn search_profiles(
        &self,
        request: &Value,
    ) -> Result<ProfileSearchResponse, AppError> {
        let (query, query_tokens, limit) = validate_request(request)?;
        let candidates: SearchCandidates<ProfileSearchCandidate> = self
            .uow
            .transaction(READ_ONLY, async |tx| {
                self.repo.search_profile_candidates(tx, &query_tokens).await
            })
            .await?;
        Ok(format_response(query, query_tokens, limit, &candidates))
    }
}

/// Orders the positive candidates and builds the response (score descending, then identifier).
fn format_response(
    query: String,
    query_tokens: Vec<String>,
    limit: usize,
    candidates: &SearchCandidates<ProfileSearchCandidate>,
) -> ProfileSearchResponse {
    let mut matches: Vec<&ProfileSearchCandidate> = candidates
        .items()
        .iter()
        .filter(|candidate| candidate.score > 0)
        .collect();
    matches.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.profile.id.cmp(&right.profile.id))
    });
    let returned: Vec<ProfileSearchMatch> = matches
        .into_iter()
        .take(limit)
        .map(|candidate| ProfileSearchMatch {
            profile: candidate.profile.clone().without_private_fields(),
            score: candidate.score,
            matched_fields: matched_fields(&candidate.field_keys),
        })
        .collect();
    let has_more = candidates.has_more();
    let matched_count = candidates.matched_count();
    ProfileSearchResponse {
        query,
        query_tokens,
        total_matches: if has_more { None } else { Some(matched_count) },
        matched_count,
        has_more,
        returned_count: returned.len(),
        profiles: returned,
    }
}

/// Validates the query and the limit.
fn validate_request(request: &Value) -> Result<(String, Vec<String>, usize), AppError> {
    let Value::Object(map) = request else {
        return Err(invalid("request", "must not be null"));
    };
    let Some(Value::String(raw_query)) = map.get("query") else {
        return Err(invalid("query", "must not be blank"));
    };
    let query = strip_text(raw_query);
    if query.is_empty() {
        return Err(invalid("query", "must not be blank"));
    }
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
            Some(found) if (1..=i64::try_from(MAX_LIMIT).unwrap_or(i64::MAX)).contains(&found) => {
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
    AppError::from(validation_error(SEARCH_MESSAGE, Some(field), Some(reason)))
}

/// The matched field of one posting key, or `None` for a key outside the response.
fn matched_field(field_key: &str) -> Option<String> {
    if matches!(
        field_key,
        "profile.fullName" | "profile.email" | "profile.summary"
    ) {
        return Some(field_key.to_owned());
    }
    if field_key.starts_with("contacts:") {
        return Some("contacts".to_owned());
    }
    if field_key.starts_with("links:") {
        return Some("links".to_owned());
    }
    if field_key.starts_with("skills:") {
        let category = field_key.ends_with(":category");
        return Some(
            if category {
                "skills.category"
            } else {
                "skills"
            }
            .to_owned(),
        );
    }
    if field_key.starts_with("languages:") {
        return Some("languages".to_owned());
    }
    if field_key.starts_with("education:") {
        return Some("education".to_owned());
    }
    if field_key.starts_with("experience:") {
        let suffix = field_key.rsplit(':').next().unwrap_or("");
        let field = format!("experience.{suffix}");
        return MATCHED_FIELD_ORDER
            .contains(&field.as_str())
            .then_some(field);
    }
    if field_key.starts_with("projects:") {
        return Some("projects".to_owned());
    }
    if field_key.starts_with("technologies:") {
        return Some("projectTechnologies".to_owned());
    }
    None
}

/// The distinct matched fields, grouped by aggregate order. Within one group the first occurrence
/// order is kept (the sort is stable).
fn matched_fields(field_keys: &[String]) -> Vec<String> {
    let mut matched: Vec<String> = Vec::new();
    for key in field_keys {
        if let Some(field) = matched_field(key)
            && !matched.contains(&field)
        {
            matched.push(field);
        }
    }
    matched.sort_by_key(|field| group_order(field));
    matched
}

fn group_order(field: &str) -> usize {
    let group = if field == "skills.category" {
        "skills"
    } else if field.starts_with("experience.") {
        "experience.company"
    } else {
        field
    };
    MATCHED_FIELD_ORDER
        .iter()
        .position(|candidate| *candidate == group)
        .unwrap_or(usize::MAX)
}
