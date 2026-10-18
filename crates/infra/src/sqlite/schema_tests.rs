//! The SQLite baseline schema: it loads, its constraints and triggers hold, and foreign keys are
//! enforced on pool connections.

use rusqlite::Connection;

use crate::sqlite::error::SqliteError;
use crate::sqlite::migrations::migrate;
use crate::sqlite::pool::SqlitePool;
use crate::sqlite::unit_of_work::SqliteUnitOfWork;
use jobniac_core::transactions::TransactionOptions;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WRITE: TransactionOptions = TransactionOptions {
    read_only: false,
    requires_new: false,
};

/// Ids of the shape the schema checks (36 lowercase hex and dash characters).
const PROFILE: &str = "00000000-0000-0000-0000-0000000000a1";
const JOB: &str = "00000000-0000-0000-0000-0000000000b1";
const DOCUMENT: &str = "00000000-0000-0000-0000-0000000000c1";
const BLOB: &str = "00000000-0000-0000-0000-0000000000d1";
const ROOT: &str = "00000000-0000-0000-0000-0000000000e1";
const ASSET: &str = "00000000-0000-0000-0000-0000000000f1";
const TS: &str = "2026-01-01T00:00:00.000000Z";
const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A migrated database file with a connection that has foreign keys on, like the pool's.
fn migrated()
-> Result<(tempfile::TempDir, std::path::PathBuf, Connection), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("schema.sqlite3");
    migrate(&path)?;
    let connection = Connection::open(&path)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok((directory, path, connection))
}

/// The SQLite result code of a failed statement.
fn failure_code(result: rusqlite::Result<usize>) -> Option<i32> {
    match result {
        Err(rusqlite::Error::SqliteFailure(error, _)) => Some(error.extended_code),
        _ => None,
    }
}

fn insert_profile(connection: &Connection, id: &str, email: &str) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO profile_profiles (id, full_name, email, created_at, updated_at) \
         VALUES (?1, 'Test Person', ?2, ?3, ?3)",
        rusqlite::params![id, email, TS],
    )
}

fn insert_job(connection: &Connection, id: &str, fingerprint: &str) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO job_jobs (id, source_method, title, description, canonical_fingerprint, \
         created_at, updated_at) VALUES (?1, 'text', 'Title', 'Description', ?2, ?3, ?3)",
        rusqlite::params![id, fingerprint, TS],
    )
}

fn insert_document(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
         VALUES (?1, ?2, 3, x'010203', ?3)",
        rusqlite::params![BLOB, SHA, TS],
    )?;
    connection.execute(
        "INSERT INTO document_documents \
         (id, blob_id, original_file_name, media_type, created_at, updated_at) \
         VALUES (?1, ?2, 'a.pdf', 'application/pdf', ?3, ?3)",
        rusqlite::params![DOCUMENT, BLOB, TS],
    )?;
    Ok(())
}

fn insert_root_and_asset(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO jobniac_generated_asset_roots (id, created_at) VALUES (?1, ?2)",
        rusqlite::params![ROOT, TS],
    )?;
    connection.execute(
        "INSERT INTO jobniac_generated_assets \
         (asset_id, root_id, relative_path, sha256, size_bytes, lifecycle, created_at) \
         VALUES (?1, ?2, ?3, ?4, 10, 'RESERVED', ?5)",
        rusqlite::params![ASSET, ROOT, format!("{ASSET}.pdf"), SHA, TS],
    )?;
    Ok(())
}

#[test]
fn the_baseline_loads_every_table_as_strict() -> TestResult {
    let (_directory, _path, connection) = migrated()?;
    let mut statement = connection.prepare(
        "SELECT name, strict FROM pragma_table_list \
         WHERE schema = 'main' AND type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
         ORDER BY name",
    )?;
    let tables = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    // 38 baseline tables plus the migration history.
    assert_eq!(tables.len(), 39, "{tables:?}");
    for (name, strict) in &tables {
        assert_eq!(*strict, 1, "{name} is STRICT");
    }
    let prefixes = [
        "cover_letter_",
        "document_",
        "job_",
        "jobniac_",
        "match_",
        "profile_",
        "resume_",
    ];
    for (name, _) in &tables {
        assert!(
            name == "schema_migrations" || prefixes.iter().any(|prefix| name.starts_with(prefix)),
            "{name} has no schema prefix"
        );
    }
    // The foreign-key graph is consistent: the engine finds no dangling reference.
    let violations: i64 =
        connection.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })?;
    assert_eq!(violations, 0);
    let quick: String = connection.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    assert_eq!(quick, "ok");
    Ok(())
}

