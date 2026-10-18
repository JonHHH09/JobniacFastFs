//! SQLite store of the German resume aggregate.
//!
//! A replace is one `BEGIN IMMEDIATE` transaction: the owner revisions are compared, the old
//! resume and its variants and the letters of the same pair are read, the bindings are verified
//! before any delete, then the old tree and the pair's cover letters are deleted, the new tree is
//! inserted, and the owned assets that nothing references any more are retired last.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::clock::{Clock, truncate_to_micros};
use jobniac_core::errors::{AppError, ApplicationError};
use jobniac_core::generated_assets::{
    GeneratedReservation, GenerationDescriptor, GenerationPurpose, IntentReservation,
    validate_binding_intents,
};
use jobniac_core::german::resume_store::{
    GermanResumeRepository, ReplaceResult, Resume, ResumeAggregateWrite, ResumeFields,
    ResumeVariant, Revision, VariantFields, invalid_resume_input, validate_resume_aggregate,
};
use jobniac_core::transactions::TransactionOptions;
use jobniac_text::java::{BudgetField, PreparationBudget, java_strip};
use rusqlite::{Connection, ToSql};
use uuid::Uuid;

use super::{
    binding_matches, check_owners, intent_error, removed_parts, retire_unreferenced, unavailable,
};
use crate::sqlite::codec::{Day, Id, Instant};
use crate::sqlite::generated::bindings::{lock_pdf_bindings, removed_assets, verify_pdf_binding};
use crate::sqlite::generated::guards::guard_document_path_reference;
use crate::sqlite::generated::intents::{admit_generation_bindings, complete_generation_bindings};
use crate::sqlite::profiles::{col, execute, query_rows, with_connection};
use crate::sqlite::unit_of_work::{SqliteTransaction, SqliteUnitOfWork};

/// The text of the fixed owner messages of this store.
const WHAT: &str = "German resume";

const RESUME_COLUMNS: &str =
    "id, profile_id, job_id, format, profile_revision, job_revision, created_at, updated_at";
const VARIANT_COLUMNS: &str =
    "id, resume_id, language, document_id, file_path, created_at, updated_at";

