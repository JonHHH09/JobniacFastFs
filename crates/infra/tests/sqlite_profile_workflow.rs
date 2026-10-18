//! Profile write and read workflows against a disposable SQLite database. All data is synthetic.

mod sqlite_profile_support;

use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::profiles::ProfileService;
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::sqlite::error::SqliteError;
use jobniac_infra::sqlite::profiles::SqliteProfileRepository;
use jobniac_infra::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};
use rusqlite::params;
use serde_json::{Value, json};
use sqlite_profile_support::{TestResult, details, personal, postings, request, start};
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

/// The revision of the profile and the number of its contacts, read with raw SQL on the
/// transaction's connection.
fn revision_and_contacts(
    tx: &SqliteTransaction,
    profile_id: Uuid,
) -> Result<(i64, i64), SqliteError> {
    tx.run(|conn| {
        let revision: i64 = conn.query_row(
            "SELECT revision FROM profile_profiles WHERE id = ?1",
            params![profile_id.to_string()],
            |row| row.get(0),
        )?;
        let contacts: i64 = conn.query_row(
            "SELECT count(*) FROM profile_profile_contacts WHERE profile_id = ?1",
            params![profile_id.to_string()],
            |row| row.get(0),
        )?;
        Ok((revision, contacts))
    })
}

#[tokio::test]
async fn a_project_row_lost_during_the_patch_rolls_back_the_profile_revision() -> TestResult {
    let fixture = start();
    let service = details(&fixture.uow);
    let created = service
        .create_profile(&request(EMAIL, None, PROJECT_ID, TECHNOLOGY_ID))
        .await?;
    let profile_id = created.profile.id.to_string();

    // The profile row update deletes the project rows, so the project lock finds nothing and the
    // patch must roll back to its savepoint.
    let client = fixture.connection();
    client.execute_batch(
        "CREATE TRIGGER test_drop_project_rows BEFORE UPDATE ON profile_profiles \
         FOR EACH ROW BEGIN \
         DELETE FROM profile_project_technologies \
             WHERE project_id IN (SELECT id FROM profile_projects WHERE profile_id = NEW.id); \
         DELETE FROM profile_projects WHERE profile_id = NEW.id; \
         END;",
    )?;
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
    client.execute_batch("DROP TRIGGER test_drop_project_rows")?;

    let current = service.get_profile(&profile_id).await?;
    assert_eq!(current.profile.revision, 0);
    assert_eq!(current.profile.updated_at, created.profile.updated_at);
    assert_eq!(current.projects[0].name.as_deref(), Some("Compiler"));
    assert_eq!(current, created);
    Ok(())
}

#[tokio::test]
async fn a_personal_details_failure_rolls_back_the_parent_revision() -> TestResult {
    let fixture = start();
    let service = details(&fixture.uow);
    let created = service
        .create_profile(&request(EMAIL, None, PROJECT_ID, TECHNOLOGY_ID))
        .await?;
    let profile_id = created.profile.id.to_string();

    let client = fixture.connection();
    // An engine error that is not an integrity violation, carrying a value that must not leak.
    client.execute_batch(
        "CREATE TRIGGER test_raise_on_details BEFORE INSERT ON profile_profile_personal_details \
         FOR EACH ROW BEGIN \
         SELECT json_extract('{secret details value', '$'); \
         END;",
    )?;
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
    client.execute_batch("DROP TRIGGER test_raise_on_details")?;

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
    let fixture = start();
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

    // Seeds a photo reference. Switching foreign keys off skips the document foreign key, which
    // would need a generated document row that this test does not create.
    let client = fixture.connection();
    let photo = Uuid::from_u128(0x5a5a_5a5a_0000_4000_8000_0000_0000_0001);
    client.execute_batch("PRAGMA foreign_keys = OFF")?;
    let seeded = client.execute(
        "UPDATE profile_profile_personal_details SET photo_document_id = ?1 WHERE profile_id = ?2",
        params![photo.to_string(), profile_id.to_string()],
    );
    // The reset runs even when the seeding UPDATE fails, so the raw connection never keeps it off.
    client.execute_batch("PRAGMA foreign_keys = ON")?;
    seeded?;

    let before_graph = service.get_profile(&profile_id.to_string()).await?;
    let before_terms = postings(&client, profile_id)?;
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
    assert_eq!(postings(&client, profile_id)?, before_terms);
    Ok(())
}

#[tokio::test]
async fn a_reader_keeps_the_old_snapshot_until_its_scope_ends() -> TestResult {
    let fixture = start();
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
            // The first read of the scope takes its snapshot.
            let before = revision_and_contacts(tx, profile_id).map_err(|_| AppError::Internal)?;
            // A writer commits a replacement on another session while this scope is open.
            service
                .update_profile(&id_text, &json!(0), &contacts_request(1))
                .await?;
            let during = revision_and_contacts(tx, profile_id).map_err(|_| AppError::Internal)?;
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
    let fixture = start();
    let service: ProfileService<SqliteUnitOfWork, SqliteProfileRepository> = details(&fixture.uow);
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
