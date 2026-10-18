//! Writes and single reads of the advisory reviews.
//!
//! A submission is one transaction: the report is loaded, the review factory runs on it, the review
//! is stored, the disagreement factory runs on the stored review, and the disagreement is stored.
//! Both inserts are `ON CONFLICT DO NOTHING` and read back by their content fingerprint, so a
//! resubmission returns the stored row.

use jobniac_core::clock::format_instant;
use jobniac_core::errors::AppError;
use jobniac_core::reviews::{
    DisagreementFactory, DisagreementStatus, MatchDisagreement, MatchReview, ReviewFactory,
    ReviewSubmission, disagreement_fingerprint, review_fingerprint, validate_match_disagreement,
    validate_match_review,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::{Map, Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::sqlite::codec::{instant_text, json_text, uuid_text};
use crate::sqlite::error::SqliteError;
use crate::sqlite::matching::store::{read_report, report_columns};
use crate::sqlite::matching::value::{
    Failure, components_json, corrupted, evidence_json, instant_column, json_array, outcome_text,
    raw_json, run, uuid_column,
};
use crate::sqlite::unit_of_work::SqliteTransaction;

/// The evidence defect codes a stored disagreement may name (the PostgreSQL check
/// `evidence_defect_codes <@ [...]`, which SQLite cannot express without a sub-query).
const DEFECT_CODES: [&str; 4] = [
    "structured_evidence_missing",
    "structured_evidence_incorrect",
    "requirement_provenance_missing",
    "outcome_calibration_issue",
];

/// The report of a submission, or `match report not found`.
const SELECT_REPORT_SQL: &str = concat!(
    "SELECT ",
    report_columns!("report"),
    " FROM match_reports AS report WHERE report.id = ?1"
);

/// The columns of a stored review, in the order of [`read_review`].
macro_rules! review_columns {
    ($alias:literal) => {
        concat!(
            $alias,
            ".id, ",
            $alias,
            ".report_id, ",
            $alias,
            ".reviewer, ",
            $alias,
            ".model, ",
            $alias,
            ".review_version, ",
            $alias,
            ".overall_score, ",
            $alias,
            ".outcome, ",
            $alias,
            ".blocker_mismatch, ",
            $alias,
            ".components, ",
            $alias,
            ".evidence, ",
            $alias,
            ".summary, ",
            $alias,
            ".created_at"
        )
    };
}

/// The columns of a stored disagreement, in the order of [`read_disagreement`].
macro_rules! disagreement_columns {
    ($alias:literal) => {
        concat!(
            $alias,
            ".id, ",
            $alias,
            ".report_id, ",
            $alias,
            ".review_id, ",
            $alias,
            ".policy_version, ",
            $alias,
            ".reasons, ",
            $alias,
            ".evidence_defect_codes, ",
            $alias,
            ".status, ",
            $alias,
            ".linear_issue_id, ",
            $alias,
            ".created_at, ",
            $alias,
            ".updated_at"
        )
    };
}

pub(crate) use disagreement_columns;
pub(crate) use review_columns;

/// The insert of a review, deduplicated by its fingerprint.
const INSERT_REVIEW_SQL: &str = "
INSERT INTO match_reviews
    (id, fingerprint, report_id, reviewer, model, review_version, overall_score, outcome,
     blocker_mismatch, components, evidence, summary, created_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
ON CONFLICT DO NOTHING
";

/// The stored review with the fingerprint of the inserted one.
const SELECT_REVIEW_BY_FINGERPRINT_SQL: &str = concat!(
    "SELECT ",
    review_columns!("review"),
    " FROM match_reviews AS review WHERE review.fingerprint = ?1"
);

/// One review by identifier.
const SELECT_REVIEW_SQL: &str = concat!(
    "SELECT ",
    review_columns!("review"),
    " FROM match_reviews AS review WHERE review.id = ?1"
);

/// The insert of a disagreement, deduplicated by its fingerprint.
const INSERT_DISAGREEMENT_SQL: &str = "
INSERT INTO match_disagreements
    (id, fingerprint, report_id, review_id, policy_version, reasons, evidence_defect_codes,
     status, linear_issue_id, created_at, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
ON CONFLICT DO NOTHING
";

/// The stored disagreement with the fingerprint of the inserted one.
const SELECT_DISAGREEMENT_BY_FINGERPRINT_SQL: &str = concat!(
    "SELECT ",
    disagreement_columns!("disagreement"),
    " FROM match_disagreements AS disagreement WHERE disagreement.fingerprint = ?1"
);

/// The status, the issue (kept when absent) and `updated_at`. `RETURNING` cannot name a table
/// alias, so the columns of [`read_disagreement`] are listed unqualified.
const CHANGE_DISAGREEMENT_SQL: &str = "UPDATE match_disagreements SET status = ?1, \
     linear_issue_id = COALESCE(?2, linear_issue_id), updated_at = ?3 \
     WHERE id = ?4 \
     RETURNING id, report_id, review_id, policy_version, reasons, evidence_defect_codes, \
               status, linear_issue_id, created_at, updated_at";

/// One submission, in the caller's transaction.
pub(crate) fn submit(
    tx: &SqliteTransaction,
    report_id: Uuid,
    review_factory: &ReviewFactory<'_>,
    disagreement_factory: &DisagreementFactory<'_>,
) -> Result<ReviewSubmission, Failure> {
    let raw = run(tx, |connection| {
        Ok(connection
            .query_row(SELECT_REPORT_SQL, [uuid_text(report_id)], read_report)
            .optional()?)
    })?;
    let Some(raw) = raw else {
        return Err(Failure(AppError::DomainValue(format!(
            "match report not found: {report_id}"
        ))));
    };
    let report = raw.into_report()?;
    let review = save_review(tx, review_factory(&report)?)?;
    let disagreement = match disagreement_factory(&report, &review) {
        None => None,
        Some(candidate) => Some(save_disagreement(tx, candidate)?),
    };
    Ok(ReviewSubmission {
        review,
        disagreement,
    })
}

/// The stored review, or `None`.
pub(crate) fn get_review(
    tx: &SqliteTransaction,
    review_id: Uuid,
) -> Result<Option<MatchReview>, Failure> {
    run(tx, |connection| {
        connection
            .query_row(SELECT_REVIEW_SQL, [uuid_text(review_id)], read_review)
            .optional()?
            .map(RawReview::into_review)
            .transpose()
    })
}

/// Inserts the review unless its fingerprint is stored, then returns the
/// stored row (with its original identifier and creation instant).
pub(crate) fn save_review(
    tx: &SqliteTransaction,
    review: MatchReview,
) -> Result<MatchReview, Failure> {
    let fingerprint = uuid_text(review_fingerprint(&review));
    let overall_score =
        i32::try_from(review.overall_score).map_err(|_| Failure(AppError::Internal))?;
    let components = json_text(&components_json(&review.components));
    let evidence = json_text(&evidence_json(&review.evidence));
    run(tx, |connection| {
        connection.execute(
            INSERT_REVIEW_SQL,
            params![
                uuid_text(review.id),
                fingerprint,
                uuid_text(review.report_id),
                review.reviewer,
                review.model,
                review.review_version,
                overall_score,
                outcome_text(review.outcome),
                review.blocker_mismatch,
                components,
                evidence,
                review.summary.as_str(),
                instant_text(review.created_at),
            ],
        )?;
        connection
            .query_row(
                SELECT_REVIEW_BY_FINGERPRINT_SQL,
                [&fingerprint],
                read_review,
            )
            .optional()?
            .ok_or_else(corrupted)?
            .into_review()
    })
}

/// Inserts the disagreement unless its dedupe key is stored, then returns
/// the stored row. A stored disagreement keeps its status and linked issue.
pub(crate) fn save_disagreement(
    tx: &SqliteTransaction,
    disagreement: MatchDisagreement,
) -> Result<MatchDisagreement, Failure> {
    let fingerprint = uuid_text(disagreement_fingerprint(
        disagreement.report_id,
        &disagreement.reasons,
        &disagreement.evidence_defect_codes,
        &disagreement.policy_version,
    ));
    let reasons = Value::Array(
        disagreement
            .reasons
            .iter()
            .map(|reason| Value::String(reason.as_str().to_owned()))
            .collect(),
    );
    let codes = Value::Array(
        disagreement
            .evidence_defect_codes
            .iter()
            .map(|code| Value::String(code.as_str().to_owned()))
            .collect(),
    );
    run(tx, |connection| {
        if !defect_codes_known(&codes) {
            return Err(Failure::from(SqliteError::constraint(
                "disagreements_evidence_defect_codes_check",
            )));
        }
        connection.execute(
            INSERT_DISAGREEMENT_SQL,
            params![
                uuid_text(disagreement.id),
                fingerprint,
                uuid_text(disagreement.report_id),
                uuid_text(disagreement.review_id),
                disagreement.policy_version,
                json_text(&reasons),
                json_text(&codes),
                disagreement.status.as_str(),
                disagreement.linear_issue_id,
                instant_text(disagreement.created_at),
                instant_text(disagreement.updated_at),
            ],
        )?;
        read_stored_disagreement(connection, &fingerprint)
    })
}

/// Whether every element of the JSON array of defect codes is a known code.
fn defect_codes_known(codes: &Value) -> bool {
    codes.as_array().is_some_and(|items| {
        items.iter().all(|item| {
            item.as_str()
                .is_some_and(|code| DEFECT_CODES.contains(&code))
        })
    })
}

/// The stored disagreement with `fingerprint`.
fn read_stored_disagreement(
    connection: &Connection,
    fingerprint: &str,
) -> Result<MatchDisagreement, Failure> {
    connection
        .query_row(
            SELECT_DISAGREEMENT_BY_FINGERPRINT_SQL,
            [fingerprint],
            read_disagreement,
        )
        .optional()?
        .ok_or_else(corrupted)?
        .into_disagreement()
}

/// The changed row, or `None` when it does not exist.
pub(crate) fn change_disagreement(
    tx: &SqliteTransaction,
    disagreement_id: Uuid,
    status: DisagreementStatus,
    issue: Option<&str>,
    now: OffsetDateTime,
) -> Result<Option<MatchDisagreement>, Failure> {
    run(tx, |connection| {
        connection
            .query_row(
                CHANGE_DISAGREEMENT_SQL,
                params![
                    status.as_str(),
                    issue,
                    instant_text(now),
                    uuid_text(disagreement_id)
                ],
                read_disagreement,
            )
            .optional()?
            .map(RawDisagreement::into_disagreement)
            .transpose()
    })
}

/// A stored review as read, before its columns are parsed and validated.
pub(crate) struct RawReview {
    id: String,
    report_id: String,
    reviewer: String,
    model: String,
    review_version: String,
    overall_score: i64,
    outcome: String,
    blocker_mismatch: bool,
    components: String,
    evidence: String,
    summary: String,
    created_at: String,
}

/// One review, in the column order of [`review_columns`].
pub(crate) fn read_review(row: &Row<'_>) -> rusqlite::Result<RawReview> {
    Ok(RawReview {
        id: row.get(0)?,
        report_id: row.get(1)?,
        reviewer: row.get(2)?,
        model: row.get(3)?,
        review_version: row.get(4)?,
        overall_score: row.get(5)?,
        outcome: row.get(6)?,
        blocker_mismatch: row.get(7)?,
        components: row.get(8)?,
        evidence: row.get(9)?,
        summary: row.get(10)?,
        created_at: row.get(11)?,
    })
}

impl RawReview {
    /// The created-at and identifier of the review, the keyset position.
    pub(crate) fn key(&self) -> Result<(OffsetDateTime, Uuid), Failure> {
        Ok((instant_column(&self.created_at)?, uuid_column(&self.id)?))
    }

    /// A stored review, validated with the persisted-record rules. Any violation is
    /// "Local database is unavailable".
    pub(crate) fn into_review(self) -> Result<MatchReview, Failure> {
        let id = uuid_column(&self.id)?.to_string();
        let report_id = uuid_column(&self.report_id)?.to_string();
        let components = json_array(&self.components)?;
        let evidence = json_array(&self.evidence)?;
        let created_at = format_instant(instant_column(&self.created_at)?);
        let values = json!({
            "id": id,
            "reportId": report_id,
            "reviewer": self.reviewer,
            "model": self.model,
            "reviewVersion": self.review_version,
            "overallScore": self.overall_score,
            "outcome": self.outcome,
            "blockerMismatch": self.blocker_mismatch,
            "components": components,
            "evidence": evidence,
            "summary": self.summary,
            "createdAt": created_at,
        });
        let Value::Object(map) = values else {
            return Err(corrupted());
        };
        validate_match_review(&map).map_err(|_| corrupted())
    }
}

/// A stored disagreement as read, before its columns are parsed and validated.
pub(crate) struct RawDisagreement {
    id: String,
    report_id: String,
    review_id: String,
    policy_version: String,
    reasons: String,
    evidence_defect_codes: String,
    status: String,
    linear_issue_id: Option<String>,
    created_at: String,
    updated_at: String,
}

/// One disagreement, in the column order of [`disagreement_columns`].
pub(crate) fn read_disagreement(row: &Row<'_>) -> rusqlite::Result<RawDisagreement> {
    Ok(RawDisagreement {
        id: row.get(0)?,
        report_id: row.get(1)?,
        review_id: row.get(2)?,
        policy_version: row.get(3)?,
        reasons: row.get(4)?,
        evidence_defect_codes: row.get(5)?,
        status: row.get(6)?,
        linear_issue_id: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

impl RawDisagreement {
    /// The created-at and identifier of the disagreement, the keyset position.
    pub(crate) fn key(&self) -> Result<(OffsetDateTime, Uuid), Failure> {
        Ok((instant_column(&self.created_at)?, uuid_column(&self.id)?))
    }

    /// A stored disagreement, validated with the persisted-record rules.
    pub(crate) fn into_disagreement(self) -> Result<MatchDisagreement, Failure> {
        let id = uuid_column(&self.id)?.to_string();
        let report_id = uuid_column(&self.report_id)?.to_string();
        let review_id = uuid_column(&self.review_id)?.to_string();
        let reasons = raw_json(&self.reasons)?;
        let codes = raw_json(&self.evidence_defect_codes)?;
        let created_at = format_instant(instant_column(&self.created_at)?);
        let updated_at = format_instant(instant_column(&self.updated_at)?);
        let values: Map<String, Value> = match json!({
            "id": id,
            "reportId": report_id,
            "reviewId": review_id,
            "policyVersion": self.policy_version,
            "reasons": reasons,
            "evidenceDefectCodes": codes,
            "status": self.status,
            "linearIssueId": self.linear_issue_id,
            "createdAt": created_at,
            "updatedAt": updated_at,
        }) {
            Value::Object(map) => map,
            _ => return Err(corrupted()),
        };
        validate_match_disagreement(&values).map_err(|_| corrupted())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::defect_codes_known;

    #[test]
    fn only_the_four_defect_codes_are_known() {
        assert!(defect_codes_known(&json!([])));
        assert!(defect_codes_known(&json!([
            "structured_evidence_missing",
            "outcome_calibration_issue"
        ])));
        assert!(!defect_codes_known(&json!(["bogus"])));
        assert!(!defect_codes_known(&json!({"a": 1})));
    }
}
