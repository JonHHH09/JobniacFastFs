//! Bounded profile search against a disposable PostgreSQL 18 server.
//!
//! The server-backed tests need
//! `JOBNIAC_TEST_PG_PORT` and skip with a notice when it is unset. All data is synthetic.

mod profile_support;

use jobniac_core::errors::{AppError, error_envelope};
use jobniac_infra::postgres::profiles::search::{
    MAX_POSTINGS_PER_TOKEN, MAX_SEARCH_CANDIDATES, POSTING_FETCH_LIMIT, SEARCH_CANDIDATES_SQL,
};
use jobniac_text::search::prefixes;
use profile_support::{TestResult, details, search, start};
use serde_json::json;
use time::{OffsetDateTime, macros::datetime};
use tokio_postgres::Client;
use uuid::Uuid;

const CREATED_AT: OffsetDateTime = datetime!(2026-07-13 12:00 UTC);
const UPDATED_AT: OffsetDateTime = datetime!(2026-07-14 12:00 UTC);

/// One synthetic profile row: identifier, full name, email and optional summary.
type ProfileRow = (Uuid, String, String, Option<String>);

/// One synthetic posting row: profile, field key, term and weight.
type TermRow = (Uuid, String, String, i16);

/// Inserts profile rows stamped with `created`, `updated` and revision 1.
async fn insert_profiles(
    client: &Client,
    rows: &[ProfileRow],
    created: OffsetDateTime,
    updated: OffsetDateTime,
) -> TestResult {
    for (id, full_name, email, summary) in rows {
        client
            .execute(
                "INSERT INTO profile.profiles \
                 (id, full_name, email, summary, created_at, updated_at, revision) \
                 VALUES ($1, $2, $3, $4, $5, $6, 1)",
                &[id, full_name, email, summary, &created, &updated],
            )
            .await?;
    }
    Ok(())
}

/// Inserts posting rows.
async fn insert_terms(client: &Client, rows: &[TermRow]) -> TestResult {
    for (profile_id, field_key, term, weight) in rows {
        client
            .execute(
                "INSERT INTO profile.search_terms (profile_id, field_key, term, weight) \
                 VALUES ($1, $2, $3, $4)",
                &[profile_id, field_key, term, weight],
            )
            .await?;
    }
    Ok(())
}

/// Name-only profiles with one `java` posting each, as the candidate tests insert them.
fn name_profiles(count: u128, name: impl Fn(u128) -> String) -> (Vec<ProfileRow>, Vec<TermRow>) {
    let profiles: Vec<ProfileRow> = (1..=count)
        .map(|index| {
            (
                Uuid::from_u128(index),
                name(index),
                format!("p{index}@example.test"),
                None,
            )
        })
        .collect();
    let terms: Vec<TermRow> = profiles
        .iter()
        .map(|(id, _, _, _)| (*id, "profile.fullName".to_owned(), "java".to_owned(), 8))
        .collect();
    (profiles, terms)
}

#[tokio::test]
async fn search_matches_both_prefix_directions_and_keeps_the_ranking() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let ids: Vec<Uuid> = [1, 2, 3].map(Uuid::from_u128).to_vec();
    let profiles: Vec<ProfileRow> = vec![
        (
            ids[0],
            "Zeta Java Profile".to_owned(),
            "zeta@example.test".to_owned(),
            Some("A private summary".to_owned()),
        ),
        (
            ids[1],
            "Alpha Jav Profile".to_owned(),
            "alpha@example.test".to_owned(),
            None,
        ),
        (
            ids[2],
            "Middle Profile".to_owned(),
            "middle@example.test".to_owned(),
            Some("Spr token".to_owned()),
        ),
    ];
    let skill_one = "10000000-0000-0000-0000-000000000001";
    let skill_two = "20000000-0000-0000-0000-000000000002";
    let terms: Vec<TermRow> = vec![
        (ids[0], "profile.fullName".to_owned(), "java".to_owned(), 8),
        (
            ids[0],
            format!("skills:{skill_one}:skill"),
            "spring".to_owned(),
            7,
        ),
        (
            ids[0],
            format!("skills:{skill_one}:normalized"),
            "spring".to_owned(),
            7,
        ),
        (ids[1], "profile.fullName".to_owned(), "jav".to_owned(), 8),
        (
            ids[1],
            format!("skills:{skill_two}:skill"),
            "spring".to_owned(),
            7,
        ),
        (
            ids[1],
            format!("skills:{skill_two}:normalized"),
            "spring".to_owned(),
            7,
        ),
        (ids[2], "profile.summary".to_owned(), "spr".to_owned(), 3),
    ];
    let client = fixture.client().await;
    insert_profiles(&client, &profiles, CREATED_AT, UPDATED_AT).await?;
    insert_terms(&client, &terms).await?;

    let response = search(&fixture.uow)
        .search_profiles(&json!({"query": " java sprin "}))
        .await?;
    assert_eq!(response.query, "java sprin");
    assert_eq!(response.query_tokens, ["java", "sprin"]);
    assert_eq!(response.total_matches, Some(3));
    assert_eq!(response.matched_count, 3);
    assert!(!response.has_more);
    assert_eq!(response.returned_count, 3);
    let order: Vec<Uuid> = response.profiles.iter().map(|m| m.profile.id).collect();
    assert_eq!(order, ids);
    let scores: Vec<i64> = response.profiles.iter().map(|m| m.score).collect();
    assert_eq!(scores, [22, 22, 3]);
    let matched: Vec<Vec<String>> = response
        .profiles
        .iter()
        .map(|m| m.matched_fields.clone())
        .collect();
    assert_eq!(
        matched,
        [
            vec!["profile.fullName".to_owned(), "skills".to_owned()],
            vec!["profile.fullName".to_owned(), "skills".to_owned()],
            vec!["profile.summary".to_owned()],
        ]
    );
    assert!(response.profiles[0].profile.raw_resume_text.is_none());
    assert!(response.profiles[0].profile.embedding.is_none());

    let injected = search(&fixture.uow)
        .search_profiles(&json!({"query": "java' OR TRUE --"}))
        .await?;
    assert_eq!(injected.query_tokens, ["java", "or", "true"]);
    let injected_order: Vec<Uuid> = injected.profiles.iter().map(|m| m.profile.id).collect();
    assert_eq!(injected_order, [ids[0], ids[1]]);
    Ok(())
}

