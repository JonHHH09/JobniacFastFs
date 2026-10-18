//! The job aggregate in SQLite.
//!
//! Reads run in a read-only scope, which is one WAL snapshot (see the unit of work), so the job
//! row, its skills and its provenance come from one snapshot. Repositories never begin a
//! transaction themselves. Writes run under `BEGIN IMMEDIATE`, so a create reads the text hash and
//! the canonical fingerprint and then writes with no other writer in between.
//!
//! The job insert uses `ON CONFLICT (canonical_fingerprint) DO NOTHING`, so a fingerprint
//! duplicate yields the existing job instead of an error. The update is a compare-and-set on the
//! revision. The provenance rule of a job (exactly one provenance row, of its source method) is a
//! pre-commit check of the transaction.

use jobniac_core::errors::AppError;
use jobniac_core::job_links::urls::validate_persisted_job_urls;
use jobniac_core::jobs::{
    JobAggregate, JobRecord, JobSkillRecord, LinkIngestionRecord, TextIngestionRecord,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::{Map, Value};
use time::OffsetDateTime;
use uuid::Uuid;

use super::terms::job_search_terms;
use super::{db, invalid_shape, non_blank, run, unavailable};
use crate::sqlite::codec::{
    instant_text, optional_instant_text, parse_instant_text, parse_uuid, uuid_text,
};
use crate::sqlite::error::SqliteError;
use crate::sqlite::unit_of_work::{PreCommitCheck, SqliteTransaction};

/// Selects the columns of one job, in the order of [`read_job`].
pub(crate) const JOB_SELECT_SQL: &str = r#"
SELECT id, source_method, source_label, title, company, location, description,
       experience_requirement, employment_type, seniority, posted_at, canonical_fingerprint,
       created_at, updated_at, revision
FROM job_jobs
WHERE id = ?1
"#;

/// Selects the columns of several jobs with `json_each`, the batch form of the search hydration.
pub(crate) const JOB_SELECT_BATCH_SQL: &str = r#"
SELECT id, source_method, source_label, title, company, location, description,
       experience_requirement, employment_type, seniority, posted_at, canonical_fingerprint,
       created_at, updated_at, revision
FROM job_jobs
WHERE id IN (SELECT value FROM json_each(?1))
"#;

/// Skills of one job, in the order of [`read_skill`].
pub(crate) const SKILLS_SQL: &str = r#"
SELECT id, job_id, skill, normalized_skill, required, display_order, created_at
FROM job_job_skills
WHERE job_id = ?1
ORDER BY display_order, skill, id
"#;

/// The batch skills of the search hydration.
pub(crate) const SKILLS_BATCH_SQL: &str = r#"
SELECT id, job_id, skill, normalized_skill, required, display_order, created_at
FROM job_job_skills
WHERE job_id IN (SELECT value FROM json_each(?1))
ORDER BY job_id, display_order, skill, id
"#;

/// Link provenance of one job.
const LINKS_SQL: &str = r#"
SELECT id, job_id, url, normalized_url, fetched_at, http_status, source_title, created_at
FROM job_job_link_ingestions
WHERE job_id = ?1
"#;

/// Text provenance of one job.
const TEXTS_SQL: &str = r#"
SELECT id, job_id, source_label, input_text_hash, created_at
FROM job_job_text_ingestions
WHERE job_id = ?1
"#;

/// The job that owns a text hash.
const TEXT_OWNER_SQL: &str =
    "SELECT job_id FROM job_job_text_ingestions WHERE input_text_hash = ?1";

/// The job with a canonical fingerprint.
const FINGERPRINT_SQL: &str = "SELECT id FROM job_jobs WHERE canonical_fingerprint = ?1";

/// The job row, with the fingerprint conflict left to the caller.
const INSERT_JOB_SQL: &str = r#"
INSERT INTO job_jobs
    (id, source_method, source_label, title, company, location, description,
     experience_requirement, employment_type, seniority, posted_at,
     canonical_fingerprint, created_at, updated_at, revision)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
ON CONFLICT (canonical_fingerprint) DO NOTHING
RETURNING id
"#;

/// One skill row.
const INSERT_SKILL_SQL: &str = r#"
INSERT INTO job_job_skills
    (id, job_id, skill, normalized_skill, required, display_order, created_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
"#;

/// One search posting row.
const INSERT_TERM_SQL: &str =
    "INSERT INTO job_search_terms (job_id, field_key, term, weight) VALUES (?1, ?2, ?3, ?4)";

/// The text provenance row.
const INSERT_TEXT_SQL: &str = r#"
INSERT INTO job_job_text_ingestions (id, job_id, source_label, input_text_hash, created_at)
VALUES (?1, ?2, ?3, ?4, ?5)
"#;

/// The compare-and-set on the revision. The literal is the signed
/// 64-bit maximum, so an exhausted revision never matches.
const UPDATE_JOB_SQL: &str = r#"
UPDATE job_jobs
SET source_label = ?1, title = ?2, company = ?3, location = ?4, description = ?5,
    experience_requirement = ?6, employment_type = ?7, seniority = ?8, posted_at = ?9,
    canonical_fingerprint = ?10, updated_at = ?11, revision = revision + 1
WHERE id = ?12 AND revision = ?13 AND revision < 9223372036854775807
RETURNING id
"#;

/// The skills replaced by an update.
const DELETE_SKILLS_SQL: &str = "DELETE FROM job_job_skills WHERE job_id = ?1";

/// The postings replaced by an update.
const DELETE_TERMS_SQL: &str = "DELETE FROM job_search_terms WHERE job_id = ?1";

/// Reads a job by identifier, or `None` when it is missing.
pub(crate) fn get_job(
    tx: &SqliteTransaction,
    job_id: Uuid,
) -> Result<Option<JobAggregate>, AppError> {
    run(tx, |connection| load_aggregate(connection, job_id))
}

/// The job that owns a text hash, with its aggregate.
pub(crate) fn find_by_text_hash(
    tx: &SqliteTransaction,
    input_text_hash: &str,
) -> Result<Option<JobAggregate>, AppError> {
    run(tx, |connection| {
        match text_owner(connection, input_text_hash)? {
            None => Ok(None),
            Some(job_id) => load_aggregate(connection, job_id),
        }
    })
}

/// The job whose canonical fingerprint is `fingerprint`, with its aggregate.
pub(crate) fn find_by_canonical_fingerprint(
    tx: &SqliteTransaction,
    fingerprint: &str,
) -> Result<Option<JobAggregate>, AppError> {
    run(tx, |connection| {
        find_by_fingerprint(connection, fingerprint)
    })
}

/// Paste-first create. Returns the stored aggregate and `true`, or the
/// job that already has the text hash or the canonical fingerprint and `false`.
pub(crate) fn create_text_job(
    tx: &mut SqliteTransaction,
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

    let outcome = run(tx, |connection| {
        if let Some(job_id) = text_owner(connection, input_text_hash)?
            && let Some(existing) = load_aggregate(connection, job_id)?
        {
            return Ok((existing, false));
        }
        if let Some(existing) = find_by_fingerprint(connection, fingerprint)? {
            return Ok((existing, false));
        }
        if !insert_children(connection, graph)? {
            let existing = find_by_fingerprint(connection, fingerprint)?.ok_or_else(unavailable)?;
            return Ok((existing, false));
        }
        db(connection.execute(
            INSERT_TEXT_SQL,
            params![
                uuid_text(text.id),
                uuid_text(job.id),
                text.source_label,
                input_text_hash,
                instant_text(text.created_at),
            ],
        ))?;
        let persisted = load_aggregate(connection, job.id)?.ok_or_else(unavailable)?;
        Ok((persisted, true))
    })?;
    if outcome.1 {
        tx.before_commit(provenance_check(job.id));
    }
    Ok(outcome)
}

/// The job that owns a canonical source URL.
const LINK_OWNER_SQL: &str = "SELECT job_id FROM job_job_link_ingestions WHERE normalized_url = ?1";

/// The link provenance row.
const INSERT_LINK_SQL: &str = r#"
INSERT INTO job_job_link_ingestions
    (id, job_id, url, normalized_url, fetched_at, http_status, source_title, created_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
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
        fetched_at: instant_text_field(link.fields.get("fetchedAt"))?,
        created_at: instant_text_field(link.fields.get("createdAt"))?,
    })
}

