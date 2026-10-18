//! Shared fixtures of the German resume and cover-letter SQLite tests.
//!
//! Every test works on its own migrated temporary database file (`SqliteFixture`). The including
//! test declares `mod sqlite_support;` first. All data is synthetic: reserved `.test` domains,
//! generated identifiers and fixed instants.

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use jobniac_core::clock::{Clock, FixedClock};
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
use jobniac_infra::sqlite::cleanup::SqliteCleanupRepository;
use jobniac_infra::sqlite::codec::{instant_text, parse_instant_text, parse_uuid};
use jobniac_infra::sqlite::generated::IntentError;
use jobniac_infra::sqlite::generated::intents::insert_generation_intent;
use jobniac_infra::sqlite::german::{
    SqliteGermanCoverLetterRepository, SqliteGermanResumeRepository,
};
use jobniac_infra::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, params};
use time::macros::{date, datetime};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::sqlite_support::SqliteFixture;

/// The result type of the integration tests.
pub type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

/// The owner revision instant every synthetic profile and job carries.
pub fn now() -> OffsetDateTime {
    datetime!(2026-10-02 0:00 UTC)
}

/// The instant of every aggregate write and of the repository clock.
pub fn later() -> OffsetDateTime {
    now() + Duration::seconds(1)
}

/// The text of an instant as the repositories store it.
pub fn stored(at: OffsetDateTime) -> String {
    instant_text(at)
}

/// The tables whose rows a failed or refused replacement must leave untouched.
pub const TABLES: [&str; 14] = [
    "resume_resumes",
    "resume_resume_variants",
    "resume_resume_sections",
    "resume_resume_entries",
    "resume_resume_entry_bullets",
    "cover_letter_cover_letters",
    "cover_letter_cover_letter_variants",
    "cover_letter_cover_letter_paragraphs",
    "jobniac_generated_assets",
    "jobniac_generated_asset_cleanups",
    "jobniac_generation_intents",
    "document_documents",
    "document_blobs",
    "profile_profile_personal_details",
];

/// A migrated temporary database with one synthetic profile, one job and one asset root.
pub struct Fixture {
    /// The temporary database; removed with the fixture.
    pub sqlite: SqliteFixture,
    /// The unit of work of the fixture's own statements.
    pub uow: SqliteUnitOfWork,
    /// The synthetic profile.
    pub profile: Uuid,
    /// The synthetic job.
    pub job: Uuid,
    /// The generated-asset root.
    pub root: Uuid,
}

/// A fresh migrated database.
pub fn start() -> Result<Fixture, Box<dyn Error + Send + Sync>> {
    let sqlite = SqliteFixture::start();
    let fixture = Fixture {
        uow: sqlite.unit_of_work(),
        sqlite,
        profile: Uuid::new_v4(),
        job: Uuid::new_v4(),
        root: Uuid::new_v4(),
    };
    let admin = fixture.admin();
    insert_owners(&admin, fixture.profile, fixture.job, "Synthetic")?;
    admin.execute(
        "INSERT INTO jobniac_generated_asset_roots (id, created_at) VALUES (?1, ?2)",
        params![fixture.root.to_string(), stored(now())],
    )?;
    Ok(fixture)
}

/// Inserts one synthetic profile and one job.
pub fn insert_owners(
    admin: &Connection,
    profile: Uuid,
    job: Uuid,
    label: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    admin.execute(
        "INSERT INTO profile_profiles (id, full_name, email, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![
            profile.to_string(),
            format!("{label} Person"),
            format!("{profile}@example.test"),
            stored(now())
        ],
    )?;
    admin.execute(
        "INSERT INTO job_jobs (id, source_method, title, description, \
         canonical_fingerprint, created_at, updated_at) \
         VALUES (?1, 'text', ?2, 'Synthetic description', ?3, ?4, ?4)",
        params![
            job.to_string(),
            format!("{label} Job"),
            job.to_string(),
            stored(now())
        ],
    )?;
    admin.execute(
        "INSERT INTO job_job_text_ingestions (id, job_id, input_text_hash, created_at) \
         VALUES (?1, ?2, ?3, ?4)",
        params![
            Uuid::new_v4().to_string(),
            job.to_string(),
            sha256_hex(job.to_string().as_bytes()),
            stored(now())
        ],
    )?;
    Ok(())
}

