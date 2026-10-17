//! Profile write and read workflows against a disposable PostgreSQL 18 server.
//!
//! The server-backed tests need `JOBNIAC_TEST_PG_PORT` and skip with a notice when it is unset. All
//! data is synthetic.

mod profile_support;

use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::profiles::ProfileService;
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::postgres::profiles::PgProfileRepository;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use profile_support::{TestResult, details, personal, postings, request, start};
use serde_json::{Value, json};
use tokio_postgres::Client;
use uuid::Uuid;

const CONTACT_ID: &str = "11111111-1111-1111-1111-111111111111";
const PROJECT_ID: &str = "77777777-7777-7777-7777-777777777777";
const TECHNOLOGY_ID: &str = "88888888-8888-8888-8888-888888888888";
const EMAIL: &str = "workflow@example.test";

/// A complete request whose contacts are `count` phone numbers. Contact `i` has identifier `i` and
/// value `555-i`, so the number of contacts equals the number of replacements that wrote them.
fn contacts_request(count: i64) -> Value {
    let mut value = request(EMAIL, None, PROJECT_ID, TECHNOLOGY_ID);
    let contacts: Vec<Value> = (1..=count)
        .map(|index| {
            json!({
                "id": format!("{index:08x}-0000-4000-8000-{index:012x}"),
                "contactType": "phone",
                "contactValue": format!("555-{index:04}"),
                "label": "work",
            })
        })
        .collect();
    value["contacts"] = json!(contacts);
    value
}

/// The revision of the profile and the number of its contacts, read with raw SQL on `client`.
async fn revision_and_contacts(
    client: &Client,
    profile_id: Uuid,
) -> Result<(i64, i64), tokio_postgres::Error> {
    let revision: i64 = client
        .query_one(
            "SELECT revision FROM profile.profiles WHERE id = $1",
            &[&profile_id],
        )
        .await?
        .get(0);
    let contacts: i64 = client
        .query_one(
            "SELECT count(*) FROM profile.profile_contacts WHERE profile_id = $1",
            &[&profile_id],
        )
        .await?
        .get(0);
    Ok((revision, contacts))
}

#[tokio::test]
async fn a_project_row_lost_during_the_patch_rolls_back_the_profile_revision() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let service = details(&fixture.uow);
    let created = service
        .create_profile(&request(EMAIL, None, PROJECT_ID, TECHNOLOGY_ID))
        .await?;
    let profile_id = created.profile.id.to_string();

    // The profile row update deletes the project rows, so the project lock finds nothing and the
    // patch must roll back to its savepoint.
    let client = fixture.client().await;
    client
        .batch_execute(
            "CREATE OR REPLACE FUNCTION profile.test_drop_project_rows() RETURNS trigger \
             LANGUAGE plpgsql AS $fn$ BEGIN \
             DELETE FROM profile.project_technologies \
                 WHERE project_id IN (SELECT id FROM profile.projects WHERE profile_id = NEW.id); \
             DELETE FROM profile.projects WHERE profile_id = NEW.id; \
             RETURN NEW; END $fn$; \
             CREATE TRIGGER test_drop_project_rows BEFORE UPDATE ON profile.profiles \
             FOR EACH ROW EXECUTE FUNCTION profile.test_drop_project_rows();",
        )
        .await?;
    let conflict = service
        .update_profile_project(&json!({
            "profileId": profile_id,
            "projectId": PROJECT_ID,
            "expectedRevision": 0,
            "name": "Lost row",
        }))
        .await
        .err()
        .ok_or("a lost project row must conflict")?;
    assert_eq!(
        error_envelope(&conflict).message,
        "Profile revision conflict"
    );
    client
        .batch_execute("DROP TRIGGER test_drop_project_rows ON profile.profiles")
        .await?;

    let current = service.get_profile(&profile_id).await?;
    assert_eq!(current.profile.revision, 0);
    assert_eq!(current.profile.updated_at, created.profile.updated_at);
    assert_eq!(current.projects[0].name.as_deref(), Some("Compiler"));
    assert_eq!(current, created);
    Ok(())
}

#[tokio::test]
async fn a_personal_details_failure_rolls_back_the_parent_revision() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let service = details(&fixture.uow);
    let created = service
        .create_profile(&request(EMAIL, None, PROJECT_ID, TECHNOLOGY_ID))
        .await?;
    let profile_id = created.profile.id.to_string();

    let client = fixture.client().await;
    client
        .batch_execute(
            "CREATE OR REPLACE FUNCTION profile.test_raise_on_details() RETURNS trigger \
             LANGUAGE plpgsql AS $fn$ BEGIN \
             RAISE EXCEPTION 'secret details value' USING ERRCODE = 'P0001'; \
             END $fn$; \
             CREATE TRIGGER test_raise_on_details BEFORE INSERT ON profile.profile_personal_details \
             FOR EACH ROW EXECUTE FUNCTION profile.test_raise_on_details();",
        )
        .await?;
    let failure = personal(&fixture.uow)
        .update_personal_details(
            &profile_id,
            &json!(0),
            &json!({"dateOfBirth": "1906-12-09", "nationality": "Swiss"}),
        )
        .await
        .err()
        .ok_or("the details insert must fail")?;
    let envelope = error_envelope(&failure);
    assert_eq!(envelope.message, "Local database is unavailable");
    assert!(!format!("{failure:?}").contains("secret details value"));
    client
        .batch_execute("DROP TRIGGER test_raise_on_details ON profile.profile_personal_details")
        .await?;

    let after = service.get_profile(&profile_id).await?;
    assert_eq!(after.profile.revision, 0);
    assert_eq!(after.profile.updated_at, created.profile.updated_at);
    assert_eq!(after, created);
    let envelope = personal(&fixture.uow)
        .get_personal_details(&profile_id)
        .await?;
    assert_eq!(envelope.profile_revision, 0);
    assert!(envelope.personal_details.is_none());
    Ok(())
}

