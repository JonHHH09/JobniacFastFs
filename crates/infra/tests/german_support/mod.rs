//! Shared fixtures of the German resume and cover-letter PostgreSQL tests.
//!
//! Every server-backed test works on its own disposable, migrated database. Without a configured
//! server the fixture is `None` and the test returns early with a skip notice. All data is
//! synthetic: reserved `.test` domains, generated identifiers and fixed instants.

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration as StdDuration, Instant};

use jobniac_core::clock::{Clock, FixedClock};
use jobniac_core::env::Environment;
use jobniac_core::errors::{AppError, ErrorCode, error_envelope};
use jobniac_core::generated_assets::{
    AssetIdentity, GeneratedReservation, GenerationDescriptor, IntentReservation, sha256_hex,
};
use jobniac_core::german::cover_letter_store::{
    CoverLetter, CoverLetterAggregateWrite, CoverLetterFields, CoverLetterParagraph,
    CoverLetterVariant, ParagraphFields, VariantFields as LetterVariantFields,
    VariantWrite as LetterVariantWrite,
};
use jobniac_core::german::resume_store::{
    BulletFields, DisplayOrder, EntryFields, EntryWrite, Resume, ResumeAggregateWrite, ResumeEntry,
    ResumeEntryBullet, ResumeFields, ResumeSection, ResumeVariant, Revision, SectionFields,
    SectionWrite, VariantFields, VariantWrite,
};
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::postgres::cleanup::PgCleanupRepository;
use jobniac_infra::postgres::config::{DatabaseConfig, load_database_config};
use jobniac_infra::postgres::generated::IntentError;
use jobniac_infra::postgres::generated::intents::insert_generation_intent;
use jobniac_infra::postgres::german::{
    PgGermanCoverLetterRepository, PgGermanResumeRepository, cover_letter, resume,
};
use jobniac_infra::postgres::migrations::migrate;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::{PgTransaction, PgUnitOfWork};
use jobniac_testkit::pg::{TestDatabase, TestServer};
use time::macros::{date, datetime};
use time::{Duration, OffsetDateTime};
use tokio_postgres::Client;
use uuid::Uuid;

/// The result type of the integration tests.
pub type TestResult = Result<(), Box<dyn Error>>;

/// The owner revision instant every synthetic profile and job carries.
pub fn now() -> OffsetDateTime {
    datetime!(2026-10-02 0:00 UTC)
}

/// The instant of every aggregate write and of the repository clock.
pub fn later() -> OffsetDateTime {
    now() + Duration::seconds(1)
}

/// The tables whose rows a failed or refused replacement must leave untouched.
pub const TABLES: [&str; 14] = [
    "resume.resumes",
    "resume.resume_variants",
    "resume.resume_sections",
    "resume.resume_entries",
    "resume.resume_entry_bullets",
    "cover_letter.cover_letters",
    "cover_letter.cover_letter_variants",
    "cover_letter.cover_letter_paragraphs",
    "jobniac.generated_assets",
    "jobniac.generated_asset_cleanups",
    "jobniac.generation_intents",
    "document.documents",
    "document.blobs",
    "profile.profile_personal_details",
];

/// A migrated disposable database with one synthetic profile, one job and one asset root.
pub struct Fixture {
    /// The disposable database; dropped with the fixture.
    pub database: TestDatabase,
    /// The validated configuration of [`Fixture::database`].
    pub config: DatabaseConfig,
    /// The unit of work of the fixture's own statements (plain `jobniac` sessions).
    pub uow: PgUnitOfWork,
    /// The synthetic profile.
    pub profile: Uuid,
    /// The synthetic job.
    pub job: Uuid,
    /// The generated-asset root.
    pub root: Uuid,
}

