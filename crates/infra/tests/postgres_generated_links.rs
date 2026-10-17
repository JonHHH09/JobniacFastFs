//! Profile resume-link replacement against a disposable PostgreSQL 18 server. The owner CAS and timestamp refusal, retirement of the
//! replaced asset in the same transaction, every reference class that keeps an asset alive, the
//! classified fence and its completion, the lock order against profile edits and cleanup, the
//! read-committed owner recheck, rollback of every row, and the advisory lock keys.
//!
//! Races are deterministic: a session holds an advisory key that a trigger on the link, asset or
//! intent table waits for, so a replacement stops at a known point while the test observes the
//! other sessions. The server-backed tests skip with a notice when `JOBNIAC_TEST_PG_PORT` is unset,
//! and panic instead when `CI` is set. The two tests that need no server (driver failures and
//! refusals before any connection) always run. All data is synthetic.
//!
//! Cases the Rust types make impossible are not written: a subclass of a link or binding, a
//! naive datetime, a non-UUID identifier, and a mutated asset identity (its fields are private).
//! A lost commit acknowledgement cannot be simulated: the transaction layer has no seam for it.

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use jobniac_core::cleanup::executor::{CleanupRepository, OwnedAssetRemover};
use jobniac_core::cleanup::model::{RemovalFailure, RemovalOutcome};
use jobniac_core::clock::{Clock, FixedClock, truncate_to_micros};
use jobniac_core::env::Environment;
use jobniac_core::errors::{AppError, ErrorCode, error_envelope};
use jobniac_core::generated_assets::{
    AssetIdentity, GENERATION_LEASE, GeneratedAssetRepository, GeneratedReservation,
    GenerationDescriptor, GenerationIntent, GenerationPurpose, GenerationState, IntentReservation,
    PreparationOutcome, ProfileResumeDocument, RETRY_BATCH_SIZE, sha256_hex,
};
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::postgres::cleanup::PgCleanupRepository;
use jobniac_infra::postgres::config::{DatabaseConfig, load_database_config};
use jobniac_infra::postgres::generated::IntentError;
use jobniac_infra::postgres::generated::assets::retire_asset;
use jobniac_infra::postgres::generated::bindings::{
    ASSET_COLUMNS, AssetRow, asset_lock_key_input, document_lock_key_input,
};
use jobniac_infra::postgres::generated::intents::{
    find_generation_intent, insert_generation_intent, renew_generation_intent,
};
use jobniac_infra::postgres::generated::profile_links::PgProfileResumeLinks;
use jobniac_infra::postgres::generated::repository::PgGeneratedAssetRepository;
use jobniac_infra::postgres::migrations::migrate;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::{PgTransaction, PgUnitOfWork};
use jobniac_testkit::pg::{TestDatabase, TestServer};
use std::collections::BTreeMap;
use time::{Duration, OffsetDateTime};
use tokio_postgres::Client;
use tokio_postgres::error::SqlState;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const MASTER: &str = "master_resume";
const CANADIAN: &str = "canadian_resume";
const CANADIAN_FR: &str = "canadian_resume_fr";

/// `application_name` of the profile-link sessions.
const LINK_APP: &str = "jobniac-profile-resume-links";
/// `application_name` of the cleanup and generation sessions.
const CLEANUP_APP: &str = "jobniac-cleanup";
/// The advisory keys the gates hold (`913719` for links, `913718` for retirement).
const GATE_LINK: i64 = 913_719;
const GATE_RETIREMENT: i64 = 913_718;
/// Keys of the gates on the classified intent and the expiry recovery.
const GATE_RENEWAL: i64 = 913_720;
const GATE_COMPLETION: i64 = 913_721;
const GATE_RECOVERY: i64 = 913_722;

/// The tables whose rows a refused or failed replacement must leave untouched.
const STATE_TABLES: [&str; 8] = [
    "profile.profiles",
    "profile.profile_resume_documents",
    "document.documents",
    "document.blobs",
    "jobniac.generated_assets",
    "jobniac.generated_asset_cleanups",
    "jobniac.generation_intents",
    "profile.profile_pdf_sources",
];

/// Every row of every table of [`STATE_TABLES`] as text, keyed by table.
type State = BTreeMap<&'static str, Vec<String>>;

/// A migrated disposable database with one synthetic profile, one job and one asset root.
struct Fixture {
    database: TestDatabase,
    config: DatabaseConfig,
    uow: PgUnitOfWork,
    profile: Uuid,
    job: Uuid,
    root: Uuid,
    /// The profile's `created_at` and `updated_at`: the owner revision the CAS compares against.
    base: OffsetDateTime,
}

/// A fresh migrated database, or `None` without a configured server.
async fn start() -> Result<Option<Fixture>, Box<dyn Error>> {
    let Some(server) = TestServer::from_env() else {
        // On CI a missing server must fail, never pass vacuously.
        assert!(
            std::env::var_os("CI").is_none(),
            "JOBNIAC_TEST_PG_PORT must be set in CI so that the profile link PostgreSQL tests run"
        );
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(None);
    };
    let database = server.fresh_database().await;
    let config = load_database_config(&database.environment())
        .ok_or("the disposable database configuration is invalid")?;
    migrate(&config).await.map_err(|error| error.0)?;
    let uow = PgUnitOfWork::new(PgPool::new(Some(config.clone())));
    let base = truncate_to_micros(OffsetDateTime::now_utc()) - Duration::hours(1);
    let profile = Uuid::new_v4();
    let job = Uuid::new_v4();
    let root = Uuid::new_v4();
    uow.transaction(options(), async |tx| {
        tx.client()
            .execute(
                "INSERT INTO profile.profiles (id, full_name, email, created_at, updated_at) \
                 VALUES ($1, 'Synthetic Person', $2, $3, $3)",
                &[&profile, &format!("{profile}@example.test"), &base],
            )
            .await
            .map_err(fixture_failure)?;
        tx.client()
            .execute(
                "INSERT INTO jobniac.generated_asset_roots (id, created_at) VALUES ($1, $2)",
                &[&root, &base],
            )
            .await
            .map_err(fixture_failure)?;
        Ok::<(), IntentError>(())
    })
    .await?;
    insert_job(&database, job, base).await?;
    Ok(Some(Fixture {
        database,
        config,
        uow,
        profile,
        job,
        root,
        base,
    }))
}

/// Inserts one synthetic job with its text ingestion, which the job schema requires at commit.
async fn insert_job(database: &TestDatabase, job: Uuid, at: OffsetDateTime) -> TestResult {
    let admin = database.admin_client().await;
    admin.batch_execute("BEGIN").await?;
    admin
        .execute(
            "INSERT INTO job_schema.jobs (id, source_method, title, description, \
             canonical_fingerprint, created_at, updated_at) \
             VALUES ($1, 'text', 'Synthetic Job', 'Synthetic description', $2, $3, $3)",
            &[&job, &job.to_string(), &at],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO job_schema.job_text_ingestions (id, job_id, input_text_hash, created_at) \
             VALUES ($1, $2, $3, $4)",
            &[
                &Uuid::new_v4(),
                &job,
                &sha256_hex(job.to_string().as_bytes()),
                &at,
            ],
        )
        .await?;
    admin.batch_execute("COMMIT").await?;
    Ok(())
}

fn options() -> TransactionOptions {
    TransactionOptions {
        read_only: false,
        requires_new: false,
    }
}

/// The database failure of a raw fixture statement, kept as an opaque internal error.
fn fixture_failure(_: tokio_postgres::Error) -> IntentError {
    IntentError::Database(AppError::Internal)
}

/// Synthetic PDF bytes, unique per call so that no two blobs share a digest.
fn pdf(label: &str) -> Vec<u8> {
    format!("%PDF-1.7\n% synthetic {label} {}\n", Uuid::new_v4()).into_bytes()
}

/// A clock that returns its instants in order and then repeats the last one.
#[derive(Debug)]
struct TickClock(Mutex<Vec<OffsetDateTime>>);

impl Clock for TickClock {
    fn now(&self) -> OffsetDateTime {
        let mut ticks = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if ticks.len() > 1 {
            ticks.remove(0)
        } else {
            ticks.first().copied().unwrap_or(OffsetDateTime::UNIX_EPOCH)
        }
    }
}

fn fixed(at: OffsetDateTime) -> Arc<dyn Clock> {
    Arc::new(FixedClock::new(at))
}

/// The profile-link repository of the fixture, on sessions named like the production sessions.
fn links(fixture: &Fixture, clock: Arc<dyn Clock>) -> PgProfileResumeLinks {
    let pool = PgPool::with_application_name(Some(fixture.config.clone()), LINK_APP);
    PgProfileResumeLinks::new(PgUnitOfWork::new(pool), clock)
}

/// The retirement repository of the fixture, on sessions named `jobniac-cleanup`.
fn cleanup(fixture: &Fixture, clock: Arc<dyn Clock>) -> PgCleanupRepository {
    let pool = PgPool::with_application_name(Some(fixture.config.clone()), CLEANUP_APP);
    PgCleanupRepository::new(PgUnitOfWork::new(pool), clock)
}

/// The classified generation repository of the fixture, on `jobniac-cleanup` sessions.
fn generated(fixture: &Fixture, clock: Arc<dyn Clock>) -> PgGeneratedAssetRepository {
    let pool = PgPool::with_application_name(Some(fixture.config.clone()), CLEANUP_APP);
    PgGeneratedAssetRepository::new(PgUnitOfWork::new(pool), clock)
}

/// Inserts a document and a RESERVED asset in the current transaction. The asset requires an
/// intent when `classified` is set, which the caller must then insert in the same transaction.
async fn insert_asset(
    tx: &mut PgTransaction,
    root: Uuid,
    now: OffsetDateTime,
    content: &[u8],
    classified: bool,
) -> Result<GeneratedReservation, IntentError> {
    let blob = Uuid::new_v4();
    let document = Uuid::new_v4();
    let asset_id = Uuid::new_v4();
    let digest = sha256_hex(content);
    let size = i64::try_from(content.len()).map_err(|_| IntentError::Invalid)?;
    let client = tx.client();
    client
        .execute(
            "INSERT INTO document.blobs (id, sha256, byte_size, content, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
            &[&blob, &digest, &size, &content, &now],
        )
        .await
        .map_err(fixture_failure)?;
    client
        .execute(
            "INSERT INTO document.documents (id, blob_id, original_file_name, media_type, \
             created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $5)",
            &[
                &document,
                &blob,
                &"synthetic-resume.pdf",
                &"application/pdf",
                &now,
            ],
        )
        .await
        .map_err(fixture_failure)?;
    client
        .execute(
            "INSERT INTO jobniac.generated_assets (asset_id, root_id, relative_path, sha256, \
             size_bytes, document_id, lifecycle, created_at, generation_intent_required) \
             VALUES ($1, $2, $3, $4, $5, $6, 'RESERVED', $7, $8)",
            &[
                &asset_id,
                &root,
                &format!("{asset_id}.pdf"),
                &digest,
                &size,
                &document,
                &now,
                &classified,
            ],
        )
        .await
        .map_err(fixture_failure)?;
    let size = u64::try_from(size).map_err(|_| IntentError::Invalid)?;
    let asset = AssetIdentity::new(asset_id, root, &digest, size)?;
    Ok(GeneratedReservation {
        asset,
        document_id: document,
    })
}

