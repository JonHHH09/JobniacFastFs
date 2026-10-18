//! Profile and job reads of the pair analysis, the insert-only report store and the report views.
//!
//! The SQL uses `?n` binds. Stored instants and the report revisions are fixed-width RFC 3339 text
//! (see [`crate::sqlite::jobs::codec`]), so equal instants are equal text.

use std::collections::HashMap;

use jobniac_core::clock::format_instant;
use jobniac_core::errors::AppError;
use jobniac_core::matching::{
    JobGraph, MatchReport, MatchView, ProfileGraph, is_stale, uuid_from_text,
    validate_match_component, validate_match_evidence,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use uuid::Uuid;

use super::value::{
    Failure, components_json, corrupted, evidence_json, instant_column, json_array,
    outcome_from_text, outcome_text, run, uuid_column,
};
use crate::sqlite::jobs::codec::{instant_text, json_text, uuid_array_text, uuid_text};
use crate::sqlite::unit_of_work::SqliteTransaction;

/// The profile root and whether it has an experience row.
const PROFILE_SQL: &str = r#"
SELECT profile.id, profile.updated_at,
       EXISTS (SELECT 1 FROM profile_experiences AS experience
               WHERE experience.profile_id = profile.id) AS has_experience
FROM profile_profiles AS profile
WHERE profile.id = ?1
"#;

/// The normalized skills in stored order.
const PROFILE_SKILLS_SQL: &str = "SELECT normalized_skill FROM profile_profile_skills \
    WHERE profile_id = ?1 ORDER BY display_order, skill, id";

/// The job root, read inside the pair transaction.
const JOB_SQL: &str = "SELECT id, updated_at, created_at, experience_requirement, seniority \
    FROM job_jobs WHERE id = ?1";

/// The skills of every listed job in stored order.
const JOB_SKILLS_SQL: &str = "SELECT job_id, normalized_skill FROM job_job_skills \
    WHERE job_id IN (SELECT value FROM json_each(?1)) ORDER BY job_id, display_order, skill, id";

/// The columns of a stored report, in the order of [`read_report`].
macro_rules! report_columns {
    ($alias:literal) => {
        concat!(
            $alias,
            ".id, ",
            $alias,
            ".profile_id, ",
            $alias,
            ".job_id, ",
            $alias,
            ".profile_revision, ",
            $alias,
            ".job_revision, ",
            $alias,
            ".algorithm_version, ",
            $alias,
            ".overall_score, ",
            $alias,
            ".confidence, ",
            $alias,
            ".outcome, ",
            $alias,
            ".blocker_mismatch, ",
            $alias,
            ".components, ",
            $alias,
            ".evidence, ",
            $alias,
            ".created_at"
        )
    };
}

pub(crate) use report_columns;

/// The insert of a report, deduplicated by its identity.
const INSERT_REPORT_SQL: &str = "
INSERT INTO match_reports
    (id, profile_id, job_id, profile_revision, job_revision, algorithm_version,
     overall_score, confidence, outcome, blocker_mismatch, components, evidence, created_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
ON CONFLICT DO NOTHING
";

/// The stored report with the identity of the inserted one.
const SELECT_STORED_REPORT_SQL: &str = concat!(
    "SELECT ",
    report_columns!("report"),
    " FROM match_reports AS report
WHERE report.algorithm_version = ?1 AND report.profile_id = ?2 AND report.profile_revision = ?3
  AND report.job_id = ?4 AND report.job_revision = ?5"
);

/// One report with the current revisions of its sources.
const REPORT_WITH_REVISIONS_SQL: &str = concat!(
    "SELECT ",
    report_columns!("report"),
    ", profile.updated_at AS current_profile_revision, job.updated_at AS current_job_revision
FROM match_reports AS report
LEFT JOIN profile_profiles AS profile ON profile.id = report.profile_id
LEFT JOIN job_jobs AS job ON job.id = report.job_id
WHERE report.id = ?1"
);

/// A job root as stored, before its columns are parsed.
pub(crate) struct RawJobRoot {
    id: String,
    updated_at: String,
    created_at: String,
    experience_requirement: Option<String>,
    seniority: Option<String>,
}

/// One job root, in the column order of [`JOB_SQL`]. Also the leading columns of the batch page.
pub(crate) fn read_job_root(row: &Row<'_>) -> rusqlite::Result<RawJobRoot> {
    Ok(RawJobRoot {
        id: row.get(0)?,
        updated_at: row.get(1)?,
        created_at: row.get(2)?,
        experience_requirement: row.get(3)?,
        seniority: row.get(4)?,
    })
}

impl RawJobRoot {
    /// The job root, without skills.
    pub(crate) fn graph(&self) -> Result<JobGraph, Failure> {
        Ok(JobGraph {
            id: uuid_column(&self.id)?.to_string(),
            updated_at: format_instant(instant_column(&self.updated_at)?),
            experience_requirement: self.experience_requirement.clone(),
            seniority: self.seniority.clone(),
            skills: Vec::new(),
        })
    }

    /// The created-at and identifier of the root, the keyset position.
    pub(crate) fn key(&self) -> Result<(time::OffsetDateTime, Uuid), Failure> {
        Ok((instant_column(&self.created_at)?, uuid_column(&self.id)?))
    }
}

/// The profile root with its experience flag, or `profile not found`.
pub(crate) fn load_profile(
    connection: &Connection,
    profile_id: Uuid,
) -> Result<ProfileGraph, Failure> {
    let id = uuid_text(profile_id);
    let root: Option<(String, String, bool)> = connection
        .query_row(PROFILE_SQL, [&id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    let Some((root_id, updated_at, has_experience)) = root else {
        return Err(Failure(AppError::DomainValue(format!(
            "profile not found: {profile_id}"
        ))));
    };
    let skills = connection
        .prepare_cached(PROFILE_SKILLS_SQL)?
        .query_map([&id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(ProfileGraph {
        id: uuid_column(&root_id)?.to_string(),
        updated_at: format_instant(instant_column(&updated_at)?),
        skills,
        has_experience,
    })
}

/// The job root, or `job not found`, with its skills attached.
pub(crate) fn load_job(connection: &Connection, job_id: Uuid) -> Result<JobGraph, Failure> {
    let root = connection
        .query_row(JOB_SQL, [uuid_text(job_id)], read_job_root)
        .optional()?;
    let Some(root) = root else {
        return Err(Failure(AppError::DomainValue(format!(
            "job not found: {job_id}"
        ))));
    };
    let mut graph = root.graph()?;
    attach_skills(connection, std::slice::from_mut(&mut graph))?;
    Ok(graph)
}

/// Appends each job's normalized skills, in stored order.
pub(crate) fn attach_skills(
    connection: &Connection,
    graphs: &mut [JobGraph],
) -> Result<(), Failure> {
    if graphs.is_empty() {
        return Ok(());
    }
    let mut positions = HashMap::with_capacity(graphs.len());
    let mut ids = Vec::with_capacity(graphs.len());
    for (position, graph) in graphs.iter().enumerate() {
        let id = uuid_from_text(&graph.id).ok_or(Failure(AppError::Internal))?;
        positions.insert(id, position);
        ids.push(id);
    }
    let rows = connection
        .prepare_cached(JOB_SKILLS_SQL)?
        .query_map([uuid_array_text(&ids)], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (job_id, skill) in rows {
        let job_id = uuid_column(&job_id)?;
        if let Some(position) = positions.get(&job_id) {
            graphs[*position].skills.push(skill);
        }
    }
    Ok(())
}

/// Inserts the report unless its identity is stored, then returns the stored
/// row. The returned report can therefore differ from `report` in `created_at` (the first insert
/// wins).
pub(crate) fn save_report(
    connection: &Connection,
    report: &MatchReport,
) -> Result<MatchReport, Failure> {
    let overall_score =
        i32::try_from(report.overall_score).map_err(|_| Failure(AppError::Internal))?;
    let confidence = i32::try_from(report.confidence).map_err(|_| Failure(AppError::Internal))?;
    let components = json_text(&components_json(&report.components));
    let evidence = json_text(&evidence_json(&report.evidence));
    let profile_revision = instant_text(report.profile_revision);
    let job_revision = instant_text(report.job_revision);
    connection.execute(
        INSERT_REPORT_SQL,
        params![
            uuid_text(report.id),
            uuid_text(report.profile_id),
            uuid_text(report.job_id),
            profile_revision,
            job_revision,
            report.algorithm_version,
            overall_score,
            confidence,
            outcome_text(report.outcome),
            report.blocker_mismatch,
            components,
            evidence,
            instant_text(report.created_at),
        ],
    )?;
    let raw = connection
        .query_row(
            SELECT_STORED_REPORT_SQL,
            params![
                report.algorithm_version,
                uuid_text(report.profile_id),
                profile_revision,
                uuid_text(report.job_id),
                job_revision,
            ],
            read_report,
        )
        .optional()?
        .ok_or_else(corrupted)?;
    raw.into_report()
}

/// Saves `report` in the caller's transaction.
pub(crate) fn save_report_in(
    tx: &SqliteTransaction,
    report: &MatchReport,
) -> Result<MatchReport, Failure> {
    run(tx, |connection| save_report(connection, report))
}

/// The report with its staleness, or `None`.
pub(crate) fn get_report(
    tx: &SqliteTransaction,
    report_id: Uuid,
) -> Result<Option<MatchView>, Failure> {
    run(tx, |connection| {
        connection
            .query_row(REPORT_WITH_REVISIONS_SQL, [uuid_text(report_id)], read_view)
            .optional()?
            .map(RawView::into_view)
            .transpose()
    })
}

/// A stored report as read, before its columns are parsed and validated.
pub(crate) struct RawReport {
    id: String,
    profile_id: String,
    job_id: String,
    profile_revision: String,
    job_revision: String,
    algorithm_version: String,
    overall_score: i64,
    confidence: i64,
    outcome: String,
    blocker_mismatch: bool,
    components: String,
    evidence: String,
    created_at: String,
}

/// One report, in the column order of [`report_columns`].
pub(crate) fn read_report(row: &Row<'_>) -> rusqlite::Result<RawReport> {
    Ok(RawReport {
        id: row.get(0)?,
        profile_id: row.get(1)?,
        job_id: row.get(2)?,
        profile_revision: row.get(3)?,
        job_revision: row.get(4)?,
        algorithm_version: row.get(5)?,
        overall_score: row.get(6)?,
        confidence: row.get(7)?,
        outcome: row.get(8)?,
        blocker_mismatch: row.get(9)?,
        components: row.get(10)?,
        evidence: row.get(11)?,
        created_at: row.get(12)?,
    })
}

impl RawReport {
    /// A stored report. The components and evidence are validated with the
    /// persisted-record rules, and any violation is "Local database is unavailable".
    pub(crate) fn into_report(self) -> Result<MatchReport, Failure> {
        let components = json_array(&self.components)?
            .iter()
            .map(validate_match_component)
            .collect::<Result<Vec<_>, AppError>>()
            .map_err(|_| corrupted())?;
        let evidence = json_array(&self.evidence)?
            .iter()
            .map(validate_match_evidence)
            .collect::<Result<Vec<_>, AppError>>()
            .map_err(|_| corrupted())?;
        let outcome = outcome_from_text(&self.outcome).ok_or_else(corrupted)?;
        Ok(MatchReport {
            id: uuid_column(&self.id)?,
            profile_id: uuid_column(&self.profile_id)?,
            job_id: uuid_column(&self.job_id)?,
            profile_revision: instant_column(&self.profile_revision)?,
            job_revision: instant_column(&self.job_revision)?,
            algorithm_version: self.algorithm_version,
            overall_score: self.overall_score,
            confidence: self.confidence,
            outcome,
            blocker_mismatch: self.blocker_mismatch,
            components,
            evidence,
            created_at: instant_column(&self.created_at)?,
        })
    }
}

/// A stored report with the current revisions of its sources, as read.
pub(crate) struct RawView {
    report: RawReport,
    current_profile_revision: Option<String>,
    current_job_revision: Option<String>,
}

/// One report followed by `current_profile_revision` and `current_job_revision` (columns 13 and 14).
pub(crate) fn read_view(row: &Row<'_>) -> rusqlite::Result<RawView> {
    Ok(RawView {
        report: read_report(row)?,
        current_profile_revision: row.get(13)?,
        current_job_revision: row.get(14)?,
    })
}

impl RawView {
    /// The created-at and identifier of the report, the keyset position.
    pub(crate) fn key(&self) -> Result<(time::OffsetDateTime, Uuid), Failure> {
        Ok((
            instant_column(&self.report.created_at)?,
            uuid_column(&self.report.id)?,
        ))
    }

    /// A stored report with `stale` set when a source revision moved on. The revisions come from
    /// the left joins and are absent when the source row is gone.
    pub(crate) fn into_view(self) -> Result<MatchView, Failure> {
        let current_profile = self
            .current_profile_revision
            .as_deref()
            .map(instant_column)
            .transpose()?;
        let current_job = self
            .current_job_revision
            .as_deref()
            .map(instant_column)
            .transpose()?;
        let report = self.report.into_report()?;
        let stale = is_stale(&report, current_profile, current_job);
        Ok(MatchView { report, stale })
    }
}