/// A fresh migrated database, or `None` when no server is configured.
#[allow(
    clippy::print_stdout,
    reason = "the local skip notice must be visible in the test output"
)]
pub async fn start() -> Result<Option<Fixture>, Box<dyn Error>> {
    let Some(server) = TestServer::from_env() else {
        // On CI a missing server must fail, never pass vacuously.
        assert!(
            std::env::var_os("CI").is_none(),
            "JOBNIAC_TEST_PG_PORT must be set in CI so that the German PostgreSQL tests run"
        );
        println!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(None);
    };
    let database = server.fresh_database().await;
    let config = load_database_config(&database.environment())
        .ok_or("the disposable database configuration is invalid")?;
    migrate(&config).await.map_err(|error| error.0)?;
    let uow = PgUnitOfWork::new(PgPool::new(Some(config.clone())));
    let fixture = Fixture {
        database,
        config,
        uow,
        profile: Uuid::new_v4(),
        job: Uuid::new_v4(),
        root: Uuid::new_v4(),
    };
    let admin = fixture.admin().await;
    insert_owners(&admin, fixture.profile, fixture.job, "Synthetic").await?;
    admin
        .execute(
            "INSERT INTO jobniac.generated_asset_roots (id, created_at) VALUES ($1, $2)",
            &[&fixture.root, &now()],
        )
        .await?;
    Ok(Some(fixture))
}

/// Inserts one synthetic profile, one job and the text ingestion the job schema requires.
pub async fn insert_owners(
    admin: &Client,
    profile: Uuid,
    job: Uuid,
    label: &str,
) -> Result<(), Box<dyn Error>> {
    admin
        .execute(
            "INSERT INTO profile.profiles (id, full_name, email, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $4)",
            &[
                &profile,
                &format!("{label} Person"),
                &format!("{profile}@example.test"),
                &now(),
            ],
        )
        .await?;
    // The job needs its text provenance row at commit, so both rows share one transaction.
    admin.batch_execute("BEGIN").await?;
    admin
        .execute(
            "INSERT INTO job_schema.jobs (id, source_method, title, description, \
             canonical_fingerprint, created_at, updated_at) \
             VALUES ($1, 'text', $2, 'Synthetic description', $3, $4, $4)",
            &[&job, &format!("{label} Job"), &job.to_string(), &now()],
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
                &now(),
            ],
        )
        .await?;
    admin.batch_execute("COMMIT").await?;
    Ok(())
}

impl Fixture {
    /// A superuser connection to the fixture database.
    pub async fn admin(&self) -> Client {
        self.database.admin_client().await
    }

    /// The resume repository over a pool named like the production sessions, reading `at` as time.
    pub fn resumes(&self, at: OffsetDateTime) -> PgGermanResumeRepository {
        self.resumes_with(Arc::new(FixedClock::new(at)))
    }

    /// The resume repository over `clock`.
    pub fn resumes_with(&self, clock: Arc<dyn Clock>) -> PgGermanResumeRepository {
        let pool = resume::pool(Some(self.config.clone()));
        PgGermanResumeRepository::new(PgUnitOfWork::new(pool), clock)
    }

    /// The cover-letter repository over a pool named like the production sessions, reading `at`.
    pub fn letters(&self, at: OffsetDateTime) -> PgGermanCoverLetterRepository {
        self.letters_with(Arc::new(FixedClock::new(at)))
    }

    /// The cover-letter repository over `clock`.
    pub fn letters_with(&self, clock: Arc<dyn Clock>) -> PgGermanCoverLetterRepository {
        let pool = cover_letter::pool(Some(self.config.clone()));
        PgGermanCoverLetterRepository::new(PgUnitOfWork::new(pool), clock)
    }

    /// Creates one ACTIVE, unclassified asset in its own committed transaction.
    pub async fn active_binding(&self) -> Result<GeneratedReservation, IntentError> {
        let content = pdf("german");
        self.uow
            .transaction(options(), async |tx| {
                let reservation = insert_asset(tx, self.root, now(), &content, false).await?;
                activate(tx, reservation.asset.asset_id()).await?;
                Ok(reservation)
            })
            .await
    }

    /// Creates one RESERVED (never activated) asset.
    pub async fn reserved_binding(&self) -> Result<GeneratedReservation, IntentError> {
        let content = pdf("reserved");
        self.uow
            .transaction(options(), async |tx| {
                insert_asset(tx, self.root, now(), &content, false).await
            })
            .await
    }

