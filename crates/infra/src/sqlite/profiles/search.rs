//! Bounded profile search over the posting index.
//!
//! The SQL uses `?n` binds. It finds at most [`MAX_POSTINGS_PER_TOKEN`] postings per query token,
//! scores the best 500 candidates by identity, and then scores each selected profile completely. A
//! profile whose postings were cut off, or a candidate set above the bound, sets `has_more`, which
//! the core turns into a null total.
//!
//! Query shape: the query tokens and prefixes arrive as JSON arrays read
//! with `json_each`; each token reads its postings with one `LIMIT`-bounded range scan in
//! `(term, profile_id, field_key)` order, as a correlated sub-query that returns a JSON array (its
//! position is the posting ordinal, as in the job search); `array_agg(.. ORDER BY ..)` is
//! `json_group_array(.. ORDER BY ..)`; and every nullable ascending sort key says `NULLS LAST`,
//! because SQLite sorts NULL first on `ASC`.

use jobniac_core::errors::AppError;
use jobniac_core::pagination::SearchCandidates;
use jobniac_core::profiles::model::ProfileIdentity;
use jobniac_core::profiles::search::ProfileSearchCandidate;
use jobniac_text::search::prefixes;
use rusqlite::Connection;

use super::{col, query_rows};
use crate::sqlite::codec::{Id, Instant};

/// Largest number of candidate profiles returned by one search.
pub const MAX_SEARCH_CANDIDATES: usize = 500;
/// Number of searched fields per profile, which sizes the posting bound.
pub const SEARCH_FIELD_COUNT: usize = 19;
/// Postings read per query token before the posting bound is reported.
pub const MAX_POSTINGS_PER_TOKEN: usize = (MAX_SEARCH_CANDIDATES + 1) * SEARCH_FIELD_COUNT;
/// Postings fetched per token: one more than the bound, so truncation is visible.
pub const POSTING_FETCH_LIMIT: usize = MAX_POSTINGS_PER_TOKEN + 1;
/// Candidate profiles fetched before the candidate bound is applied.
const CANDIDATE_FETCH_LIMIT: usize = MAX_SEARCH_CANDIDATES + 1;

