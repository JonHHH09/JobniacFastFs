//! The job aggregate in PostgreSQL.
//!
//! Reads run in a read-only scope, which begins at REPEATABLE READ (see the unit of work), so the
//! job row, its skills and its provenance come from one snapshot. Repositories never set the
//! isolation level themselves. Writes run at READ COMMITTED (the read-write scope default).
//!
//! The text create takes the advisory lock of the text hash, then looks the hash and the canonical
//! fingerprint up under `FOR SHARE`. The job insert uses `ON CONFLICT ON CONSTRAINT
//! jobs_canonical_fingerprint_unique DO NOTHING`, so a concurrent fingerprint duplicate yields the
//! existing job instead of an error. The update is a compare-and-set on the revision.

use jobniac_core::clock::{format_instant, parse_instant};
use jobniac_core::errors::AppError;
use jobniac_core::job_links::urls::validate_persisted_job_urls;
use jobniac_core::jobs::{
    JobAggregate, JobRecord, JobSkillRecord, LinkIngestionRecord, TextIngestionRecord,
};
use serde_json::{Map, Value};
use time::OffsetDateTime;
use tokio_postgres::{Client, Row};
use uuid::Uuid;

use super::terms::job_search_terms;
use super::{invalid_shape, job_col, non_blank};
use crate::postgres::profiles::{db, map_rows, unavailable, weight_value};

/// Selects the columns of one job.
pub(crate) const JOB_SELECT_SQL: &str = r#"
SELECT id, source_method AS "sourceMethod", source_label AS "sourceLabel", title, company,
       location, description,
       experience_requirement AS "experienceRequirement",
       employment_type AS "employmentType", seniority, posted_at AS "postedAt",
       canonical_fingerprint AS "canonicalFingerprint", created_at AS "createdAt",
       updated_at AS "updatedAt", revision
FROM job_schema.jobs
WHERE id = $1
"#;

/// Selects the columns of several jobs with `WHERE id = ANY(...)`, the batch form of the search
/// hydration.
pub(crate) const JOB_SELECT_BATCH_SQL: &str = r#"
SELECT id, source_method AS "sourceMethod", source_label AS "sourceLabel", title, company,
       location, description,
       experience_requirement AS "experienceRequirement",
       employment_type AS "employmentType", seniority, posted_at AS "postedAt",
       canonical_fingerprint AS "canonicalFingerprint", created_at AS "createdAt",
       updated_at AS "updatedAt", revision
FROM job_schema.jobs
WHERE id = ANY($1::uuid[])
"#;

/// Skills of one job.
pub(crate) const SKILLS_SQL: &str = r#"
        SELECT id, job_id AS "jobId", skill, normalized_skill AS "normalizedSkill", required,
               display_order AS "displayOrder", created_at AS "createdAt"
        FROM job_schema.job_skills
        WHERE job_id = $1
        ORDER BY display_order, skill, id
        "#;

/// The batch skills of the search hydration.
pub(crate) const SKILLS_BATCH_SQL: &str = r#"
SELECT id, job_id AS "jobId", skill, normalized_skill AS "normalizedSkill", required,
       display_order AS "displayOrder", created_at AS "createdAt"
FROM job_schema.job_skills
WHERE job_id = ANY($1::uuid[])
ORDER BY job_id, display_order, skill, id
"#;

/// Link provenance of one job.
const LINKS_SQL: &str = r#"
        SELECT id, job_id AS "jobId", url, normalized_url AS "normalizedUrl",
               fetched_at AS "fetchedAt", http_status AS "httpStatus",
               source_title AS "sourceTitle", created_at AS "createdAt"
        FROM job_schema.job_link_ingestions
        WHERE job_id = $1
        "#;

/// Text provenance of one job.
const TEXTS_SQL: &str = r#"
        SELECT id, job_id AS "jobId", source_label AS "sourceLabel",
               input_text_hash AS "inputTextHash", created_at AS "createdAt"
        FROM job_schema.job_text_ingestions
        WHERE job_id = $1
        "#;

/// The job that owns a text hash.
const TEXT_OWNER_SQL: &str = "SELECT job_id FROM job_schema.job_text_ingestions \
     WHERE input_text_hash = $1";

/// The job row under a shared lock.
const SHARE_JOB_SQL: &str = "SELECT id FROM job_schema.jobs WHERE id = $1 FOR SHARE";

/// The job with a canonical fingerprint, without a lock.
const FINGERPRINT_SQL: &str = "SELECT id FROM job_schema.jobs WHERE canonical_fingerprint = $1";

