//! PostgreSQL store of the German resume aggregate.
//!
//! A replace is one transaction. Lock order: the pair advisory lock, `profiles` and
//! `jobs` `FOR UPDATE`, the old resume and its variants, the letters of the same pair by id, their
//! variants by id, the removed-asset document and asset locks, then the `generated_assets` rows by
//! asset id. Bindings are verified before any delete; then the old tree and the pair's cover
//! letters are deleted, the new tree is inserted, and the owned assets that nothing references any
//! more are retired last.

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
use tokio_postgres::Row;
use tokio_postgres::types::ToSql;
use uuid::Uuid;

use super::{
    RESUME_APPLICATION_NAME, binding_matches, check_owners, intent_error, lock_pair, removed_parts,
    retire_unreferenced, unavailable,
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
const WHAT: &str = "German resume";

const RESUME_COLUMNS: &str =
    "id, profile_id, job_id, format, profile_revision, job_revision, created_at, updated_at";
const VARIANT_COLUMNS: &str =
    "id, resume_id, language, document_id, file_path, created_at, updated_at";

/// A pool whose sessions carry the `application_name` `jobniac-german-resume`.
#[must_use]
pub fn pool(config: Option<DatabaseConfig>) -> PgPool {
    PgPool::with_application_name(config, RESUME_APPLICATION_NAME)
}

/// The PostgreSQL adapter of [`GermanResumeRepository`]. The clock supplies the admission,
/// retirement and completion instants of a replace.
#[derive(Clone)]
pub struct PgGermanResumeRepository {
    uow: PgUnitOfWork,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for PgGermanResumeRepository {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PgGermanResumeRepository")
            .finish_non_exhaustive()
    }
}

impl PgGermanResumeRepository {
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
impl GermanResumeRepository for PgGermanResumeRepository {
    async fn find_by_id(&self, resume_id: Uuid) -> Result<Option<Resume>, AppError> {
        self.uow
            .transaction(read_only(), async |tx| {
                find_resume(tx, "id = $1", &[&resume_id], false).await
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
                find_resume(
                    tx,
                    "profile_id = $1 AND job_id = $2 AND format = $3",
                    &[&profile_id, &job_id, &format],
                    false,
                )
                .await
            })
            .await
    }

    async fn find_variants(&self, resume_id: Uuid) -> Result<Vec<ResumeVariant>, AppError> {
        self.uow
            .transaction(read_only(), async |tx| variants(tx, resume_id, false).await)
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
    tx: &mut PgTransaction,
    input: &Replace<'_>,
) -> Result<ReplaceResult, AppError> {
    let parent = input.write.resume();
    lock_pair(tx, parent.pair_key()).await?;
    check_owners(
        tx,
        parent.pair_key(),
        input.expected,
        (parent.profile_revision(), parent.job_revision()),
        WHAT,
    )
    .await?;
    let old = find_resume(
        tx,
        "profile_id = $1 AND job_id = $2 AND format = $3",
        &[&parent.profile_id(), &parent.job_id(), &"germany"],
        true,
    )
    .await?;
    let previous = match &old {
        None => Vec::new(),
        Some(old) => variants(tx, old.id(), true).await?,
    };
    // Lock parent rows before capturing their children. Foreign-key writes then cannot introduce
    // another old variant while the replacement is pending.
    let letters = db(tx
        .client()
        .query(
            "SELECT id FROM cover_letter.cover_letters \
             WHERE profile_id = $1 AND job_id = $2 ORDER BY id FOR UPDATE",
            &[&parent.profile_id(), &parent.job_id()],
        )
        .await)?;
    let letter_ids = letters
        .iter()
        .map(|row| col::<Uuid>(row, "id"))
        .collect::<Result<Vec<_>, _>>()?;
    let mut removed: Vec<(Uuid, String)> = previous
        .iter()
        .map(|variant| (variant.document_id(), variant.file_path().to_owned()))
        .collect();
    if !letter_ids.is_empty() {
        let rows = db(tx
            .client()
            .query(
                "SELECT document_id, file_path FROM cover_letter.cover_letter_variants \
                 WHERE cover_letter_id = ANY($1) AND format = 'germany' ORDER BY id FOR UPDATE",
                &[&letter_ids],
            )
            .await)?;
        for row in &rows {
            removed.push((col(row, "document_id")?, col(row, "file_path")?));
        }
    }
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
    if !letter_ids.is_empty() {
        db(tx
            .client()
            .execute(
                "DELETE FROM cover_letter.cover_letters WHERE id = ANY($1)",
                &[&letter_ids],
            )
            .await)?;
    }
    if let Some(old) = &old {
        db(tx
            .client()
            .execute("DELETE FROM resume.resumes WHERE id = $1", &[&old.id()])
            .await)?;
    }
    insert_tree(tx, input.write).await?;
    retire_unreferenced(tx, &candidates, now).await?;
    let saved = find_resume(tx, "id = $1", &[&parent.id()], false)
        .await?
        .ok_or_else(unavailable)?;
    let stored = variants(tx, parent.id(), false).await?;
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

/// Reads a resume row, optionally locking it.
async fn find_resume(
    tx: &mut PgTransaction,
    predicate: &str,
    params: &[&(dyn ToSql + Sync)],
    for_update: bool,
) -> Result<Option<Resume>, AppError> {
    let sql = format!(
        "SELECT {RESUME_COLUMNS} FROM resume.resumes WHERE {predicate}{}",
        if for_update { " FOR UPDATE" } else { "" }
    );
    let row = db(tx.client().query_opt(&sql, params).await)?;
    row.as_ref().map(resume_from_row).transpose()
}

fn resume_from_row(row: &Row) -> Result<Resume, AppError> {
    Resume::new(ResumeFields {
        id: col(row, "id")?,
        profile_id: col(row, "profile_id")?,
        job_id: col(row, "job_id")?,
        format: &col::<String>(row, "format")?,
        profile_revision: col(row, "profile_revision")?,
        job_revision: col(row, "job_revision")?,
        created_at: col(row, "created_at")?,
        updated_at: col(row, "updated_at")?,
    })
    .map_err(|_| AppError::Internal)
}

/// Reads the variants of a resume by language.
async fn variants(
    tx: &mut PgTransaction,
    resume_id: Uuid,
    for_update: bool,
) -> Result<Vec<ResumeVariant>, AppError> {
    let sql = format!(
        "SELECT {VARIANT_COLUMNS} FROM resume.resume_variants WHERE resume_id = $1 \
         ORDER BY language{}",
        if for_update { " FOR UPDATE" } else { "" }
    );
    let rows = db(tx.client().query(&sql, &[&resume_id]).await)?;
    rows.iter().map(variant_from_row).collect()
}

fn variant_from_row(row: &Row) -> Result<ResumeVariant, AppError> {
    ResumeVariant::new(VariantFields {
        id: col(row, "id")?,
        resume_id: col(row, "resume_id")?,
        language: &col::<String>(row, "language")?,
        document_id: col(row, "document_id")?,
        file_path: &col::<String>(row, "file_path")?,
        created_at: col(row, "created_at")?,
        updated_at: col(row, "updated_at")?,
    })
    .map_err(|_| AppError::Internal)
}

/// Inserts the parent, then every variant, section, entry and bullet in encounter order, one table
/// at a time.
async fn insert_tree(tx: &mut PgTransaction, write: &ResumeAggregateWrite) -> Result<(), AppError> {
    let client = tx.client();
    let parent = write.resume();
    let parent_id: Uuid = parent.id();
    db(client
        .execute(
            &format!(
                "INSERT INTO resume.resumes ({RESUME_COLUMNS}) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)"
            ),
            &[
                &parent_id,
                &parent.profile_id(),
                &parent.job_id(),
                &parent.format(),
                &parent.profile_revision(),
                &parent.job_revision(),
                &parent.created_at(),
                &parent.updated_at(),
            ],
        )
        .await)?;
    for variant_write in write.variants() {
        let row = variant_write.variant();
        db(client
            .execute(
                &format!(
                    "INSERT INTO resume.resume_variants ({VARIANT_COLUMNS}) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7)"
                ),
                &[
                    &row.id(),
                    &row.resume_id(),
                    &row.language(),
                    &row.document_id(),
                    &row.file_path(),
                    &row.created_at(),
                    &row.updated_at(),
                ],
            )
            .await)?;
    }
    // One statement per table (UNNEST of parallel arrays) keeps the table-by-table, encounter-order
    // sequence but shortens the window in which the pair lock and the row
    // locks are held from one round trip per row to one per table.
    let sections: Vec<_> = write
        .variants()
        .iter()
        .flat_map(|variant| variant.sections())
        .collect();
    if !sections.is_empty() {
        let rows: Vec<_> = sections.iter().map(|write| write.section()).collect();
        let ids: Vec<Uuid> = rows.iter().map(|row| row.id()).collect();
        let variant_ids: Vec<Uuid> = rows.iter().map(|row| row.variant_id()).collect();
        let types: Vec<&str> = rows.iter().map(|row| row.section_type()).collect();
        let titles: Vec<&str> = rows.iter().map(|row| row.title()).collect();
        let orders: Vec<i32> = rows.iter().map(|row| row.display_order().get()).collect();
        db(client
            .execute(
                "INSERT INTO resume.resume_sections \
                 (id,variant_id,section_type,title,display_order) \
                 SELECT id,variant_id,section_type,title,display_order \
                 FROM unnest($1::uuid[],$2::uuid[],$3::text[],$4::text[],$5::int4[]) \
                 WITH ORDINALITY AS row(id,variant_id,section_type,title,display_order,position) \
                 ORDER BY position",
                &[&ids, &variant_ids, &types, &titles, &orders],
            )
            .await)?;
    }
    let entries: Vec<_> = sections
        .iter()
        .flat_map(|section| section.entries())
        .collect();
    if !entries.is_empty() {
        let rows: Vec<_> = entries.iter().map(|write| write.entry()).collect();
        let ids: Vec<Uuid> = rows.iter().map(|row| row.id()).collect();
        let section_ids: Vec<Uuid> = rows.iter().map(|row| row.section_id()).collect();
        let types: Vec<&str> = rows.iter().map(|row| row.entry_type()).collect();
        let orders: Vec<i32> = rows.iter().map(|row| row.display_order().get()).collect();
        let titles: Vec<Option<&str>> = rows.iter().map(|row| row.title()).collect();
        let organizations: Vec<Option<&str>> = rows.iter().map(|row| row.organization()).collect();
        let locations: Vec<Option<&str>> = rows.iter().map(|row| row.location()).collect();
        let starts: Vec<Option<time::Date>> = rows.iter().map(|row| row.start_date()).collect();
        let ends: Vec<Option<time::Date>> = rows.iter().map(|row| row.end_date()).collect();
        let metadata: Vec<Option<&str>> = rows.iter().map(|row| row.metadata()).collect();
        db(client
            .execute(
                "INSERT INTO resume.resume_entries \
                 (id,section_id,entry_type,display_order,title,organization,location,\
                 start_date,end_date,metadata) \
                 SELECT id,section_id,entry_type,display_order,title,organization,location,\
                 start_date,end_date,metadata \
                 FROM unnest($1::uuid[],$2::uuid[],$3::text[],$4::int4[],$5::text[],$6::text[],\
                 $7::text[],$8::date[],$9::date[],$10::text[]) \
                 WITH ORDINALITY AS row(id,section_id,entry_type,display_order,title,\
                 organization,location,start_date,end_date,metadata,position) \
                 ORDER BY position",
                &[
                    &ids,
                    &section_ids,
                    &types,
                    &orders,
                    &titles,
                    &organizations,
                    &locations,
                    &starts,
                    &ends,
                    &metadata,
                ],
            )
            .await)?;
    }
    let bullets: Vec<_> = entries.iter().flat_map(|entry| entry.bullets()).collect();
    if !bullets.is_empty() {
        let ids: Vec<Uuid> = bullets.iter().map(|row| row.id()).collect();
        let entry_ids: Vec<Uuid> = bullets.iter().map(|row| row.entry_id()).collect();
        let orders: Vec<i32> = bullets
            .iter()
            .map(|row| row.display_order().get())
            .collect();
        let texts: Vec<&str> = bullets.iter().map(|row| row.text()).collect();
        db(client
            .execute(
                "INSERT INTO resume.resume_entry_bullets (id,entry_id,display_order,text) \
                 SELECT id,entry_id,display_order,text \
                 FROM unnest($1::uuid[],$2::uuid[],$3::int4[],$4::text[]) \
                 WITH ORDINALITY AS row(id,entry_id,display_order,text,position) \
                 ORDER BY position",
                &[&ids, &entry_ids, &orders, &texts],
            )
            .await)?;
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