    /// Creates one ACTIVE classified asset with its pending intent.
    pub async fn classified_claim(
        &self,
        descriptor: &GenerationDescriptor,
    ) -> Result<IntentReservation, IntentError> {
        let content = pdf("classified");
        self.uow
            .transaction(options(), async |tx| {
                let reservation = insert_asset(tx, self.root, now(), &content, true).await?;
                let claim =
                    insert_generation_intent(tx, &reservation, descriptor, Uuid::new_v4(), now())
                        .await?;
                activate(tx, reservation.asset.asset_id()).await?;
                Ok(claim)
            })
            .await
    }
}

/// A read-write scope.
pub fn options() -> TransactionOptions {
    TransactionOptions {
        read_only: false,
        requires_new: false,
    }
}

/// The database failure of a raw fixture statement, kept as an opaque internal error.
pub fn fixture_failure(_: tokio_postgres::Error) -> IntentError {
    IntentError::Database(AppError::Internal)
}

/// Synthetic PDF bytes, unique per call so that no two blobs share a digest.
pub fn pdf(label: &str) -> Vec<u8> {
    format!("%PDF-1.7\n% synthetic {label} {}\n", Uuid::new_v4()).into_bytes()
}

/// Inserts a document and a RESERVED asset in the current transaction.
pub async fn insert_asset(
    tx: &mut PgTransaction,
    root: Uuid,
    at: OffsetDateTime,
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
            &[&blob, &digest, &size, &content, &at],
        )
        .await
        .map_err(fixture_failure)?;
    client
        .execute(
            "INSERT INTO document.documents (id, blob_id, original_file_name, media_type, \
             created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $5)",
            &[&document, &blob, &"synthetic.pdf", &"application/pdf", &at],
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
                &at,
                &classified,
            ],
        )
        .await
        .map_err(fixture_failure)?;
    let size = u64::try_from(size).map_err(|_| IntentError::Invalid)?;
    Ok(GeneratedReservation {
        asset: AssetIdentity::new(asset_id, root, &digest, size)?,
        document_id: document,
    })
}

/// Marks a RESERVED asset ACTIVE and materialized, as the materialization step does.
pub async fn activate(tx: &mut PgTransaction, asset_id: Uuid) -> Result<(), IntentError> {
    tx.client()
        .execute(
            "UPDATE jobniac.generated_assets SET lifecycle = 'ACTIVE', materialized_at = $2 \
             WHERE asset_id = $1",
            &[&asset_id, &now()],
        )
        .await
        .map_err(fixture_failure)?;
    Ok(())
}

/// A revision token.
pub fn revision(value: i64) -> Revision {
    Revision::new(value).expect("a non-negative revision")
}

/// A stored position.
pub fn order(value: i64) -> DisplayOrder {
    DisplayOrder::new(value).expect("a stored position")
}

/// The synthetic resume parent row of a profile and job (format spelled with padding on purpose).
pub fn resume_row(profile: Uuid, job: Uuid) -> Result<Resume, Box<dyn Error>> {
    Ok(Resume::new(ResumeFields {
        id: Uuid::new_v4(),
        profile_id: profile,
        job_id: job,
        format: " GERMANY ",
        profile_revision: now(),
        job_revision: now(),
        created_at: now(),
        updated_at: later(),
    })?)
}

/// A variant row of `resume_id` in `language` for a reserved binding.
pub fn variant_row(
    resume_id: Uuid,
    language: &str,
    binding: &GeneratedReservation,
) -> Result<ResumeVariant, Box<dyn Error>> {
    Ok(ResumeVariant::new(VariantFields {
        id: Uuid::new_v4(),
        resume_id,
        language,
        document_id: binding.document_id,
        file_path: binding.asset.relative_path(),
        created_at: now(),
        updated_at: later(),
    })?)
}

/// A section row with the open type `experience`.
pub fn section_row(variant_id: Uuid, position: i64) -> Result<ResumeSection, Box<dyn Error>> {
    Ok(ResumeSection::new(SectionFields {
        id: Uuid::new_v4(),
        variant_id,
        section_type: " EXPERIENCE ",
        title: " Work ",
        display_order: order(position),
    })?)
}

