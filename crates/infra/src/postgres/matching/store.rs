//! Profile and job reads of the pair analysis, the insert-only report store and the report views.
//!
//! The SQL uses `$n` binds.

use std::collections::HashMap;

use jobniac_core::clock::format_instant;
use jobniac_core::errors::AppError;
use jobniac_core::matching::{
    JobGraph, MatchReport, MatchView, ProfileGraph, is_stale, uuid_from_text,
    validate_match_component, validate_match_evidence,
};
use time::OffsetDateTime;
use tokio_postgres::{Client, Row};
use uuid::Uuid;

use super::value::{
    Failure, column, components_json, corrupted, evidence_json, json_array, outcome_from_text,
    outcome_text,
};

/// The profile root and whether it has an experience row.
const PROFILE_SQL: &str = r#"
        SELECT profile.id, profile.updated_at,
               EXISTS (SELECT 1 FROM profile.experiences AS experience
                       WHERE experience.profile_id = profile.id) AS has_experience
        FROM profile.profiles AS profile
        WHERE profile.id = $1
        "#;

/// The normalized skills in stored order.
const PROFILE_SKILLS_SQL: &str = "SELECT normalized_skill FROM profile.profile_skills \
    WHERE profile_id = $1 ORDER BY display_order, skill, id";

/// The job root, read inside the pair transaction.
const JOB_SQL: &str = "SELECT id, updated_at, created_at, experience_requirement, seniority \
    FROM job_schema.jobs WHERE id = $1";

/// The skills of every listed job in stored order.
const JOB_SKILLS_SQL: &str = "SELECT job_id, normalized_skill FROM job_schema.job_skills \
    WHERE job_id = ANY($1::uuid[]) ORDER BY job_id, display_order, skill, id";

/// The insert of a report, deduplicated by its identity.
const INSERT_REPORT_SQL: &str = "
        INSERT INTO match.reports
            (id, profile_id, job_id, profile_revision, job_revision, algorithm_version,
             overall_score, confidence, outcome, blocker_mismatch, components, evidence, created_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11::jsonb,$12::jsonb,$13)
        ON CONFLICT DO NOTHING
        ";

/// The stored report with the identity of the inserted one.
const SELECT_STORED_REPORT_SQL: &str = "
        SELECT * FROM match.reports
        WHERE algorithm_version = $1 AND profile_id = $2 AND profile_revision = $3
          AND job_id = $4 AND job_revision = $5
        ";

/// One report with the current revisions of its sources.
const REPORT_WITH_REVISIONS_SQL: &str = "
SELECT report.*, profile.updated_at AS current_profile_revision,
       job.updated_at AS current_job_revision
FROM match.reports AS report
LEFT JOIN profile.profiles AS profile ON profile.id = report.profile_id
LEFT JOIN job_schema.jobs AS job ON job.id = report.job_id
WHERE report.id = $1
";

/// The profile root with its experience flag, or `profile not found`.
pub(crate) async fn load_profile(
    client: &Client,
    profile_id: Uuid,
) -> Result<ProfileGraph, Failure> {
    let Some(row) = client.query_opt(PROFILE_SQL, &[&profile_id]).await? else {
        return Err(Failure::App(AppError::DomainValue(format!(
            "profile not found: {profile_id}"
        ))));
    };
    let skill_rows = client.query(PROFILE_SKILLS_SQL, &[&profile_id]).await?;
    let skills = skill_rows
        .iter()
        .map(|skill| column::<String>(skill, "normalized_skill"))
        .collect::<Result<Vec<String>, Failure>>()?;
    Ok(ProfileGraph {
        id: column::<Uuid>(&row, "id")?.to_string(),
        updated_at: format_instant(column::<OffsetDateTime>(&row, "updated_at")?),
        skills,
        has_experience: column(&row, "has_experience")?,
    })
}

/// The job root of a row, without skills.
pub(crate) fn job_graph(row: &Row) -> Result<JobGraph, Failure> {
    Ok(JobGraph {
        id: column::<Uuid>(row, "id")?.to_string(),
        updated_at: format_instant(column::<OffsetDateTime>(row, "updated_at")?),
        experience_requirement: column(row, "experience_requirement")?,
        seniority: column(row, "seniority")?,
        skills: Vec::new(),
    })
}

/// The job root, or `job not found`, with its skills attached.
pub(crate) async fn load_job(client: &Client, job_id: Uuid) -> Result<JobGraph, Failure> {
    let Some(row) = client.query_opt(JOB_SQL, &[&job_id]).await? else {
        return Err(Failure::App(AppError::DomainValue(format!(
            "job not found: {job_id}"
        ))));
    };
    let mut graph = job_graph(&row)?;
    attach_skills(client, std::slice::from_mut(&mut graph)).await?;
    Ok(graph)
}