impl Fixture {
    /// A raw connection to the fixture database for seeding and assertions.
    pub fn admin(&self) -> Connection {
        self.sqlite.connection()
    }

    /// The resume repository reading `at` as time.
    pub fn resumes(&self, at: OffsetDateTime) -> SqliteGermanResumeRepository {
        self.resumes_with(Arc::new(FixedClock::new(at)))
    }

    /// The resume repository over `clock`.
    pub fn resumes_with(&self, clock: Arc<dyn Clock>) -> SqliteGermanResumeRepository {
        SqliteGermanResumeRepository::new(self.uow.clone(), clock)
    }

    /// The cover-letter repository reading `at`.
    pub fn letters(&self, at: OffsetDateTime) -> SqliteGermanCoverLetterRepository {
        self.letters_with(Arc::new(FixedClock::new(at)))
    }

    /// The cover-letter repository over `clock`.
    pub fn letters_with(&self, clock: Arc<dyn Clock>) -> SqliteGermanCoverLetterRepository {
        SqliteGermanCoverLetterRepository::new(self.uow.clone(), clock)
    }

    /// Creates one ACTIVE, unclassified asset in its own committed transaction.
    pub async fn active_binding(&self) -> Result<GeneratedReservation, IntentError> {
        let content = pdf("german");
        self.uow
            .transaction(options(), async |tx| {
                let reservation = insert_asset(tx, self.root, now(), &content, false)?;
                activate(tx, reservation.asset.asset_id())?;
                Ok(reservation)
            })
            .await
    }

