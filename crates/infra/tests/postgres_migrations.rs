//! Migration runner tests against a disposable PostgreSQL 18 server.
//!
//! Each test needs `JOBNIAC_TEST_PG_PORT` and is skipped with a notice when it is unset. Every
//! test works on its own fresh database with synthetic names.

use std::error::Error;

use jobniac_core::env::Environment;
use jobniac_infra::postgres::config::{DatabaseConfig, load_database_config};
use jobniac_infra::postgres::migrations::{
    SchemaState, inspect_schema, migrate, packaged_migrations,
};
use jobniac_testkit::pg::TestServer;
use time::OffsetDateTime;
use time::macros::datetime;
use tokio_postgres::Client;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const SEEDED_AT: OffsetDateTime = datetime!(2026-09-29 00:00 UTC);

/// The history table DDL of the runner, copied so that the test can record V0001 to V0003 itself.
const HISTORY_DDL: &str = "CREATE SCHEMA jobniac; \
    CREATE TABLE jobniac.schema_migrations (\
     version integer PRIMARY KEY CHECK (version > 0), \
     description text NOT NULL, \
     script text NOT NULL, \
     checksum char(64) NOT NULL, \
     applied_at timestamptz NOT NULL DEFAULT clock_timestamp(), \
     execution_ms integer NOT NULL CHECK (execution_ms >= 0))";

/// Applies the first three packaged migrations and their history rows, as an older build
/// did. The runner then applies V0004
/// onwards, which is the upgrade under test.
async fn migrate_through_v3(admin: &Client) -> TestResult {
    admin.batch_execute(HISTORY_DDL).await?;
    for migration in packaged_migrations()?.iter().take(3) {
        admin.batch_execute(&migration.sql).await?;
        admin
            .execute(
                "INSERT INTO jobniac.schema_migrations \
                 (version, description, script, checksum, execution_ms) \
                 VALUES ($1, $2, $3, $4, 0)",
                &[
                    &i32::try_from(migration.version)?,
                    &migration.description,
                    &migration.script,
                    &migration.checksum(),
                ],
            )
            .await?;
    }
    Ok(())
}

/// Seeds one text job with its provenance and one profile, as the pre-ASCII build left them. The
/// postings are added by each test, because they are the legacy data under test.
async fn seed_job_and_profile(client: &mut Client, job_id: Uuid, profile_id: Uuid) -> TestResult {
    let fingerprint = format!("{:064x}", job_id.as_u128());
    let transaction = client.transaction().await?;
    transaction
        .execute(
            "INSERT INTO job_schema.jobs \
             (id, source_method, source_label, title, description, canonical_fingerprint, \
              created_at, updated_at) \
             VALUES ($1, 'text', 'synthetic-upgrade', 'Javaıscript', 'Javaı Java ı', $2, $3, $3)",
            &[&job_id, &fingerprint, &SEEDED_AT],
        )
        .await?;
    transaction
        .execute(
            "INSERT INTO job_schema.job_text_ingestions \
             (id, job_id, source_label, input_text_hash, created_at) \
             VALUES ($1, $2, 'synthetic-upgrade', $3, $4)",
            &[
                &Uuid::from_u128(job_id.as_u128() + 1),
                &job_id,
                &fingerprint,
                &SEEDED_AT,
            ],
        )
        .await?;
    transaction
        .execute(
            "INSERT INTO profile.profiles (id, full_name, email, created_at, updated_at) \
             VALUES ($1, 'Javaıscript', 'ascii-upgrade@example.test', $2, $2)",
            &[&profile_id, &SEEDED_AT],
        )
        .await?;
    transaction.commit().await?;
    Ok(())
}

/// Legacy dotless-i postings of the job and the profile, as the old tokenizer wrote them.
async fn seed_legacy_postings(client: &Client, job_id: Uuid, profile_id: Uuid) -> TestResult {
    for (field, term, weight) in [
        ("job.title", "javaıscript", 8_i16),
        ("job.description", "javaı", 3),
        ("job.description", "java", 3),
        ("job.description", "ı", 3),
        ("job.description", "javaıjava", 3),
    ] {
        client
            .execute(
                "INSERT INTO job_schema.search_terms (job_id, field_key, term, weight) \
                 VALUES ($1, $2, $3, $4)",
                &[&job_id, &field, &term, &weight],
            )
            .await?;
    }
    for (field, term, weight) in [
        ("profile.fullName", "javaıscript", 8_i16),
        ("profile.summary", "javaı", 3),
        ("profile.summary", "ı", 3),
    ] {
        client
            .execute(
                "INSERT INTO profile.search_terms (profile_id, field_key, term, weight) \
                 VALUES ($1, $2, $3, $4)",
                &[&profile_id, &field, &term, &weight],
            )
            .await?;
    }
    Ok(())
}

