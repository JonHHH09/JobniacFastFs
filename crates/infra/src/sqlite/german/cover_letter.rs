//! SQLite store of the German cover letter.
//!
//! A replace is one `BEGIN IMMEDIATE` transaction: the owner and resume revisions are compared,
//! the existing letter and its variants are read, the binding is verified, the old letter is
//! deleted, the new tree is inserted, and retirement is last. The letter store never deletes
//! anything from the resume side.

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
use rusqlite::{Connection, ToSql};
use uuid::Uuid;

use super::{
    binding_matches, check_owners, intent_error, removed_parts, retire_unreferenced,
    source_conflict, unavailable,
};
use crate::sqlite::codec::{Id, Instant};
use crate::sqlite::generated::bindings::{lock_pdf_bindings, removed_assets, verify_pdf_binding};
use crate::sqlite::generated::guards::guard_document_path_reference;
use crate::sqlite::generated::intents::{admit_generation_bindings, complete_generation_bindings};
use crate::sqlite::profiles::{col, execute, query_rows, with_connection};
use crate::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};

/// The text of the fixed owner messages of this store.
const WHAT: &str = "German cover-letter";

const PARENT_COLUMNS: &str = "id, profile_id, job_id, resume_id, profile_revision, job_revision, \
    resume_revision, created_at, updated_at";
const VARIANT_COLUMNS: &str = "id, cover_letter_id, format, language, document_id, file_path, \
    subject, salutation, closing, signature, created_at, updated_at";