/// Marks a RESERVED asset ACTIVE and materialized, as the materialization step does.
async fn activate(
    tx: &mut PgTransaction,
    asset_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), IntentError> {
    tx.client()
        .execute(
            "UPDATE jobniac.generated_assets SET lifecycle = 'ACTIVE', materialized_at = $2 \
             WHERE asset_id = $1",
            &[&asset_id, &now],
        )
        .await
        .map_err(fixture_failure)?;
    Ok(())
}

/// Creates one ACTIVE, unclassified asset in its own committed transaction.
async fn active_asset(
    uow: &PgUnitOfWork,
    root: Uuid,
    now: OffsetDateTime,
    label: &str,
) -> Result<GeneratedReservation, IntentError> {
    let content = pdf(label);
    uow.transaction(options(), async |tx| {
        let reservation = insert_asset(tx, root, now, &content, false).await?;
        activate(tx, reservation.asset.asset_id(), now).await?;
        Ok(reservation)
    })
    .await
}

/// Creates one ACTIVE classified asset with its pending intent, committed in one transaction
///.
async fn classified_claim(
    fixture: &Fixture,
    descriptor: &GenerationDescriptor,
    now: OffsetDateTime,
) -> Result<IntentReservation, IntentError> {
    let content = pdf("classified");
    fixture
        .uow
        .transaction(options(), async |tx| {
            let reservation = insert_asset(tx, fixture.root, now, &content, true).await?;
            let claim =
                insert_generation_intent(tx, &reservation, descriptor, Uuid::new_v4(), now).await?;
            activate(tx, reservation.asset.asset_id(), now + Duration::seconds(1)).await?;
            Ok(claim)
        })
        .await
}

/// The descriptor of a profile resume claim, as the replacement computes it.
fn profile_descriptor(
    operation: Uuid,
    profile: Uuid,
    resume_type: &str,
) -> Result<GenerationDescriptor, IntentError> {
    GenerationDescriptor::new(
        operation,
        GenerationPurpose::ProfileResume,
        Some(profile),
        None,
        None,
        Some(resume_type.to_owned()),
    )
    .map_err(IntentError::from)
}

/// A synthetic current link to `binding`, with the given timestamps.
fn link(
    profile: Uuid,
    binding: &GeneratedReservation,
    resume_type: &str,
    created: OffsetDateTime,
    updated: OffsetDateTime,
) -> Result<ProfileResumeDocument, Box<dyn Error>> {
    Ok(ProfileResumeDocument::new(
        Uuid::new_v4(),
        profile,
        binding.document_id,
        binding.asset.relative_path(),
        resume_type,
        created,
        updated,
    )?)
}

/// Saves the first (master) link of `profile` to `binding` at owner revision zero.
async fn save_first(
    links: &PgProfileResumeLinks,
    profile: Uuid,
    binding: &GeneratedReservation,
    base: OffsetDateTime,
) -> Result<ProfileResumeDocument, Box<dyn Error>> {
    let candidate = link(
        profile,
        binding,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(2),
    )?;
    Ok(links
        .replace(&candidate, binding, 0, base, None)
        .await?
        .saved)
}

/// Every table of [`STATE_TABLES`], read through an admin session.
async fn state(database: &TestDatabase) -> Result<State, Box<dyn Error>> {
    let admin = database.admin_client().await;
    let mut result = State::new();
    for table in STATE_TABLES {
        let rows = admin
            .query(
                &format!("SELECT t::text AS row FROM {table} t ORDER BY 1"),
                &[],
            )
            .await?;
        let mut texts = Vec::new();
        for row in &rows {
            texts.push(row.try_get::<_, String>("row")?);
        }
        result.insert(table, texts);
    }
    Ok(result)
}

/// The lifecycle and the cleanup task status of one generated asset, read through an admin session.
async fn asset_state(
    database: &TestDatabase,
    asset: Uuid,
) -> Result<(String, Option<String>), Box<dyn Error>> {
    let admin = database.admin_client().await;
    let row = admin
        .query_one(
            "SELECT asset.lifecycle, task.status FROM jobniac.generated_assets asset \
             LEFT JOIN jobniac.generated_asset_cleanups task ON task.asset_id = asset.asset_id \
             WHERE asset.asset_id = $1",
            &[&asset],
        )
        .await?;
    Ok((row.try_get("lifecycle")?, row.try_get("status")?))
}

/// The lifecycle of one generated asset.
async fn lifecycle(uow: &PgUnitOfWork, asset_id: Uuid) -> Result<String, IntentError> {
    uow.transaction(options(), async |tx| {
        let row = tx
            .client()
            .query_one(
                "SELECT lifecycle FROM jobniac.generated_assets WHERE asset_id = $1",
                &[&asset_id],
            )
            .await
            .map_err(fixture_failure)?;
        row.try_get::<_, String>("lifecycle")
            .map_err(fixture_failure)
    })
    .await
}

/// The number of retirement tasks of one generated asset.
async fn task_count(uow: &PgUnitOfWork, asset_id: Uuid) -> Result<i64, IntentError> {
    uow.transaction(options(), async |tx| {
        let row = tx
            .client()
            .query_one(
                "SELECT count(*) AS total FROM jobniac.generated_asset_cleanups \
                 WHERE asset_id = $1",
                &[&asset_id],
            )
            .await
            .map_err(fixture_failure)?;
        row.try_get::<_, i64>("total").map_err(fixture_failure)
    })
    .await
}

/// The state of the intent of one asset.
async fn intent_state(uow: &PgUnitOfWork, asset: Uuid) -> Result<GenerationState, Box<dyn Error>> {
    let intent = uow
        .transaction(options(), async |tx| {
            find_generation_intent(tx, asset, false).await
        })
        .await?
        .ok_or("the intent must exist")?;
    Ok(intent.state)
}

/// The stored resume link of one (profile, type) pair.
async fn current(
    links: &PgProfileResumeLinks,
    profile: Uuid,
    resume_type: &str,
) -> Result<Option<ProfileResumeDocument>, Box<dyn Error>> {
    Ok(links
        .find_by_profile_id_and_resume_type(profile, resume_type)
        .await?)
}

/// The error code of a refused replacement, or a test failure when it was accepted.
fn refusal_code(
    result: Result<impl std::fmt::Debug, AppError>,
) -> Result<ErrorCode, Box<dyn Error>> {
    match result {
        Err(AppError::Application(error)) => Ok(error.code()),
        Err(other) => Err(format!("unexpected refusal: {other:?}").into()),
        Ok(value) => Err(format!("the replacement was accepted: {value:?}").into()),
    }
}

/// The code of any refused call, through its envelope, or a test failure when it was accepted.
fn code_of(result: Result<impl std::fmt::Debug, AppError>) -> Result<ErrorCode, Box<dyn Error>> {
    match result {
        Err(error) => Ok(error_envelope(&error).code),
        Ok(value) => Err(format!("the call was accepted: {value:?}").into()),
    }
}

/// Asserts the fixed unavailable refusal, and that nothing of the driver error is carried.
fn assert_unavailable<T: std::fmt::Debug>(
    result: Result<T, AppError>,
) -> Result<(), Box<dyn Error>> {
    match result {
        Err(error) => {
            let debug = format!("{error:?}");
            assert!(!debug.contains("private"), "driver text leaked: {debug}");
            let envelope = error_envelope(&error);
            assert_eq!(envelope.code, ErrorCode::InternalError);
            assert_eq!(envelope.message, "Local database is unavailable");
            Ok(())
        }
        Ok(value) => Err(format!("the call was accepted: {value:?}").into()),
    }
}

/// Attempts `pg_try_advisory_xact_lock` on a key in the transaction; the lock is kept until the
/// transaction ends.
async fn try_asset_lock(tx: &mut PgTransaction, asset: Uuid) -> Result<bool, IntentError> {
    let row = tx
        .client()
        .query_one(
            "SELECT pg_try_advisory_xact_lock(jobniac.cleanup_asset_lock_key($1)) AS locked",
            &[&asset],
        )
        .await
        .map_err(fixture_failure)?;
    row.try_get::<_, bool>("locked").map_err(fixture_failure)
}

/// Attempts the owned-document lock, keyed by the Rust text of the document (the key text is what
/// `hashtextextended` hashes).
async fn try_document_lock(tx: &mut PgTransaction, key: &str) -> Result<bool, IntentError> {
    let row = tx
        .client()
        .query_one(
            "SELECT pg_try_advisory_xact_lock(hashtextextended($1::text, 0)) AS locked",
            &[&key],
        )
        .await
        .map_err(fixture_failure)?;
    row.try_get::<_, bool>("locked").map_err(fixture_failure)
}

/// How long the lock waits below poll before they fail. Generous, because a loaded machine can
/// delay a session by seconds; a wait that is met returns on the first poll that sees it.
const LOCK_WAIT_BOUND: std::time::Duration = std::time::Duration::from_secs(30);

/// Waits until a session named `application_name` waits for a lock.
async fn wait_for_lock(database: &TestDatabase, application_name: &str) -> TestResult {
    let observer = database.admin_client().await;
    let deadline = std::time::Instant::now() + LOCK_WAIT_BOUND;
    while std::time::Instant::now() < deadline {
        let row = observer
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE application_name = $1 \
                 AND datname = current_database() AND wait_event_type = 'Lock') AS waiting",
                &[&application_name],
            )
            .await?;
        if row.try_get::<_, bool>("waiting")? {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    Err(format!("{application_name} did not reach a held lock").into())
}

/// Waits until a session named `waiter` waits for a lock held by a session named `blocker`.
async fn wait_for_blocker(database: &TestDatabase, waiter: &str, blocker: &str) -> TestResult {
    let observer = database.admin_client().await;
    let deadline = std::time::Instant::now() + LOCK_WAIT_BOUND;
    while std::time::Instant::now() < deadline {
        let row = observer
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity waiting \
                 JOIN pg_stat_activity blocking ON blocking.pid = ANY(pg_blocking_pids(waiting.pid)) \
                 WHERE waiting.application_name = $1 AND blocking.application_name = $2 \
                 AND waiting.datname = current_database() \
                 AND waiting.wait_event_type = 'Lock') AS blocked",
                &[&waiter, &blocker],
            )
            .await?;
        if row.try_get::<_, bool>("blocked")? {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    Err(format!("{waiter} did not wait for {blocker}").into())
}