/// Every posting of the job and profile tables as one sorted text row each.
async fn postings_of(client: &Client) -> Result<Vec<String>, Box<dyn Error>> {
    let rows = client
        .query(
            "SELECT 'job|' || job_id::text || '|' || field_key || '|' || term || '|' || weight::text \
             FROM job_schema.search_terms \
             UNION ALL \
             SELECT 'profile|' || profile_id::text || '|' || field_key || '|' || term || '|' || \
             weight::text FROM profile.search_terms \
             ORDER BY 1",
            &[],
        )
        .await?;
    Ok(rows.iter().map(|row| row.get::<_, String>(0)).collect())
}

/// The relations, indexes and triggers of the `job_schema`, which V0004 must not change.
async fn job_schema_objects(client: &Client) -> Result<Vec<String>, Box<dyn Error>> {
    let rows = client
        .query(
            "SELECT 'relation ' || c.relname || ' ' || c.relkind::text \
             FROM pg_catalog.pg_class AS c \
             JOIN pg_catalog.pg_namespace AS n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'job_schema' \
             UNION ALL \
             SELECT 'index ' || indexdef FROM pg_catalog.pg_indexes \
             WHERE schemaname = 'job_schema' \
             UNION ALL \
             SELECT 'trigger ' || t.tgname FROM pg_catalog.pg_trigger AS t \
             JOIN pg_catalog.pg_class AS c ON c.oid = t.tgrelid \
             JOIN pg_catalog.pg_namespace AS n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'job_schema' AND NOT t.tgisinternal \
             ORDER BY 1",
            &[],
        )
        .await?;
    Ok(rows.iter().map(|row| row.get::<_, String>(0)).collect())
}

/// The job rows and their text provenance, as JSON, which V0004 must not change.
async fn provenance_of(client: &Client) -> Result<Vec<String>, Box<dyn Error>> {
    let rows = client
        .query(
            "SELECT 'job ' || row_to_json(job)::text FROM job_schema.jobs AS job \
             UNION ALL \
             SELECT 'text ' || row_to_json(text)::text FROM job_schema.job_text_ingestions AS text \
             ORDER BY 1",
            &[],
        )
        .await?;
    Ok(rows.iter().map(|row| row.get::<_, String>(0)).collect())
}

/// Number of history rows of one version.
async fn history_rows(client: &Client, version: i32) -> Result<i64, Box<dyn Error>> {
    let row = client
        .query_one(
            "SELECT count(*) FROM jobniac.schema_migrations WHERE version = $1",
            &[&version],
        )
        .await?;
    Ok(row.get(0))
}

/// The validated configuration of a fresh database's environment.
fn config_for(environment: &Environment) -> Result<DatabaseConfig, Box<dyn Error>> {
    load_database_config(environment)
        .ok_or_else(|| "test environment is not a valid JOBNIAC_DB_* set".into())
}

#[tokio::test]
async fn migrate_applies_all_versions_to_an_empty_database() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let config = config_for(&database.environment())?;

    let before = inspect_schema(Some(&config)).await?;
    assert_eq!(before.state, SchemaState::Empty);
    assert_eq!(before.pending, vec![1, 2, 3, 4, 5, 6]);

    assert_eq!(migrate(&config).await?, vec![1, 2, 3, 4, 5, 6]);
    let after = inspect_schema(Some(&config)).await?;
    assert_eq!(after.state, SchemaState::Current);
    assert_eq!(after.current_version, Some(6));
    assert_eq!(after.latest_version, 6);
    assert!(after.pending.is_empty());
    Ok(())
}

#[tokio::test]
async fn second_migrate_applies_nothing() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let config = config_for(&database.environment())?;

    migrate(&config).await?;
    assert!(migrate(&config).await?.is_empty());
    assert_eq!(
        inspect_schema(Some(&config)).await?.state,
        SchemaState::Current
    );
    Ok(())
}