/// An entry row with padded text, a blank location and an opaque metadata string.
pub fn entry_row(
    id: Uuid,
    section_id: Uuid,
    position: i64,
    title: &str,
) -> Result<ResumeEntry, Box<dyn Error>> {
    Ok(ResumeEntry::new(EntryFields {
        id,
        section_id,
        entry_type: " EXPERIENCE ",
        display_order: order(position),
        title: Some(title),
        organization: Some(" Company "),
        location: Some("  "),
        start_date: Some(date!(2020 - 02 - 29)),
        end_date: None,
        metadata: Some(" {\"opaque\": true} "),
    })?)
}

/// A bullet row.
pub fn bullet_row(
    id: Uuid,
    entry_id: Uuid,
    position: i64,
    text: &str,
) -> Result<ResumeEntryBullet, Box<dyn Error>> {
    Ok(ResumeEntryBullet::new(BulletFields {
        id,
        entry_id,
        display_order: order(position),
        text,
    })?)
}

/// A variant write with one section, one entry (position 7) and one bullet (position 4).
pub fn variant_write(variant: ResumeVariant) -> Result<VariantWrite, Box<dyn Error>> {
    let section = section_row(variant.id(), 0)?;
    let entry = entry_row(Uuid::new_v4(), section.id(), 7, " Developer ")?;
    let bullet = bullet_row(Uuid::new_v4(), entry.id(), 4, " Built something ")?;
    let entries = vec![EntryWrite::new(entry, vec![bullet])?];
    Ok(VariantWrite::new(
        variant,
        vec![SectionWrite::new(section, entries)?],
    )?)
}

/// The aggregate of a fresh resume with one variant per binding (languages `en`, then `de`).
pub fn resume_write(
    profile: Uuid,
    job: Uuid,
    bindings: &[&GeneratedReservation],
) -> Result<ResumeAggregateWrite, Box<dyn Error>> {
    let parent = resume_row(profile, job)?;
    let mut variants = Vec::new();
    for (binding, language) in bindings.iter().zip(["en", "de"]) {
        variants.push(variant_write(variant_row(parent.id(), language, binding)?)?);
    }
    Ok(ResumeAggregateWrite::new(parent, variants)?)
}

/// Saves a resume with owner revisions `0` and no claims.
pub async fn save_resume(
    repository: &PgGermanResumeRepository,
    write: &ResumeAggregateWrite,
    bindings: &[&GeneratedReservation],
) -> Result<jobniac_core::german::resume_store::ReplaceResult, AppError> {
    use jobniac_core::german::resume_store::GermanResumeRepository;
    let owned: Vec<GeneratedReservation> =
        bindings.iter().map(|binding| (*binding).clone()).collect();
    repository
        .replace_germany_resume(write, &owned, revision(0), revision(0), None)
        .await
}

/// A letter aggregate of `resume` with one paragraph per position in `orders`.
pub fn letter_write(
    resume: &Resume,
    binding: &GeneratedReservation,
    orders: &[i64],
) -> Result<CoverLetterAggregateWrite, Box<dyn Error>> {
    let parent = CoverLetter::new(CoverLetterFields {
        id: Uuid::new_v4(),
        profile_id: resume.profile_id(),
        job_id: resume.job_id(),
        resume_id: resume.id(),
        profile_revision: resume.profile_revision(),
        job_revision: resume.job_revision(),
        resume_revision: resume.updated_at(),
        created_at: now(),
        updated_at: later(),
    })?;
    let variant = CoverLetterVariant::new(LetterVariantFields {
        id: Uuid::new_v4(),
        cover_letter_id: parent.id(),
        format: " GERMANY ",
        language: " DE ",
        document_id: binding.document_id,
        file_path: binding.asset.relative_path(),
        subject: " Bewerbung \u{1F600} ",
        salutation: " Hallo ",
        closing: " Gr\u{FC}\u{DF}e ",
        signature: " Person ",
        created_at: now(),
        updated_at: later(),
    })?;
    let mut paragraphs = Vec::new();
    for position in orders {
        paragraphs.push(CoverLetterParagraph::new(ParagraphFields {
            id: Uuid::new_v4(),
            variant_id: variant.id(),
            display_order: order(*position),
            text: &format!(" Absatz {position} \u{1F600} "),
        })?);
    }
    Ok(CoverLetterAggregateWrite::new(
        parent,
        LetterVariantWrite::new(variant, paragraphs)?,
    )?)
}