/// Installs a trigger that takes the advisory `key` for each row that matches, and holds `key` in
/// a session until [`release`].
async fn gate(
    database: &TestDatabase,
    name: &str,
    trigger: &str,
    key: i64,
) -> Result<Client, Box<dyn Error>> {
    // PostgreSQL places `FOR EACH ROW` before a `WHEN` clause, so the clause is split off the text.
    let (head, when) = match trigger.split_once(" WHEN ") {
        Some((head, condition)) => (head, format!(" WHEN {condition}")),
        None => (trigger, String::new()),
    };
    let admin = database.admin_client().await;
    admin
        .batch_execute(&format!(
            "CREATE FUNCTION jobniac.{name}() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN NEW; END $$; \
             CREATE TRIGGER {name} {head} FOR EACH ROW{when} \
             EXECUTE FUNCTION jobniac.{name}();"
        ))
        .await?;
    admin
        .execute("SELECT pg_advisory_lock($1)", &[&key])
        .await?;
    Ok(admin)
}

/// Releases a gate taken by [`gate`].
async fn release(holder: &Client, key: i64) -> TestResult {
    holder
        .execute("SELECT pg_advisory_unlock($1)", &[&key])
        .await?;
    Ok(())
}

/// Installs a trigger that raises on `event` of `table` when `condition` holds.
async fn fail_at(database: &TestDatabase, table: &str, event: &str, condition: &str) -> TestResult {
    let admin = database.admin_client().await;
    admin
        .batch_execute(&format!(
            "CREATE FUNCTION jobniac.synthetic_link_failure() RETURNS trigger \
             LANGUAGE plpgsql AS $$ BEGIN \
                 IF {condition} THEN RAISE EXCEPTION 'private synthetic failure'; END IF; \
                 IF TG_OP = 'DELETE' THEN RETURN OLD; END IF; \
                 RETURN NEW; \
             END $$; \
             CREATE TRIGGER synthetic_link_failure BEFORE {event} ON {table} \
             FOR EACH ROW EXECUTE FUNCTION jobniac.synthetic_link_failure();"
        ))
        .await?;
    Ok(())
}

/// Synthetic owners of reference rows: another profile with its own resume and cover letter on the
/// fixture's job. Returns the profile, resume and letter identifiers.
async fn reference_owners(
    database: &TestDatabase,
    job: Uuid,
    at: OffsetDateTime,
) -> Result<(Uuid, Uuid, Uuid), Box<dyn Error>> {
    let admin = database.admin_client().await;
    let (profile, resume, letter) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    admin
        .execute(
            "INSERT INTO profile.profiles (id, full_name, email, created_at, updated_at) \
             VALUES ($1, 'Reference Person', $2, $3, $3)",
            &[&profile, &format!("{profile}@example.test"), &at],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO resume.resumes (id, profile_id, job_id, format, profile_revision, \
             job_revision, created_at, updated_at) VALUES ($1, $2, $3, 'germany', $4, $4, $4, $4)",
            &[&resume, &profile, &job, &at],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO cover_letter.cover_letters (id, profile_id, job_id, resume_id, \
             profile_revision, job_revision, resume_revision, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $5, $5, $5, $5)",
            &[&letter, &profile, &job, &resume, &at],
        )
        .await?;
    Ok((profile, resume, letter))
}

/// Attaches the document and path of `binding` to a reference row of one of `owners`, as a
/// reference class of the previous asset.
async fn attach(
    database: &TestDatabase,
    kind: &str,
    document: Uuid,
    path: &str,
    owners: (Uuid, Uuid, Uuid),
    at: OffsetDateTime,
) -> TestResult {
    let admin = database.admin_client().await;
    let (profile, resume, letter) = owners;
    let identifier = Uuid::new_v4();
    match kind {
        "resume_variant" => {
            admin
                .execute(
                    "INSERT INTO resume.resume_variants (id, resume_id, language, document_id, \
                     file_path, created_at, updated_at) VALUES ($1, $2, 'en', $3, $4, $5, $5)",
                    &[&identifier, &resume, &document, &path, &at],
                )
                .await?;
        }
        "letter_variant" => {
            admin
                .execute(
                    "INSERT INTO cover_letter.cover_letter_variants (id, cover_letter_id, format, \
                     language, document_id, file_path, subject, salutation, closing, signature, \
                     created_at, updated_at) VALUES ($1, $2, 'germany', 'de', $3, $4, 'Subject', \
                     'Hello', 'Regards', 'Synthetic', $5, $5)",
                    &[&identifier, &letter, &document, &path, &at],
                )
                .await?;
        }
        "photo" => {
            admin
                .execute(
                    "INSERT INTO profile.profile_personal_details (profile_id, photo_document_id, \
                     created_at, updated_at) VALUES ($1, $2, $3, $3)",
                    &[&profile, &document, &at],
                )
                .await?;
        }
        "pdf_source" => {
            let extraction = Uuid::new_v4();
            admin
                .execute(
                    "INSERT INTO document.pdf_extractions (id, file_id, extractor, \
                     character_count, page_count, truncated, extracted_text, created_at) \
                     VALUES ($1, $2, 'synthetic', 1, 1, false, 'x', $3)",
                    &[&extraction, &document, &at],
                )
                .await?;
            admin
                .execute(
                    "INSERT INTO profile.profile_pdf_sources (id, profile_id, pdf_extraction_id, \
                     source_type, created_at) VALUES ($1, $2, $3, 'resume_pdf', $4)",
                    &[&identifier, &profile, &extraction, &at],
                )
                .await?;
        }
        other => return Err(format!("unknown reference class {other}").into()),
    }
    Ok(())
}

/// Inserts a link of `profile` that shares the document and path of `binding` through a raw
/// statement, and returns the database error it must raise (the sharing test).
async fn insert_raw_link(
    database: &TestDatabase,
    profile: Uuid,
    binding: &GeneratedReservation,
    at: OffsetDateTime,
) -> Result<tokio_postgres::Error, Box<dyn Error>> {
    let admin = database.admin_client().await;
    match admin
        .execute(
            "INSERT INTO profile.profile_resume_documents (id, profile_id, document_id, file_path, \
             resume_type, created_at, updated_at) VALUES ($1, $2, $3, $4, 'master_resume', $5, $5)",
            &[
                &Uuid::new_v4(),
                &profile,
                &binding.document_id,
                &binding.asset.relative_path(),
                &at,
            ],
        )
        .await
    {
        Err(error) => Ok(error),
        Ok(_) => Err("the shared document and path were accepted".into()),
    }
}

/// A removal port that reports every file as already absent.
struct AlreadyAbsent;

impl OwnedAssetRemover for AlreadyAbsent {
    fn remove_owned(&self, _: &AssetIdentity) -> Result<RemovalOutcome, RemovalFailure> {
        Ok(RemovalOutcome::AlreadyAbsent)
    }
}

/// A claim of a profile resume link, built in memory from valid parts. The proof fields are public,
/// so a test can change any of them after this point.
fn proof(
    profile: Uuid,
    binding: &GeneratedReservation,
) -> Result<IntentReservation, Box<dyn Error>> {
    let intent = GenerationIntent {
        asset_id: binding.asset.asset_id(),
        descriptor: profile_descriptor(Uuid::new_v4(), profile, MASTER)?,
        state: GenerationState::Pending,
        lease_token: Some(Uuid::new_v4()),
        lease_started_at: Some(OffsetDateTime::UNIX_EPOCH),
        lease_expires_at: Some(OffsetDateTime::UNIX_EPOCH + GENERATION_LEASE),
        created_at: OffsetDateTime::UNIX_EPOCH,
        finished_at: None,
    };
    Ok(IntentReservation::new(binding.clone(), intent)?)
}

/// A change that makes one field of a proof defective.
type ProofDefect = fn(&mut IntentReservation);

/// A closed local port: bound, then released, so that a connection to it is refused.
fn closed_port() -> Result<u16, Box<dyn Error>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

#[tokio::test]
async fn create_without_previous_link_inserts_and_retires_nothing() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let Fixture {
        uow,
        profile,
        root,
        base,
        ..
    } = fixture;
    let first = active_asset(&uow, root, base, "first").await?;
    let links = PgProfileResumeLinks::new(uow.clone(), Arc::new(FixedClock::new(base)));
    let candidate = link(
        profile,
        &first,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(2),
    )?;
    let result = links.replace(&candidate, &first, 0, base, None).await?;
    assert!(result.previous.is_none());
    assert_eq!(result.saved, candidate);
    assert_eq!(current(&links, profile, MASTER).await?, Some(candidate));
    assert_eq!(lifecycle(&uow, first.asset.asset_id()).await?, "ACTIVE");
    assert_eq!(task_count(&uow, first.asset.asset_id()).await?, 0);
    Ok(())
}

#[tokio::test]
async fn owner_cas_refuses_stale_revision_and_timestamp_then_replace_retires_previous() -> TestResult
{
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let Fixture {
        uow,
        profile,
        root,
        base,
        ..
    } = fixture;
    let now = base + Duration::seconds(10);
    let old = active_asset(&uow, root, now, "old").await?;
    let new = active_asset(&uow, root, now, "new").await?;
    let links = PgProfileResumeLinks::new(uow.clone(), Arc::new(FixedClock::new(now)));
    let previous = link(
        profile,
        &old,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(2),
    )?;
    links.replace(&previous, &old, 0, base, None).await?;
    let candidate = link(
        profile,
        &new,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(3),
    )?;

    // A stale revision and a stale owner timestamp are both refused before any mutation.
    let stale_revision = links.replace(&candidate, &new, 1, base, None).await;
    assert_eq!(refusal_code(stale_revision)?, ErrorCode::Conflict);
    let stale_timestamp = links
        .replace(&candidate, &new, 0, base + Duration::seconds(1), None)
        .await;
    assert_eq!(refusal_code(stale_timestamp)?, ErrorCode::Conflict);
    assert_eq!(
        current(&links, profile, MASTER).await?,
        Some(previous.clone())
    );
    assert_eq!(lifecycle(&uow, old.asset.asset_id()).await?, "ACTIVE");
    assert_eq!(lifecycle(&uow, new.asset.asset_id()).await?, "ACTIVE");

    // The matching owner CAS replaces the link and retires the old asset in the same commit.
    let result = links.replace(&candidate, &new, 0, base, None).await?;
    assert_eq!(result.previous, Some(previous.clone()));
    assert_eq!(result.saved.id, previous.id);
    assert_eq!(result.saved.created_at, previous.created_at);
    assert_eq!(result.saved.document_id, new.document_id);
    assert_eq!(result.saved.file_path, new.asset.relative_path());
    assert_eq!(current(&links, profile, MASTER).await?, Some(result.saved));
    assert_eq!(lifecycle(&uow, old.asset.asset_id()).await?, "RETIRING");
    assert_eq!(task_count(&uow, old.asset.asset_id()).await?, 1);
    assert_eq!(lifecycle(&uow, new.asset.asset_id()).await?, "ACTIVE");
    Ok(())
}

