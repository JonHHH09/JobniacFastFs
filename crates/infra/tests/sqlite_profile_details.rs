//! Profile aggregate persistence against a disposable SQLite database.
//!
//! Every test migrates its own file. The unconfigured and closed-pool cases need no file. All data
//! is synthetic.

mod sqlite_profile_support;

use jobniac_core::errors::{AppError, PERSISTENCE_CONSTRAINT_MESSAGE, error_envelope};
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use serde_json::json;
use sqlite_profile_support::{TestResult, details, has_posting, postings, request, start};
use time::macros::date;
use uuid::Uuid;

const CONTACT_ID: &str = "11111111-1111-1111-1111-111111111111";
const PROJECT_ID: &str = "77777777-7777-7777-7777-777777777777";
const TECHNOLOGY_ID: &str = "88888888-8888-8888-8888-888888888888";

#[tokio::test]
async fn an_unconfigured_pool_reports_the_unavailable_message_without_connecting() -> TestResult {
    let uow = SqliteUnitOfWork::new(SqlitePool::unconfigured());
    let service = details(&uow);
    let error = service
        .get_profile(&Uuid::nil().to_string())
        .await
        .err()
        .ok_or("a read without a configuration must fail")?;
    assert_eq!(
        error_envelope(&error).message,
        "Local database is not configured"
    );
    Ok(())
}

#[tokio::test]
async fn a_closed_pool_reports_unavailable_without_the_cause() -> TestResult {
    let fixture = start();
    let pool = fixture.database.pool();
    pool.close();
    let uow = SqliteUnitOfWork::new(pool);
    let service = details(&uow);
    let error = service
        .get_profile(&Uuid::nil().to_string())
        .await
        .err()
        .ok_or("a read from a closed pool must fail")?;
    let envelope = error_envelope(&error);
    assert_eq!(envelope.message, "Local database is unavailable");
    assert!(!envelope.message.contains("jobniac.sqlite3"));
    Ok(())
}

#[tokio::test]
async fn round_trips_all_children_and_refreshes_search_terms() -> TestResult {
    let fixture = start();
    let service = details(&fixture.uow);
    let created = service
        .create_profile(&request(
            "persisted@example.test",
            Some(CONTACT_ID),
            PROJECT_ID,
            TECHNOLOGY_ID,
        ))
        .await?;
    let profile_id = created.profile.id;
    assert_eq!(created.profile.revision, 0);
    assert_eq!(created.contacts[0].contact_type, "phone");
    assert_eq!(created.links[0].url, "https://example.test/grace");
    assert_eq!(created.skills[0].normalized_skill, "cobol");
    assert_eq!(created.languages[0].normalized_language, "english");
    assert_eq!(created.education[0].end_date, Some(date!(1934 - 01 - 01)));
    assert_eq!(
        created.experiences[0].description.as_deref(),
        Some("Compiler development")
    );
    assert_eq!(
        created.projects[0].technologies[0].normalized_technology,
        "cobol"
    );
    assert_eq!(
        created.project_technologies,
        created.projects[0].technologies
    );
    assert_eq!(service.get_profile(&profile_id.to_string()).await?, created);

    let client = fixture.connection();
    let old = postings(&client, profile_id)?;
    assert!(has_posting(&old, "profile.fullName", "grace", 8));
    let skill_id = created.skills[0].id;
    assert!(has_posting(
        &old,
        &format!("skills:{skill_id}:normalized"),
        "cobol",
        7
    ));

    let patched = service
        .update_profile_project(&json!({
            "profileId": profile_id.to_string(),
            "projectId": PROJECT_ID,
            "expectedRevision": 0,
            "name": "Renamed compiler",
            "technologies": [],
        }))
        .await?;
    assert_eq!(patched.profile_revision, 1);
    assert_eq!(patched.project.name.as_deref(), Some("Renamed compiler"));
    assert_eq!(
        patched.project.url.as_deref(),
        Some("https://example.test/compiler")
    );
    assert!(patched.project.technologies.is_empty());
    let patched_graph = service.get_profile(&profile_id.to_string()).await?;
    assert_eq!(patched_graph.contacts, created.contacts);
    assert!(patched_graph.project_technologies.is_empty());
    let renamed = postings(&client, profile_id)?;
    assert!(has_posting(
        &renamed,
        &format!("projects:{PROJECT_ID}:name"),
        "renamed",
        5
    ));
    assert!(
        !renamed
            .iter()
            .any(|(key, _, _)| key.starts_with("technologies:"))
    );

    let replaced = service
        .update_profile(
            &profile_id.to_string(),
            &json!(1),
            &json!({
                "fullName": "Grace Brewster Hopper",
                "email": "persisted@example.test",
                "summary": "Updated",
            }),
        )
        .await?;
    assert_eq!(replaced.profile.revision, 2);
    assert_eq!(replaced.profile.created_at, created.profile.created_at);
    assert!(replaced.contacts.is_empty());
    assert!(replaced.projects.is_empty());
    let terms = postings(&client, profile_id)?;
    assert!(has_posting(&terms, "profile.fullName", "grace", 8));
    assert!(!terms.iter().any(|(key, _, _)| key.starts_with("skills:")));
    assert!(!terms.iter().any(|(key, _, _)| key.starts_with("projects:")));
    Ok(())
}

