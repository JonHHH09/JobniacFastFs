//! Bounded profile search over the posting index.
//!
//! The SQL uses `$n` binds. It finds at most
//! [`MAX_POSTINGS_PER_TOKEN`] postings per query token, scores the best 500 candidates by identity,
//! and then scores each selected profile completely. A profile whose postings were cut off, or a
//! candidate set above the bound, sets `has_more`, which the core turns into a null total.

use jobniac_core::errors::AppError;
use jobniac_core::pagination::SearchCandidates;
use jobniac_core::profiles::model::ProfileIdentity;
use jobniac_core::profiles::search::ProfileSearchCandidate;
use jobniac_text::search::prefixes;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{col, db};
use tokio_postgres::Client;

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

/// The candidate sample query. Binds in order: query tokens, prefix values, prefix
/// owners, posting bound, posting fetch limit (twice in the sample), candidate fetch limit, the
/// candidate bound (twice).
pub const SEARCH_CANDIDATES_SQL: &str = r#"
WITH query_tokens AS (
    SELECT token FROM unnest($1::text[]) AS token
), query_prefixes AS (
    SELECT prefix, query_token
    FROM unnest($2::text[], $3::text[]) AS value(prefix, query_token)
), query_prefix_sets AS (
    SELECT query_token, array_agg(prefix ORDER BY prefix COLLATE "C") AS prefixes
    FROM query_prefixes
    GROUP BY query_token
), raw_hits AS (
    SELECT posting.profile_id, posting.field_key, query.token AS query_token, posting.weight,
           posting.ordinal > $4 AS truncated
    FROM query_tokens AS query
    CROSS JOIN LATERAL (
        WITH RECURSIVE sample(term, profile_id, field_key, weight, ordinal) AS (
            (
                SELECT posting.term, posting.profile_id, posting.field_key,
                       posting.weight, 1::bigint
                FROM profile.search_terms AS posting
                WHERE posting.term COLLATE "C" >= query.token COLLATE "C"
                  AND posting.term COLLATE "C" < (query.token || '~') COLLATE "C"
                ORDER BY posting.term COLLATE "C", posting.profile_id, posting.field_key
                LIMIT 1
            )
            UNION ALL
            SELECT next.term, next.profile_id, next.field_key, next.weight, sample.ordinal + 1
            FROM sample
            CROSS JOIN LATERAL (
                SELECT posting.term, posting.profile_id, posting.field_key, posting.weight
                FROM profile.search_terms AS posting
                WHERE posting.term COLLATE "C" >= query.token COLLATE "C"
                  AND posting.term COLLATE "C" < (query.token || '~') COLLATE "C"
                  AND (posting.term COLLATE "C", posting.profile_id, posting.field_key)
                      > (sample.term COLLATE "C", sample.profile_id, sample.field_key)
                ORDER BY posting.term COLLATE "C", posting.profile_id, posting.field_key
                LIMIT 1
            ) AS next
            WHERE sample.ordinal < $5
        )
        SELECT * FROM sample
    ) AS posting
    UNION ALL
    SELECT posting.profile_id, posting.field_key, prefix_set.query_token, posting.weight,
           posting.ordinal > $6 AS truncated
    FROM query_prefix_sets AS prefix_set
    CROSS JOIN LATERAL (
        SELECT sample.*,
               row_number() OVER (
                   ORDER BY sample.term COLLATE "C", sample.profile_id, sample.field_key
               ) AS ordinal
        FROM (
            SELECT posting.term, posting.profile_id, posting.field_key, posting.weight
            FROM profile.search_terms AS posting
            WHERE posting.term COLLATE "C" = ANY(prefix_set.prefixes)
            ORDER BY posting.term COLLATE "C", posting.profile_id, posting.field_key
            LIMIT $7
        ) AS sample
    ) AS posting
), posting_bounds AS (
    SELECT COALESCE(bool_or(truncated), false) AS posting_truncated FROM raw_hits
), posting_hits AS (
    SELECT DISTINCT profile_id, field_key, query_token, weight
    FROM raw_hits
    WHERE NOT truncated
), scored AS (
    SELECT profile_id, SUM(weight)::integer AS score
    FROM posting_hits
    GROUP BY profile_id
), field_keys AS (
    SELECT profile_id, array_agg(field_key ORDER BY field_key COLLATE "C") AS field_keys
    FROM (SELECT DISTINCT profile_id, field_key FROM posting_hits) AS matches
    GROUP BY profile_id
), candidate_pool AS (
    SELECT profile.id, profile.full_name, profile.email, profile.summary,
           profile.created_at, profile.updated_at, profile.revision,
           scored.score AS candidate_score
    FROM scored
    JOIN field_keys ON field_keys.profile_id = scored.profile_id
    CROSS JOIN LATERAL (
        SELECT profile.id, profile.full_name, profile.email, profile.summary,
               profile.created_at, profile.updated_at, profile.revision
        FROM profile.profiles AS profile
        WHERE profile.id = scored.profile_id
        LIMIT 1
    ) AS profile
    ORDER BY scored.score DESC, profile.full_name COLLATE "C", profile.id::text COLLATE "C"
    LIMIT $8
), candidate_bounds AS (
    SELECT count(*) > $9 AS candidate_truncated
    FROM candidate_pool
), selected_candidates AS (
    SELECT *
    FROM candidate_pool
    ORDER BY candidate_score DESC, full_name COLLATE "C", id::text COLLATE "C"
    LIMIT $10
), complete_hits AS (
    SELECT DISTINCT candidate.id AS profile_id, posting.field_key,
                    query.token AS query_token, posting.weight
    FROM selected_candidates AS candidate
    CROSS JOIN query_tokens AS query
    JOIN query_prefix_sets AS prefix_set ON prefix_set.query_token = query.token
    CROSS JOIN LATERAL (
        SELECT posting.field_key, posting.weight
        FROM profile.search_terms AS posting
        WHERE posting.profile_id = candidate.id
          AND (
              (posting.term COLLATE "C" >= query.token COLLATE "C"
               AND posting.term COLLATE "C" < (query.token || '~') COLLATE "C")
              OR posting.term = ANY(prefix_set.prefixes)
          )
        -- Keep this profile_id lookup parameterized; flattening can scan all postings.
        OFFSET 0
    ) AS posting
), complete_scored AS (
    SELECT profile_id, SUM(weight)::integer AS score
    FROM complete_hits
    GROUP BY profile_id
), complete_field_keys AS (
    -- The field key order follows aggregate child traversal.
    -- Skills and experiences can first match different subfields in different rows.
    SELECT matches.profile_id, array_agg(matches.field_key ORDER BY
        CASE split_part(matches.field_key, ':', 1)
            WHEN 'profile.fullName' THEN 0 WHEN 'profile.email' THEN 1
            WHEN 'profile.summary' THEN 2 WHEN 'contacts' THEN 3
            WHEN 'links' THEN 4 WHEN 'skills' THEN 5 WHEN 'languages' THEN 6
            WHEN 'education' THEN 7 WHEN 'experience' THEN 8
            WHEN 'projects' THEN 9 WHEN 'technologies' THEN 10 ELSE 11
        END,
        skill.display_order, skill.skill,
        experience.display_order, experience.start_date DESC NULLS LAST,
        COALESCE(skill.id, experience.id),
        CASE split_part(matches.field_key, ':', 3)
            WHEN 'skill' THEN 0 WHEN 'normalized' THEN 1 WHEN 'category' THEN 2
            WHEN 'company' THEN 0 WHEN 'title' THEN 1 WHEN 'location' THEN 2
            WHEN 'description' THEN 3 ELSE 0
        END,
        matches.field_key COLLATE "C"
    ) AS field_keys
    FROM (SELECT DISTINCT profile_id, field_key FROM complete_hits) AS matches
    LEFT JOIN profile.profile_skills AS skill
      ON skill.profile_id = matches.profile_id
     AND split_part(matches.field_key, ':', 1) = 'skills'
     AND skill.id::text = split_part(matches.field_key, ':', 2)
    LEFT JOIN profile.experiences AS experience
      ON experience.profile_id = matches.profile_id
     AND split_part(matches.field_key, ':', 1) = 'experience'
     AND experience.id::text = split_part(matches.field_key, ':', 2)
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
ORDER BY complete_scored.score DESC, candidate.id::text COLLATE "C"
"#;

/// Returns the bounded candidates for normalized query tokens. An empty token list matches nothing and reads nothing.
pub(crate) async fn search_profile_candidates(
    client: &Client,
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
    let maximum_postings = count_bind(MAX_POSTINGS_PER_TOKEN)?;
    let posting_fetch = count_bind(POSTING_FETCH_LIMIT)?;
    let candidate_fetch = count_bind(CANDIDATE_FETCH_LIMIT)?;
    let maximum_candidates = count_bind(MAX_SEARCH_CANDIDATES)?;
    let rows = db(client
        .query(
            SEARCH_CANDIDATES_SQL,
            &[
                &query_tokens,
                &query_prefixes.values(),
                &owners,
                &maximum_postings,
                &posting_fetch,
                &maximum_postings,
                &posting_fetch,
                &candidate_fetch,
                &maximum_candidates,
                &maximum_candidates,
            ],
        )
        .await)?;
    let mut truncated = rows.len() > MAX_SEARCH_CANDIDATES;
    let mut candidates = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        // The last select item is the unnamed `posting_truncated OR candidate_truncated` flag,
        // read by position; a value that does not decode is corrupted data, not an outage.
        let bounded: bool = row.try_get(9).map_err(|_| AppError::Internal)?;
        truncated |= bounded;
        if index >= MAX_SEARCH_CANDIDATES {
            continue;
        }
        let id: Uuid = col(row, "id")?;
        let score: i32 = col(row, "score")?;
        let field_keys: Vec<String> = col(row, "field_keys")?;
        candidates.push(ProfileSearchCandidate {
            profile: ProfileIdentity::new(
                id,
                col(row, "full_name")?,
                col(row, "email")?,
                col(row, "summary")?,
                col::<OffsetDateTime>(row, "created_at")?,
                col(row, "updated_at")?,
                col(row, "revision")?,
            ),
            score: i64::from(score),
            field_keys,
        });
    }
    let matched = i64::try_from(candidates.len()).map_err(|_| AppError::Internal)?;
    SearchCandidates::new(matched, truncated, candidates)
}

/// A count as a `bigint` bind.
fn count_bind(value: usize) -> Result<i64, AppError> {
    i64::try_from(value).map_err(|_| AppError::Internal)
}