#[tokio::test]
async fn older_candidate_timestamp_conflicts_before_any_mutation() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let Fixture {
        uow,
        profile,
        root,
        base,
        ..
    } = fixture;
    let old = active_asset(&uow, root, base, "old").await?;
    let new = active_asset(&uow, root, base, "new").await?;
    let links = PgProfileResumeLinks::new(uow.clone(), Arc::new(FixedClock::new(base)));
    let previous = link(
        profile,
        &old,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(5),
    )?;
    links.replace(&previous, &old, 0, base, None).await?;
    let older = link(
        profile,
        &new,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(2),
    )?;
    let refused = links.replace(&older, &new, 0, base, None).await;
    assert_eq!(refusal_code(refused)?, ErrorCode::Conflict);
    assert_eq!(current(&links, profile, MASTER).await?, Some(previous));
    assert_eq!(lifecycle(&uow, old.asset.asset_id()).await?, "ACTIVE");
    assert_eq!(lifecycle(&uow, new.asset.asset_id()).await?, "ACTIVE");
    assert_eq!(task_count(&uow, old.asset.asset_id()).await?, 0);
    Ok(())
}

#[tokio::test]
async fn unactivated_or_mismatched_binding_is_refused_and_keeps_the_previous_link() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let Fixture {
        uow,
        profile,
        root,
        base,
        ..
    } = fixture;
    let old = active_asset(&uow, root, base, "old").await?;
    let links = PgProfileResumeLinks::new(uow.clone(), Arc::new(FixedClock::new(base)));
    let previous = link(
        profile,
        &old,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(2),
    )?;
    links.replace(&previous, &old, 0, base, None).await?;

    // A binding whose asset is still RESERVED (its activation never committed) is refused.
    let reserved_content = pdf("reserved");
    let reserved = uow
        .transaction(options(), async |tx| {
            insert_asset(tx, root, base, &reserved_content, false).await
        })
        .await?;
    let candidate = link(
        profile,
        &reserved,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(3),
    )?;
    let refused = links.replace(&candidate, &reserved, 0, base, None).await;
    assert_eq!(refusal_code(refused)?, ErrorCode::ValidationError);

    // A link that names a different document than its binding is refused before the database.
    let other = active_asset(&uow, root, base, "other").await?;
    let mismatched = link(
        profile,
        &other,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(3),
    )?;
    let refused = links.replace(&mismatched, &reserved, 0, base, None).await;
    assert_eq!(refusal_code(refused)?, ErrorCode::ValidationError);

    assert_eq!(current(&links, profile, MASTER).await?, Some(previous));
    assert_eq!(lifecycle(&uow, old.asset.asset_id()).await?, "ACTIVE");
    assert_eq!(
        lifecycle(&uow, reserved.asset.asset_id()).await?,
        "RESERVED"
    );
    assert_eq!(task_count(&uow, old.asset.asset_id()).await?, 0);
    Ok(())
}

#[tokio::test]
async fn classified_replacement_requires_the_live_fence_and_completes_the_intent() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let Fixture {
        uow,
        profile,
        root,
        base,
        ..
    } = fixture;
    let operation = Uuid::new_v4();
    let stale_token = Uuid::new_v4();
    let renewed_token = Uuid::new_v4();
    let content = pdf("classified");
    let claim = uow
        .transaction(options(), async |tx| {
            let reservation = insert_asset(tx, root, base, &content, true).await?;
            let descriptor = profile_descriptor(operation, profile, CANADIAN)?;
            // The birth intent starts its lease at the asset's creation instant.
            let claim =
                insert_generation_intent(tx, &reservation, &descriptor, stale_token, base).await?;
            activate(
                tx,
                reservation.asset.asset_id(),
                base + Duration::seconds(1),
            )
            .await?;
            Ok::<IntentReservation, IntentError>(claim)
        })
        .await?;
    let renewed = uow
        .transaction(options(), async |tx| {
            renew_generation_intent(tx, &claim, renewed_token, base + Duration::seconds(2)).await
        })
        .await?;
    let links = PgProfileResumeLinks::new(
        uow.clone(),
        Arc::new(FixedClock::new(base + Duration::seconds(3))),
    );
    let candidate = link(
        profile,
        &claim.reservation,
        CANADIAN,
        base + Duration::seconds(1),
        base + Duration::seconds(3),
    )?;

    // The superseded fence cannot admit the replacement, and nothing is written.
    let stale = links
        .replace(&candidate, &claim.reservation, 0, base, Some(&claim))
        .await;
    assert_eq!(refusal_code(stale)?, ErrorCode::ValidationError);
    assert_eq!(current(&links, profile, CANADIAN).await?, None);
    let pending = uow
        .transaction(options(), async |tx| {
            find_generation_intent(tx, claim.asset_id(), false).await
        })
        .await?
        .ok_or("the birth intent must exist")?;
    assert_eq!(pending.state, GenerationState::Pending);

    // The renewed claim, which holds the current fence, completes the replacement.
    let result = links
        .replace(&candidate, &claim.reservation, 0, base, Some(&renewed))
        .await?;
    assert!(result.previous.is_none());
    assert_eq!(current(&links, profile, CANADIAN).await?, Some(candidate));
    let completed = uow
        .transaction(options(), async |tx| {
            find_generation_intent(tx, claim.asset_id(), false).await
        })
        .await?
        .ok_or("the completed intent must remain as history")?;
    assert_eq!(completed.state, GenerationState::Completed);
    Ok(())
}

#[tokio::test]
async fn concurrent_replaces_of_one_profile_serialize_to_one_current_link() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let Fixture {
        uow,
        profile,
        root,
        base,
        ..
    } = fixture;
    let now = base + Duration::seconds(10);
    let first = active_asset(&uow, root, now, "first").await?;
    let second = active_asset(&uow, root, now, "second").await?;
    let links = PgProfileResumeLinks::new(uow.clone(), Arc::new(FixedClock::new(now)));
    let one = link(
        profile,
        &first,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(2),
    )?;
    let two = link(
        profile,
        &second,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(2),
    )?;
    // Both writers hold the same owner revision. The profile row lock serializes them: the second
    // one reads the first committed link as its previous link, and the timestamps are equal, so
    // the second replacement is admitted and retires the first asset. Exactly one current link
    // remains, pointing at the second asset.
    let (a, b) = tokio::join!(
        links.replace(&one, &first, 0, base, None),
        links.replace(&two, &second, 0, base, None),
    );
    let accepted = [a.is_ok(), b.is_ok()];
    assert_eq!(accepted, [true, true]);
    let stored = current(&links, profile, MASTER)
        .await?
        .ok_or("one current link must remain")?;
    let (survivor, retired) = if stored.document_id == second.document_id {
        (second.asset.asset_id(), first.asset.asset_id())
    } else {
        (first.asset.asset_id(), second.asset.asset_id())
    };
    assert_eq!(lifecycle(&uow, survivor).await?, "ACTIVE");
    assert_eq!(lifecycle(&uow, retired).await?, "RETIRING");
    assert_eq!(task_count(&uow, retired).await?, 1);
    assert_eq!(task_count(&uow, survivor).await?, 0);
    Ok(())
}

#[tokio::test]
async fn reserved_leftover_and_dropped_replacement_leave_the_old_link_and_asset_untouched()
-> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let Fixture {
        uow,
        profile,
        root,
        base,
        ..
    } = fixture;
    let old = active_asset(&uow, root, base, "old").await?;
    let links = PgProfileResumeLinks::new(uow.clone(), Arc::new(FixedClock::new(base)));
    let previous = link(
        profile,
        &old,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(2),
    )?;
    links.replace(&previous, &old, 0, base, None).await?;

    // A replacement whose transaction is dropped after it retired the old asset and rewrote the
    // link rolls back completely: the old link and the ACTIVE asset are restored.
    let dropped_content = pdf("dropped");
    let dropped_profile = profile;
    let abandoned = uow
        .transaction(options(), async |tx| {
            let reservation = insert_asset(tx, root, base, &dropped_content, false).await?;
            activate(tx, reservation.asset.asset_id(), base).await?;
            tx.client()
                .execute(
                    "UPDATE profile.profile_resume_documents SET document_id = $1, \
                     file_path = $2, updated_at = $3 WHERE profile_id = $4 AND resume_type = $5",
                    &[
                        &reservation.document_id,
                        &reservation.asset.relative_path(),
                        &(base + Duration::seconds(3)),
                        &dropped_profile,
                        &MASTER,
                    ],
                )
                .await
                .map_err(fixture_failure)?;
            let row = tx
                .client()
                .query_one(
                    &format!(
                        "SELECT {ASSET_COLUMNS} FROM jobniac.generated_assets \
                         WHERE asset_id = $1 FOR UPDATE"
                    ),
                    &[&old.asset.asset_id()],
                )
                .await
                .map_err(fixture_failure)?;
            let row = AssetRow::from_row(&row)?;
            retire_asset(tx, &row, base + Duration::seconds(3), false, None).await?;
            Err::<(), IntentError>(IntentError::Unavailable)
        })
        .await;
    assert!(matches!(abandoned, Err(IntentError::Unavailable)));
    assert_eq!(
        current(&links, profile, MASTER).await?,
        Some(previous.clone())
    );
    assert_eq!(lifecycle(&uow, old.asset.asset_id()).await?, "ACTIVE");
    assert_eq!(task_count(&uow, old.asset.asset_id()).await?, 0);

    // A RESERVED asset left by a committed reserve whose activation never ran is not a binding.
    let leftover = uow
        .transaction(options(), async |tx| {
            insert_asset(tx, root, base, &pdf("leftover"), false).await
        })
        .await?;
    let candidate = link(
        profile,
        &leftover,
        MASTER,
        base + Duration::seconds(1),
        base + Duration::seconds(4),
    )?;
    let refused = links.replace(&candidate, &leftover, 0, base, None).await;
    assert_eq!(refusal_code(refused)?, ErrorCode::ValidationError);
    assert_eq!(current(&links, profile, MASTER).await?, Some(previous));
    assert_eq!(lifecycle(&uow, old.asset.asset_id()).await?, "ACTIVE");
    assert_eq!(
        lifecycle(&uow, leftover.asset.asset_id()).await?,
        "RESERVED"
    );
    Ok(())
}