/// The candidate sample query. Binds in order: query tokens (JSON array), prefix values (JSON
/// array), prefix owners (JSON array), posting bound, posting fetch limit, posting bound, posting
/// fetch limit, candidate fetch limit, the candidate bound (twice).
pub const SEARCH_CANDIDATES_SQL: &str = r#"
WITH query_tokens AS (
    SELECT key AS position, value AS token FROM json_each(?1)
), query_prefixes AS (
    SELECT prefix.value AS prefix, owner.value AS query_token
    FROM json_each(?2) AS prefix
    JOIN json_each(?3) AS owner ON owner.key = prefix.key
), query_prefix_sets AS (
    SELECT query_token, json_group_array(prefix) AS prefixes
    FROM query_prefixes GROUP BY query_token
), token_samples AS (
    SELECT query.token AS query_token,
           (SELECT json_group_array(json_array(sample.profile_id, sample.field_key, sample.weight))
            FROM (
                SELECT posting.profile_id, posting.field_key, posting.weight
                FROM profile_search_terms AS posting
                WHERE posting.term >= query.token
                  AND posting.term < query.token || '~'
                ORDER BY posting.term, posting.profile_id, posting.field_key
                LIMIT ?5
            ) AS sample) AS postings
    FROM query_tokens AS query
), prefix_samples AS (
    SELECT prefix_set.query_token AS query_token,
           (SELECT json_group_array(json_array(sample.profile_id, sample.field_key, sample.weight))
            FROM (
                SELECT posting.profile_id, posting.field_key, posting.weight
                FROM profile_search_terms AS posting
                WHERE posting.term IN (SELECT value FROM json_each(prefix_set.prefixes))
                ORDER BY posting.term, posting.profile_id, posting.field_key
                LIMIT ?7
            ) AS sample) AS postings
    FROM query_prefix_sets AS prefix_set
), raw_hits AS (
    SELECT json_extract(hit.value, '$[0]') AS profile_id, json_extract(hit.value, '$[1]') AS field_key,
           sample.query_token AS query_token, json_extract(hit.value, '$[2]') AS weight,
           hit.key + 1 > ?4 AS truncated
    FROM token_samples AS sample, json_each(sample.postings) AS hit
    UNION ALL
    SELECT json_extract(hit.value, '$[0]'), json_extract(hit.value, '$[1]'),
           sample.query_token, json_extract(hit.value, '$[2]'),
           hit.key + 1 > ?6
    FROM prefix_samples AS sample, json_each(sample.postings) AS hit
), posting_bounds AS (
    SELECT COALESCE(max(truncated), 0) AS posting_truncated FROM raw_hits
), posting_hits AS (
    SELECT DISTINCT profile_id, field_key, query_token, weight
    FROM raw_hits
    WHERE truncated = 0
), scored AS (
    SELECT profile_id, SUM(weight) AS score
    FROM posting_hits
    GROUP BY profile_id
), candidate_pool AS (
    SELECT profile.id, profile.full_name, profile.email, profile.summary,
           profile.created_at, profile.updated_at, profile.revision,
           scored.score AS candidate_score
    FROM scored
    JOIN profile_profiles AS profile ON profile.id = scored.profile_id
    ORDER BY scored.score DESC, profile.full_name, profile.id
    LIMIT ?8
), candidate_bounds AS (
    SELECT count(*) > ?9 AS candidate_truncated
    FROM candidate_pool
), selected_candidates AS (
    SELECT *
    FROM candidate_pool
    ORDER BY candidate_score DESC, full_name, id
    LIMIT ?10
), complete_hits AS (
    SELECT DISTINCT candidate.id AS profile_id, posting.field_key,
                    query.token AS query_token, posting.weight
    FROM selected_candidates AS candidate
    CROSS JOIN query_tokens AS query
    JOIN query_prefix_sets AS prefix_set ON prefix_set.query_token = query.token
    JOIN profile_search_terms AS posting
      ON posting.profile_id = candidate.id
     AND (
          (posting.term >= query.token AND posting.term < query.token || '~')
          OR posting.term IN (
              SELECT prefix FROM query_prefixes WHERE query_token = prefix_set.query_token
          )
     )
), complete_scored AS (
    SELECT profile_id, SUM(weight) AS score
    FROM complete_hits
    GROUP BY profile_id
), matched_fields AS (
    SELECT DISTINCT profile_id, field_key FROM complete_hits
), split_one AS (
    -- split_part(field_key, ':', 1) and the rest after the first separator.
    SELECT profile_id, field_key,
           substr(field_key || ':', 1, instr(field_key || ':', ':') - 1) AS part1,
           substr(field_key, instr(field_key || ':', ':') + 1) AS rest1
    FROM matched_fields
), split_two AS (
    SELECT profile_id, field_key, part1, part2, rest2,
           substr(rest2 || ':', 1, instr(rest2 || ':', ':') - 1) AS part3
    FROM (
        SELECT profile_id, field_key, part1,
               substr(rest1 || ':', 1, instr(rest1 || ':', ':') - 1) AS part2,
               substr(rest1, instr(rest1 || ':', ':') + 1) AS rest2
        FROM split_one
    )
), complete_field_keys AS (
    -- The field key order follows aggregate child traversal.
    -- Skills and experiences can first match different subfields in different rows.
    SELECT matches.profile_id, json_group_array(matches.field_key ORDER BY
        CASE matches.part1
            WHEN 'profile.fullName' THEN 0 WHEN 'profile.email' THEN 1
            WHEN 'profile.summary' THEN 2 WHEN 'contacts' THEN 3
            WHEN 'links' THEN 4 WHEN 'skills' THEN 5 WHEN 'languages' THEN 6
            WHEN 'education' THEN 7 WHEN 'experience' THEN 8
            WHEN 'projects' THEN 9 WHEN 'technologies' THEN 10 ELSE 11
        END,
        skill.display_order NULLS LAST, skill.skill NULLS LAST,
        experience.display_order NULLS LAST, experience.start_date DESC NULLS LAST,
        COALESCE(skill.id, experience.id) NULLS LAST,
        CASE matches.part3
            WHEN 'skill' THEN 0 WHEN 'normalized' THEN 1 WHEN 'category' THEN 2
            WHEN 'company' THEN 0 WHEN 'title' THEN 1 WHEN 'location' THEN 2
            WHEN 'description' THEN 3 ELSE 0
        END,
        matches.field_key
    ) AS field_keys
    FROM split_two AS matches
    LEFT JOIN profile_profile_skills AS skill
      ON skill.profile_id = matches.profile_id
     AND matches.part1 = 'skills'
     AND skill.id = matches.part2
    LEFT JOIN profile_experiences AS experience
      ON experience.profile_id = matches.profile_id
     AND matches.part1 = 'experience'
     AND experience.id = matches.part2
    GROUP BY matches.profile_id
)
SELECT candidate.id, candidate.full_name, candidate.email, candidate.summary,
       candidate.created_at, candidate.updated_at, candidate.revision, complete_scored.score,
       complete_field_keys.field_keys,
       (posting_bounds.posting_truncated OR candidate_bounds.candidate_truncated)