/// Appends each job's normalized skills, in stored order.
pub(crate) async fn attach_skills(client: &Client, graphs: &mut [JobGraph]) -> Result<(), Failure> {
    if graphs.is_empty() {
        return Ok(());
    }
    let mut positions = HashMap::with_capacity(graphs.len());
    let mut ids = Vec::with_capacity(graphs.len());
    for (position, graph) in graphs.iter().enumerate() {
        let id = uuid_from_text(&graph.id).ok_or(Failure::App(AppError::Internal))?;
        positions.insert(id, position);
        ids.push(id);
    }
    let rows = client.query(JOB_SKILLS_SQL, &[&ids]).await?;
    for row in &rows {
        let job_id: Uuid = column(row, "job_id")?;
        let skill: String = column(row, "normalized_skill")?;
        if let Some(position) = positions.get(&job_id) {
            graphs[*position].skills.push(skill);
        }
    }
    Ok(())
}

/// Inserts the report unless its identity is stored, then returns the stored
/// row. The returned report can therefore differ from `report` in `created_at` (the first insert
/// wins).
pub(crate) async fn save_report(
    client: &Client,
    report: &MatchReport,
) -> Result<MatchReport, Failure> {
    let overall_score =
        i32::try_from(report.overall_score).map_err(|_| Failure::App(AppError::Internal))?;
    let confidence =
        i32::try_from(report.confidence).map_err(|_| Failure::App(AppError::Internal))?;
    let components = components_json(&report.components);
    let evidence = evidence_json(&report.evidence);
    client
        .execute(
            INSERT_REPORT_SQL,
            &[
                &report.id,
                &report.profile_id,
                &report.job_id,
                &report.profile_revision,
                &report.job_revision,
                &report.algorithm_version,
                &overall_score,
                &confidence,
                &outcome_text(report.outcome),
                &report.blocker_mismatch,
                &components,
                &evidence,
                &report.created_at,
            ],
        )
        .await?;
    let row = client
        .query_opt(
            SELECT_STORED_REPORT_SQL,
            &[
                &report.algorithm_version,
                &report.profile_id,
                &report.profile_revision,
                &report.job_id,
                &report.job_revision,
            ],
        )
        .await?
        .ok_or_else(corrupted)?;
    report_from_row(&row)
}

/// The report with its staleness, or `None`.
pub(crate) async fn get_report(
    client: &Client,
    report_id: Uuid,
) -> Result<Option<MatchView>, Failure> {
    let row = client
        .query_opt(REPORT_WITH_REVISIONS_SQL, &[&report_id])
        .await?;
    row.as_ref().map(view_from_row).transpose()
}

/// A stored report. The components and evidence are validated with the
/// persisted-record rules, and any violation is "Local database is unavailable".
pub(crate) fn report_from_row(row: &Row) -> Result<MatchReport, Failure> {
    let components = json_array(row, "components")?
        .iter()
        .map(validate_match_component)
        .collect::<Result<Vec<_>, AppError>>()
        .map_err(|_| corrupted())?;
    let evidence = json_array(row, "evidence")?
        .iter()
        .map(validate_match_evidence)
        .collect::<Result<Vec<_>, AppError>>()
        .map_err(|_| corrupted())?;
    let outcome = outcome_from_text(&column::<String>(row, "outcome")?).ok_or_else(corrupted)?;
    let overall_score = i64::from(column::<i32>(row, "overall_score")?);
    let confidence = i64::from(column::<i32>(row, "confidence")?);
    Ok(MatchReport {
        id: column(row, "id")?,
        profile_id: column(row, "profile_id")?,
        job_id: column(row, "job_id")?,
        profile_revision: column(row, "profile_revision")?,
        job_revision: column(row, "job_revision")?,
        algorithm_version: column(row, "algorithm_version")?,
        overall_score,
        confidence,
        outcome,
        blocker_mismatch: column(row, "blocker_mismatch")?,
        components,
        evidence,
        created_at: column(row, "created_at")?,
    })
}

/// A stored report with `stale` set when a source revision moved on. The row carries
/// `current_profile_revision` and `current_job_revision` from the left joins.
pub(crate) fn view_from_row(row: &Row) -> Result<MatchView, Failure> {
    let report = report_from_row(row)?;
    let current_profile: Option<OffsetDateTime> = column(row, "current_profile_revision")?;
    let current_job: Option<OffsetDateTime> = column(row, "current_job_revision")?;
    let stale = is_stale(&report, current_profile, current_job);
    Ok(MatchView { report, stale })
}

/// Reads a `jsonb` column of a stored record without validating it (the record's own validator does).
pub(crate) fn raw_json(row: &Row, name: &str) -> Result<serde_json::Value, Failure> {
    column(row, name)
}