/// The SQLite adapter of [`GermanCoverLetterRepository`]. The clock supplies the admission,
/// retirement and completion instants of a replace.
#[derive(Clone)]
pub struct SqliteGermanCoverLetterRepository {
    uow: SqliteUnitOfWork,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for SqliteGermanCoverLetterRepository {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteGermanCoverLetterRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteGermanCoverLetterRepository {
    /// A repository over `uow`, stamping retirements with `clock`.
    #[must_use]
    pub fn new(uow: SqliteUnitOfWork, clock: Arc<dyn Clock>) -> Self {
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
impl GermanCoverLetterRepository for SqliteGermanCoverLetterRepository {
    async fn find_by_id(&self, cover_letter_id: Uuid) -> Result<Option<CoverLetter>, AppError> {
        self.uow
            .transaction(read_only(), async |tx| {
                with_connection(tx, |conn| {
                    find_letter(conn, "id = ?1", &[&Id(cover_letter_id)])
                })
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
                with_connection(tx, |conn| {
                    find_letter(
                        conn,
                        "profile_id = ?1 AND job_id = ?2 AND resume_id = ?3",
                        &[&Id(profile_id), &Id(job_id), &Id(resume_id)],
                    )
                })
            })
            .await
    }

    async fn find_variants(
        &self,
        cover_letter_id: Uuid,
    ) -> Result<Vec<CoverLetterVariant>, AppError> {
        self.uow
            .transaction(read_only(), async |tx| {
                with_connection(tx, |conn| variants(conn, cover_letter_id))
            })
            .await
    }

    async fn find_paragraphs(
        &self,
        variant_id: Uuid,
    ) -> Result<Vec<CoverLetterParagraph>, AppError> {
        self.uow
            .transaction(read_only(), async |tx| {
                with_connection(tx, |conn| {
                    query_rows(
                        conn,
                        "SELECT id, variant_id, display_order, text \
                         FROM cover_letter_cover_letter_paragraphs \
                         WHERE variant_id = ?1 ORDER BY display_order",
                        &[&Id(variant_id)],
                        paragraph_from_row,
                    )
                })
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
    tx: &mut SqliteTransaction,
    input: &Replace<'_>,
) -> Result<ReplaceResult, AppError> {
    let parent = input.write.cover_letter();
    check_owners(
        tx,
        parent.pair_key(),
        input.expected,
        (parent.profile_revision(), parent.job_revision()),
        WHAT,
    )?;
    with_connection(tx, |conn| check_resume(conn, parent))?;
    let (old, previous) = with_connection(tx, |conn| {
        let old = find_letter(
            conn,
            "profile_id = ?1 AND job_id = ?2 AND resume_id = ?3",
            &[
                &Id(parent.profile_id()),
                &Id(parent.job_id()),
                &Id(parent.resume_id()),
            ],
        )?;
        let previous = match &old {
            None => Vec::new(),
            Some(old) => variants(conn, old.id())?,
        };
        Ok((old, previous))
    })?;
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
    with_connection(tx, |conn| {
        if let Some(old) = &old {
            execute(
                conn,
                "DELETE FROM cover_letter_cover_letters WHERE id = ?1",
                &[&Id(old.id())],
            )?;
        }
        insert_tree(conn, input.write)
    })?;
    retire_unreferenced(tx, &candidates, now).await?;
    let (saved, mut stored) = with_connection(tx, |conn| {
        let saved = find_letter(conn, "id = ?1", &[&Id(parent.id())])?;
        Ok((saved, variants(conn, parent.id())?))
    })?;
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

/// Requires the resume row to be the exact German resume the letter was built from (the second
/// half of the source check).
fn check_resume(conn: &Connection, parent: &CoverLetter) -> Result<(), AppError> {
    let resumes = query_rows(
        conn,
        "SELECT profile_id, job_id, format, profile_revision, job_revision, updated_at \
         FROM resume_resumes WHERE id = ?1",
        &[&Id(parent.resume_id())],
        |row| {
            Ok((
                col::<Id>(row, "profile_id")?.0,
                col::<Id>(row, "job_id")?.0,
                col::<String>(row, "format")?,
                col::<Instant>(row, "profile_revision")?.0,
                col::<Instant>(row, "job_revision")?.0,
                col::<Instant>(row, "updated_at")?.0,
            ))
        },
    )?;
    let Some((profile_id, job_id, format, profile_revision, job_revision, updated_at)) =
        resumes.into_iter().next()
    else {
        return Err(source_conflict(WHAT));
    };
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

/// Reads a letter row.
fn find_letter(
    conn: &Connection,
    predicate: &str,
    params: &[&dyn ToSql],
) -> Result<Option<CoverLetter>, AppError> {
    let sql = format!("SELECT {PARENT_COLUMNS} FROM cover_letter_cover_letters WHERE {predicate}");
    query_rows(conn, &sql, params, letter_from_row).map(|mut found| found.pop())
}

fn letter_from_row(row: &rusqlite::Row<'_>) -> Result<CoverLetter, AppError> {
    CoverLetter::new(CoverLetterFields {
        id: col::<Id>(row, "id")?.0,
        profile_id: col::<Id>(row, "profile_id")?.0,
        job_id: col::<Id>(row, "job_id")?.0,
        resume_id: col::<Id>(row, "resume_id")?.0,
        profile_revision: col::<Instant>(row, "profile_revision")?.0,
        job_revision: col::<Instant>(row, "job_revision")?.0,
        resume_revision: col::<Instant>(row, "resume_revision")?.0,
        created_at: col::<Instant>(row, "created_at")?.0,
        updated_at: col::<Instant>(row, "updated_at")?.0,
    })
    .map_err(|_| AppError::Internal)
}

/// Reads the variants of a letter by format and language.
fn variants(conn: &Connection, cover_letter_id: Uuid) -> Result<Vec<CoverLetterVariant>, AppError> {
    let sql = format!(
        "SELECT {VARIANT_COLUMNS} FROM cover_letter_cover_letter_variants \
         WHERE cover_letter_id = ?1 ORDER BY format, language"
    );
    query_rows(conn, &sql, &[&Id(cover_letter_id)], variant_from_row)
}

fn variant_from_row(row: &rusqlite::Row<'_>) -> Result<CoverLetterVariant, AppError> {
    CoverLetterVariant::new(VariantFields {
        id: col::<Id>(row, "id")?.0,
        cover_letter_id: col::<Id>(row, "cover_letter_id")?.0,
        format: &col::<String>(row, "format")?,
        language: &col::<String>(row, "language")?,
        document_id: col::<Id>(row, "document_id")?.0,
        file_path: &col::<String>(row, "file_path")?,
        subject: &col::<String>(row, "subject")?,
        salutation: &col::<String>(row, "salutation")?,
        closing: &col::<String>(row, "closing")?,
        signature: &col::<String>(row, "signature")?,
        created_at: col::<Instant>(row, "created_at")?.0,
        updated_at: col::<Instant>(row, "updated_at")?.0,
    })
    .map_err(|_| AppError::Internal)
}

fn paragraph_from_row(row: &rusqlite::Row<'_>) -> Result<CoverLetterParagraph, AppError> {
    let order: i32 = col(row, "display_order")?;
    CoverLetterParagraph::new(ParagraphFields {
        id: col::<Id>(row, "id")?.0,
        variant_id: col::<Id>(row, "variant_id")?.0,
        display_order: DisplayOrder::new(i64::from(order)).ok_or(AppError::Internal)?,
        text: &col::<String>(row, "text")?,
    })
    .map_err(|_| AppError::Internal)
}

/// Inserts the parent, the variant and the paragraphs in encounter order. The variant passes the
/// owned-document path guard first (rule 14).
fn insert_tree(conn: &Connection, write: &CoverLetterAggregateWrite) -> Result<(), AppError> {
    let parent = write.cover_letter();
    execute(
        conn,
        &format!(
            "INSERT INTO cover_letter_cover_letters ({PARENT_COLUMNS}) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)"
        ),
        &[
            &Id(parent.id()),
            &Id(parent.profile_id()),
            &Id(parent.job_id()),
            &Id(parent.resume_id()),
            &Instant(parent.profile_revision()),
            &Instant(parent.job_revision()),
            &Instant(parent.resume_revision()),
            &Instant(parent.created_at()),
            &Instant(parent.updated_at()),
        ],
    )?;
    let variant = write.variant().variant();
    guard_document_path_reference(conn, variant.document_id(), variant.file_path())?;
    execute(
        conn,
        &format!(
            "INSERT INTO cover_letter_cover_letter_variants ({VARIANT_COLUMNS}) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)"
        ),
        &[
            &Id(variant.id()),
            &Id(variant.cover_letter_id()),
            &variant.format(),
            &variant.language(),
            &Id(variant.document_id()),
            &variant.file_path(),
            &variant.subject(),
            &variant.salutation(),
            &variant.closing(),
            &variant.signature(),
            &Instant(variant.created_at()),
            &Instant(variant.updated_at()),
        ],
    )?;
    for paragraph in write.variant().paragraphs() {
        execute(
            conn,
            "INSERT INTO cover_letter_cover_letter_paragraphs (id,variant_id,display_order,text) \
             VALUES (?1,?2,?3,?4)",
            &[
                &Id(paragraph.id()),
                &Id(paragraph.variant_id()),
                &paragraph.display_order().get(),
                &paragraph.text(),
            ],
        )?;
    }
    Ok(())
}