FROM selected_candidates AS candidate
JOIN complete_scored ON complete_scored.profile_id = candidate.id
JOIN complete_field_keys ON complete_field_keys.profile_id = candidate.id
CROSS JOIN posting_bounds
CROSS JOIN candidate_bounds
ORDER BY complete_scored.score DESC, candidate.id
"#;

/// Returns the bounded candidates for normalized query tokens. An empty token list matches nothing and reads nothing.
pub(crate) fn search_profile_candidates(
    conn: &Connection,
    query_tokens: &[String],
) -> Result<SearchCandidates<ProfileSearchCandidate>, AppError> {
    if query_tokens.is_empty() {
        return SearchCandidates::new(0, false, Vec::new());
    }
    let query_prefixes = prefixes(query_tokens).ok_or(AppError::Internal)?;
    let owners: Vec<&str> = query_prefixes
        .owners()
        .iter()
        .map(|owner| &**owner)
        .collect();
    let tokens = json_array(query_tokens)?;
    let prefix_values = json_array(query_prefixes.values())?;
    let prefix_owners = json_array(&owners)?;
    let maximum_postings = count_bind(MAX_POSTINGS_PER_TOKEN)?;
    let posting_fetch = count_bind(POSTING_FETCH_LIMIT)?;
    let candidate_fetch = count_bind(CANDIDATE_FETCH_LIMIT)?;
    let maximum_candidates = count_bind(MAX_SEARCH_CANDIDATES)?;
    let found = query_rows(
        conn,
        SEARCH_CANDIDATES_SQL,
        &[
            &tokens,
            &prefix_values,
            &prefix_owners,
            &maximum_postings,
            &posting_fetch,
            &maximum_postings,
            &posting_fetch,
            &candidate_fetch,
            &maximum_candidates,
            &maximum_candidates,
        ],
        |row| {
            // The last select item is the unnamed `posting_truncated OR candidate_truncated`
            // flag, read by position; a value that does not decode is corrupted data, not an
            // outage.
            let bounded: bool = row.get(9).map_err(|_| AppError::Internal)?;
            let field_keys: String = col(row, "field_keys")?;
            let field_keys: Vec<String> =
                serde_json::from_str(&field_keys).map_err(|_| AppError::Internal)?;
            let candidate = ProfileSearchCandidate {
                profile: ProfileIdentity::new(
                    col::<Id>(row, "id")?.0,
                    col(row, "full_name")?,
                    col(row, "email")?,
                    col(row, "summary")?,
                    col::<Instant>(row, "created_at")?.0,
                    col::<Instant>(row, "updated_at")?.0,
                    col(row, "revision")?,
                ),
                score: col(row, "score")?,
                field_keys,
            };
            Ok((bounded, candidate))
        },
    )?;
    let mut truncated = found.len() > MAX_SEARCH_CANDIDATES;
    let mut candidates = Vec::new();
    for (index, (bounded, candidate)) in found.into_iter().enumerate() {
        truncated |= bounded;
        if index < MAX_SEARCH_CANDIDATES {
            candidates.push(candidate);
        }
    }
    let matched = i64::try_from(candidates.len()).map_err(|_| AppError::Internal)?;
    SearchCandidates::new(matched, truncated, candidates)
}

/// A list of strings as the JSON array text that `json_each` reads.
fn json_array<S: AsRef<str>>(values: &[S]) -> Result<String, AppError> {
    let texts: Vec<&str> = values.iter().map(AsRef::as_ref).collect();
    serde_json::to_string(&texts).map_err(|_| AppError::Internal)
}

/// A count as an integer bind.
fn count_bind(value: usize) -> Result<i64, AppError> {
    i64::try_from(value).map_err(|_| AppError::Internal)
}