#[tokio::test]
async fn advisory_lock_keys_match_the_derivation_and_contend_across_sessions() -> TestResult {
    let Some(fixture) = start().await? else {
        return Ok(());
    };
    let uow = fixture.uow;
    let asset = Uuid::new_v4();
    let document = Uuid::new_v4();
    let asset_key = asset_lock_key_input(asset);
    let document_key = document_lock_key_input(document);

    // PostgreSQL hashes the Rust text to the same key as the packaged SQL function.
    let same = uow
        .transaction(options(), async |tx| {
            let row = tx
                .client()
                .query_one(
                    "SELECT hashtextextended($1::text, 0) = jobniac.cleanup_asset_lock_key($2) \
                     AS same",
                    &[&asset_key, &asset],
                )
                .await
                .map_err(fixture_failure)?;
            row.try_get::<_, bool>("same").map_err(fixture_failure)
        })
        .await?;
    assert!(same);

    // Session one holds both keys. Session two, on its own connection, cannot take either.
    uow.transaction(options(), async |holder| {
        assert!(try_asset_lock(holder, asset).await?);
        assert!(try_document_lock(holder, &document_key).await?);
        let contender = uow
            .transaction(options(), async |other| {
                let asset_busy = !try_asset_lock(other, asset).await?;
                let document_busy = !try_document_lock(other, &document_key).await?;
                Ok::<(bool, bool), IntentError>((asset_busy, document_busy))
            })
            .await?;
        assert_eq!(contender, (true, true));
        Ok::<(), IntentError>(())
    })
    .await?;

    // After the holder commits, the keys are free again.
    let freed = uow
        .transaction(options(), async |tx| {
            Ok::<bool, IntentError>(
                try_asset_lock(tx, asset).await? && try_document_lock(tx, &document_key).await?,
            )
        })
        .await?;
    assert!(freed);
    Ok(())
}

#[tokio::test]
async fn coexisting_types_keep_their_links_and_only_the_replaced_asset_retires() -> TestResult {
    let Some(f) = start().await? else {
        return Ok(());
    };
    let writer = links(&f, fixed(f.base));
    let mut saved = Vec::new();
    for kind in [MASTER, CANADIAN, CANADIAN_FR] {
        let binding = active_asset(&f.uow, f.root, f.base, kind).await?;
        let candidate = link(
            f.profile,
            &binding,
            kind,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(2),
        )?;
        let result = writer
            .replace(&candidate, &binding, 0, f.base, None)
            .await?;
        assert_eq!(result.saved, candidate);
        assert!(result.previous.is_none());
        saved.push((kind, candidate, binding));
    }
    let (_, old_link, old) = &saved[0];
    let new = active_asset(&f.uow, f.root, f.base, "new").await?;
    let replacement = link(
        f.profile,
        &new,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(3),
    )?;
    let result = writer.replace(&replacement, &new, 0, f.base, None).await?;
    assert_eq!(result.previous.as_ref(), Some(old_link));
    assert_eq!(result.saved.id, old_link.id);
    assert_eq!(result.saved.created_at, old_link.created_at);
    assert_eq!(result.saved.file_path, new.asset.relative_path());
    for (kind, candidate, binding) in &saved[1..] {
        assert_eq!(
            current(&writer, f.profile, kind).await?,
            Some(candidate.clone())
        );
        assert_eq!(lifecycle(&f.uow, binding.asset.asset_id()).await?, "ACTIVE");
        assert_eq!(task_count(&f.uow, binding.asset.asset_id()).await?, 0);
    }
    assert_eq!(lifecycle(&f.uow, old.asset.asset_id()).await?, "RETIRING");
    assert_eq!(task_count(&f.uow, old.asset.asset_id()).await?, 1);
    assert_eq!(lifecycle(&f.uow, new.asset.asset_id()).await?, "ACTIVE");
    assert_eq!(task_count(&f.uow, new.asset.asset_id()).await?, 0);
    Ok(())
}

#[tokio::test]
async fn every_reference_class_keeps_the_previous_asset_active() -> TestResult {
    // The reference classes are a resume variant, a cover-letter variant, the profile photo and a
    // PDF source; the last case is the link itself (the same binding is chosen again).
    for kind in [
        "resume_variant",
        "letter_variant",
        "photo",
        "pdf_source",
        "reuse",
    ] {
        let Some(f) = start().await? else {
            return Ok(());
        };
        let links = links(&f, fixed(f.base));
        let old = active_asset(&f.uow, f.root, f.base, "old").await?;
        let new = active_asset(&f.uow, f.root, f.base, "new").await?;
        save_first(&links, f.profile, &old, f.base).await?;
        let chosen = if kind == "reuse" {
            old.clone()
        } else {
            let owners = reference_owners(&f.database, f.job, f.base).await?;
            attach(
                &f.database,
                kind,
                old.document_id,
                old.asset.relative_path(),
                owners,
                f.base,
            )
            .await?;
            new.clone()
        };
        let candidate = link(
            f.profile,
            &chosen,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(3),
        )?;
        links.replace(&candidate, &chosen, 0, f.base, None).await?;
        assert_eq!(
            lifecycle(&f.uow, old.asset.asset_id()).await?,
            "ACTIVE",
            "{kind}"
        );
        assert_eq!(task_count(&f.uow, old.asset.asset_id()).await?, 0, "{kind}");
        let outcome = cleanup(&f, fixed(f.base))
            .prepare_cleanup(old.asset.asset_id())
            .await?;
        assert_eq!(outcome, PreparationOutcome::Referenced, "{kind}");
    }
    Ok(())
}

#[tokio::test]
async fn a_document_and_path_cannot_be_shared_with_a_second_profile() -> TestResult {
    let Some(f) = start().await? else {
        return Ok(());
    };
    let links = links(&f, fixed(f.base));
    let binding = active_asset(&f.uow, f.root, f.base, "shared").await?;
    let previous = save_first(&links, f.profile, &binding, f.base).await?;
    let other = reference_owners(&f.database, f.job, f.base).await?.0;
    let before = state(&f.database).await?;
    let error = insert_raw_link(&f.database, other, &binding, f.base).await?;
    let db = error.as_db_error().ok_or("a database error was expected")?;
    assert_eq!(db.code(), &SqlState::UNIQUE_VIOLATION);
    assert!(matches!(
        db.constraint(),
        Some("idx_profile_resume_documents_document_id_unique")
            | Some("idx_profile_resume_documents_file_path_unique")
    ));
    assert_eq!(state(&f.database).await?, before);
    assert_eq!(current(&links, f.profile, MASTER).await?, Some(previous));
    assert_eq!(current(&links, other, MASTER).await?, None);
    assert_eq!(lifecycle(&f.uow, binding.asset.asset_id()).await?, "ACTIVE");
    assert_eq!(task_count(&f.uow, binding.asset.asset_id()).await?, 0);
    let outcome = cleanup(&f, fixed(f.base))
        .prepare_cleanup(binding.asset.asset_id())
        .await?;
    assert_eq!(outcome, PreparationOutcome::Referenced);
    Ok(())
}

#[tokio::test]
async fn source_preconditions_preserve_links_and_assets() -> TestResult {
    // A changed owner revision, a changed owner timestamp and a missing owner.
    for change in ["numeric", "timestamp", "missing"] {
        let Some(f) = start().await? else {
            return Ok(());
        };
        let links = links(&f, fixed(f.base));
        let old = active_asset(&f.uow, f.root, f.base, "old").await?;
        let new = active_asset(&f.uow, f.root, f.base, "new").await?;
        save_first(&links, f.profile, &old, f.base).await?;
        let admin = f.database.admin_client().await;
        let candidate_profile = if change == "missing" {
            Uuid::new_v4()
        } else {
            f.profile
        };
        match change {
            "numeric" => {
                admin
                    .execute(
                        "UPDATE profile.profiles SET revision = 1 WHERE id = $1",
                        &[&f.profile],
                    )
                    .await?;
            }
            "timestamp" => {
                admin
                    .execute(
                        "UPDATE profile.profiles SET updated_at = $2 WHERE id = $1",
                        &[&f.profile, &(f.base + Duration::seconds(1))],
                    )
                    .await?;
            }
            _ => {}
        }
        let candidate = link(
            candidate_profile,
            &new,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(2),
        )?;
        let before = state(&f.database).await?;
        let expected = if change == "missing" {
            ErrorCode::NotFound
        } else {
            ErrorCode::Conflict
        };
        let result = links.replace(&candidate, &new, 0, f.base, None).await;
        assert_eq!(code_of(result)?, expected, "{change}");
        assert_eq!(state(&f.database).await?, before, "{change}");
    }
    Ok(())
}

#[tokio::test]
async fn wrong_binding_cannot_replace_the_previous_link() -> TestResult {
    // Root, digest and size cannot differ here: the asset identity is built only by its
    // constructors. The document and the path are the variants that can.
    for mismatch in ["document", "path"] {
        let Some(f) = start().await? else {
            return Ok(());
        };
        let links = links(&f, fixed(f.base));
        let old = active_asset(&f.uow, f.root, f.base, "old").await?;
        let new = active_asset(&f.uow, f.root, f.base, "new").await?;
        save_first(&links, f.profile, &old, f.base).await?;
        let mut candidate = link(
            f.profile,
            &new,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(2),
        )?;
        let mut binding = new.clone();
        if mismatch == "document" {
            binding.document_id = Uuid::new_v4();
        } else {
            candidate.file_path = "legacy/name.pdf".to_owned();
        }
        let before = state(&f.database).await?;
        let result = links.replace(&candidate, &binding, 0, f.base, None).await;
        assert_eq!(code_of(result)?, ErrorCode::ValidationError, "{mismatch}");
        assert_eq!(state(&f.database).await?, before, "{mismatch}");
    }
    Ok(())
}