/// An instant stored as its `Instant.toString()` text in a provenance field.
fn instant_text_field(value: Option<&Value>) -> Result<OffsetDateTime, AppError> {
    let Some(Value::String(text)) = value else {
        return Err(invalid_shape());
    };
    parse_instant_text(text).ok_or_else(invalid_shape)
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

/// Whether a URL starts with `http://` or `https://` (any case) followed by userinfo, the rule of
/// the PostgreSQL pattern `^https?://[^/?#]*@`. SQLite has no regular expressions, so the two
/// `safe_identity` and `safe_display` checks of the link table run here.
fn has_userinfo(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    authority.contains('@')
}

/// The job that owns `normalized_url`.
fn link_owner(connection: &Connection, normalized_url: &str) -> Result<Option<Uuid>, AppError> {
    let owner: Option<String> = db(connection
        .query_row(LINK_OWNER_SQL, [normalized_url], |row| row.get(0))
        .optional())?;
    owner
        .map(|text| parse_uuid(&text).ok_or_else(invalid_shape))
        .transpose()
}

/// The job that owns a canonical source URL, with its aggregate.
pub(crate) fn find_by_normalized_url(
    tx: &SqliteTransaction,
    normalized_url: &str,
) -> Result<Option<JobAggregate>, AppError> {
    run(tx, |connection| {
        match link_owner(connection, normalized_url)? {
            None => Ok(None),
            Some(job_id) => load_aggregate(connection, job_id),
        }
    })
}

/// Link create. The URL and the fingerprint are looked up first. Returns the
/// stored aggregate and `true`, or the job that already has the URL or the fingerprint and `false`.
pub(crate) fn create_link_job(
    tx: &mut SqliteTransaction,
    graph: &JobAggregate,
) -> Result<(JobAggregate, bool), AppError> {
    let columns = checked_link(graph)?;
    let link = graph.link_ingestion.as_ref().ok_or_else(invalid_shape)?;
    let job = &graph.job;
    let outcome = run(tx, |connection| {
        if let Some(job_id) = link_owner(connection, &columns.normalized_url)?
            && let Some(existing) = load_aggregate(connection, job_id)?
        {
            return Ok((existing, false));
        }
        if let Some(existing) = find_by_fingerprint(connection, &columns.fingerprint)? {
            return Ok((existing, false));
        }
        if !insert_children(connection, graph)? {
            let existing =
                find_by_fingerprint(connection, &columns.fingerprint)?.ok_or_else(unavailable)?;
            return Ok((existing, false));
        }
        if has_userinfo(&columns.normalized_url) {
            return Err(constraint_error(
                "job_link_ingestions_normalized_url_safe_identity",
            ));
        }
        if has_userinfo(&columns.url) {
            return Err(constraint_error("job_link_ingestions_url_safe_display"));
        }
        db(connection.execute(
            INSERT_LINK_SQL,
            params![
                uuid_text(link.id),
                uuid_text(job.id),
                columns.url,
                columns.normalized_url,
                instant_text(columns.fetched_at),
                columns.http_status,
                columns.source_title,
                instant_text(columns.created_at),
            ],
        ))?;
        let persisted = load_aggregate(connection, job.id)?.ok_or_else(unavailable)?;
        Ok((persisted, true))
    })?;
    if outcome.1 {
        tx.before_commit(provenance_check(job.id));
    }
    Ok(outcome)
}

/// A check-violation error of a rule that the SQL schema cannot express.
fn constraint_error(name: &str) -> AppError {
    crate::sqlite::error::details_app_error(SqliteError::constraint(name))
}

/// Compare-and-set update of a job and its children. Returns the
/// stored aggregate, or `None` when the stored revision is not `expected_revision`, with nothing
/// written. The skills and the postings are replaced in the same transaction as the job row.
pub(crate) fn update_job_aggregate(
    tx: &SqliteTransaction,
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
    run(tx, |connection| {
        let updated: Option<String> = db(connection
            .query_row(
                UPDATE_JOB_SQL,
                params![
                    job.source_label,
                    title,
                    job.company,
                    job.location,
                    description,
                    job.experience_requirement,
                    job.employment_type,
                    job.seniority,
                    optional_instant_text(job.posted_at),
                    fingerprint,
                    instant_text(job.updated_at),
                    uuid_text(job.id),
                    expected_revision,
                ],
                |row| row.get(0),
            )
            .optional())?;
        if updated.is_none() {
            return Ok(None);
        }
        db(connection.execute(DELETE_SKILLS_SQL, [uuid_text(job.id)]))?;
        insert_skills(connection, &graph.skills)?;
        db(connection.execute(DELETE_TERMS_SQL, [uuid_text(job.id)]))?;
        insert_terms(connection, graph)?;
        load_aggregate(connection, job.id)?
            .ok_or_else(unavailable)
            .map(Some)
    })
}

/// Loads the job row, its skills and its single ingestion record. A job
/// whose provenance does not match its source method fails closed as unavailable.
pub(crate) fn load_aggregate(
    connection: &Connection,
    job_id: Uuid,
) -> Result<Option<JobAggregate>, AppError> {
    let id = uuid_text(job_id);
    let raw: Option<RawJob> = db(connection
        .prepare_cached(JOB_SELECT_SQL)
        .and_then(|mut statement| statement.query_row([&id], read_job).optional()))?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let job = raw.into_record()?;
    let skills = query_all(connection, SKILLS_SQL, &id, read_skill)?
        .into_iter()
        .map(RawSkill::into_record)
        .collect::<Result<Vec<_>, AppError>>()?;
    let links = query_all(connection, LINKS_SQL, &id, read_link)?
        .into_iter()
        .map(RawLink::into_record)
        .collect::<Result<Vec<_>, AppError>>()?;
    let texts = query_all(connection, TEXTS_SQL, &id, read_text)?
        .into_iter()
        .map(RawText::into_record)
        .collect::<Result<Vec<_>, AppError>>()?;
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

/// Runs a one-bind query and decodes every row with `read`, keeping the row order.
fn query_all<T>(
    connection: &Connection,
    sql: &str,
    bind: &str,
    read: impl Fn(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>, AppError> {
    db(connection.prepare_cached(sql).and_then(|mut statement| {
        statement
            .query_map([bind], |row| read(row))?
            .collect::<rusqlite::Result<Vec<T>>>()
    }))
}

/// The job that owns `input_text_hash`.
fn text_owner(connection: &Connection, input_text_hash: &str) -> Result<Option<Uuid>, AppError> {
    let owner: Option<String> = db(connection
        .query_row(TEXT_OWNER_SQL, [input_text_hash], |row| row.get(0))
        .optional())?;
    owner
        .map(|text| parse_uuid(&text).ok_or_else(invalid_shape))
        .transpose()
}

/// The job with a canonical fingerprint, with its aggregate.
fn find_by_fingerprint(
    connection: &Connection,
    fingerprint: &str,
) -> Result<Option<JobAggregate>, AppError> {
    let owner: Option<String> = db(connection
        .query_row(FINGERPRINT_SQL, [fingerprint], |row| row.get(0))
        .optional())?;
    let Some(owner) = owner else {
        return Ok(None);
    };
    let job_id = parse_uuid(&owner).ok_or_else(invalid_shape)?;
    load_aggregate(connection, job_id)
}

/// Inserts the job row, then its skills and postings. Returns `false` when
/// the fingerprint conflict left the row unwritten (nothing else is written then).
fn insert_children(connection: &Connection, graph: &JobAggregate) -> Result<bool, AppError> {
    let job = &graph.job;
    let title = non_blank(&job.title)?;
    let description = non_blank(&job.description)?;
    let fingerprint = non_blank(&job.canonical_fingerprint)?;
    let inserted: Option<String> = db(connection
        .query_row(
            INSERT_JOB_SQL,
            params![
                uuid_text(job.id),
                job.source_method,
                job.source_label,
                title,
                job.company,
                job.location,
                description,
                job.experience_requirement,
                job.employment_type,
                job.seniority,
                optional_instant_text(job.posted_at),
                fingerprint,
                instant_text(job.created_at),
                instant_text(job.updated_at),
                job.revision,
            ],
            |row| row.get(0),
        )
        .optional())?;
    if inserted.is_none() {
        return Ok(false);
    }
    insert_skills(connection, &graph.skills)?;
    insert_terms(connection, graph)?;
    Ok(true)
}

/// The skill rows of a job, in display order.
fn insert_skills(connection: &Connection, skills: &[JobSkillRecord]) -> Result<(), AppError> {
    if skills.is_empty() {
        return Ok(());
    }
    // Prepared once: the statement cache would otherwise look it up for every skill.
    let mut statement = db(connection.prepare_cached(INSERT_SKILL_SQL))?;
    for skill in skills {
        let skill_text = non_blank(&skill.skill)?;
        let normalized = non_blank(&skill.normalized_skill)?;
        let display_order = display_order_value(skill.display_order)?;
        db(statement.execute(params![
            uuid_text(skill.id),
            uuid_text(skill.job_id),
            skill_text,
            normalized,
            skill.required,
            display_order,
            instant_text(skill.created_at),
        ]))?;
    }
    Ok(())
}

/// The postings of a job: its text fields and skills, in posting order.
fn insert_terms(connection: &Connection, graph: &JobAggregate) -> Result<(), AppError> {
    let terms = job_search_terms(&graph.job, &graph.skills);
    if terms.is_empty() {
        return Ok(());
    }
    let mut statement = db(connection.prepare_cached(INSERT_TERM_SQL))?;
    for term in terms {
        let weight = weight_value(term.weight)?;
        db(statement.execute(params![
            uuid_text(term.entity_id),
            term.field_key,
            term.term,
            weight,
        ]))?;
    }
    Ok(())
}

/// Narrows a display order to the `integer` column range. An out-of-range
/// value is a data error, not an integrity error, so it is database-unavailable.
fn display_order_value(value: i64) -> Result<i32, AppError> {
    i32::try_from(value).map_err(|_| unavailable())
}

/// Narrows a core search weight to the `smallint` range of the posting column.
fn weight_value(value: i64) -> Result<i16, AppError> {
    i16::try_from(value).map_err(|_| AppError::Internal)
}

/// The pre-commit replacement of the deferred provenance constraint triggers: at commit the job
/// has exactly one provenance row, of its source method, and none of the other kind.
pub fn provenance_check(job_id: Uuid) -> PreCommitCheck {
    Box::new(move |connection: &Connection| {
        let id = uuid_text(job_id);
        let source: Option<String> = connection
            .query_row(
                "SELECT source_method FROM job_jobs WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(source) = source else {
            return Ok(());
        };
        let links: i64 = connection.query_row(
            "SELECT count(*) FROM job_job_link_ingestions WHERE job_id = ?1",
            [&id],
            |row| row.get(0),
        )?;
        let texts: i64 = connection.query_row(
            "SELECT count(*) FROM job_job_text_ingestions WHERE job_id = ?1",
            [&id],
            |row| row.get(0),
        )?;
        let valid = match source.as_str() {
            "link" => links == 1 && texts == 0,
            "text" => texts == 1 && links == 0,
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err(SqliteError::constraint("job_source_provenance"))
        }
    })
}

/// One job row as stored, before its columns are parsed.
pub(crate) struct RawJob {
    id: String,
    source_method: String,
    source_label: Option<String>,
    title: String,
    company: Option<String>,
    location: Option<String>,
    description: String,
    experience_requirement: Option<String>,
    employment_type: Option<String>,
    seniority: Option<String>,
    posted_at: Option<String>,
    canonical_fingerprint: String,
    created_at: String,
    updated_at: String,
    revision: i64,
}

impl RawJob {
    /// The record, or the shape error when an identifier or an instant does not parse.
    pub(crate) fn into_record(self) -> Result<JobRecord, AppError> {
        Ok(JobRecord {
            id: parse_uuid(&self.id).ok_or_else(invalid_shape)?,
            source_method: self.source_method,
            source_label: self.source_label,
            title: self.title,
            company: self.company,
            location: self.location,
            description: self.description,
            experience_requirement: self.experience_requirement,
            employment_type: self.employment_type,
            seniority: self.seniority,
            posted_at: self
                .posted_at
                .map(|text| parse_instant_text(&text).ok_or_else(invalid_shape))
                .transpose()?,
            canonical_fingerprint: self.canonical_fingerprint,
            created_at: parse_instant_text(&self.created_at).ok_or_else(invalid_shape)?,
            updated_at: parse_instant_text(&self.updated_at).ok_or_else(invalid_shape)?,
            revision: self.revision,
        })
    }
}

/// One job row, in the column order of [`JOB_SELECT_SQL`].
pub(crate) fn read_job(row: &Row<'_>) -> rusqlite::Result<RawJob> {
    Ok(RawJob {
        id: row.get(0)?,
        source_method: row.get(1)?,
        source_label: row.get(2)?,
        title: row.get(3)?,
        company: row.get(4)?,
        location: row.get(5)?,
        description: row.get(6)?,
        experience_requirement: row.get(7)?,
        employment_type: row.get(8)?,
        seniority: row.get(9)?,
        posted_at: row.get(10)?,
        canonical_fingerprint: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        revision: row.get(14)?,
    })
}

/// One skill row as stored.
pub(crate) struct RawSkill {
    id: String,
    job_id: String,
    skill: String,
    normalized_skill: String,
    required: bool,
    display_order: i64,
    created_at: String,
}

impl RawSkill {
    /// The record, or the shape error when an identifier or an instant does not parse.
    pub(crate) fn into_record(self) -> Result<JobSkillRecord, AppError> {
        Ok(JobSkillRecord {
            id: parse_uuid(&self.id).ok_or_else(invalid_shape)?,
            job_id: parse_uuid(&self.job_id).ok_or_else(invalid_shape)?,
            skill: self.skill,
            normalized_skill: self.normalized_skill,
            required: self.required,
            display_order: self.display_order,
            created_at: parse_instant_text(&self.created_at).ok_or_else(invalid_shape)?,
        })
    }
}

/// One skill row, in the column order of [`SKILLS_SQL`].
pub(crate) fn read_skill(row: &Row<'_>) -> rusqlite::Result<RawSkill> {
    Ok(RawSkill {
        id: row.get(0)?,
        job_id: row.get(1)?,
        skill: row.get(2)?,
        normalized_skill: row.get(3)?,
        required: row.get(4)?,
        display_order: row.get(5)?,
        created_at: row.get(6)?,
    })
}

/// One link provenance row as stored.
struct RawLink {
    id: String,
    job_id: String,
    url: String,
    normalized_url: String,
    fetched_at: String,
    http_status: Option<i64>,
    source_title: Option<String>,
    created_at: String,
}

impl RawLink {
    /// Only the identity and the normalized URL are typed; the other columns travel in `fields`
    /// under their column aliases, with the instants in their `Instant.toString()` form.
    fn into_record(self) -> Result<LinkIngestionRecord, AppError> {
        let fetched_at = parse_instant_text(&self.fetched_at).ok_or_else(invalid_shape)?;
        let created_at = parse_instant_text(&self.created_at).ok_or_else(invalid_shape)?;
        let mut fields = Map::new();
        fields.insert("url".to_owned(), Value::String(self.url));
        fields.insert(
            "fetchedAt".to_owned(),
            Value::String(jobniac_core::clock::format_instant(fetched_at)),
        );
        fields.insert(
            "httpStatus".to_owned(),
            self.http_status.map_or(Value::Null, Value::from),
        );
        fields.insert(
            "sourceTitle".to_owned(),
            self.source_title.map_or(Value::Null, Value::String),
        );
        fields.insert(
            "createdAt".to_owned(),
            Value::String(jobniac_core::clock::format_instant(created_at)),
        );
        Ok(LinkIngestionRecord {
            id: parse_uuid(&self.id).ok_or_else(invalid_shape)?,
            job_id: parse_uuid(&self.job_id).ok_or_else(invalid_shape)?,
            normalized_url: self.normalized_url,
            fields,
        })
    }
}

fn read_link(row: &Row<'_>) -> rusqlite::Result<RawLink> {
    Ok(RawLink {
        id: row.get(0)?,
        job_id: row.get(1)?,
        url: row.get(2)?,
        normalized_url: row.get(3)?,
        fetched_at: row.get(4)?,
        http_status: row.get(5)?,
        source_title: row.get(6)?,
        created_at: row.get(7)?,
    })
}

/// One text provenance row as stored.
struct RawText {
    id: String,
    job_id: String,
    source_label: Option<String>,
    input_text_hash: String,
    created_at: String,
}

impl RawText {
    fn into_record(self) -> Result<TextIngestionRecord, AppError> {
        Ok(TextIngestionRecord {
            id: parse_uuid(&self.id).ok_or_else(invalid_shape)?,
            job_id: parse_uuid(&self.job_id).ok_or_else(invalid_shape)?,
            source_label: self.source_label,
            input_text_hash: self.input_text_hash,
            created_at: parse_instant_text(&self.created_at).ok_or_else(invalid_shape)?,
        })
    }
}

fn read_text(row: &Row<'_>) -> rusqlite::Result<RawText> {
    Ok(RawText {
        id: row.get(0)?,
        job_id: row.get(1)?,
        source_label: row.get(2)?,
        input_text_hash: row.get(3)?,
        created_at: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::has_userinfo;

    #[test]
    fn userinfo_is_detected_in_the_authority_only() {
        assert!(has_userinfo("https://user@example.test/jobs"));
        assert!(has_userinfo("HTTP://user:secret@example.test"));
        assert!(!has_userinfo("https://example.test/jobs/a@b"));
        assert!(!has_userinfo("https://example.test?mail=a@b"));
        assert!(!has_userinfo("ftp://user@example.test"));
    }
}