/// Saves a letter with owner revisions `0` and no claim.
pub async fn save_letter(
    repository: &PgGermanCoverLetterRepository,
    write: &CoverLetterAggregateWrite,
    binding: &GeneratedReservation,
) -> Result<jobniac_core::german::cover_letter_store::ReplaceResult, AppError> {
    use jobniac_core::german::cover_letter_store::GermanCoverLetterRepository;
    repository
        .replace_german_cover_letter(write, binding, revision(0), revision(0), None)
        .await
}

/// Seeds a prepared letter row directly and returns its id.
pub async fn insert_letter(
    admin: &Client,
    resume: &Resume,
    binding: &GeneratedReservation,
) -> Result<Uuid, Box<dyn Error>> {
    let letter = Uuid::new_v4();
    let variant = Uuid::new_v4();
    admin
        .execute(
            "INSERT INTO cover_letter.cover_letters (id, profile_id, job_id, resume_id, \
             profile_revision, job_revision, resume_revision, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            &[
                &letter,
                &resume.profile_id(),
                &resume.job_id(),
                &resume.id(),
                &now(),
                &now(),
                &later(),
                &now(),
                &later(),
            ],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO cover_letter.cover_letter_variants (id, cover_letter_id, format, \
             language, document_id, file_path, subject, salutation, closing, signature, \
             created_at, updated_at) \
             VALUES ($1,$2,'germany','de',$3,$4,'Subject','Hello','Regards','Synthetic',$5,$6)",
            &[
                &variant,
                &letter,
                &binding.document_id,
                &binding.asset.relative_path(),
                &now(),
                &later(),
            ],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO cover_letter.cover_letter_paragraphs (id, variant_id, display_order, \
             text) VALUES ($1, $2, 0, 'Synthetic paragraph')",
            &[&Uuid::new_v4(), &variant],
        )
        .await?;
    Ok(letter)
}

/// Attaches an asset's document as the profile photo, which references it.
pub async fn attach_photo(
    admin: &Client,
    profile: Uuid,
    binding: &GeneratedReservation,
) -> Result<(), Box<dyn Error>> {
    admin
        .execute(
            "INSERT INTO profile.profile_personal_details \
             (profile_id, photo_document_id, created_at, updated_at) VALUES ($1, $2, $3, $3)",
            &[&profile, &binding.document_id, &now()],
        )
        .await?;
    Ok(())
}

/// Every row of every table of [`TABLES`] as text, in a stable order.
pub async fn snapshot(admin: &Client) -> Result<Vec<Vec<String>>, Box<dyn Error>> {
    let mut result = Vec::new();
    for table in TABLES {
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
        result.push(texts);
    }
    Ok(result)
}

/// The rows of one table in [`TABLES`] of a snapshot.
pub fn table_rows<'a>(snapshot: &'a [Vec<String>], table: &str) -> &'a [String] {
    TABLES
        .iter()
        .position(|candidate| *candidate == table)
        .and_then(|index| snapshot.get(index))
        .map_or(&[], Vec::as_slice)
}

/// The lifecycle and the cleanup task of one generated asset.
#[derive(Debug)]
pub struct AssetState {
    pub lifecycle: String,
    pub retirement_id: Option<Uuid>,
    pub retired_at: Option<OffsetDateTime>,
    pub status: Option<String>,
    pub task_id: Option<Uuid>,
    pub next_attempt_at: Option<OffsetDateTime>,
    /// The retirement identity the cleanup task was created with.
    pub task_retirement_id: Option<Uuid>,
}