#[tokio::test]
async fn tampered_checksum_is_refused_before_any_write() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let config = config_for(&database.environment())?;
    migrate(&config).await?;

    let admin = database.admin_client().await;
    admin
        .batch_execute(
            "UPDATE jobniac.schema_migrations SET checksum = repeat('0', 64) WHERE version = 3",
        )
        .await?;

    let status = inspect_schema(Some(&config)).await?;
    assert_eq!(status.state, SchemaState::Foreign);
    assert_eq!(
        status.detail,
        "checksum mismatch for applied migration V0003"
    );

    let refusal = migrate(&config).await.err().ok_or("migrate must refuse")?;
    assert_eq!(refusal.0, "checksum mismatch for applied migration V0003");
    let rows = admin
        .query_one("SELECT count(*) FROM jobniac.schema_migrations", &[])
        .await?;
    assert_eq!(rows.get::<_, i64>(0), 6);
    Ok(())
}

#[tokio::test]
async fn history_row_newer_than_this_build_is_ahead() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let config = config_for(&database.environment())?;
    migrate(&config).await?;

    let admin = database.admin_client().await;
    admin
        .batch_execute(
            "INSERT INTO jobniac.schema_migrations \
             (version, description, script, checksum, execution_ms) \
             VALUES (7, 'future', 'V0007__future.sql', repeat('a', 64), 0)",
        )
        .await?;

    let status = inspect_schema(Some(&config)).await?;
    assert_eq!(status.state, SchemaState::Ahead);
    assert_eq!(status.current_version, Some(7));
    assert!(status.pending.is_empty());

    let refusal = migrate(&config).await.err().ok_or("migrate must refuse")?;
    assert_eq!(
        refusal.0,
        "database has migration V0007 unknown to this Jobniac build"
    );
    Ok(())
}

#[tokio::test]
async fn legacy_schema_without_history_is_refused() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let config = config_for(&database.environment())?;

    let admin = database.admin_client().await;
    admin
        .batch_execute(
            "CREATE SCHEMA profile; \
             CREATE TABLE profile.flyway_schema_history (installed_rank integer)",
        )
        .await?;

    let status = inspect_schema(Some(&config)).await?;
    assert_eq!(status.state, SchemaState::Foreign);
    assert_eq!(
        status.detail,
        "database holds schemas not managed by Jobniac"
    );

    let refusal = migrate(&config).await.err().ok_or("migrate must refuse")?;
    assert_eq!(
        refusal.0,
        "database contains a legacy job-engine-spring schema; \
         use the importer rehearsal against a fresh Jobniac database"
    );
    Ok(())
}

#[tokio::test]
async fn managed_schema_without_history_is_refused_before_any_write() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let config = config_for(&database.environment())?;

    let admin = database.admin_client().await;
    admin.batch_execute("CREATE SCHEMA resume").await?;

    let status = inspect_schema(Some(&config)).await?;
    assert_eq!(status.state, SchemaState::Foreign);
    assert_eq!(
        status.detail,
        "database holds schemas not managed by Jobniac"
    );

    let refusal = migrate(&config).await.err().ok_or("migrate must refuse")?;
    assert_eq!(
        refusal.0,
        "database contains Jobniac-managed schema names without Jobniac history"
    );
    let history = admin
        .query_one(
            "SELECT to_regclass('jobniac.schema_migrations') IS NULL",
            &[],
        )
        .await?;
    assert!(history.get::<_, bool>(0), "no history table was created");
    Ok(())
}

/// The V0004 upgrade rewrites the legacy dotless-i postings into ASCII terms, and leaves the job
/// schema objects, the job rows and their provenance exactly as they were.
#[tokio::test]
async fn test_v4_upgrade_restores_search_without_changing_objects_or_provenance() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let config = config_for(&database.environment())?;
    let mut admin = database.admin_client().await;
    migrate_through_v3(&admin).await?;
    let job_id = Uuid::from_u128(0x4a0b_0001);
    let profile_id = Uuid::from_u128(0x4a0b_0002);
    seed_job_and_profile(&mut admin, job_id, profile_id).await?;
    seed_legacy_postings(&admin, job_id, profile_id).await?;
    let objects_before = job_schema_objects(&admin).await?;
    let provenance_before = provenance_of(&admin).await?;

    assert_eq!(migrate(&config).await?, vec![4, 5, 6]);

    assert_eq!(job_schema_objects(&admin).await?, objects_before);
    assert_eq!(provenance_of(&admin).await?, provenance_before);
    let postings = postings_of(&admin).await?;
    assert!(
        postings.iter().all(|row| !row.contains('ı')),
        "no dotless-i posting survives the upgrade"
    );
    for expected in [
        format!("job|{job_id}|job.title|java|8"),
        format!("job|{job_id}|job.title|script|8"),
        format!("profile|{profile_id}|profile.fullName|java|8"),
        format!("profile|{profile_id}|profile.fullName|script|8"),
    ] {
        assert!(
            postings.contains(&expected),
            "missing upgraded posting {expected}"
        );
    }
    Ok(())
}