#[tokio::test]
async fn unavailable_binding_preserves_the_previous_link() -> TestResult {
    // A binding that is being retired, or whose retirement is complete, cannot become the link.
    for lifecycle_case in ["RETIRING", "DELETED"] {
        let Some(f) = start().await? else {
            return Ok(());
        };
        let links = links(&f, fixed(f.base));
        let old = active_asset(&f.uow, f.root, f.base, "old").await?;
        let new = active_asset(&f.uow, f.root, f.base, "new").await?;
        save_first(&links, f.profile, &old, f.base).await?;
        let retirements = cleanup(&f, fixed(f.base));
        assert_eq!(
            retirements.prepare_cleanup(new.asset.asset_id()).await?,
            PreparationOutcome::Prepared
        );
        if lifecycle_case == "DELETED" {
            let claims = retirements.claim_due(f.root, RETRY_BATCH_SIZE).await?;
            let claim = claims.first().ok_or("the retirement task must be due")?;
            retirements.execute_claim(claim, &AlreadyAbsent).await?;
        }
        let before = state(&f.database).await?;
        let candidate = link(
            f.profile,
            &new,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(3),
        )?;
        let result = links.replace(&candidate, &new, 0, f.base, None).await;
        assert_eq!(
            code_of(result)?,
            ErrorCode::ValidationError,
            "{lifecycle_case}"
        );
        assert_eq!(state(&f.database).await?, before, "{lifecycle_case}");
    }
    Ok(())
}

#[tokio::test]
async fn link_and_retirement_failure_roll_back_every_row() -> TestResult {
    // The link write, the asset retirement and the task insert each fail in turn.
    let cases = [
        ("profile.profile_resume_documents", "UPDATE", "TRUE"),
        (
            "jobniac.generated_assets",
            "UPDATE",
            "NEW.lifecycle = 'RETIRING'",
        ),
        ("jobniac.generated_asset_cleanups", "INSERT", "TRUE"),
    ];
    for (table, event, condition) in cases {
        let Some(f) = start().await? else {
            return Ok(());
        };
        let links = links(&f, fixed(f.base));
        let old = active_asset(&f.uow, f.root, f.base, "old").await?;
        let new = active_asset(&f.uow, f.root, f.base, "new").await?;
        save_first(&links, f.profile, &old, f.base).await?;
        let before = state(&f.database).await?;
        fail_at(&f.database, table, event, condition).await?;
        let candidate = link(
            f.profile,
            &new,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(3),
        )?;
        let result = links.replace(&candidate, &new, 0, f.base, None).await;
        assert_unavailable(result)?;
        assert_eq!(state(&f.database).await?, before, "{table} {event}");
    }
    Ok(())
}

#[tokio::test]
async fn a_previous_link_with_an_unowned_path_is_replaced_without_touching_its_storage()
-> TestResult {
    let Some(f) = start().await? else {
        return Ok(());
    };
    let owned = active_asset(&f.uow, f.root, f.base, "owned").await?;
    let admin = f.database.admin_client().await;
    let (blob, document, legacy) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let content = b"%PDF-test".to_vec();
    admin
        .execute(
            "INSERT INTO document.blobs (id, sha256, byte_size, content, created_at) \
             VALUES ($1, $2, 9, $3, $4)",
            &[&blob, &"1".repeat(64), &content, &f.base],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO document.documents (id, blob_id, original_file_name, media_type, \
             created_at, updated_at) VALUES ($1, $2, 'legacy.pdf', 'application/pdf', $3, $3)",
            &[&document, &blob, &f.base],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO profile.profile_resume_documents (id, profile_id, document_id, file_path, \
             resume_type, created_at, updated_at) \
             VALUES ($1, $2, $3, 'legacy/person/resume.pdf', 'master_resume', $4, $4)",
            &[&legacy, &f.profile, &document, &f.base],
        )
        .await?;
    let before = state(&f.database).await?;
    let links = links(&f, fixed(f.base));
    let candidate = link(
        f.profile,
        &owned,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(2),
    )?;
    let result = links.replace(&candidate, &owned, 0, f.base, None).await?;
    assert_eq!(result.saved.id, legacy);
    assert_eq!(result.saved.file_path, owned.asset.relative_path());
    let previous = result
        .previous
        .ok_or("the legacy link must be the previous one")?;
    assert_eq!(previous.file_path, "legacy/person/resume.pdf");
    let after = state(&f.database).await?;
    for table in [
        "document.documents",
        "document.blobs",
        "jobniac.generated_assets",
        "jobniac.generated_asset_cleanups",
    ] {
        assert_eq!(after[table], before[table], "{table}");
    }
    Ok(())
}

#[tokio::test]
async fn readers_see_the_old_state_until_the_link_and_retirement_commit() -> TestResult {
    let Some(f) = start().await? else {
        return Ok(());
    };
    let writer = links(&f, fixed(f.base));
    let old = active_asset(&f.uow, f.root, f.base, "old").await?;
    let new = active_asset(&f.uow, f.root, f.base, "new").await?;
    let previous = save_first(&writer, f.profile, &old, f.base).await?;
    let before = state(&f.database).await?;
    let candidate = link(
        f.profile,
        &new,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(2),
    )?;
    let holder = gate(
        &f.database,
        "synthetic_link_gate",
        "BEFORE INSERT OR UPDATE ON profile.profile_resume_documents",
        GATE_LINK,
    )
    .await?;
    let observer = async {
        wait_for_lock(&f.database, LINK_APP).await?;
        assert_eq!(state(&f.database).await?, before);
        assert_eq!(
            current(&writer, f.profile, MASTER).await?,
            Some(previous.clone())
        );
        release(&holder, GATE_LINK).await
    };
    let (written, observed) =
        tokio::join!(writer.replace(&candidate, &new, 0, f.base, None), observer);
    observed?;
    assert_eq!(written?.previous, Some(previous));
    assert_eq!(lifecycle(&f.uow, old.asset.asset_id()).await?, "RETIRING");
    Ok(())
}

