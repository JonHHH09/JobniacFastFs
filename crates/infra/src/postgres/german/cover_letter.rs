//! PostgreSQL store of the German cover letter.
//!
//! A replace is one transaction. Lock order: the pair advisory lock (shared with the
//! resume writer), `profiles`, `jobs` and the resume `FOR UPDATE`, the existing letter and its
//! variants, the removed-asset locks, then the candidate `generated_assets` rows by asset id.
//! Retirement is last. The letter store never deletes anything from the resume side.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::clock::{Clock, truncate_to_micros};
use jobniac_core::errors::{AppError, ApplicationError};
use jobniac_core::generated_assets::{
    GeneratedReservation, GenerationDescriptor, GenerationPurpose, IntentReservation,
    validate_binding_intents,
};
use jobniac_core::german::cover_letter_store::{
    CoverLetter, CoverLetterAggregateWrite, CoverLetterFields, CoverLetterParagraph,
    CoverLetterVariant, DisplayOrder, GermanCoverLetterRepository, ParagraphFields, ReplaceResult,
    Revision, VariantFields, invalid_cover_letter_input, validate_cover_letter_aggregate,
};
use jobniac_core::transactions::TransactionOptions;
use time::OffsetDateTime;
use tokio_postgres::Row;
use tokio_postgres::types::ToSql;
use uuid::Uuid;

use super::{
    COVER_LETTER_APPLICATION_NAME, binding_matches, check_owners, intent_error, lock_pair,
    removed_parts, retire_unreferenced, source_conflict, unavailable,
};
use crate::postgres::config::DatabaseConfig;
use crate::postgres::generated::bindings::{lock_pdf_bindings, removed_assets, verify_pdf_binding};
use crate::postgres::generated::intents::{
    admit_generation_bindings, complete_generation_bindings,
};
use crate::postgres::pool::PgPool;
use crate::postgres::profiles::{col, db};
use crate::postgres::unit_of_work::{PgTransaction, PgUnitOfWork};

/// The text of the fixed owner messages of this store.
const WHAT: &str = "German cover-letter";

const PARENT_COLUMNS: &str = "id, profile_id, job_id, resume_id, profile_revision, job_revision, \
    resume_revision, created_at, updated_at";
const VARIANT_COLUMNS: &str = "id, cover_letter_id, format, language, document_id, file_path, \
    subject, salutation, closing, signature, created_at, updated_at";

/// A pool whose sessions carry the `application_name` `jobniac-german-cover-letter`.
#[must_use]
pub fn pool(config: Option<DatabaseConfig>) -> PgPool {
    PgPool::with_application_name(config, COVER_LETTER_APPLICATION_NAME)
}

/// The PostgreSQL adapter of [`GermanCoverLetterRepository`]. The clock supplies the admission,
/// retirement and completion instants of a replace.
#[derive(Clone)]
pub struct PgGermanCoverLetterRepository {
    uow: PgUnitOfWork,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for PgGermanCoverLetterRepository {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PgGermanCoverLetterRepository")
            .finish_non_exhaustive()
    }
}

impl PgGermanCoverLetterRepository {
    /// A repository over `uow`, stamping retirements with `clock`. Build the unit of work over a
    /// pool from [`pool`] so the sessions carry the `application_name`.
    #[must_use]
    pub fn new(uow: PgUnitOfWork, clock: Arc<dyn Clock>) -> Self {
        Self { uow, clock }
    }
}

fn read_only() -> TransactionOptions {
    TransactionOptions {
        read_only: true,
        requires_new: false,
    }
}

fn read_write() -> TransactionOptions {
    TransactionOptions {
        read_only: false,
        requires_new: false,
    }
}

#[async_trait]
impl GermanCoverLetterRepository for PgGermanCoverLetterRepository {
    async fn find_by_id(&self, cover_letter_id: Uuid) -> Result<Option<CoverLetter>, AppError> {
        self.uow
            .transaction(read_only(), async |tx| {
                find_letter(tx, "id = $1", &[&cover_letter_id], false).await
            })
            .await
    }

    async fn find_by_profile_job_resume(
        &self,
        profile_id: Uuid,
        job_id: Uuid,
        resume_id: Uuid,
    ) -> Result<Option<CoverLetter>, AppError> {
        self.uow
            .transaction(read_only(), async |tx| {
                find_letter(
                    tx,
                    "profile_id = $1 AND job_id = $2 AND resume_id = $3",
                    &[&profile_id, &job_id, &resume_id],
                    false,
                )
                .await
            })
            .await
    }