#[tokio::test]
async fn a_candidate_bound_gives_a_lower_bound_and_a_null_total() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let (profiles, terms) = name_profiles(501, |index| format!("Java Profile {index:03}"));
    let client = fixture.client().await;
    insert_profiles(&client, &profiles, CREATED_AT, CREATED_AT).await?;
    insert_terms(&client, &terms).await?;

    let response = search(&fixture.uow)
        .search_profiles(&json!({"query": "java", "limit": 1}))
        .await?;
    assert_eq!(response.total_matches, None);
    assert_eq!(response.matched_count, 500);
    assert!(response.has_more);
    assert_eq!(response.returned_count, 1);
    assert_eq!(response.profiles[0].profile.id, Uuid::from_u128(1));
    Ok(())
}

#[tokio::test]
async fn the_candidate_bound_uses_the_name_tiebreak_before_the_identifier() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let (profiles, terms) = name_profiles(501, |index| match index {
        1 => "Z Candidate".to_owned(),
        501 => "A Candidate".to_owned(),
        other => format!("M Candidate {other:03}"),
    });
    let client = fixture.client().await;
    insert_profiles(&client, &profiles, CREATED_AT, UPDATED_AT).await?;
    insert_terms(&client, &terms).await?;

    let response = search(&fixture.uow)
        .search_profiles(&json!({"query": "java", "limit": 100}))
        .await?;
    assert_eq!(response.total_matches, None);
    assert_eq!(response.matched_count, 500);
    assert!(response.has_more);
    assert_eq!(response.returned_count, 100);
    let order: Vec<Uuid> = response.profiles.iter().map(|m| m.profile.id).collect();
    let expected: Vec<Uuid> = (2..=101).map(Uuid::from_u128).collect();
    assert_eq!(order, expected);
    assert!(response.profiles.iter().all(|m| m.score == 8));
    Ok(())
}

#[tokio::test]
async fn the_posting_bound_scores_selected_profiles_completely_and_reports_the_remainder()
-> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let partial = Uuid::from_u128(1);
    let complete = Uuid::from_u128(2);
    let partial_postings: i64 = 5_000;
    let complete_postings: i64 = 5_919;
    let bound = i64::try_from(MAX_POSTINGS_PER_TOKEN).map_err(|_| "bound")?;
    assert!(partial_postings < bound);
    assert!(partial_postings + complete_postings > bound);
    let client = fixture.client().await;
    insert_profiles(
        &client,
        &[
            (
                partial,
                "Partial Candidate".to_owned(),
                "partial@example.test".to_owned(),
                None,
            ),
            (
                complete,
                "Full Score Candidate".to_owned(),
                "complete@example.test".to_owned(),
                None,
            ),
        ],
        CREATED_AT,
        UPDATED_AT,
    )
    .await?;
    client
        .execute(
            "INSERT INTO profile.search_terms (profile_id, field_key, term, weight) \
             SELECT $1, 'contacts:' || value::text || ':value', 'java', 1 \
             FROM generate_series(1, $2::bigint) AS value",
            &[&partial, &partial_postings],
        )
        .await?;
    client
        .execute(
            "INSERT INTO profile.search_terms (profile_id, field_key, term, weight) \
             SELECT $1, \
                    CASE WHEN value <= 3000 \
                         THEN 'contacts:' || value::text || ':value' \
                         ELSE 'skills:' || value::text || ':skill' \
                    END, \
                    'java', 1 \
             FROM generate_series(1, $2::bigint) AS value",
            &[&complete, &complete_postings],
        )
        .await?;

    let response = search(&fixture.uow)
        .search_profiles(&json!({"query": "java", "limit": 10}))
        .await?;
    assert_eq!(response.total_matches, None);
    assert_eq!(response.matched_count, 2);
    assert!(response.has_more);
    assert_eq!(response.returned_count, 2);
    let order: Vec<Uuid> = response.profiles.iter().map(|m| m.profile.id).collect();
    assert_eq!(order, [complete, partial]);
    let scores: Vec<i64> = response.profiles.iter().map(|m| m.score).collect();
    assert_eq!(scores, [complete_postings, partial_postings]);
    let matched: Vec<Vec<String>> = response
        .profiles
        .iter()
        .map(|m| m.matched_fields.clone())
        .collect();
    assert_eq!(
        matched,
        [
            vec!["contacts".to_owned(), "skills".to_owned()],
            vec!["contacts".to_owned()],
        ]
    );
    Ok(())
}