#[test]
fn foreign_keys_are_enforced_and_cascade() -> TestResult {
    let (_directory, _path, connection) = migrated()?;
    // A skill of a job that does not exist is refused.
    let orphan = connection.execute(
        "INSERT INTO job_job_skills (id, job_id, skill, normalized_skill, created_at) \
         VALUES ('00000000-0000-0000-0000-000000000001', ?1, 'Rust', 'rust', ?2)",
        rusqlite::params![JOB, TS],
    );
    assert_eq!(
        failure_code(orphan),
        Some(rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY)
    );
    insert_job(&connection, JOB, "fingerprint-1")?;
    connection.execute(
        "INSERT INTO job_job_skills (id, job_id, skill, normalized_skill, created_at) \
         VALUES ('00000000-0000-0000-0000-000000000001', ?1, 'Rust', 'rust', ?2)",
        rusqlite::params![JOB, TS],
    )?;
    // ON DELETE CASCADE removes the skill with its job.
    connection.execute("DELETE FROM job_jobs WHERE id = ?1", [JOB])?;
    let skills: i64 =
        connection.query_row("SELECT count(*) FROM job_job_skills", [], |r| r.get(0))?;
    assert_eq!(skills, 0);
    // ON DELETE RESTRICT: a blob used by a document cannot be deleted.
    insert_document(&connection)?;
    let restricted = connection.execute("DELETE FROM document_blobs WHERE id = ?1", [BLOB]);
    // The engine reports a RESTRICT action with the trigger code; the classifier maps it back.
    assert!(matches!(
        restricted.map_err(SqliteError::from),
        Err(SqliteError::Constraint { ref sqlstate, .. }) if sqlstate == "23503"
    ));
    Ok(())
}

#[tokio::test]
async fn pool_connections_enforce_foreign_keys_and_classify_errors() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("pool-schema.sqlite3");
    migrate(&path)?;
    let uow = SqliteUnitOfWork::new(SqlitePool::open(&path)?);

    let orphan: Result<(), SqliteError> = uow
        .transaction(WRITE, async |tx| {
            tx.run(|conn| {
                conn.execute(
                    "INSERT INTO job_job_skills (id, job_id, skill, normalized_skill, created_at) \
                     VALUES ('00000000-0000-0000-0000-000000000001', ?1, 'Rust', 'rust', ?2)",
                    rusqlite::params![JOB, TS],
                )?;
                Ok(())
            })
        })
        .await;
    assert!(matches!(
        orphan,
        Err(SqliteError::Constraint { ref sqlstate, .. }) if sqlstate == "23503"
    ));

    let duplicate: Result<(), SqliteError> = uow
        .transaction(WRITE, async |tx| {
            tx.run(|conn| {
                insert_profile(conn, PROFILE, "a@example.test")?;
                insert_profile(
                    conn,
                    "00000000-0000-0000-0000-0000000000a2",
                    "a@example.test",
                )?;
                Ok(())
            })
        })
        .await;
    assert!(matches!(
        duplicate,
        Err(SqliteError::Constraint { ref sqlstate, constraint: Some(ref name) })
            if sqlstate == "23505"
                && (name == "profile_profiles.email" || name == "idx_profiles_email_canonical_unique")
    ));

    let blank: Result<(), SqliteError> = uow
        .transaction(WRITE, async |tx| {
            tx.run(|conn| {
                conn.execute(
                    "INSERT INTO profile_profiles (id, full_name, email, created_at, updated_at) \
                     VALUES (?1, '   ', 'b@example.test', ?2, ?2)",
                    rusqlite::params![PROFILE, TS],
                )?;
                Ok(())
            })
        })
        .await;
    assert!(matches!(
        blank,
        Err(SqliteError::Constraint { ref sqlstate, constraint: Some(ref name) })
            if sqlstate == "23514" && name == "profiles_full_name_not_blank"
    ));
    Ok(())
}