    async fn find_variants(
        &self,
        cover_letter_id: Uuid,
    ) -> Result<Vec<CoverLetterVariant>, AppError> {
        self.uow
            .transaction(read_only(), async |tx| {
                variants(tx, cover_letter_id, false).await
            })
            .await
    }

    async fn find_paragraphs(
        &self,
        variant_id: Uuid,
    ) -> Result<Vec<CoverLetterParagraph>, AppError> {
        self.uow
            .transaction(read_only(), async |tx| {
                let rows = db(tx
                    .client()
                    .query(
                        "SELECT id, variant_id, display_order, text \
                         FROM cover_letter.cover_letter_paragraphs \
                         WHERE variant_id = $1 ORDER BY display_order",
                        &[&variant_id],
                    )
                    .await)?;
                rows.iter().map(paragraph_from_row).collect()
            })
            .await
    }

    async fn replace_german_cover_letter(
        &self,
        write: &CoverLetterAggregateWrite,
        binding: &GeneratedReservation,
        expected_profile_revision: Revision,
        expected_job_revision: Revision,
        generation_claim: Option<&IntentReservation>,
    ) -> Result<ReplaceResult, AppError> {
        validate_cover_letter_aggregate(write)?;
        let variant = write.variant().variant();
        if !binding_matches(binding, variant.document_id(), variant.file_path()) {
            return Err(invalid_cover_letter_input().into());
        }
        let claims = generation_claim.map(std::slice::from_ref);
        validate_binding_intents(std::slice::from_ref(binding), claims)
            .map_err(|_| invalid_cover_letter_input())?;
        let descriptors = descriptors(write, generation_claim)?;
        let input = Replace {
            write,
            binding,
            expected: (expected_profile_revision, expected_job_revision),
            claims,
            descriptors: &descriptors,
            clock: self.clock.as_ref(),
        };
        self.uow
            .transaction(read_write(), async |tx| replace_in(tx, &input).await)
            .await
    }
}

/// The inputs of one replace transaction.
struct Replace<'a> {
    write: &'a CoverLetterAggregateWrite,
    binding: &'a GeneratedReservation,
    expected: (Revision, Revision),
    claims: Option<&'a [IntentReservation]>,
    descriptors: &'a [GenerationDescriptor],
    clock: &'a dyn Clock,
}

/// The descriptor the claim must carry (empty without a claim); a claim that differs is invalid.
fn descriptors(
    write: &CoverLetterAggregateWrite,
    claim: Option<&IntentReservation>,
) -> Result<Vec<GenerationDescriptor>, ApplicationError> {
    let Some(claim) = claim else {
        return Ok(Vec::new());
    };
    let parent = write.cover_letter();
    let descriptor = GenerationDescriptor::new(
        claim.intent.descriptor.operation_id,
        GenerationPurpose::GermanCoverLetter,
        Some(parent.profile_id()),
        Some(parent.job_id()),
        Some(parent.resume_id()),
        Some(write.variant().variant().language().to_owned()),
    )
    .map_err(|_| invalid_cover_letter_input())?;
    if claim.intent.descriptor != descriptor {
        return Err(invalid_cover_letter_input());
    }
    Ok(vec![descriptor])
}