#[tokio::test]
async fn clearing_personal_details_keeps_photo_children_and_search_postings() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let service = details(&fixture.uow);
    let created = service
        .create_profile(&request(EMAIL, Some(CONTACT_ID), PROJECT_ID, TECHNOLOGY_ID))
        .await?;
    let profile_id = created.profile.id;
    let people = personal(&fixture.uow);
    let written = people
        .update_personal_details(
            &profile_id.to_string(),
            &json!(0),
            &json!({"dateOfBirth": "1906-12-09", "nationality": "Swiss"}),
        )
        .await?;
    assert_eq!(written.profile_revision, 1);
    let written_details = written
        .personal_details
        .ok_or("the written details must be present")?;

    // Seeds a photo reference. Replica mode skips the document foreign key and its guard trigger,
    // which would need a generated document row that this test does not create.
    let client = fixture.client().await;
    let photo = Uuid::from_u128(0x5a5a_5a5a_0000_4000_8000_0000_0000_0001);
    client
        .batch_execute("SET session_replication_role = replica")
        .await?;
    let seeded = client
        .execute(
            "UPDATE profile.profile_personal_details SET photo_document_id = $1 \
             WHERE profile_id = $2",
            &[&photo, &profile_id],
        )
        .await;
    // The reset runs even when the seeding UPDATE fails, so the admin session never keeps replica mode.
    client
        .batch_execute("SET session_replication_role = origin")
        .await?;
    seeded?;

    let before_graph = service.get_profile(&profile_id.to_string()).await?;
    let before_terms = postings(&client, profile_id).await?;
    let cleared = people
        .update_personal_details(
            &profile_id.to_string(),
            &json!(1),
            &json!({"dateOfBirth": null, "nationality": null}),
        )
        .await?;
    assert_eq!(cleared.profile_revision, 2);
    let cleared_details = cleared
        .personal_details
        .ok_or("a clear keeps the details row")?;
    assert_eq!(cleared_details.date_of_birth, None);
    assert_eq!(cleared_details.nationality, None);
    assert_eq!(cleared_details.photo_document_id, Some(photo));
    assert_eq!(cleared_details.created_at, written_details.created_at);

    let after_graph = service.get_profile(&profile_id.to_string()).await?;
    assert_eq!(after_graph.profile.revision, 2);
    assert_eq!(after_graph.contacts, before_graph.contacts);
    assert_eq!(after_graph.links, before_graph.links);
    assert_eq!(after_graph.skills, before_graph.skills);
    assert_eq!(after_graph.languages, before_graph.languages);
    assert_eq!(after_graph.education, before_graph.education);
    assert_eq!(after_graph.experiences, before_graph.experiences);
    assert_eq!(after_graph.projects, before_graph.projects);
    assert_eq!(postings(&client, profile_id).await?, before_terms);
    Ok(())
}

#[tokio::test]
async fn a_reader_keeps_the_old_snapshot_until_its_scope_ends() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let service = details(&fixture.uow);
    let created = service
        .create_profile(&request(EMAIL, None, PROJECT_ID, TECHNOLOGY_ID))
        .await?;
    let profile_id = created.profile.id;
    let id_text = profile_id.to_string();
    let read_only = TransactionOptions {
        read_only: true,
        ..TransactionOptions::default()
    };

    let (before, during) = fixture
        .uow
        .transaction::<_, AppError>(read_only, async |tx| {
            tx.client()
                .batch_execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                .await
                .map_err(|_| AppError::Internal)?;
            let before = revision_and_contacts(tx.client(), profile_id)
                .await
                .map_err(|_| AppError::Internal)?;
            // A writer commits a replacement on another session while this scope is open.
            service
                .update_profile(&id_text, &json!(0), &contacts_request(1))
                .await?;
            let during = revision_and_contacts(tx.client(), profile_id)
                .await
                .map_err(|_| AppError::Internal)?;
            Ok((before, during))
        })
        .await?;
    assert_eq!(before, (0, 0));
    assert_eq!(during, before, "the reader must keep its snapshot");

    let after = service.get_profile(&id_text).await?;
    assert_eq!(after.profile.revision, 1);
    assert_eq!(after.contacts.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_detail_read_never_mixes_revisions_during_a_concurrent_replace() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let service: ProfileService<PgUnitOfWork, PgProfileRepository> = details(&fixture.uow);
    let created = service.create_profile(&contacts_request(0)).await?;
    let id_text = created.profile.id.to_string();
    const WRITES: i64 = 12;
    const READS: usize = 40;

    let writer = async {
        for expected in 0..WRITES {
            service
                .update_profile(&id_text, &json!(expected), &contacts_request(expected + 1))
                .await
                .map_err(|error| format!("{error:?}"))?;
        }
        Ok::<(), String>(())
    };
    let reader = async {
        for _ in 0..READS {
            let graph = service
                .get_profile(&id_text)
                .await
                .map_err(|error| format!("{error:?}"))?;
            let contacts =
                i64::try_from(graph.contacts.len()).map_err(|error| error.to_string())?;
            if graph.profile.revision != contacts {
                return Err(format!(
                    "revision {} read with {contacts} contacts",
                    graph.profile.revision
                ));
            }
        }
        Ok::<(), String>(())
    };
    let (written, read) = tokio::join!(writer, reader);
    written?;
    read?;

    let stored = service.get_profile(&id_text).await?;
    assert_eq!(stored.profile.revision, WRITES);
    assert_eq!(stored.contacts.len(), usize::try_from(WRITES)?);
    Ok(())
}