    /// Creates one RESERVED (never activated) asset.
    pub async fn reserved_binding(&self) -> Result<GeneratedReservation, IntentError> {
        let content = pdf("reserved");
        self.uow
            .transaction(options(), async |tx| {
                insert_asset(tx, self.root, now(), &content, false)
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
                let reservation = insert_asset(tx, self.root, now(), &content, true)?;
                let claim =
                    insert_generation_intent(tx, &reservation, descriptor, Uuid::new_v4(), now())
                        .await?;
                activate(tx, reservation.asset.asset_id())?;
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

/// Synthetic PDF bytes, unique per call so that no two blobs share a digest.
pub fn pdf(label: &str) -> Vec<u8> {
    format!("%PDF-1.7\n% synthetic {label} {}\n", Uuid::new_v4()).into_bytes()
}

/// Inserts a document and a RESERVED asset in the current transaction.
pub fn insert_asset(
    tx: &mut SqliteTransaction,
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
    tx.run(|conn| {
        conn.execute(
            "INSERT INTO document_blobs (id, sha256, byte_size, content, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![blob.to_string(), digest, size, content, stored(at)],
        )?;
        conn.execute(
            "INSERT INTO document_documents (id, blob_id, original_file_name, media_type, \
             created_at, updated_at) VALUES (?1, ?2, 'synthetic.pdf', 'application/pdf', ?3, ?3)",
            params![document.to_string(), blob.to_string(), stored(at)],
        )?;
        conn.execute(
            "INSERT INTO jobniac_generated_assets (asset_id, root_id, relative_path, sha256, \
             size_bytes, document_id, lifecycle, created_at, generation_intent_required) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'RESERVED', ?7, ?8)",
            params![
                asset_id.to_string(),
                root.to_string(),
                format!("{asset_id}.pdf"),
                digest,
                size,
                document.to_string(),
                stored(at),
                classified
            ],
        )?;
        Ok(())
    })?;
    let size = u64::try_from(size).map_err(|_| IntentError::Invalid)?;
    Ok(GeneratedReservation {
        asset: AssetIdentity::new(asset_id, root, &digest, size)?,
        document_id: document,
    })
}

/// Marks a RESERVED asset ACTIVE and materialized, as the materialization step does.
pub fn activate(tx: &mut SqliteTransaction, asset_id: Uuid) -> Result<(), IntentError> {
    tx.run(|conn| {
        conn.execute(
            "UPDATE jobniac_generated_assets SET lifecycle = 'ACTIVE', materialized_at = ?2 \
             WHERE asset_id = ?1",
            params![asset_id.to_string(), stored(now())],
        )?;
        Ok(())
    })?;
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
pub fn resume_row(profile: Uuid, job: Uuid) -> Result<Resume, Box<dyn Error + Send + Sync>> {
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
) -> Result<ResumeVariant, Box<dyn Error + Send + Sync>> {
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
pub fn section_row(
    variant_id: Uuid,
    position: i64,
) -> Result<ResumeSection, Box<dyn Error + Send + Sync>> {
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
) -> Result<ResumeEntry, Box<dyn Error + Send + Sync>> {
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
) -> Result<ResumeEntryBullet, Box<dyn Error + Send + Sync>> {
    Ok(ResumeEntryBullet::new(BulletFields {
        id,
        entry_id,
        display_order: order(position),
        text,
    })?)
}

/// A variant write with one section, one entry (position 7) and one bullet (position 4).
pub fn variant_write(variant: ResumeVariant) -> Result<VariantWrite, Box<dyn Error + Send + Sync>> {
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
) -> Result<ResumeAggregateWrite, Box<dyn Error + Send + Sync>> {
    let parent = resume_row(profile, job)?;
    let mut variants = Vec::new();
    for (binding, language) in bindings.iter().zip(["en", "de"]) {
        variants.push(variant_write(variant_row(parent.id(), language, binding)?)?);
    }
    Ok(ResumeAggregateWrite::new(parent, variants)?)
}

/// Saves a resume with owner revisions `0` and no claims.
pub async fn save_resume(
    repository: &SqliteGermanResumeRepository,
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
) -> Result<CoverLetterAggregateWrite, Box<dyn Error + Send + Sync>> {
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
    repository: &SqliteGermanCoverLetterRepository,
    write: &CoverLetterAggregateWrite,
    binding: &GeneratedReservation,
) -> Result<jobniac_core::german::cover_letter_store::ReplaceResult, AppError> {
    use jobniac_core::german::cover_letter_store::GermanCoverLetterRepository;
    repository
        .replace_german_cover_letter(write, binding, revision(0), revision(0), None)
        .await
}

/// Seeds a prepared letter row directly and returns its id.
pub fn insert_letter(
    admin: &Connection,
    resume: &Resume,
    binding: &GeneratedReservation,
) -> Result<Uuid, Box<dyn Error + Send + Sync>> {
    let letter = Uuid::new_v4();
    let variant = Uuid::new_v4();
    admin.execute(
        "INSERT INTO cover_letter_cover_letters (id, profile_id, job_id, resume_id, \
         profile_revision, job_revision, resume_revision, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            letter.to_string(),
            resume.profile_id().to_string(),
            resume.job_id().to_string(),
            resume.id().to_string(),
            stored(now()),
            stored(now()),
            stored(later()),
            stored(now()),
            stored(later())
        ],
    )?;
    admin.execute(
        "INSERT INTO cover_letter_cover_letter_variants (id, cover_letter_id, format, \
         language, document_id, file_path, subject, salutation, closing, signature, \
         created_at, updated_at) \
         VALUES (?1,?2,'germany','de',?3,?4,'Subject','Hello','Regards','Synthetic',?5,?6)",
        params![
            variant.to_string(),
            letter.to_string(),
            binding.document_id.to_string(),
            binding.asset.relative_path(),
            stored(now()),
            stored(later())
        ],
    )?;
    admin.execute(
        "INSERT INTO cover_letter_cover_letter_paragraphs (id, variant_id, display_order, \
         text) VALUES (?1, ?2, 0, 'Synthetic paragraph')",
        params![Uuid::new_v4().to_string(), variant.to_string()],
    )?;
    Ok(letter)
}

/// Attaches an asset's document as the profile photo, which references it.
pub fn attach_photo(
    admin: &Connection,
    profile: Uuid,
    binding: &GeneratedReservation,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    admin.execute(
        "INSERT INTO profile_profile_personal_details \
         (profile_id, photo_document_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![
            profile.to_string(),
            binding.document_id.to_string(),
            stored(now())
        ],
    )?;
    Ok(())
}

/// Every row of every table of [`TABLES`] as text, in a stable order.
pub fn snapshot(admin: &Connection) -> Result<Vec<Vec<String>>, Box<dyn Error + Send + Sync>> {
    let mut result = Vec::new();
    for table in TABLES {
        let mut statement = admin.prepare(&format!("SELECT * FROM {table}"))?;
        let columns = statement.column_count();
        let mut rows = statement.query([])?;
        let mut texts = Vec::new();
        while let Some(row) = rows.next()? {
            let mut cells = Vec::with_capacity(columns);
            for index in 0..columns {
                cells.push(match row.get_ref(index)? {
                    ValueRef::Null => "NULL".to_owned(),
                    ValueRef::Integer(value) => value.to_string(),
                    ValueRef::Real(value) => value.to_string(),
                    ValueRef::Text(bytes) => String::from_utf8_lossy(bytes).into_owned(),
                    ValueRef::Blob(bytes) => format!("blob:{}", sha256_hex(bytes)),
                });
            }
            texts.push(cells.join("|"));
        }
        texts.sort();
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
pub fn asset_state(
    admin: &Connection,
    binding: &GeneratedReservation,
) -> Result<AssetState, Box<dyn Error + Send + Sync>> {
    type Columns = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let (lifecycle, retirement, retired, status, task, next, task_retirement): Columns = admin
        .query_row(
            "SELECT asset.lifecycle, asset.retirement_id, asset.retired_at, task.status, \
         task.task_id, task.next_attempt_at, task.retirement_id \
         FROM jobniac_generated_assets asset \
         LEFT JOIN jobniac_generated_asset_cleanups task ON task.asset_id = asset.asset_id \
         WHERE asset.asset_id = ?1",
            [binding.asset.asset_id().to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )?;
    let id = |text: Option<String>| text.and_then(|text| parse_uuid(&text));
    let instant = |text: Option<String>| text.and_then(|text| parse_instant_text(&text));
    Ok(AssetState {
        lifecycle,
        retirement_id: id(retirement),
        retired_at: instant(retired),
        status,
        task_id: id(task),
        next_attempt_at: instant(next),
        task_retirement_id: id(task_retirement),
    })
}

/// Installs a trigger that fails every `event` (`INSERT`, `UPDATE`, `DELETE`) on `table` for which
/// the SQL `condition` holds, to prove that a failed write rolls the whole scope back.
pub fn fail_at(
    admin: &Connection,
    table: &str,
    event: &str,
    condition: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let condition = if condition.eq_ignore_ascii_case("TRUE") {
        "1"
    } else {
        condition
    };
    admin.execute_batch(&format!(
        "CREATE TRIGGER synthetic_german_failure BEFORE {event} ON {table} FOR EACH ROW \
         WHEN {condition} BEGIN SELECT RAISE(ABORT, 'private synthetic failure'); END"
    ))?;
    Ok(())
}

/// A cleanup repository whose clock reads `at`.
pub fn cleanup_repository(fixture: &Fixture, at: OffsetDateTime) -> SqliteCleanupRepository {
    SqliteCleanupRepository::new(fixture.uow.clone(), Arc::new(FixedClock::new(at)))
}

/// The code and message of the envelope of a refused call, or a test failure when it succeeded.
pub fn refusal<T: std::fmt::Debug>(
    result: Result<T, AppError>,
) -> Result<(ErrorCode, String), Box<dyn Error + Send + Sync>> {
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