/// The replace transaction body.
async fn replace_in(
    tx: &mut PgTransaction,
    input: &Replace<'_>,
) -> Result<ReplaceResult, AppError> {
    let parent = input.write.cover_letter();
    lock_pair(tx, parent.pair_key()).await?;
    check_owners(
        tx,
        parent.pair_key(),
        input.expected,
        (parent.profile_revision(), parent.job_revision()),
        WHAT,
    )
    .await?;
    check_resume(tx, parent).await?;
    let old = find_letter(
        tx,
        "profile_id = $1 AND job_id = $2 AND resume_id = $3",
        &[&parent.profile_id(), &parent.job_id(), &parent.resume_id()],
        true,
    )
    .await?;
    let previous = match &old {
        None => Vec::new(),
        Some(old) => variants(tx, old.id(), true).await?,
    };
    let removed: Vec<(Uuid, String)> = previous
        .iter()
        .map(|row| (row.document_id(), row.file_path().to_owned()))
        .collect();
    let (document_ids, paths) = removed_parts(&removed);
    let removed_documents: Vec<Option<Uuid>> = document_ids.iter().copied().map(Some).collect();
    let invalid = invalid_cover_letter_input;
    let candidates = removed_assets(tx, &document_ids, &paths)
        .await
        .map_err(|error| intent_error(error, invalid))?;
    lock_pdf_bindings(
        tx,
        &candidates,
        std::slice::from_ref(input.binding),
        &removed_documents,
    )
    .await
    .map_err(|error| intent_error(error, invalid))?;
    verify_pdf_binding(tx, input.binding)
        .await
        .map_err(|error| intent_error(error, invalid))?;
    admit_generation_bindings(
        tx,
        std::slice::from_ref(input.binding),
        input.claims,
        input.descriptors,
        truncate_to_micros(input.clock.now()),
    )
    .await
    .map_err(|error| intent_error(error, invalid))?;

    let now = truncate_to_micros(input.clock.now());
    if let Some(old) = &old {
        db(tx
            .client()
            .execute(
                "DELETE FROM cover_letter.cover_letters WHERE id = $1",
                &[&old.id()],
            )
            .await)?;
    }
    insert_tree(tx, input.write).await?;
    retire_unreferenced(tx, &candidates, now).await?;
    let saved = find_letter(tx, "id = $1", &[&parent.id()], false).await?;
    let mut stored = variants(tx, parent.id(), false).await?;
    let (Some(saved), 1) = (saved, stored.len()) else {
        return Err(unavailable());
    };
    complete_generation_bindings(
        tx,
        input.claims,
        input.descriptors,
        truncate_to_micros(input.clock.now()),
    )
    .await
    .map_err(|error| intent_error(error, invalid))?;
    let variant = stored.remove(0);
    Ok(ReplaceResult {
        cover_letter: saved,
        variant,
        previous_variants: previous,
    })
}

/// Locks the resume row and requires it to be the exact German resume the letter was built from
/// (the second half of the source check).
async fn check_resume(tx: &mut PgTransaction, parent: &CoverLetter) -> Result<(), AppError> {
    let resume = db(tx
        .client()
        .query_opt(
            "SELECT profile_id, job_id, format, profile_revision, job_revision, updated_at \
             FROM resume.resumes WHERE id = $1 FOR UPDATE",
            &[&parent.resume_id()],
        )
        .await)?;
    let Some(resume) = resume else {
        return Err(source_conflict(WHAT));
    };
    let profile_id: Uuid = col(&resume, "profile_id")?;
    let job_id: Uuid = col(&resume, "job_id")?;
    let format: String = col(&resume, "format")?;
    let profile_revision: OffsetDateTime = col(&resume, "profile_revision")?;
    let job_revision: OffsetDateTime = col(&resume, "job_revision")?;
    let updated_at: OffsetDateTime = col(&resume, "updated_at")?;
    if profile_id != parent.profile_id()
        || job_id != parent.job_id()
        || format != "germany"
        || updated_at != parent.resume_revision()
        || profile_revision != parent.profile_revision()
        || job_revision != parent.job_revision()
    {
        return Err(source_conflict(WHAT));
    }
    Ok(())
}

/// Reads a letter row, optionally locking it.
async fn find_letter(
    tx: &mut PgTransaction,
    predicate: &str,
    params: &[&(dyn ToSql + Sync)],
    for_update: bool,
) -> Result<Option<CoverLetter>, AppError> {
    let sql = format!(
        "SELECT {PARENT_COLUMNS} FROM cover_letter.cover_letters WHERE {predicate}{}",
        if for_update { " FOR UPDATE" } else { "" }
    );
    let row = db(tx.client().query_opt(&sql, params).await)?;
    row.as_ref().map(letter_from_row).transpose()
}

fn letter_from_row(row: &Row) -> Result<CoverLetter, AppError> {
    CoverLetter::new(CoverLetterFields {
        id: col(row, "id")?,
        profile_id: col(row, "profile_id")?,
        job_id: col(row, "job_id")?,
        resume_id: col(row, "resume_id")?,
        profile_revision: col(row, "profile_revision")?,
        job_revision: col(row, "job_revision")?,
        resume_revision: col(row, "resume_revision")?,
        created_at: col(row, "created_at")?,
        updated_at: col(row, "updated_at")?,
    })
    .map_err(|_| AppError::Internal)
}