#[test]
fn check_constraints_reject_malformed_values() -> TestResult {
    let (_directory, _path, connection) = migrated()?;
    let check = rusqlite::ffi::SQLITE_CONSTRAINT_CHECK;

    // The id shape.
    assert_eq!(
        failure_code(insert_profile(&connection, "NOT-A-UUID", "a@example.test")),
        Some(check)
    );
    // Canonical email: upper case is refused.
    assert_eq!(
        failure_code(insert_profile(&connection, PROFILE, "A@Example.test")),
        Some(check)
    );
    // Blob checksum shape and size.
    let bad_sha = connection.execute(
        "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
         VALUES (?1, 'ZZ', 3, x'010203', ?2)",
        rusqlite::params![BLOB, TS],
    );
    assert_eq!(failure_code(bad_sha), Some(check));
    let bad_size = connection.execute(
        "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
         VALUES (?1, ?2, 9, x'010203', ?3)",
        rusqlite::params![BLOB, SHA, TS],
    );
    assert_eq!(failure_code(bad_size), Some(check));
    // Invalid JSON text.
    insert_profile(&connection, PROFILE, "a@example.test")?;
    insert_job(&connection, JOB, "fingerprint-1")?;
    let bad_json = connection.execute(
        "INSERT INTO match_reports (id, profile_id, job_id, profile_revision, job_revision, \
         algorithm_version, overall_score, confidence, outcome, blocker_mismatch, components, \
         evidence, created_at) VALUES ('00000000-0000-0000-0000-000000000009', ?1, ?2, ?3, ?3, \
         'v1', 50, 50, 'WEAK_MATCH', 0, 'not json', '[]', ?3)",
        rusqlite::params![PROFILE, JOB, TS],
    );
    assert_eq!(failure_code(bad_json), Some(check));
    // A JSON value of the wrong type.
    let wrong_type = connection.execute(
        "INSERT INTO match_reports (id, profile_id, job_id, profile_revision, job_revision, \
         algorithm_version, overall_score, confidence, outcome, blocker_mismatch, components, \
         evidence, created_at) VALUES ('00000000-0000-0000-0000-000000000009', ?1, ?2, ?3, ?3, \
         'v1', 50, 50, 'WEAK_MATCH', 0, '{}', '[]', ?3)",
        rusqlite::params![PROFILE, JOB, TS],
    );
    assert_eq!(failure_code(wrong_type), Some(check));
    // The IN (...) list of an enum column.
    let bad_outcome = connection.execute(
        "INSERT INTO match_reports (id, profile_id, job_id, profile_revision, job_revision, \
         algorithm_version, overall_score, confidence, outcome, blocker_mismatch, components, \
         evidence, created_at) VALUES ('00000000-0000-0000-0000-000000000009', ?1, ?2, ?3, ?3, \
         'v1', 50, 50, 'GREAT', 0, '[]', '[]', ?3)",
        rusqlite::params![PROFILE, JOB, TS],
    );
    assert_eq!(failure_code(bad_outcome), Some(check));
    // A valid report goes in.
    connection.execute(
        "INSERT INTO match_reports (id, profile_id, job_id, profile_revision, job_revision, \
         algorithm_version, overall_score, confidence, outcome, blocker_mismatch, components, \
         evidence, created_at) VALUES ('00000000-0000-0000-0000-000000000009', ?1, ?2, ?3, ?3, \
         'v1', 50, 50, 'WEAK_MATCH', 0, '[]', '[]', ?3)",
        rusqlite::params![PROFILE, JOB, TS],
    )?;
    Ok(())
}