/// A failure inside V0004 rolls back its posting changes and its history row together. Once the
/// fault is removed, the retry applies V0004 onwards.
#[tokio::test]
async fn test_v4_failure_rolls_back_both_indexes_and_history_then_allows_retry() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let config = config_for(&database.environment())?;
    let mut admin = database.admin_client().await;
    migrate_through_v3(&admin).await?;
    let job_id = Uuid::from_u128(0x4a0c_0001);
    let profile_id = Uuid::from_u128(0x4a0c_0002);
    seed_job_and_profile(&mut admin, job_id, profile_id).await?;
    seed_legacy_postings(&admin, job_id, profile_id).await?;
    let postings_before = postings_of(&admin).await?;
    let objects_before = job_schema_objects(&admin).await?;

    // A fault on the job posting insert: V0004 deletes the legacy rows first, then fails here.
    admin
        .batch_execute(
            "CREATE FUNCTION job_schema.jobniac_v4_fault() RETURNS trigger \
             LANGUAGE plpgsql AS $fault$ BEGIN PERFORM 1 / 0; RETURN NULL; END $fault$; \
             CREATE TRIGGER jobniac_v4_fault BEFORE INSERT ON job_schema.search_terms \
             FOR EACH ROW EXECUTE FUNCTION job_schema.jobniac_v4_fault();",
        )
        .await?;
    let objects_with_fault = job_schema_objects(&admin).await?;

    let failure = migrate(&config)
        .await
        .err()
        .ok_or("the injected fault must fail V0004")?;
    assert_eq!(
        failure.0,
        "migration failed (SQLSTATE 22012); no partial migration was committed"
    );
    assert_eq!(history_rows(&admin, 4).await?, 0, "no V0004 history row");
    assert_eq!(history_rows(&admin, 3).await?, 1, "V0003 stays recorded");
    assert_eq!(postings_of(&admin).await?, postings_before);
    assert_eq!(job_schema_objects(&admin).await?, objects_with_fault);
    assert_eq!(
        objects_with_fault.len(),
        objects_before.len() + 1,
        "only the injected trigger was added"
    );

    admin
        .batch_execute(
            "DROP TRIGGER jobniac_v4_fault ON job_schema.search_terms; \
             DROP FUNCTION job_schema.jobniac_v4_fault();",
        )
        .await?;
    assert_eq!(migrate(&config).await?, vec![4, 5, 6]);
    assert_eq!(history_rows(&admin, 4).await?, 1);
    assert!(
        postings_of(&admin)
            .await?
            .iter()
            .all(|row| !row.contains('ı')),
        "the retry removes the dotless-i postings"
    );
    Ok(())
}

/// Two postings that disagree on the weight of one ASCII term collide during the V0004 upgrade.
/// The migration refuses with the check-violation mapping and writes nothing.
#[tokio::test]
async fn test_v4_inconsistent_collision_weights_refuse_without_writes() -> TestResult {
    let Some(server) = TestServer::from_env() else {
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(());
    };
    let database = server.fresh_database().await;
    let config = config_for(&database.environment())?;
    let mut admin = database.admin_client().await;
    migrate_through_v3(&admin).await?;
    let job_id = Uuid::from_u128(0x4a0d_0001);
    let profile_id = Uuid::from_u128(0x4a0d_0002);
    seed_job_and_profile(&mut admin, job_id, profile_id).await?;
    // Both rows convert to the term `a` in job.title, with weights 5 and 3.
    for (term, weight) in [("aı", 5_i16), ("ıa", 3)] {
        admin
            .execute(
                "INSERT INTO job_schema.search_terms (job_id, field_key, term, weight) \
                 VALUES ($1, 'job.title', $2, $3)",
                &[&job_id, &term, &weight],
            )
            .await?;
    }
    let postings_before = postings_of(&admin).await?;

    let refusal = migrate(&config)
        .await
        .err()
        .ok_or("inconsistent collision weights must refuse the upgrade")?;
    assert_eq!(
        refusal.0,
        "migration failed (SQLSTATE 23514); no partial migration was committed"
    );
    assert_eq!(history_rows(&admin, 4).await?, 0, "no V0004 history row");
    assert_eq!(postings_of(&admin).await?, postings_before);
    Ok(())
}