/// Reads the state of an asset.
pub async fn asset_state(
    admin: &Client,
    binding: &GeneratedReservation,
) -> Result<AssetState, Box<dyn Error>> {
    let row = admin
        .query_one(
            "SELECT asset.lifecycle, asset.retirement_id, asset.retired_at, task.status, \
             task.task_id, task.next_attempt_at, task.retirement_id AS task_retirement_id \
             FROM jobniac.generated_assets asset \
             LEFT JOIN jobniac.generated_asset_cleanups task ON task.asset_id = asset.asset_id \
             WHERE asset.asset_id = $1",
            &[&binding.asset.asset_id()],
        )
        .await?;
    Ok(AssetState {
        lifecycle: row.try_get("lifecycle")?,
        retirement_id: row.try_get("retirement_id")?,
        retired_at: row.try_get("retired_at")?,
        status: row.try_get("status")?,
        task_id: row.try_get("task_id")?,
        next_attempt_at: row.try_get("next_attempt_at")?,
        task_retirement_id: row.try_get("task_retirement_id")?,
    })
}

/// Installs a trigger that raises on `event` of `table` when `condition` holds.
pub async fn fail_at(
    admin: &Client,
    table: &str,
    event: &str,
    condition: &str,
) -> Result<(), Box<dyn Error>> {
    admin
        .batch_execute(&format!(
            "CREATE FUNCTION jobniac.synthetic_german_failure() RETURNS trigger \
             LANGUAGE plpgsql AS $$ BEGIN \
                 IF {condition} THEN RAISE EXCEPTION 'private synthetic failure'; END IF; \
                 IF TG_OP = 'DELETE' THEN RETURN OLD; END IF; \
                 RETURN NEW; \
             END $$; \
             CREATE TRIGGER synthetic_german_failure BEFORE {event} ON {table} \
             FOR EACH ROW EXECUTE FUNCTION jobniac.synthetic_german_failure();"
        ))
        .await?;
    Ok(())
}

/// Installs a trigger that waits for the advisory `key` before each insert into `table`, and
/// returns the session that holds that key until [`release_gate`].
pub async fn gate_inserts(
    database: &TestDatabase,
    table: &str,
    key: i64,
) -> Result<Client, Box<dyn Error>> {
    let admin = database.admin_client().await;
    admin
        .batch_execute(&format!(
            "CREATE FUNCTION jobniac.synthetic_gate_{key}() RETURNS trigger LANGUAGE plpgsql \
             AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN NEW; END $$; \
             CREATE TRIGGER synthetic_gate_{key} BEFORE INSERT ON {table} \
             FOR EACH ROW EXECUTE FUNCTION jobniac.synthetic_gate_{key}();"
        ))
        .await?;
    let gate = database.admin_client().await;
    gate.execute("SELECT pg_advisory_lock($1)", &[&key]).await?;
    Ok(gate)
}

/// Releases the gate taken by [`gate_inserts`].
pub async fn release_gate(gate: &Client, key: i64) -> Result<(), Box<dyn Error>> {
    gate.execute("SELECT pg_advisory_unlock($1)", &[&key])
        .await?;
    Ok(())
}

/// Installs a trigger that waits for the advisory `key` before an asset is retired, and returns
/// the session that holds that key until [`release_gate`].
pub async fn gate_retirements(database: &TestDatabase, key: i64) -> Result<Client, Box<dyn Error>> {
    let admin = database.admin_client().await;
    admin
        .batch_execute(&format!(
            "CREATE FUNCTION jobniac.synthetic_retire_gate_{key}() RETURNS trigger \
             LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN NEW; \
             END $$; \
             CREATE TRIGGER synthetic_retire_gate_{key} BEFORE UPDATE OF lifecycle \
             ON jobniac.generated_assets FOR EACH ROW WHEN (NEW.lifecycle = 'RETIRING') \
             EXECUTE FUNCTION jobniac.synthetic_retire_gate_{key}();"
        ))
        .await?;
    let gate = database.admin_client().await;
    gate.execute("SELECT pg_advisory_lock($1)", &[&key]).await?;
    Ok(gate)
}