#[test]
fn canonical_unique_indexes_cover_nulls_and_case() -> TestResult {
    let (_directory, _path, connection) = migrated()?;
    insert_profile(&connection, PROFILE, "a@example.test")?;
    // Rust fills the Unicode key; the index covers the key, not `lower()`.
    let add_experience = |id: &str, company: Option<&str>| {
        connection.execute(
            "INSERT INTO profile_experiences (id, profile_id, company, title, created_at, \
             company_key, title_key) VALUES (?1, ?2, ?3, 'Dev', ?4, ?5, 'dev')",
            rusqlite::params![id, PROFILE, company, TS, company.map(str::to_lowercase)],
        )
    };
    add_experience("00000000-0000-0000-0000-000000000011", None)?;
    // NULL company coalesces to '' : a second NULL-company row is a duplicate.
    assert_eq!(
        failure_code(add_experience("00000000-0000-0000-0000-000000000012", None)),
        Some(rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
    );
    add_experience("00000000-0000-0000-0000-000000000013", Some("Acme"))?;
    Ok(())
}

#[test]
fn match_history_is_immutable() -> TestResult {
    let (_directory, _path, connection) = migrated()?;
    insert_profile(&connection, PROFILE, "a@example.test")?;
    insert_job(&connection, JOB, "fingerprint-1")?;
    connection.execute(
        "INSERT INTO match_reports (id, profile_id, job_id, profile_revision, job_revision, \
         algorithm_version, overall_score, confidence, outcome, blocker_mismatch, components, \
         evidence, created_at) VALUES ('00000000-0000-0000-0000-000000000009', ?1, ?2, ?3, ?3, \
         'v1', 50, 50, 'WEAK_MATCH', 0, '[]', '[]', ?3)",
        rusqlite::params![PROFILE, JOB, TS],
    )?;
    let update = connection.execute("UPDATE match_reports SET overall_score = 60", []);
    assert_eq!(
        failure_code(update),
        Some(rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER)
    );
    Ok(())
}

#[test]
fn generated_asset_triggers_keep_identity_and_lifecycle() -> TestResult {
    let (_directory, _path, connection) = migrated()?;
    let trigger = rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER;
    insert_root_and_asset(&connection)?;

    // Assets are born RESERVED.
    let active_birth = connection.execute(
        "INSERT INTO jobniac_generated_assets \
         (asset_id, root_id, relative_path, sha256, size_bytes, lifecycle, created_at, materialized_at) \
         VALUES ('00000000-0000-0000-0000-0000000000f2', ?1, '00000000-0000-0000-0000-0000000000f2.pdf', \
         ?2, 10, 'ACTIVE', ?3, ?3)",
        rusqlite::params![ROOT, SHA, TS],
    );
    assert_eq!(failure_code(active_birth), Some(trigger));
    // Identity columns are immutable.
    let rebind = connection.execute(
        "UPDATE jobniac_generated_assets SET size_bytes = 11 WHERE asset_id = ?1",
        [ASSET],
    );
    assert_eq!(failure_code(rebind), Some(trigger));
    // The lifecycle only moves forward: RESERVED to DELETED is not a step.
    let skip = connection.execute(
        "UPDATE jobniac_generated_assets SET lifecycle = 'DELETED' WHERE asset_id = ?1",
        [ASSET],
    );
    assert!(skip.is_err());
    // The birth marker is immutable.
    let marker = connection.execute(
        "UPDATE jobniac_generated_assets SET generation_intent_required = 1 WHERE asset_id = ?1",
        [ASSET],
    );
    assert_eq!(failure_code(marker), Some(trigger));
    // Tombstones are retained.
    let delete = connection.execute("DELETE FROM jobniac_generated_assets", []);
    assert_eq!(failure_code(delete), Some(trigger));

    // A legal step works and bumps row_version (the replacement of the xmin read).
    let before: i64 = connection.query_row(
        "SELECT row_version FROM jobniac_generated_assets WHERE asset_id = ?1",
        [ASSET],
        |r| r.get(0),
    )?;
    connection.execute(
        "UPDATE jobniac_generated_assets SET lifecycle = 'ACTIVE', materialized_at = ?2 \
         WHERE asset_id = ?1",
        rusqlite::params![ASSET, TS],
    )?;
    let after: i64 = connection.query_row(
        "SELECT row_version FROM jobniac_generated_assets WHERE asset_id = ?1",
        [ASSET],
        |r| r.get(0),
    )?;
    assert_eq!(after, before + 1);
    // And it does not regress.
    let back = connection.execute(
        "UPDATE jobniac_generated_assets SET lifecycle = 'RESERVED', materialized_at = NULL \
         WHERE asset_id = ?1",
        [ASSET],
    );
    assert_eq!(failure_code(back), Some(trigger));
    Ok(())
}

#[test]
fn generation_intents_enforce_shape_and_uniqueness() -> TestResult {
    let (_directory, _path, connection) = migrated()?;
    let check = rusqlite::ffi::SQLITE_CONSTRAINT_CHECK;
    insert_root_and_asset(&connection)?;
    let other_asset = "00000000-0000-0000-0000-0000000000f2";
    connection.execute(
        "INSERT INTO jobniac_generated_assets \
         (asset_id, root_id, relative_path, sha256, size_bytes, lifecycle, created_at) \
         VALUES (?1, ?2, ?3, ?4, 10, 'RESERVED', ?5)",
        rusqlite::params![other_asset, ROOT, format!("{other_asset}.pdf"), SHA, TS],
    )?;
    let operation = "00000000-0000-0000-0000-000000000077";
    let lease = "00000000-0000-0000-0000-000000000078";
    let insert = |asset: &str, purpose: &str, scope: Option<&str>, profile: Option<&str>| {
        connection.execute(
            "INSERT INTO jobniac_generation_intents (asset_id, operation_id, purpose, profile_id, \
             output_scope, state, lease_token, lease_started_at, lease_expires_at, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'PENDING', ?6, ?7, '2026-01-01T00:01:00.000000Z', ?7)",
            rusqlite::params![asset, operation, purpose, profile, scope, lease, TS],
        )
    };
    // A stand-alone PDF has no scope and no profile.
    assert_eq!(
        failure_code(insert(ASSET, "STANDALONE_PDF", Some("en"), None)),
        Some(check)
    );
    insert(ASSET, "STANDALONE_PDF", None, None)?;
    // NULLS NOT DISTINCT: the same operation cannot have two NULL-scoped stand-alone outputs.
    assert_eq!(
        failure_code(insert(other_asset, "STANDALONE_PDF", None, None)),
        Some(rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
    );
    // Intent history is retained.
    let delete = connection.execute("DELETE FROM jobniac_generation_intents", []);
    assert_eq!(
        failure_code(delete),
        Some(rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER)
    );
    Ok(())
}