#[tokio::test]
async fn the_common_prefix_search_uses_the_profile_term_prefix_index() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let client = fixture.client().await;
    client
        .batch_execute(
            "INSERT INTO profile.profiles \
                 (id, full_name, email, summary, created_at, updated_at, revision) \
             SELECT md5('profile-search-plan-' || value)::uuid, \
                    'Plan Profile ' || value, 'plan-' || value || '@example.test', \
                    'Nonmatching summary', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, 1 \
             FROM generate_series(1, 10000) AS value; \
             INSERT INTO profile.search_terms (profile_id, field_key, term, weight) \
             SELECT md5('profile-search-plan-' || value)::uuid, 'profile.fullName', 'jose', 8 \
             FROM generate_series(1, 10000) AS value; \
             ANALYZE profile.profiles; \
             ANALYZE profile.search_terms;",
        )
        .await?;

    let tokens = vec!["jose".to_owned()];
    let query_prefixes = prefixes(&tokens).ok_or("the query prefixes must build")?;
    let owners: Vec<&str> = query_prefixes
        .owners()
        .iter()
        .map(|owner| &**owner)
        .collect();
    let bound = i64::try_from(MAX_POSTINGS_PER_TOKEN)?;
    let fetch = i64::try_from(POSTING_FETCH_LIMIT)?;
    let candidates = i64::try_from(MAX_SEARCH_CANDIDATES + 1)?;
    let maximum = i64::try_from(MAX_SEARCH_CANDIDATES)?;
    let explain = format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT) {SEARCH_CANDIDATES_SQL}");
    let rows = client
        .query(
            &explain,
            &[
                &tokens,
                &query_prefixes.values(),
                &owners,
                &bound,
                &fetch,
                &bound,
                &fetch,
                &candidates,
                &maximum,
                &maximum,
            ],
        )
        .await?;
    let plan: Vec<String> = rows.iter().map(|row| row.get(0)).collect();
    let plan = plan.join("\n");
    assert!(
        plan.contains("profile_search_terms_term_prefix_idx"),
        "plan does not use the term prefix index:\n{plan}"
    );
    assert!(!plan.contains("Seq Scan on search_terms"), "{plan}");
    assert!(!plan.contains("Seq Scan on profiles"), "{plan}");
    Ok(())
}

#[tokio::test]
async fn matched_fields_follow_the_child_traversal_order() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let body = json!({
        "fullName": "Ada",
        "email": "order@example.test",
        "skills": [
            {"skill": "Other", "category": "Python", "displayOrder": 0},
            {"skill": "Python", "displayOrder": 1},
        ],
        "experiences": [
            {"company": "Other", "title": "Python", "displayOrder": 0},
            {"company": "Python", "title": "Other", "displayOrder": 1},
        ],
    });
    let created = details(&fixture.uow).create_profile(&body).await?;

    let result = search(&fixture.uow)
        .search_profiles(&json!({"query": "python"}))
        .await?;
    let top = result
        .profiles
        .first()
        .ok_or("the profile matching the query must be found")?;
    assert_eq!(top.profile.id, created.profile.id);
    assert_eq!(
        top.matched_fields,
        [
            "skills.category",
            "skills",
            "experience.title",
            "experience.company",
        ]
    );
    Ok(())
}

#[tokio::test]
async fn an_unconfigured_search_fails_before_reading_with_the_unavailable_message() -> TestResult {
    let uow = jobniac_infra::postgres::unit_of_work::PgUnitOfWork::new(
        jobniac_infra::postgres::pool::PgPool::new(None),
    );
    let error: AppError = search(&uow)
        .search_profiles(&json!({"query": "java"}))
        .await
        .err()
        .ok_or("a search without a database must fail")?;
    assert_eq!(
        error_envelope(&error).message,
        "Local database is not configured"
    );
    Ok(())
}