/// A cleanup repository whose sessions carry the `application_name` `jobniac-cleanup`.
pub fn cleanup_repository(config: &DatabaseConfig, at: OffsetDateTime) -> PgCleanupRepository {
    let pool = PgPool::with_application_name(Some(config.clone()), "jobniac-cleanup");
    PgCleanupRepository::new(PgUnitOfWork::new(pool), Arc::new(FixedClock::new(at)))
}

/// A configuration whose loopback port is closed, so every connection attempt fails.
pub fn closed_port_config(fixture: &Fixture) -> Result<DatabaseConfig, Box<dyn Error>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let environment = fixture.database.environment();
    let get = |key: &str| environment.get(key).unwrap_or_default().to_owned();
    let environment = Environment::from_pairs([
        ("JOBNIAC_DB_HOST", get("JOBNIAC_DB_HOST")),
        ("JOBNIAC_DB_PORT", port.to_string()),
        ("JOBNIAC_DB_NAME", get("JOBNIAC_DB_NAME")),
        ("JOBNIAC_DB_USER", get("JOBNIAC_DB_USER")),
        ("JOBNIAC_DB_PASSWORD", get("JOBNIAC_DB_PASSWORD")),
    ]);
    load_database_config(&environment).ok_or_else(|| "the closed-port configuration".into())
}

/// Waits until a session named `application_name` waits for a lock.
pub async fn wait_for_lock(
    database: &TestDatabase,
    application_name: &str,
) -> Result<(), Box<dyn Error>> {
    let observer = database.admin_client().await;
    let deadline = Instant::now() + StdDuration::from_secs(5);
    while Instant::now() < deadline {
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
        tokio::time::sleep(StdDuration::from_millis(10)).await;
    }
    Err(format!("{application_name} did not reach the held database lock").into())
}

/// Waits until `waiter` is blocked by a session named `blocker`.
pub async fn wait_for_blocker(
    database: &TestDatabase,
    waiter: &str,
    blocker: &str,
) -> Result<(), Box<dyn Error>> {
    let observer = database.admin_client().await;
    let deadline = Instant::now() + StdDuration::from_secs(5);
    while Instant::now() < deadline {
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
        tokio::time::sleep(StdDuration::from_millis(10)).await;
    }
    Err(format!("{waiter} did not wait for {blocker}").into())
}

/// The code and message of the envelope of a refused call, or a test failure when it succeeded.
pub fn refusal<T: std::fmt::Debug>(
    result: Result<T, AppError>,
) -> Result<(ErrorCode, String), Box<dyn Error>> {
    match result {
        Err(error) => {
            let envelope = error_envelope(&error);
            Ok((envelope.code, envelope.message))
        }
        Ok(value) => Err(format!("the call was accepted: {value:?}").into()),
    }
}

/// The fixed validation refusal of the resume store.
pub const RESUME_INVALID: (ErrorCode, &str) = (
    ErrorCode::ValidationError,
    "Invalid German resume persistence input",
);

/// The fixed validation refusal of the cover-letter store.
pub const LETTER_INVALID: (ErrorCode, &str) = (
    ErrorCode::ValidationError,
    "Invalid German cover-letter persistence input",
);

/// The fixed unavailable refusal.
pub const UNAVAILABLE: (ErrorCode, &str) =
    (ErrorCode::InternalError, "Local database is unavailable");

/// The fixed refusal of a repository built without a database configuration.
pub const NOT_CONFIGURED: (ErrorCode, &str) =
    (ErrorCode::InternalError, "Local database is not configured");

/// A clock that returns its instants in order and then repeats the last one.
#[derive(Debug)]
pub struct SequenceClock(Mutex<Vec<OffsetDateTime>>);

impl SequenceClock {
    /// A clock over `ticks`, which must not be empty.
    pub fn new(ticks: Vec<OffsetDateTime>) -> Self {
        Self(Mutex::new(ticks))
    }
}

impl Clock for SequenceClock {
    fn now(&self) -> OffsetDateTime {
        let mut ticks = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if ticks.len() > 1 {
            ticks.remove(0)
        } else {
            ticks.first().copied().unwrap_or_else(now)
        }
    }
}