/// The job with a canonical fingerprint, under a shared lock.
const FINGERPRINT_SHARE_SQL: &str = concat!(
    "SELECT id FROM job_schema.jobs WHERE canonical_fingerprint = $1",
    " FOR SHARE"
);

/// The advisory lock of the text hash.
const ADVISORY_LOCK_SQL: &str = "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))";

/// The job row, with the fingerprint conflict left to the caller.
const INSERT_JOB_SQL: &str = r#"
        INSERT INTO job_schema.jobs
            (id, source_method, source_label, title, company, location, description,
             experience_requirement, employment_type, seniority, posted_at,
             canonical_fingerprint, created_at, updated_at, revision)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        ON CONFLICT ON CONSTRAINT jobs_canonical_fingerprint_unique DO NOTHING
        RETURNING id
        "#;

/// One skill row.
const INSERT_SKILL_SQL: &str = r#"
            INSERT INTO job_schema.job_skills
                (id, job_id, skill, normalized_skill, required, display_order, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#;

/// Every search posting of a job in one statement. The columns are parallel arrays, so the rows
/// land in posting order and a duplicate or non-positive posting still fails the same constraint as
/// the per-row insert did.
const INSERT_TERMS_SQL: &str = r#"
            INSERT INTO job_schema.search_terms (job_id, field_key, term, weight)
            SELECT job_id, field_key, term, weight
            FROM unnest($1::uuid[], $2::text[], $3::text[], $4::smallint[])
                 WITH ORDINALITY AS t(job_id, field_key, term, weight, ord)
            ORDER BY ord
            "#;

/// The text provenance row.
const INSERT_TEXT_SQL: &str = r#"
        INSERT INTO job_schema.job_text_ingestions
            (id, job_id, source_label, input_text_hash, created_at)
        VALUES ($1, $2, $3, $4, $5)
        "#;

/// The compare-and-set on the revision. The literal is the signed
/// 64-bit maximum, so an exhausted revision never matches.
const UPDATE_JOB_SQL: &str = r#"
        UPDATE job_schema.jobs
        SET source_label = $1, title = $2, company = $3, location = $4, description = $5,
            experience_requirement = $6, employment_type = $7, seniority = $8, posted_at = $9,
            canonical_fingerprint = $10, updated_at = $11, revision = revision + 1
        WHERE id = $12 AND revision = $13 AND revision < 9223372036854775807
        RETURNING id
        "#;

/// The skills replaced by an update.
const DELETE_SKILLS_SQL: &str = "DELETE FROM job_schema.job_skills WHERE job_id = $1";

/// The postings replaced by an update.
const DELETE_TERMS_SQL: &str = "DELETE FROM job_schema.search_terms WHERE job_id = $1";

/// Reads a job by identifier, or `None` when it is missing.
pub(crate) async fn get_job(
    client: &Client,
    job_id: Uuid,
) -> Result<Option<JobAggregate>, AppError> {
    load_aggregate(client, job_id).await
}

/// The job that owns a text hash, with its aggregate.
pub(crate) async fn find_by_text_hash(
    client: &Client,
    input_text_hash: &str,
) -> Result<Option<JobAggregate>, AppError> {
    match text_owner(client, input_text_hash).await? {
        None => Ok(None),
        Some(job_id) => load_aggregate(client, job_id).await,
    }
}

/// The job whose canonical fingerprint is `fingerprint`, with its aggregate.
pub(crate) async fn find_by_canonical_fingerprint(
    client: &Client,
    fingerprint: &str,
) -> Result<Option<JobAggregate>, AppError> {
    find_by_fingerprint(client, fingerprint, false).await
}

/// Paste-first create. Returns the stored aggregate and `true`, or the
/// job that already has the text hash or the canonical fingerprint and `false`.
pub(crate) async fn create_text_job(
    client: &Client,
    graph: &JobAggregate,
) -> Result<(JobAggregate, bool), AppError> {
    let job = &graph.job;
    let text = graph.text_ingestion.as_ref().ok_or_else(invalid_shape)?;
    if graph.link_ingestion.is_some() || job.source_method != "text" {
        return Err(invalid_shape());
    }
    if text.job_id != job.id || graph.skills.iter().any(|skill| skill.job_id != job.id) {
        return Err(invalid_shape());
    }
    let input_text_hash = non_blank(&text.input_text_hash)?;
    let fingerprint = non_blank(&job.canonical_fingerprint)?;

    db(client.query(ADVISORY_LOCK_SQL, &[&input_text_hash]).await)?;
    if let Some(existing) = find_by_text_hash_locked(client, input_text_hash).await? {
        return Ok((existing, false));
    }
    if let Some(existing) = find_by_fingerprint(client, fingerprint, true).await? {
        return Ok((existing, false));
    }
    if !insert_children(client, graph).await? {
        let existing = find_by_fingerprint(client, fingerprint, true)
            .await?
            .ok_or_else(unavailable)?;
        return Ok((existing, false));
    }
    db(client
        .execute(
            INSERT_TEXT_SQL,
            &[
                &text.id,
                &job.id,
                &text.source_label,
                &input_text_hash,
                &text.created_at,
            ],
        )
        .await)?;
    let persisted = load_aggregate(client, job.id)
        .await?
        .ok_or_else(unavailable)?;
    Ok((persisted, true))
}

/// The job that owns a canonical source URL, without a lock.
const LINK_OWNER_SQL: &str =
    "SELECT job_id FROM job_schema.job_link_ingestions WHERE normalized_url = $1";

/// The link provenance row.
const INSERT_LINK_SQL: &str = r#"
        INSERT INTO job_schema.job_link_ingestions
            (id, job_id, url, normalized_url, fetched_at, http_status, source_title, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#;

/// The link provenance columns of a link-sourced aggregate, after the link checks. The checks run
/// before any SQL, so a forged graph never reaches the database.
struct LinkColumns {
    url: String,
    normalized_url: String,
    fingerprint: String,
    http_status: Option<i32>,
    source_title: Option<String>,
    fetched_at: OffsetDateTime,
    created_at: OffsetDateTime,
}

/// The link checks before any SQL: source and provenance shape, the persisted URL pair
/// (`validate_persisted_job_urls`, which never echoes a URL), the status range and the title type.
fn checked_link(graph: &JobAggregate) -> Result<LinkColumns, AppError> {
    let job = &graph.job;
    let link = graph.link_ingestion.as_ref().ok_or_else(invalid_shape)?;
    if graph.text_ingestion.is_some() || job.source_method != "link" {
        return Err(invalid_shape());
    }
    if link.job_id != job.id || graph.skills.iter().any(|skill| skill.job_id != job.id) {
        return Err(invalid_shape());
    }
    let url = non_blank(
        link.fields
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(invalid_shape)?,
    )?;
    let normalized_url = non_blank(&link.normalized_url)?;
    validate_persisted_job_urls(url, normalized_url).map_err(AppError::from)?;
    let http_status = match link.fields.get("httpStatus") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => {
            let status = number
                .as_i64()
                .filter(|status| (100..=599).contains(status))
                .ok_or_else(invalid_shape)?;
            Some(i32::try_from(status).map_err(|_| invalid_shape())?)
        }
        Some(_) => return Err(invalid_shape()),
    };
    let source_title = match link.fields.get("sourceTitle") {
        None | Some(Value::Null) => None,
        Some(Value::String(title)) => Some(title.clone()),
        Some(_) => return Err(invalid_shape()),
    };
    Ok(LinkColumns {
        url: url.to_owned(),
        normalized_url: normalized_url.to_owned(),
        fingerprint: non_blank(&job.canonical_fingerprint)?.to_owned(),
        http_status,
        source_title,
        fetched_at: instant_text(link.fields.get("fetchedAt"))?,
        created_at: instant_text(link.fields.get("createdAt"))?,
    })
}

/// An instant stored as its `Instant.toString()` text in a provenance field.
fn instant_text(value: Option<&Value>) -> Result<OffsetDateTime, AppError> {
    let Some(Value::String(text)) = value else {
        return Err(invalid_shape());
    };
    parse_instant(text).map_err(|_| invalid_shape())
}

/// The link-create checks without a connection: the shape of a link-sourced aggregate and its
/// provenance, in that order. Refusals carry no value text.
///
/// # Errors
///
/// The persistence-constraint shape error, or the `Invalid job link provenance` validation error
/// for a display and canonical URL pair that does not match.
pub fn check_link_graph(graph: &JobAggregate) -> Result<(), AppError> {
    checked_link(graph).map(|_| ())
}

/// The job that owns `normalized_url`, without a lock.
async fn link_owner(client: &Client, normalized_url: &str) -> Result<Option<Uuid>, AppError> {
    let rows = db(client.query(LINK_OWNER_SQL, &[&normalized_url]).await)?;
    rows.first()
        .map(|row| job_col::<Uuid>(row, "job_id"))
        .transpose()
}

/// The job that owns a canonical source URL, under a shared lock.
async fn link_owner_locked(
    client: &Client,
    normalized_url: &str,
) -> Result<Option<JobAggregate>, AppError> {
    let Some(job_id) = link_owner(client, normalized_url).await? else {
        return Ok(None);
    };
    let locked = db(client.query(SHARE_JOB_SQL, &[&job_id]).await)?;
    if locked.is_empty() {
        return Ok(None);
    }
    load_aggregate(client, job_id).await
}

/// The job that owns a canonical source URL, with its aggregate.
pub(crate) async fn find_by_normalized_url(
    client: &Client,
    normalized_url: &str,
) -> Result<Option<JobAggregate>, AppError> {
    match link_owner(client, normalized_url).await? {
        None => Ok(None),
        Some(job_id) => load_aggregate(client, job_id).await,
    }
}

/// Link create. The advisory lock is taken on `job-link:` plus the
/// canonical URL, then the URL and the fingerprint are looked up under `FOR SHARE`. Returns the
/// stored aggregate and `true`, or the job that already has the URL or the fingerprint and `false`.
pub(crate) async fn create_link_job(
    client: &Client,
    graph: &JobAggregate,
) -> Result<(JobAggregate, bool), AppError> {
    let columns = checked_link(graph)?;
    let link = graph.link_ingestion.as_ref().ok_or_else(invalid_shape)?;
    let job = &graph.job;
    let lock_key = format!("job-link:{}", columns.normalized_url);
    db(client.query(ADVISORY_LOCK_SQL, &[&lock_key]).await)?;
    if let Some(existing) = link_owner_locked(client, &columns.normalized_url).await? {
        return Ok((existing, false));
    }
    if let Some(existing) = find_by_fingerprint(client, &columns.fingerprint, true).await? {
        return Ok((existing, false));
    }
    if !insert_children(client, graph).await? {
        let existing = find_by_fingerprint(client, &columns.fingerprint, true)
            .await?
            .ok_or_else(unavailable)?;
        return Ok((existing, false));
    }
    db(client
        .execute(
            INSERT_LINK_SQL,
            &[
                &link.id,
                &job.id,
                &columns.url,
                &columns.normalized_url,
                &columns.fetched_at,
                &columns.http_status,
                &columns.source_title,
                &columns.created_at,
            ],
        )
        .await)?;
    let persisted = load_aggregate(client, job.id)
        .await?
        .ok_or_else(unavailable)?;
    Ok((persisted, true))
}

/// Compare-and-set update of a job and its children. Returns the
/// stored aggregate, or `None` when the stored revision is not `expected_revision`, with nothing
/// written. The skills and the postings are replaced in the same transaction as the job row.
pub(crate) async fn update_job_aggregate(
    client: &Client,
    graph: &JobAggregate,
    expected_revision: i64,
) -> Result<Option<JobAggregate>, AppError> {
    let job = &graph.job;
    if graph.skills.iter().any(|skill| skill.job_id != job.id) {
        return Err(invalid_shape());
    }
    let title = non_blank(&job.title)?;
    let description = non_blank(&job.description)?;
    let fingerprint = non_blank(&job.canonical_fingerprint)?;
    let updated = db(client
        .query_opt(
            UPDATE_JOB_SQL,
            &[
                &job.source_label,
                &title,
                &job.company,
                &job.location,
                &description,
                &job.experience_requirement,
                &job.employment_type,
                &job.seniority,
                &job.posted_at,
                &fingerprint,
                &job.updated_at,
                &job.id,
                &expected_revision,
            ],
        )
        .await)?;
    if updated.is_none() {
        return Ok(None);
    }
    db(client.execute(DELETE_SKILLS_SQL, &[&job.id]).await)?;
    insert_skills(client, &graph.skills).await?;
    db(client.execute(DELETE_TERMS_SQL, &[&job.id]).await)?;
    insert_terms(client, graph).await?;
    load_aggregate(client, job.id)
        .await?
        .ok_or_else(unavailable)
        .map(Some)
}

/// Loads the job row, its skills and its single ingestion record. A job
/// whose provenance does not match its source method fails closed as unavailable.
pub(crate) async fn load_aggregate(
    client: &Client,
    job_id: Uuid,
) -> Result<Option<JobAggregate>, AppError> {
    let rows = db(client.query(JOB_SELECT_SQL, &[&job_id]).await)?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let job = map_one(row, job_from_row)?;
    let skill_rows = db(client.query(SKILLS_SQL, &[&job_id]).await)?;
    let skills = map_rows(&skill_rows, |row| map_one(row, skill_from_row))?;
    let link_rows = db(client.query(LINKS_SQL, &[&job_id]).await)?;
    let links = map_rows(&link_rows, |row| map_one(row, link_from_row))?;
    let text_rows = db(client.query(TEXTS_SQL, &[&job_id]).await)?;
    let texts = map_rows(&text_rows, |row| map_one(row, text_from_row))?;
    let has_link = !links.is_empty();
    if links.len() + texts.len() != 1 || (job.source_method == "link") != has_link {
        return Err(unavailable());
    }
    Ok(Some(JobAggregate {
        job,
        skills,
        link_ingestion: links.into_iter().next(),
        text_ingestion: texts.into_iter().next(),
    }))
}

/// Decodes one row with `decode`, under the write and read policy of `_run`.
fn map_one<T>(
    row: &Row,
    decode: impl Fn(&Row) -> Result<T, tokio_postgres::Error>,
) -> Result<T, AppError> {
    decode(row).map_err(|_| invalid_shape())
}

/// The job that owns `input_text_hash`, without a lock.
async fn text_owner(client: &Client, input_text_hash: &str) -> Result<Option<Uuid>, AppError> {
    let rows = db(client.query(TEXT_OWNER_SQL, &[&input_text_hash]).await)?;
    rows.first()
        .map(|row| job_col::<Uuid>(row, "job_id"))
        .transpose()
}

/// The job that owns a text hash, under a shared lock.
async fn find_by_text_hash_locked(
    client: &Client,
    input_text_hash: &str,
) -> Result<Option<JobAggregate>, AppError> {
    let Some(job_id) = text_owner(client, input_text_hash).await? else {
        return Ok(None);
    };
    let locked = db(client.query(SHARE_JOB_SQL, &[&job_id]).await)?;
    if locked.is_empty() {
        return Ok(None);
    }
    load_aggregate(client, job_id).await
}

/// The job with a canonical fingerprint, with the shared lock when `lock` is set.
async fn find_by_fingerprint(
    client: &Client,
    fingerprint: &str,
    lock: bool,
) -> Result<Option<JobAggregate>, AppError> {
    let sql = if lock {
        FINGERPRINT_SHARE_SQL
    } else {
        FINGERPRINT_SQL
    };
    let rows = db(client.query(sql, &[&fingerprint]).await)?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let job_id: Uuid = job_col(row, "id")?;
    load_aggregate(client, job_id).await
}

/// Inserts the job row, then its skills and postings. Returns `false` when
/// the fingerprint conflict left the row unwritten (nothing else is written then).
async fn insert_children(client: &Client, graph: &JobAggregate) -> Result<bool, AppError> {
    let job = &graph.job;
    let title = non_blank(&job.title)?;
    let description = non_blank(&job.description)?;
    let fingerprint = non_blank(&job.canonical_fingerprint)?;
    let inserted = db(client
        .query_opt(
            INSERT_JOB_SQL,
            &[
                &job.id,
                &job.source_method,
                &job.source_label,
                &title,
                &job.company,
                &job.location,
                &description,
                &job.experience_requirement,
                &job.employment_type,
                &job.seniority,
                &job.posted_at,
                &fingerprint,
                &job.created_at,
                &job.updated_at,
                &job.revision,
            ],
        )
        .await)?;
    if inserted.is_none() {
        return Ok(false);
    }
    insert_skills(client, &graph.skills).await?;
    insert_terms(client, graph).await?;
    Ok(true)
}

/// The skill rows of a job, in display order.
async fn insert_skills(client: &Client, skills: &[JobSkillRecord]) -> Result<(), AppError> {
    if skills.is_empty() {
        return Ok(());
    }
    // Prepared once: the per-row `execute(&str)` would re-parse the statement for every skill.
    let statement = db(client.prepare(INSERT_SKILL_SQL).await)?;
    for skill in skills {
        let skill_text = non_blank(&skill.skill)?;
        let normalized = non_blank(&skill.normalized_skill)?;
        let display_order = display_order_value(skill.display_order)?;
        db(client
            .execute(
                &statement,
                &[
                    &skill.id,
                    &skill.job_id,
                    &skill_text,
                    &normalized,
                    &skill.required,
                    &display_order,
                    &skill.created_at,
                ],
            )
            .await)?;
    }
    Ok(())
}

/// The postings of a job: its text fields and skills.
async fn insert_terms(client: &Client, graph: &JobAggregate) -> Result<(), AppError> {
    let terms = job_search_terms(&graph.job, &graph.skills);
    if terms.is_empty() {
        return Ok(());
    }
    let mut ids = Vec::with_capacity(terms.len());
    let mut keys = Vec::with_capacity(terms.len());
    let mut texts = Vec::with_capacity(terms.len());
    let mut weights = Vec::with_capacity(terms.len());
    for term in terms {
        weights.push(weight_value(term.weight)?);
        ids.push(term.entity_id);
        keys.push(term.field_key);
        texts.push(term.term);
    }
    db(client
        .execute(INSERT_TERMS_SQL, &[&ids, &keys, &texts, &weights])
        .await)?;
    Ok(())
}

/// Narrows a display order to the `integer` column. An out-of-range
/// `integer` is a data error, not an integrity error, so it is database-unavailable.
fn display_order_value(value: i64) -> Result<i32, AppError> {
    i32::try_from(value).map_err(|_| unavailable())
}

/// One job row, decoded by its camelCase aliases.
pub(crate) fn job_from_row(row: &Row) -> Result<JobRecord, tokio_postgres::Error> {
    Ok(JobRecord {
        id: row.try_get("id")?,
        source_method: row.try_get("sourceMethod")?,
        source_label: row.try_get("sourceLabel")?,
        title: row.try_get("title")?,
        company: row.try_get("company")?,
        location: row.try_get("location")?,
        description: row.try_get("description")?,
        experience_requirement: row.try_get("experienceRequirement")?,
        employment_type: row.try_get("employmentType")?,
        seniority: row.try_get("seniority")?,
        posted_at: row.try_get("postedAt")?,
        canonical_fingerprint: row.try_get("canonicalFingerprint")?,
        created_at: row.try_get("createdAt")?,
        updated_at: row.try_get("updatedAt")?,
        revision: row.try_get("revision")?,
    })
}

/// One skill row, decoded by its camelCase aliases.
pub(crate) fn skill_from_row(row: &Row) -> Result<JobSkillRecord, tokio_postgres::Error> {
    let display_order: i32 = row.try_get("displayOrder")?;
    Ok(JobSkillRecord {
        id: row.try_get("id")?,
        job_id: row.try_get("jobId")?,
        skill: row.try_get("skill")?,
        normalized_skill: row.try_get("normalizedSkill")?,
        required: row.try_get("required")?,
        display_order: i64::from(display_order),
        created_at: row.try_get("createdAt")?,
    })
}

/// One link provenance row. Only the identity and the normalized URL are typed; the other columns
/// travel in `fields` under their column aliases.
fn link_from_row(row: &Row) -> Result<LinkIngestionRecord, tokio_postgres::Error> {
    let url: String = row.try_get("url")?;
    let fetched_at: OffsetDateTime = row.try_get("fetchedAt")?;
    let http_status: Option<i32> = row.try_get("httpStatus")?;
    let source_title: Option<String> = row.try_get("sourceTitle")?;
    let created_at: OffsetDateTime = row.try_get("createdAt")?;
    let mut fields = Map::new();
    fields.insert("url".to_owned(), Value::String(url));
    fields.insert(
        "fetchedAt".to_owned(),
        Value::String(format_instant(fetched_at)),
    );
    fields.insert(
        "httpStatus".to_owned(),
        http_status.map_or(Value::Null, Value::from),
    );
    fields.insert(
        "sourceTitle".to_owned(),
        source_title.map_or(Value::Null, Value::String),
    );
    fields.insert(
        "createdAt".to_owned(),
        Value::String(format_instant(created_at)),
    );
    Ok(LinkIngestionRecord {
        id: row.try_get("id")?,
        job_id: row.try_get("jobId")?,
        normalized_url: row.try_get("normalizedUrl")?,
        fields,
    })
}

/// One text provenance row, decoded by its camelCase aliases.
fn text_from_row(row: &Row) -> Result<TextIngestionRecord, tokio_postgres::Error> {
    Ok(TextIngestionRecord {
        id: row.try_get("id")?,
        job_id: row.try_get("jobId")?,
        source_label: row.try_get("sourceLabel")?,
        input_text_hash: row.try_get("inputTextHash")?,
        created_at: row.try_get("createdAt")?,
    })
}