/// The SQLite adapter of [`GermanResumeRepository`]. The clock supplies the admission, retirement
/// and completion instants of a replace.
#[derive(Clone)]
pub struct SqliteGermanResumeRepository {
    uow: SqliteUnitOfWork,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for SqliteGermanResumeRepository {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteGermanResumeRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteGermanResumeRepository {
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

/// Normalizes the format of a read: only `germany` after strip and lowercase.
fn normalize_format(value: &str) -> Result<String, ApplicationError> {
    PreparationBudget::new(BudgetField::Profile).add_text(Some(value))?;
    let normalized = java_strip(value).to_lowercase();
    if normalized == "germany" {
        Ok(normalized)
    } else {
        Err(invalid_resume_input())
    }
}

#[async_trait]
impl GermanResumeRepository for SqliteGermanResumeRepository {
    async fn find_by_id(&self, resume_id: Uuid) -> Result<Option<Resume>, AppError> {
        self.uow
            .transaction(read_only(), async |tx| {
                with_connection(tx, |conn| find_resume(conn, "id = ?1", &[&Id(resume_id)]))
            })
            .await
    }

    async fn find_by_profile_job_format(
        &self,
        profile_id: Uuid,
        job_id: Uuid,
        format: &str,
    ) -> Result<Option<Resume>, AppError> {
        let format = normalize_format(format)?;
        self.uow
            .transaction(read_only(), async |tx| {
                with_connection(tx, |conn| {
                    find_resume(
                        conn,
                        "profile_id = ?1 AND job_id = ?2 AND format = ?3",
                        &[&Id(profile_id), &Id(job_id), &format],
                    )
                })
            })
            .await
    }

    async fn find_variants(&self, resume_id: Uuid) -> Result<Vec<ResumeVariant>, AppError> {
        self.uow
            .transaction(read_only(), async |tx| {
                with_connection(tx, |conn| variants(conn, resume_id))
            })
            .await
    }

    async fn replace_germany_resume(
        &self,
        write: &ResumeAggregateWrite,
        bindings: &[GeneratedReservation],
        expected_profile_revision: Revision,
        expected_job_revision: Revision,
        generation_claims: Option<&[IntentReservation]>,
    ) -> Result<ReplaceResult, AppError> {
        validate_resume_aggregate(write)?;
        validate_bindings(write, bindings)?;
        validate_binding_intents(bindings, generation_claims)
            .map_err(|_| invalid_resume_input())?;
        let descriptors = descriptors(write, generation_claims)?;
        let input = Replace {
            write,
            bindings,
            expected: (expected_profile_revision, expected_job_revision),
            claims: generation_claims,
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
    write: &'a ResumeAggregateWrite,
    bindings: &'a [GeneratedReservation],
    expected: (Revision, Revision),
    claims: Option<&'a [IntentReservation]>,
    descriptors: &'a [GenerationDescriptor],
    clock: &'a dyn Clock,
}

/// Checks one binding per variant: the same document and path, no asset used twice.
fn validate_bindings(
    write: &ResumeAggregateWrite,
    bindings: &[GeneratedReservation],
) -> Result<(), ApplicationError> {
    if bindings.len() != write.variants().len() {
        return Err(invalid_resume_input());
    }
    let mut assets = HashSet::new();
    for (variant_write, binding) in write.variants().iter().zip(bindings) {
        let variant = variant_write.variant();
        if !binding_matches(binding, variant.document_id(), variant.file_path())
            || !assets.insert(binding.asset.asset_id())
        {
            return Err(invalid_resume_input());
        }
    }
    Ok(())
}

/// The descriptor each claim must carry, one per variant in order; a claim that differs is invalid.
fn descriptors(
    write: &ResumeAggregateWrite,
    claims: Option<&[IntentReservation]>,
) -> Result<Vec<GenerationDescriptor>, ApplicationError> {
    let Some(claims) = claims else {
        return Ok(Vec::new());
    };
    let parent = write.resume();
    let mut expected = Vec::with_capacity(claims.len());
    for (claim, variant_write) in claims.iter().zip(write.variants()) {
        let descriptor = GenerationDescriptor::new(
            claim.intent.descriptor.operation_id,
            GenerationPurpose::GermanResume,
            Some(parent.profile_id()),
            Some(parent.job_id()),
            Some(parent.id()),
            Some(variant_write.variant().language().to_owned()),
        )
        .map_err(|_| invalid_resume_input())?;
        if claim.intent.descriptor != descriptor {
            return Err(invalid_resume_input());
        }
        expected.push(descriptor);
    }
    Ok(expected)
}

/// The replace transaction body.
async fn replace_in(
    tx: &mut SqliteTransaction,
    input: &Replace<'_>,
) -> Result<ReplaceResult, AppError> {
    let parent = input.write.resume();
    check_owners(
        tx,
        parent.pair_key(),
        input.expected,
        (parent.profile_revision(), parent.job_revision()),
        WHAT,
    )?;
    let (old, previous, letter_ids, removed) = with_connection(tx, |conn| {
        let old = find_resume(
            conn,
            "profile_id = ?1 AND job_id = ?2 AND format = ?3",
            &[
                &Id(parent.profile_id()),
                &Id(parent.job_id()),
                &"germany".to_owned(),
            ],
        )?;
        let previous = match &old {
            None => Vec::new(),
            Some(old) => variants(conn, old.id())?,
        };
        let letter_ids = query_rows(
            conn,
            "SELECT id FROM cover_letter_cover_letters \
             WHERE profile_id = ?1 AND job_id = ?2 ORDER BY id",
            &[&Id(parent.profile_id()), &Id(parent.job_id())],
            |row| Ok(col::<Id>(row, "id")?.0),
        )?;
        let mut removed: Vec<(Uuid, String)> = previous
            .iter()
            .map(|variant| (variant.document_id(), variant.file_path().to_owned()))
            .collect();
        for letter_id in &letter_ids {
            removed.extend(query_rows(
                conn,
                "SELECT document_id, file_path FROM cover_letter_cover_letter_variants \
                 WHERE cover_letter_id = ?1 AND format = 'germany' ORDER BY id",
                &[&Id(*letter_id)],
                |row| Ok((col::<Id>(row, "document_id")?.0, col(row, "file_path")?)),
            )?);
        }
        Ok((old, previous, letter_ids, removed))
    })?;
    let (document_ids, paths) = removed_parts(&removed);
    let removed_documents: Vec<Option<Uuid>> = document_ids.iter().copied().map(Some).collect();
    let invalid = invalid_resume_input;
    let candidates = removed_assets(tx, &document_ids, &paths)
        .await
        .map_err(|error| intent_error(error, invalid))?;
    lock_pdf_bindings(tx, &candidates, input.bindings, &removed_documents)
        .await
        .map_err(|error| intent_error(error, invalid))?;
    for binding in input.bindings {
        verify_pdf_binding(tx, binding)
            .await
            .map_err(|error| intent_error(error, invalid))?;
    }
    admit_generation_bindings(
        tx,
        input.bindings,
        input.claims,
        input.descriptors,
        truncate_to_micros(input.clock.now()),
    )
    .await
    .map_err(|error| intent_error(error, invalid))?;

    let now = truncate_to_micros(input.clock.now());
    with_connection(tx, |conn| {
        for letter_id in &letter_ids {
            execute(
                conn,
                "DELETE FROM cover_letter_cover_letters WHERE id = ?1",
                &[&Id(*letter_id)],
            )?;
        }
        if let Some(old) = &old {
            execute(
                conn,
                "DELETE FROM resume_resumes WHERE id = ?1",
                &[&Id(old.id())],
            )?;
        }
        insert_tree(conn, input.write)
    })?;
    retire_unreferenced(tx, &candidates, now).await?;
    let (saved, stored) = with_connection(tx, |conn| {
        let saved = find_resume(conn, "id = ?1", &[&Id(parent.id())])?;
        Ok((saved, variants(conn, parent.id())?))
    })?;
    let saved = saved.ok_or_else(unavailable)?;
    complete_generation_bindings(
        tx,
        input.claims,
        input.descriptors,
        truncate_to_micros(input.clock.now()),
    )
    .await
    .map_err(|error| intent_error(error, invalid))?;
    let mut ordered = Vec::with_capacity(input.write.variants().len());
    for variant_write in input.write.variants() {
        let id = variant_write.variant().id();
        ordered.push(
            stored
                .iter()
                .find(|variant| variant.id() == id)
                .cloned()
                .ok_or(AppError::Internal)?,
        );
    }
    Ok(ReplaceResult {
        resume: saved,
        variants: ordered,
        previous_variants: previous,
    })
}

/// Reads a resume row.
fn find_resume(
    conn: &Connection,
    predicate: &str,
    params: &[&dyn ToSql],
) -> Result<Option<Resume>, AppError> {
    let sql = format!("SELECT {RESUME_COLUMNS} FROM resume_resumes WHERE {predicate}");
    query_rows(conn, &sql, params, resume_from_row).map(|mut found| found.pop())
}

fn resume_from_row(row: &rusqlite::Row<'_>) -> Result<Resume, AppError> {
    Resume::new(ResumeFields {
        id: col::<Id>(row, "id")?.0,
        profile_id: col::<Id>(row, "profile_id")?.0,
        job_id: col::<Id>(row, "job_id")?.0,
        format: &col::<String>(row, "format")?,
        profile_revision: col::<Instant>(row, "profile_revision")?.0,
        job_revision: col::<Instant>(row, "job_revision")?.0,
        created_at: col::<Instant>(row, "created_at")?.0,
        updated_at: col::<Instant>(row, "updated_at")?.0,
    })
    .map_err(|_| AppError::Internal)
}

/// Reads the variants of a resume by language.
fn variants(conn: &Connection, resume_id: Uuid) -> Result<Vec<ResumeVariant>, AppError> {
    let sql = format!(
        "SELECT {VARIANT_COLUMNS} FROM resume_resume_variants WHERE resume_id = ?1 \
         ORDER BY language"
    );
    query_rows(conn, &sql, &[&Id(resume_id)], variant_from_row)
}

fn variant_from_row(row: &rusqlite::Row<'_>) -> Result<ResumeVariant, AppError> {
    ResumeVariant::new(VariantFields {
        id: col::<Id>(row, "id")?.0,
        resume_id: col::<Id>(row, "resume_id")?.0,
        language: &col::<String>(row, "language")?,
        document_id: col::<Id>(row, "document_id")?.0,
        file_path: &col::<String>(row, "file_path")?,
        created_at: col::<Instant>(row, "created_at")?.0,
        updated_at: col::<Instant>(row, "updated_at")?.0,
    })
    .map_err(|_| AppError::Internal)
}

/// Inserts the parent, then every variant, section, entry and bullet in encounter order, one table
/// at a time. Each variant passes the owned-document path guard first (rule 14).
fn insert_tree(conn: &Connection, write: &ResumeAggregateWrite) -> Result<(), AppError> {
    let parent = write.resume();
    execute(
        conn,
        &format!("INSERT INTO resume_resumes ({RESUME_COLUMNS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)"),
        &[
            &Id(parent.id()),
            &Id(parent.profile_id()),
            &Id(parent.job_id()),
            &parent.format(),
            &Instant(parent.profile_revision()),
            &Instant(parent.job_revision()),
            &Instant(parent.created_at()),
            &Instant(parent.updated_at()),
        ],
    )?;
    for variant_write in write.variants() {
        let row = variant_write.variant();
        guard_document_path_reference(conn, row.document_id(), row.file_path())?;
        execute(
            conn,
            &format!(
                "INSERT INTO resume_resume_variants ({VARIANT_COLUMNS}) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7)"
            ),
            &[
                &Id(row.id()),
                &Id(row.resume_id()),
                &row.language(),
                &Id(row.document_id()),
                &row.file_path(),
                &Instant(row.created_at()),
                &Instant(row.updated_at()),
            ],
        )?;
    }
    let sections: Vec<_> = write
        .variants()
        .iter()
        .flat_map(|variant| variant.sections())
        .collect();
    for write in &sections {
        let row = write.section();
        execute(
            conn,
            "INSERT INTO resume_resume_sections (id,variant_id,section_type,title,display_order) \
             VALUES (?1,?2,?3,?4,?5)",
            &[
                &Id(row.id()),
                &Id(row.variant_id()),
                &row.section_type(),
                &row.title(),
                &row.display_order().get(),
            ],
        )?;
    }
    let entries: Vec<_> = sections
        .iter()
        .flat_map(|section| section.entries())
        .collect();
    for write in &entries {
        let row = write.entry();
        execute(
            conn,
            "INSERT INTO resume_resume_entries \
             (id,section_id,entry_type,display_order,title,organization,location,\
              start_date,end_date,metadata) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            &[
                &Id(row.id()),
                &Id(row.section_id()),
                &row.entry_type(),
                &row.display_order().get(),
                &row.title(),
                &row.organization(),
                &row.location(),
                &row.start_date().map(Day),
                &row.end_date().map(Day),
                &row.metadata(),
            ],
        )?;
    }
    for bullet in entries.iter().flat_map(|entry| entry.bullets()) {
        execute(
            conn,
            "INSERT INTO resume_resume_entry_bullets (id,entry_id,display_order,text) \
             VALUES (?1,?2,?3,?4)",
            &[
                &Id(bullet.id()),
                &Id(bullet.entry_id()),
                &bullet.display_order().get(),
                &bullet.text(),
            ],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_format_is_stripped_and_lowercased() {
        assert_eq!(
            normalize_format(" GERMANY ").as_deref().ok(),
            Some("germany")
        );
        assert_eq!(normalize_format("germany").as_deref().ok(), Some("germany"));
    }

    #[test]
    fn read_format_other_than_germany_is_invalid() {
        for format in ["", "france", "german y", "\u{0}germany"] {
            assert_eq!(
                normalize_format(format),
                Err(invalid_resume_input()),
                "{format:?}"
            );
        }
    }
}