#[tokio::test]
async fn a_failed_child_insert_rolls_back_the_replace_and_the_search_refresh() -> TestResult {
    let fixture = start();
    let service = details(&fixture.uow);
    let first = service
        .create_profile(&request(
            "persisted@example.test",
            Some(CONTACT_ID),
            PROJECT_ID,
            TECHNOLOGY_ID,
        ))
        .await?;
    let second_project = "99999999-9999-9999-9999-999999999999";
    let second_technology = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    let second = service
        .create_profile(&request(
            "second@example.test",
            None,
            second_project,
            second_technology,
        ))
        .await?;
    let second_id = second.profile.id;

    // The contact identifier belongs to the first profile, so the child insert violates the key.
    let failure = service
        .update_profile(
            &second_id.to_string(),
            &json!(0),
            &request(
                "second@example.test",
                Some(CONTACT_ID),
                second_project,
                second_technology,
            ),
        )
        .await
        .err()
        .ok_or("the duplicate contact must be refused")?;
    assert!(matches!(failure, AppError::PersistenceConstraint { .. }));
    assert_eq!(
        error_envelope(&failure).message,
        PERSISTENCE_CONSTRAINT_MESSAGE
    );

    let current = service.get_profile(&second_id.to_string()).await?;
    assert_eq!(current.profile.full_name, second.profile.full_name);
    assert_eq!(current.profile.revision, 0);
    assert!(current.contacts.is_empty());
    let client = fixture.connection();
    let terms = postings(&client, second_id)?;
    assert!(has_posting(&terms, "profile.fullName", "grace", 8));
    assert_ne!(first.profile.id, second_id);
    Ok(())
}

#[tokio::test]
async fn a_stale_project_patch_changes_nothing() -> TestResult {
    let fixture = start();
    let service = details(&fixture.uow);
    let created = service
        .create_profile(&request(
            "stale@example.test",
            None,
            PROJECT_ID,
            TECHNOLOGY_ID,
        ))
        .await?;
    let profile_id = created.profile.id.to_string();
    service
        .update_profile(
            &profile_id,
            &json!(0),
            &request("stale@example.test", None, PROJECT_ID, TECHNOLOGY_ID),
        )
        .await?;

    let failure = service
        .update_profile_project(&json!({
            "profileId": profile_id,
            "projectId": PROJECT_ID,
            "expectedRevision": 0,
            "name": "Stale rename",
        }))
        .await
        .err()
        .ok_or("a stale project revision must conflict")?;
    assert_eq!(
        error_envelope(&failure).message,
        "Profile revision conflict"
    );

    let current = service.get_profile(&profile_id).await?;
    assert_eq!(current.profile.revision, 1);
    assert_eq!(current.projects[0].name.as_deref(), Some("Compiler"));
    Ok(())
}

#[tokio::test]
async fn concurrent_replacements_with_one_revision_have_one_winner() -> TestResult {
    let fixture = start();
    let service = details(&fixture.uow);
    let created = service
        .create_profile(&request(
            "race@example.test",
            None,
            PROJECT_ID,
            TECHNOLOGY_ID,
        ))
        .await?;
    let profile_id = created.profile.id.to_string();
    let mut other = request("race@example.test", None, PROJECT_ID, TECHNOLOGY_ID);
    other["fullName"] = json!("Grace Other");
    let first_request = request("race@example.test", None, PROJECT_ID, TECHNOLOGY_ID);

    let revision = json!(0);
    let (first, second) = tokio::join!(
        service.update_profile(&profile_id, &revision, &first_request),
        service.update_profile(&profile_id, &revision, &other),
    );
    let outcomes = [first, second];
    let winners: Vec<_> = outcomes
        .iter()
        .filter_map(|outcome| outcome.as_ref().ok())
        .collect();
    assert_eq!(winners.len(), 1, "exactly one replacement must win");
    let losers = outcomes.iter().filter(|outcome| outcome.is_err()).count();
    assert_eq!(losers, 1);
    let stored = service.get_profile(&profile_id).await?;
    assert_eq!(stored.profile.revision, 1);
    assert_eq!(stored.profile.full_name, winners[0].profile.full_name);
    let client = fixture.connection();
    let terms = postings(&client, created.profile.id)?;
    let winner_token = if stored.profile.full_name == "Grace Other" {
        "other"
    } else {
        "grace"
    };
    assert!(has_posting(&terms, "profile.fullName", winner_token, 8));
    Ok(())
}