#[tokio::test]
async fn classified_profile_link_requires_scoped_live_pending_proof() -> TestResult {
    // A missing proof, an expired lease, a proof for another scope and a superseded token are each
    // refused, and the pending intent is left as it was.
    for failure in ["missing_proof", "expired", "wrong_scope", "wrong_token"] {
        let Some(f) = start().await? else {
            return Ok(());
        };
        let old = active_asset(&f.uow, f.root, f.base, "old").await?;
        save_first(&links(&f, fixed(f.base)), f.profile, &old, f.base).await?;
        let scope = if failure == "wrong_scope" {
            CANADIAN
        } else {
            MASTER
        };
        let claim = classified_claim(
            &f,
            &profile_descriptor(Uuid::new_v4(), f.profile, scope)?,
            f.base,
        )
        .await?;
        let expires = claim
            .intent
            .lease_expires_at
            .ok_or("a pending intent has a lease")?;
        let clock = if failure == "expired" {
            fixed(expires)
        } else {
            fixed(f.base + Duration::seconds(1))
        };
        let writer = links(&f, clock);
        let mut proof_claim = claim.clone();
        if failure == "wrong_token" {
            proof_claim.intent.lease_token = Some(Uuid::new_v4());
        }
        let proof_arg = if failure == "missing_proof" {
            None
        } else {
            Some(&proof_claim)
        };
        let candidate = link(
            f.profile,
            &claim.reservation,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(3),
        )?;
        let before = state(&f.database).await?;
        let result = writer
            .replace(&candidate, &claim.reservation, 0, f.base, proof_arg)
            .await;
        assert_eq!(code_of(result)?, ErrorCode::ValidationError, "{failure}");
        assert_eq!(state(&f.database).await?, before, "{failure}");
        assert_eq!(
            intent_state(&f.uow, claim.asset_id()).await?,
            GenerationState::Pending,
            "{failure}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn classified_completion_failure_rolls_back_the_link_and_the_retirement() -> TestResult {
    // An expiry after the link is written, and a failure of the intent completion, are both
    // rolled back.
    for failure in ["expiry_after_link", "completion_failure"] {
        let Some(f) = start().await? else {
            return Ok(());
        };
        let old = active_asset(&f.uow, f.root, f.base, "old").await?;
        save_first(&links(&f, fixed(f.base)), f.profile, &old, f.base).await?;
        let claim = classified_claim(
            &f,
            &profile_descriptor(Uuid::new_v4(), f.profile, MASTER)?,
            f.base,
        )
        .await?;
        let expires = claim
            .intent
            .lease_expires_at
            .ok_or("a pending intent has a lease")?;
        let clock: Arc<dyn Clock> = if failure == "expiry_after_link" {
            // The replacement reads the clock for admission, retirement and completion.
            Arc::new(TickClock(Mutex::new(vec![
                f.base + Duration::seconds(1),
                f.base + Duration::seconds(1),
                expires,
            ])))
        } else {
            fixed(f.base + Duration::seconds(1))
        };
        if failure == "completion_failure" {
            fail_at(
                &f.database,
                "jobniac.generation_intents",
                "UPDATE",
                "NEW.state = 'COMPLETED'",
            )
            .await?;
        }
        let writer = links(&f, clock);
        let candidate = link(
            f.profile,
            &claim.reservation,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(3),
        )?;
        let before = state(&f.database).await?;
        let result = writer
            .replace(&candidate, &claim.reservation, 0, f.base, Some(&claim))
            .await;
        if failure == "expiry_after_link" {
            assert_eq!(code_of(result)?, ErrorCode::ValidationError);
        } else {
            assert_unavailable(result)?;
        }
        assert_eq!(state(&f.database).await?, before, "{failure}");
        assert_eq!(lifecycle(&f.uow, old.asset.asset_id()).await?, "ACTIVE");
        assert_eq!(lifecycle(&f.uow, claim.asset_id()).await?, "ACTIVE");
        assert_eq!(
            intent_state(&f.uow, claim.asset_id()).await?,
            GenerationState::Pending,
            "{failure}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn profile_edit_and_deletion_wait_for_the_writer_root_lock() -> TestResult {
    // The writer holds the profile row while it waits at the link gate; an edit or a deletion of
    // the profile must wait for it.
    for action in ["edit", "delete"] {
        let Some(f) = start().await? else {
            return Ok(());
        };
        let binding = active_asset(&f.uow, f.root, f.base, "bound").await?;
        let writer = links(&f, fixed(f.base));
        let candidate = link(
            f.profile,
            &binding,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(2),
        )?;
        let change_sql = if action == "edit" {
            "UPDATE profile.profiles SET revision = revision + 1 WHERE id = $1"
        } else {
            "DELETE FROM profile.profiles WHERE id = $1"
        };
        let holder = gate(
            &f.database,
            "synthetic_link_gate",
            "BEFORE INSERT OR UPDATE ON profile.profile_resume_documents",
            GATE_LINK,
        )
        .await?;
        let changer = async {
            wait_for_lock(&f.database, LINK_APP).await?;
            let session = f.database.admin_client().await;
            session
                .batch_execute(
                    "SET application_name = 'synthetic-profile-change'; \
                     SET lock_timeout = '10s'; SET statement_timeout = '15s'",
                )
                .await?;
            session.execute(change_sql, &[&f.profile]).await?;
            Ok::<(), Box<dyn Error>>(())
        };
        let observer = async {
            wait_for_lock(&f.database, LINK_APP).await?;
            wait_for_blocker(&f.database, "synthetic-profile-change", LINK_APP).await?;
            release(&holder, GATE_LINK).await
        };
        let (written, changed, observed) = tokio::join!(
            writer.replace(&candidate, &binding, 0, f.base, None),
            changer,
            observer
        );
        observed?;
        changed?;
        written?;
        let admin = f.database.admin_client().await;
        let remaining = admin
            .query_opt(
                "SELECT revision FROM profile.profiles WHERE id = $1",
                &[&f.profile],
            )
            .await?;
        if action == "edit" {
            let revision: i64 = remaining
                .ok_or("the edited profile must exist")?
                .try_get(0)?;
            assert_eq!(revision, 1);
        } else {
            assert!(remaining.is_none(), "the deleted profile must be gone");
        }
        let later = writer.replace(&candidate, &binding, 0, f.base, None).await;
        let expected = if action == "edit" {
            ErrorCode::Conflict
        } else {
            ErrorCode::NotFound
        };
        assert_eq!(code_of(later)?, expected, "{action}");
    }
    Ok(())
}

#[tokio::test]
async fn waiting_writer_rechecks_the_committed_owner_at_read_committed() -> TestResult {
    // The database default is REPEATABLE READ here. The writer waits on the owner row, and must
    // still reread the owner as committed once the other session commits.
    for action in ["edit", "delete"] {
        let Some(f) = start().await? else {
            return Ok(());
        };
        let binding = active_asset(&f.uow, f.root, f.base, "bound").await?;
        let admin = f.database.admin_client().await;
        admin
            .execute(
                &format!(
                    "ALTER DATABASE {} SET default_transaction_isolation = 'repeatable read'",
                    f.database.database_name()
                ),
                &[],
            )
            .await?;
        // A pool created after the change opens sessions with the new default.
        let writer = links(&f, fixed(f.base));
        let owner = f.database.admin_client().await;
        owner.batch_execute("BEGIN").await?;
        owner
            .query_one(
                "SELECT id FROM profile.profiles WHERE id = $1 FOR UPDATE",
                &[&f.profile],
            )
            .await?;
        let change_sql = if action == "edit" {
            "UPDATE profile.profiles SET revision = 1 WHERE id = $1"
        } else {
            "DELETE FROM profile.profiles WHERE id = $1"
        };
        let candidate = link(
            f.profile,
            &binding,
            MASTER,
            f.base + Duration::seconds(1),
            f.base + Duration::seconds(2),
        )?;
        let observer = async {
            wait_for_lock(&f.database, LINK_APP).await?;
            owner.execute(change_sql, &[&f.profile]).await?;
            owner.batch_execute("COMMIT").await?;
            Ok::<(), Box<dyn Error>>(())
        };
        let (written, observed) = tokio::join!(
            writer.replace(&candidate, &binding, 0, f.base, None),
            observer
        );
        observed?;
        let expected = if action == "edit" {
            ErrorCode::Conflict
        } else {
            ErrorCode::NotFound
        };
        assert_eq!(code_of(written)?, expected, "{action}");
        assert!(state(&f.database).await?["profile.profile_resume_documents"].is_empty());
        assert_eq!(lifecycle(&f.uow, binding.asset.asset_id()).await?, "ACTIVE");
    }
    Ok(())
}

#[tokio::test]
async fn competing_creation_serializes_and_preserves_the_first_identity() -> TestResult {
    let Some(f) = start().await? else {
        return Ok(());
    };
    let first = active_asset(&f.uow, f.root, f.base, "first").await?;
    let second = active_asset(&f.uow, f.root, f.base, "second").await?;
    let writer = links(&f, fixed(f.base));
    let one = link(
        f.profile,
        &first,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(2),
    )?;
    let two = link(
        f.profile,
        &second,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(2),
    )?;
    let holder = gate(
        &f.database,
        "synthetic_link_gate",
        "BEFORE INSERT OR UPDATE ON profile.profile_resume_documents",
        GATE_LINK,
    )
    .await?;
    let first_writer = writer.replace(&one, &first, 0, f.base, None);
    let second_writer = async {
        wait_for_lock(&f.database, LINK_APP).await?;
        Ok::<_, Box<dyn Error>>(writer.replace(&two, &second, 0, f.base, None).await?)
    };
    let observer = async {
        wait_for_lock(&f.database, LINK_APP).await?;
        wait_for_blocker(&f.database, LINK_APP, LINK_APP).await?;
        release(&holder, GATE_LINK).await
    };
    let (created, replaced, observed) = tokio::join!(first_writer, second_writer, observer);
    observed?;
    assert_eq!(created?.saved, one);
    let result = replaced?;
    assert_eq!(result.previous, Some(one.clone()));
    assert_eq!(result.saved.id, one.id);
    assert_eq!(result.saved.document_id, second.document_id);
    assert_eq!(
        state(&f.database).await?["profile.profile_resume_documents"].len(),
        1
    );
    assert_eq!(lifecycle(&f.uow, first.asset.asset_id()).await?, "RETIRING");
    Ok(())
}

#[tokio::test]
async fn cleanup_before_link_wins_and_preserves_the_previous_link() -> TestResult {
    let Some(f) = start().await? else {
        return Ok(());
    };
    let writer = links(&f, fixed(f.base));
    let old = active_asset(&f.uow, f.root, f.base, "old").await?;
    let new = active_asset(&f.uow, f.root, f.base, "new").await?;
    let previous = save_first(&writer, f.profile, &old, f.base).await?;
    let before = state(&f.database).await?;
    let candidate = link(
        f.profile,
        &new,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(2),
    )?;
    let retirements = cleanup(&f, fixed(f.base));
    let holder = gate(
        &f.database,
        "synthetic_cleanup_gate",
        "BEFORE UPDATE OF lifecycle ON jobniac.generated_assets \
         WHEN (NEW.lifecycle = 'RETIRING')",
        GATE_RETIREMENT,
    )
    .await?;
    let preparation = retirements.prepare_cleanup(new.asset.asset_id());
    let late_writer = async {
        wait_for_lock(&f.database, CLEANUP_APP).await?;
        Ok::<_, Box<dyn Error>>(writer.replace(&candidate, &new, 0, f.base, None).await)
    };
    let observer = async {
        wait_for_lock(&f.database, CLEANUP_APP).await?;
        wait_for_blocker(&f.database, LINK_APP, CLEANUP_APP).await?;
        assert_eq!(state(&f.database).await?, before);
        release(&holder, GATE_RETIREMENT).await
    };
    let (prepared, written, observed) = tokio::join!(preparation, late_writer, observer);
    observed?;
    assert_eq!(prepared?, PreparationOutcome::Prepared);
    assert_eq!(code_of(written?)?, ErrorCode::ValidationError);
    assert_eq!(current(&writer, f.profile, MASTER).await?, Some(previous));
    assert_eq!(lifecycle(&f.uow, old.asset.asset_id()).await?, "ACTIVE");
    assert_eq!(lifecycle(&f.uow, new.asset.asset_id()).await?, "RETIRING");
    Ok(())
}

#[tokio::test]
async fn link_before_cleanup_wins() -> TestResult {
    let Some(f) = start().await? else {
        return Ok(());
    };
    let writer = links(&f, fixed(f.base));
    let binding = active_asset(&f.uow, f.root, f.base, "bound").await?;
    let candidate = link(
        f.profile,
        &binding,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(2),
    )?;
    let retirements = cleanup(&f, fixed(f.base));
    let holder = gate(
        &f.database,
        "synthetic_link_gate",
        "BEFORE INSERT OR UPDATE ON profile.profile_resume_documents",
        GATE_LINK,
    )
    .await?;
    let replacement = writer.replace(&candidate, &binding, 0, f.base, None);
    let preparation = async {
        wait_for_lock(&f.database, LINK_APP).await?;
        Ok::<_, Box<dyn Error>>(retirements.prepare_cleanup(binding.asset.asset_id()).await)
    };
    let observer = async {
        wait_for_lock(&f.database, LINK_APP).await?;
        wait_for_blocker(&f.database, CLEANUP_APP, LINK_APP).await?;
        release(&holder, GATE_LINK).await
    };
    let (written, prepared, observed) = tokio::join!(replacement, preparation, observer);
    observed?;
    written?;
    assert_eq!(prepared??, PreparationOutcome::Referenced);
    assert_eq!(lifecycle(&f.uow, binding.asset.asset_id()).await?, "ACTIVE");
    Ok(())
}

#[tokio::test]
async fn renewal_commit_fences_a_link_waiting_for_the_classified_binding() -> TestResult {
    let Some(f) = start().await? else {
        return Ok(());
    };
    let old = active_asset(&f.uow, f.root, f.base, "old").await?;
    let writer = links(&f, fixed(f.base));
    let previous = save_first(&writer, f.profile, &old, f.base).await?;
    let claim = classified_claim(
        &f,
        &profile_descriptor(Uuid::new_v4(), f.profile, MASTER)?,
        f.base,
    )
    .await?;
    let before = state(&f.database).await?;
    let renewed_token = Uuid::new_v4();
    let renewals = generated(&f, fixed(f.base + Duration::seconds(1)));
    let candidate = link(
        f.profile,
        &claim.reservation,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(3),
    )?;
    // The renewal holds its row until the gate opens, after the new token is written.
    let holder = gate(
        &f.database,
        "synthetic_renewal_gate",
        "AFTER UPDATE ON jobniac.generation_intents WHEN (NEW.state = 'PENDING')",
        GATE_RENEWAL,
    )
    .await?;
    let renewal = renewals.renew_generation(&claim, renewed_token);
    let stale_writer = async {
        wait_for_lock(&f.database, CLEANUP_APP).await?;
        Ok::<_, Box<dyn Error>>(
            writer
                .replace(&candidate, &claim.reservation, 0, f.base, Some(&claim))
                .await,
        )
    };
    let observer = async {
        wait_for_lock(&f.database, CLEANUP_APP).await?;
        wait_for_blocker(&f.database, LINK_APP, CLEANUP_APP).await?;
        release(&holder, GATE_RENEWAL).await
    };
    let (renewed, refused, observed) = tokio::join!(renewal, stale_writer, observer);
    observed?;
    let renewed = renewed?.ok_or("the live claim must renew")?;
    assert_ne!(renewed.intent.lease_token, claim.intent.lease_token);
    assert_eq!(code_of(refused?)?, ErrorCode::ValidationError);
    assert_eq!(current(&writer, f.profile, MASTER).await?, Some(previous));
    assert_eq!(lifecycle(&f.uow, old.asset.asset_id()).await?, "ACTIVE");
    assert_eq!(lifecycle(&f.uow, claim.asset_id()).await?, "ACTIVE");
    assert_eq!(task_count(&f.uow, claim.asset_id()).await?, 0);
    let stored = f
        .uow
        .transaction(options(), async |tx| {
            find_generation_intent(tx, claim.asset_id(), false).await
        })
        .await?
        .ok_or("the renewed intent must exist")?;
    assert_eq!(stored, renewed.intent);
    let after = state(&f.database).await?;
    for table in STATE_TABLES {
        if table != "jobniac.generation_intents" {
            assert_eq!(after[table], before[table], "{table}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn classified_link_completion_wins_over_an_expiry_recovery_that_runs_meanwhile() -> TestResult
{
    let Some(f) = start().await? else {
        return Ok(());
    };
    let claim = classified_claim(
        &f,
        &profile_descriptor(Uuid::new_v4(), f.profile, MASTER)?,
        f.base,
    )
    .await?;
    let expires = claim
        .intent
        .lease_expires_at
        .ok_or("a pending intent has a lease")?;
    let writer = links(&f, fixed(f.base + Duration::seconds(1)));
    let recovery = generated(&f, fixed(expires));
    let candidate = link(
        f.profile,
        &claim.reservation,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(3),
    )?;
    // The link waits at the intent completion, holding the identity locks. The recovery starts
    // only after that, so its try-locks fail; the completion then commits, and the recovery must
    // not resurrect or abandon the claim. The gate is released only after the recovery has
    // returned, in the same task that waited for the link: two tasks both polling for the link's
    // lock would race, as the first could release the gate before the second saw the lock.
    let holder = gate(
        &f.database,
        "synthetic_completion_gate",
        "AFTER UPDATE ON jobniac.generation_intents WHEN (NEW.state = 'COMPLETED')",
        GATE_COMPLETION,
    )
    .await?;
    let replacement = writer.replace(&candidate, &claim.reservation, 0, f.base, Some(&claim));
    let sweep = async {
        let waited = wait_for_lock(&f.database, LINK_APP).await;
        let swept = match waited {
            Ok(()) => Some(
                recovery
                    .recover_expired_generations(f.root, RETRY_BATCH_SIZE)
                    .await,
            ),
            Err(_) => None,
        };
        // The link must be let go on every path, or the replacement never finishes.
        let released = release(&holder, GATE_COMPLETION).await;
        waited?;
        released?;
        Ok::<_, Box<dyn Error>>(swept.ok_or("the recovery did not run")?)
    };
    let (saved, swept) = tokio::join!(replacement, sweep);
    let swept = swept?;
    let saved = saved?.saved;
    assert_eq!(swept?, Vec::<Uuid>::new());
    assert_eq!(current(&writer, f.profile, MASTER).await?, Some(saved));
    assert_eq!(
        cleanup(&f, fixed(f.base))
            .prepare_cleanup(claim.asset_id())
            .await?,
        PreparationOutcome::Referenced
    );
    assert_eq!(lifecycle(&f.uow, claim.asset_id()).await?, "ACTIVE");
    assert_eq!(task_count(&f.uow, claim.asset_id()).await?, 0);
    assert_eq!(
        intent_state(&f.uow, claim.asset_id()).await?,
        GenerationState::Completed
    );
    Ok(())
}

#[tokio::test]
async fn expiry_recovery_commit_fences_a_waiting_link_without_resurrection() -> TestResult {
    let Some(f) = start().await? else {
        return Ok(());
    };
    let old = active_asset(&f.uow, f.root, f.base, "old").await?;
    let writer = links(&f, fixed(f.base));
    let previous = save_first(&writer, f.profile, &old, f.base).await?;
    let claim = classified_claim(
        &f,
        &profile_descriptor(Uuid::new_v4(), f.profile, MASTER)?,
        f.base,
    )
    .await?;
    let expires = claim
        .intent
        .lease_expires_at
        .ok_or("a pending intent has a lease")?;
    let recovery = generated(&f, fixed(expires));
    let candidate = link(
        f.profile,
        &claim.reservation,
        MASTER,
        f.base + Duration::seconds(1),
        f.base + Duration::seconds(3),
    )?;
    // The recovery retires the claim's asset and holds the row until the gate opens; the link
    // waits on that row and must be refused once the retirement commits.
    let holder = gate(
        &f.database,
        "synthetic_recovery_gate",
        "AFTER UPDATE OF lifecycle ON jobniac.generated_assets \
         WHEN (NEW.lifecycle = 'RETIRING')",
        GATE_RECOVERY,
    )
    .await?;
    let sweep = recovery.recover_expired_generations(f.root, RETRY_BATCH_SIZE);
    let late_writer = async {
        wait_for_lock(&f.database, CLEANUP_APP).await?;
        Ok::<_, Box<dyn Error>>(
            writer
                .replace(&candidate, &claim.reservation, 0, f.base, Some(&claim))
                .await,
        )
    };
    let observer = async {
        wait_for_lock(&f.database, CLEANUP_APP).await?;
        wait_for_blocker(&f.database, LINK_APP, CLEANUP_APP).await?;
        release(&holder, GATE_RECOVERY).await
    };
    let (swept, refused, observed) = tokio::join!(sweep, late_writer, observer);
    observed?;
    assert_eq!(swept?, vec![claim.asset_id()]);
    assert_eq!(code_of(refused?)?, ErrorCode::ValidationError);
    assert_eq!(current(&writer, f.profile, MASTER).await?, Some(previous));
    assert_eq!(lifecycle(&f.uow, old.asset.asset_id()).await?, "ACTIVE");
    assert_eq!(
        asset_state(&f.database, claim.asset_id()).await?,
        ("RETIRING".to_owned(), Some("PENDING".to_owned()))
    );
    assert_eq!(
        intent_state(&f.uow, claim.asset_id()).await?,
        GenerationState::Abandoned
    );
    Ok(())
}

#[tokio::test]
async fn driver_failures_are_refused_with_the_fixed_message_and_no_driver_text() -> TestResult {
    // The configuration names a closed local port, so every session attempt is refused by the
    // driver. The caller sees only the fixed unavailable refusal. This needs no server.
    let port = closed_port()?;
    let environment = Environment::from_pairs([
        ("JOBNIAC_DB_HOST", "127.0.0.1".to_owned()),
        ("JOBNIAC_DB_PORT", port.to_string()),
        ("JOBNIAC_DB_NAME", "synthetic_db".to_owned()),
        ("JOBNIAC_DB_USER", "synthetic_user".to_owned()),
        ("JOBNIAC_DB_PASSWORD", "private-password-marker".to_owned()),
    ]);
    let config =
        load_database_config(&environment).ok_or("the synthetic configuration is invalid")?;
    let links = PgProfileResumeLinks::new(
        PgUnitOfWork::new(PgPool::new(Some(config))),
        fixed(OffsetDateTime::UNIX_EPOCH),
    );
    let read = links
        .find_by_profile_id_and_resume_type(Uuid::new_v4(), MASTER)
        .await;
    assert_unavailable(read)?;

    let binding = GeneratedReservation {
        asset: AssetIdentity::new(Uuid::new_v4(), Uuid::new_v4(), &sha256_hex(b"%PDF-test"), 9)?,
        document_id: Uuid::new_v4(),
    };
    let profile = Uuid::new_v4();
    let candidate = link(
        profile,
        &binding,
        MASTER,
        OffsetDateTime::UNIX_EPOCH,
        OffsetDateTime::UNIX_EPOCH,
    )?;
    let write = links
        .replace(&candidate, &binding, 0, OffsetDateTime::UNIX_EPOCH, None)
        .await;
    assert_unavailable(write)?;
    Ok(())
}

#[tokio::test]
async fn refusals_come_before_any_connection() -> TestResult {
    // The pool has no configuration, so any session attempt would report the database
    // unavailable. A validation refusal therefore proves that nothing was attempted. The valid
    // control shows that the same pool does reach the session attempt.
    let links = PgProfileResumeLinks::new(
        PgUnitOfWork::new(PgPool::new(None)),
        fixed(OffsetDateTime::UNIX_EPOCH),
    );
    let binding = GeneratedReservation {
        asset: AssetIdentity::new(Uuid::new_v4(), Uuid::new_v4(), &sha256_hex(b"%PDF-test"), 9)?,
        document_id: Uuid::new_v4(),
    };
    let profile = Uuid::new_v4();
    let at = OffsetDateTime::UNIX_EPOCH;
    let candidate = link(profile, &binding, MASTER, at, at)?;

    let control = links.replace(&candidate, &binding, 0, at, None).await;
    assert_ne!(code_of(control)?, ErrorCode::ValidationError);

    let negative = links.replace(&candidate, &binding, -1, at, None).await;
    assert_eq!(code_of(negative)?, ErrorCode::ValidationError);
    for resume_type in ["unsupported", "germany"] {
        let lookup = links
            .find_by_profile_id_and_resume_type(profile, resume_type)
            .await;
        assert_eq!(
            code_of(lookup)?,
            ErrorCode::ValidationError,
            "{resume_type}"
        );
    }
    let long = links
        .find_by_profile_id_and_resume_type(profile, &"x".repeat(100_000))
        .await;
    assert_eq!(code_of(long)?, ErrorCode::ValidationError);

    // Each proof defect is refused in memory: the proof fields are public, so each case changes one.
    let valid = proof(profile, &binding)?;
    let cases: [(&str, ProofDefect); 5] = [
        ("lease", |claim| claim.intent.lease_token = None),
        ("owner", |claim| {
            claim.intent.descriptor.profile_id = Some(Uuid::new_v4());
        }),
        ("scope", |claim| {
            claim.intent.descriptor.output_scope = Some(CANADIAN.to_owned());
        }),
        ("asset", |claim| claim.intent.asset_id = Uuid::new_v4()),
        ("document", |claim| {
            claim.reservation.document_id = Uuid::new_v4()
        }),
    ];
    for (defect, change) in cases {
        let mut claim = valid.clone();
        change(&mut claim);
        let result = links
            .replace(&candidate, &binding, 0, at, Some(&claim))
            .await;
        assert_eq!(code_of(result)?, ErrorCode::ValidationError, "{defect}");
    }
    // The purpose defect: a standalone descriptor is not a profile resume claim.
    let mut purpose = valid.clone();
    purpose.intent.descriptor = GenerationDescriptor::new(
        Uuid::new_v4(),
        GenerationPurpose::StandalonePdf,
        None,
        None,
        None,
        None,
    )?;
    let result = links
        .replace(&candidate, &binding, 0, at, Some(&purpose))
        .await;
    assert_eq!(code_of(result)?, ErrorCode::ValidationError, "purpose");
    Ok(())
}