/// Reads the variants of a letter by format and language.
async fn variants(
    tx: &mut PgTransaction,
    cover_letter_id: Uuid,
    for_update: bool,
) -> Result<Vec<CoverLetterVariant>, AppError> {
    let sql = format!(
        "SELECT {VARIANT_COLUMNS} FROM cover_letter.cover_letter_variants \
         WHERE cover_letter_id = $1 ORDER BY format, language{}",
        if for_update { " FOR UPDATE" } else { "" }
    );
    let rows = db(tx.client().query(&sql, &[&cover_letter_id]).await)?;
    rows.iter().map(variant_from_row).collect()
}

fn variant_from_row(row: &Row) -> Result<CoverLetterVariant, AppError> {
    CoverLetterVariant::new(VariantFields {
        id: col(row, "id")?,
        cover_letter_id: col(row, "cover_letter_id")?,
        format: &col::<String>(row, "format")?,
        language: &col::<String>(row, "language")?,
        document_id: col(row, "document_id")?,
        file_path: &col::<String>(row, "file_path")?,
        subject: &col::<String>(row, "subject")?,
        salutation: &col::<String>(row, "salutation")?,
        closing: &col::<String>(row, "closing")?,
        signature: &col::<String>(row, "signature")?,
        created_at: col(row, "created_at")?,
        updated_at: col(row, "updated_at")?,
    })
    .map_err(|_| AppError::Internal)
}

fn paragraph_from_row(row: &Row) -> Result<CoverLetterParagraph, AppError> {
    let order: i32 = col(row, "display_order")?;
    CoverLetterParagraph::new(ParagraphFields {
        id: col(row, "id")?,
        variant_id: col(row, "variant_id")?,
        display_order: DisplayOrder::new(i64::from(order)).ok_or(AppError::Internal)?,
        text: &col::<String>(row, "text")?,
    })
    .map_err(|_| AppError::Internal)
}

/// Inserts the parent, the variant and the paragraphs in encounter order.
async fn insert_tree(
    tx: &mut PgTransaction,
    write: &CoverLetterAggregateWrite,
) -> Result<(), AppError> {
    let client = tx.client();
    let parent = write.cover_letter();
    db(client
        .execute(
            &format!(
                "INSERT INTO cover_letter.cover_letters ({PARENT_COLUMNS}) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"
            ),
            &[
                &parent.id(),
                &parent.profile_id(),
                &parent.job_id(),
                &parent.resume_id(),
                &parent.profile_revision(),
                &parent.job_revision(),
                &parent.resume_revision(),
                &parent.created_at(),
                &parent.updated_at(),
            ],
        )
        .await)?;
    let variant = write.variant().variant();
    db(client
        .execute(
            &format!(
                "INSERT INTO cover_letter.cover_letter_variants ({VARIANT_COLUMNS}) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)"
            ),
            &[
                &variant.id(),
                &variant.cover_letter_id(),
                &variant.format(),
                &variant.language(),
                &variant.document_id(),
                &variant.file_path(),
                &variant.subject(),
                &variant.salutation(),
                &variant.closing(),
                &variant.signature(),
                &variant.created_at(),
                &variant.updated_at(),
            ],
        )
        .await)?;
    let paragraphs = write.variant().paragraphs();
    if !paragraphs.is_empty() {
        // One statement for the whole paragraph table (UNNEST, encounter order) instead of one
        // round trip per paragraph while the pair lock and the row locks are held.
        let ids: Vec<Uuid> = paragraphs.iter().map(|row| row.id()).collect();
        let variant_ids: Vec<Uuid> = paragraphs.iter().map(|row| row.variant_id()).collect();
        let orders: Vec<i32> = paragraphs
            .iter()
            .map(|row| row.display_order().get())
            .collect();
        let texts: Vec<&str> = paragraphs.iter().map(|row| row.text()).collect();
        db(client
            .execute(
                "INSERT INTO cover_letter.cover_letter_paragraphs \
                 (id,variant_id,display_order,text) \
                 SELECT id,variant_id,display_order,text \
                 FROM unnest($1::uuid[],$2::uuid[],$3::int4[],$4::text[]) \
                 WITH ORDINALITY AS row(id,variant_id,display_order,text,position) \
                 ORDER BY position",
                &[&ids, &variant_ids, &orders, &texts],
            )
            .await)?;
    }
    Ok(())
}
